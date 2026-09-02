// SPDX-License-Identifier: AGPL-3.0-only

//! The writer (§9.1): one thread, one transaction per batch, rows in the
//! order subjects, studies, series with their detail rows, instances, source
//! files, diagnostics, then the epoch. Three caches keep the rows a batch is
//! likely to meet again; a miss costs one keyed select for the whole batch,
//! never a query per file.
//!
//! A row that exists is never rewritten: the first record of a subject, a
//! study or a series stands, and a file that disagrees with it raises a
//! `field_disagreement` naming the field.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::time::Instant;

use crossbeam_channel::{Receiver, RecvTimeoutError};
use lru::LruCache;
use nils_dicom::{Diagnostic, DiagnosticKind, Level, Value};
use nils_registry::dialect::Conflict;
use nils_registry::schema::{Column, Table, Type, table};
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::now_iso;
use nils_registry::{HomeError, Registry, Scheme, pseudonym};

use crate::batch::{
    Batch, Fields, Item, ParsedFile, detail_level, hash_value, hash32, identity_of,
};
use crate::progress::{PROGRESS_EVERY, Progress};
use crate::report::{Counts, Written};
use crate::resume::status;

/// Rows each of the writer's caches holds (§9.1).
pub const CACHE_ROWS: usize = 200_000;

struct SubjectEntry {
    id: i64,
    hashes: Box<[u32]>,
}

struct StudyEntry {
    id: i64,
    subject_id: i64,
    hashes: Box<[u32]>,
}

struct SeriesEntry {
    id: i64,
    study_id: i64,
    /// The detail table the hashes past the series row belong to.
    level: Option<Level>,
    /// The series row's hashes, then the detail row's.
    hashes: Box<[u32]>,
}

/// How one parsed file was filed.
struct Filed {
    status: &'static str,
    instance_id: i64,
    /// The file is the instance's own: `instance.source_file_id` points back.
    own: bool,
    /// The instance an earlier run filed this path under, now another one.
    orphan: Option<i64>,
}

pub struct Writer<'a> {
    registry: &'a mut Registry,
    /// The pseudonym key: read from the key store, written nowhere (§7.2).
    key: Vec<u8>,
    scheme: Scheme,
    display_length: usize,
    source_id: i64,
    batch_id: i64,
    job_id: Option<i64>,
    subjects: LruCache<String, SubjectEntry>,
    studies: LruCache<String, StudyEntry>,
    series: LruCache<String, SeriesEntry>,
    subject_fields: Fields,
    study_fields: Fields,
    /// The series row alone, as the `series` table holds it.
    series_fields: Fields,
    /// The series row and the detail row, per modality.
    series_detail: HashMap<String, Fields>,
    /// The writer's own diagnostics, for the report.
    pub counts: Counts,
    pub written: Written,
    last_heartbeat: Instant,
}

impl Drop for Writer<'_> {
    fn drop(&mut self) {
        self.key.fill(0);
    }
}

impl<'a> Writer<'a> {
    pub fn new(
        registry: &'a mut Registry,
        source_id: i64,
        batch_id: i64,
        job_id: Option<i64>,
    ) -> Result<Writer<'a>, HomeError> {
        let key = registry.pseudonym_key()?;
        let meta = registry.meta();
        let scheme = meta.pseudonym_scheme;
        let display_length = meta.display_length;
        let cap = NonZeroUsize::new(CACHE_ROWS).unwrap_or(NonZeroUsize::MIN);
        Ok(Writer {
            registry,
            key,
            scheme,
            display_length,
            source_id,
            batch_id,
            job_id,
            subjects: LruCache::new(cap),
            studies: LruCache::new(cap),
            series: LruCache::new(cap),
            subject_fields: Fields::subject(),
            study_fields: Fields::study(),
            series_fields: Fields::of(&[Level::Series]),
            series_detail: HashMap::new(),
            counts: Counts::default(),
            written: Written {
                batch_id,
                ..Written::default()
            },
            last_heartbeat: Instant::now(),
        })
    }

    /// Write one batch in one transaction; nothing of it lands on an error.
    pub fn write(&mut self, batch: &Batch, progress: &Progress) -> Result<(), HomeError> {
        self.registry.store().begin()?;
        match self.write_rows(batch, progress) {
            Ok(()) => {
                self.registry.store().commit()?;
                self.written.writes += 1;
                Ok(())
            }
            Err(e) => {
                let _ = self.registry.store().rollback();
                Err(e)
            }
        }
    }

    fn write_rows(&mut self, batch: &Batch, progress: &Progress) -> Result<(), HomeError> {
        let now = now_iso();
        // this batch's diagnostics, for the diagnostic table
        let mut tally = Counts::default();
        let parsed: Vec<&ParsedFile> = batch
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Parsed(p) => Some(&**p),
                _ => None,
            })
            .collect();
        for p in &parsed {
            for d in &p.extracted.diagnostics {
                tally.diagnostic(d);
            }
        }
        for i in &batch.items {
            if let Item::WalkError { error } = i {
                tally.walk_error(error);
            }
        }
        let subject_ids = self.subjects(&parsed, &now, &mut tally)?;
        let study_ids = self.studies(&parsed, &subject_ids, &mut tally)?;
        let series_ids = self.series(&parsed, &study_ids, &subject_ids, &mut tally)?;
        let filed = self.instances(&parsed, &series_ids, &mut tally)?;
        self.source_files(batch, &filed, &now, progress)?;
        self.diagnostics(&tally, &now)?;
        self.written.epoch = self.registry.next_epoch()?;
        Ok(())
    }

    /// Subjects: the code of every file's identity, a row for each code the
    /// registry does not hold, the fields of a known row compared.
    fn subjects(
        &mut self,
        parsed: &[&ParsedFile],
        now: &str,
        tally: &mut Counts,
    ) -> Result<Vec<i64>, HomeError> {
        let t = table("subject");
        let mut codes: Vec<pseudonym::Code> = Vec::with_capacity(parsed.len());
        let mut hashes: Vec<Box<[u32]>> = Vec::with_capacity(parsed.len());
        let mut rows = Vec::new();
        // the codes asked of the store this batch, with the first file's hashes
        let mut pending: HashMap<String, Box<[u32]>> = HashMap::new();
        for p in parsed {
            let x = &p.extracted;
            let (ident, _) = identity_of(x);
            let code = pseudonym::code(self.scheme, &self.key, ident, self.display_length);
            let h: Box<[u32]> = x.row(Level::Subject).map(|(_, v)| hash_value(v)).collect();
            if !self.subjects.contains(&code.code) && !pending.contains_key(&code.code) {
                let mut row = vec![
                    Param::from(code.code.as_str()),
                    Param::Bytes(code.digest.clone()),
                ];
                row.extend(x.row(Level::Subject).map(|(_, v)| Param::from(v)));
                row.push(Param::Int(self.batch_id));
                row.push(Param::from(now));
                rows.push(row);
                pending.insert(code.code.clone(), h.clone());
            }
            codes.push(code);
            hashes.push(h);
        }
        if !rows.is_empty() {
            let spec = Insert::all(t)
                .on_conflict(Conflict::Nothing(&["code"]))
                .returning(&["id", "code"]);
            let returned = self.registry.store().insert(&spec, &rows)?;
            for r in &returned {
                let code = r.text(1)?;
                let hashes = pending.remove(code).unwrap_or_default();
                self.subjects.put(
                    code.to_string(),
                    SubjectEntry {
                        id: r.int(0)?,
                        hashes,
                    },
                );
            }
            self.written.subjects_created += returned.len() as u64;
            let missing: Vec<String> = pending.keys().cloned().collect();
            if !missing.is_empty() {
                let cols = columns(t, &["id", "code"], &self.subject_fields);
                let found = self
                    .registry
                    .store()
                    .select_by_keys(t, &cols, "code", &missing)?;
                for r in &found {
                    self.subjects.put(
                        r.text(1)?.to_string(),
                        SubjectEntry {
                            id: r.int(0)?,
                            hashes: self.subject_fields.hash_cells(&r.0[2..]),
                        },
                    );
                }
            }
        }
        let mut ids = Vec::with_capacity(parsed.len());
        let mut diags = Vec::new();
        for ((p, code), h) in parsed.iter().zip(&codes).zip(&hashes) {
            let entry = self
                .subjects
                .get_mut(&code.code)
                .ok_or_else(|| missing_row("subject"))?;
            ids.push(entry.id);
            for (i, _) in self.subject_fields.differing(h, &entry.hashes) {
                diags.push(disagreement(
                    DiagnosticKind::SubjectFieldDisagreement,
                    &self.subject_fields,
                    i,
                    &p.extracted,
                ));
            }
            fill(
                self.registry.store(),
                &self.subject_fields,
                h,
                &mut entry.hashes,
                entry.id,
                &p.extracted,
            )?;
        }
        self.note(tally, diags);
        Ok(ids)
    }

    /// Studies: a row per study UID the registry does not hold, filed under
    /// the first file's subject; a known row's subject and fields compared.
    fn studies(
        &mut self,
        parsed: &[&ParsedFile],
        subject_ids: &[i64],
        tally: &mut Counts,
    ) -> Result<Vec<i64>, HomeError> {
        let t = table("study");
        let mut rows = Vec::new();
        let mut pending: HashMap<String, (i64, Box<[u32]>)> = HashMap::new();
        for (p, &subject_id) in parsed.iter().zip(subject_ids) {
            let x = &p.extracted;
            if self.studies.contains(&x.study_uid) || pending.contains_key(&x.study_uid) {
                continue;
            }
            let mut row = vec![Param::from(x.study_uid.as_str()), Param::Int(subject_id)];
            row.extend(x.row(Level::Study).map(|(_, v)| Param::from(v)));
            row.push(Param::Int(self.batch_id));
            rows.push(row);
            pending.insert(x.study_uid.clone(), (subject_id, p.hashes.study.clone()));
        }
        if !rows.is_empty() {
            let spec = Insert::all(t)
                .on_conflict(Conflict::Nothing(&["study_instance_uid"]))
                .returning(&["id", "study_instance_uid"]);
            let returned = self.registry.store().insert(&spec, &rows)?;
            for r in &returned {
                let uid = r.text(1)?;
                let (subject_id, hashes) = pending.remove(uid).unwrap_or_default();
                self.studies.put(
                    uid.to_string(),
                    StudyEntry {
                        id: r.int(0)?,
                        subject_id,
                        hashes,
                    },
                );
            }
            self.written.studies_created += returned.len() as u64;
            let missing: Vec<String> = pending.keys().cloned().collect();
            if !missing.is_empty() {
                let cols = columns(
                    t,
                    &["id", "study_instance_uid", "subject_id"],
                    &self.study_fields,
                );
                let found = self.registry.store().select_by_keys(
                    t,
                    &cols,
                    "study_instance_uid",
                    &missing,
                )?;
                for r in &found {
                    self.studies.put(
                        r.text(1)?.to_string(),
                        StudyEntry {
                            id: r.int(0)?,
                            subject_id: r.int(2)?,
                            hashes: self.study_fields.hash_cells(&r.0[3..]),
                        },
                    );
                }
            }
        }
        let mut ids = Vec::with_capacity(parsed.len());
        let mut diags = Vec::new();
        for (p, &subject_id) in parsed.iter().zip(subject_ids) {
            let x = &p.extracted;
            let entry = self
                .studies
                .get_mut(&x.study_uid)
                .ok_or_else(|| missing_row("study"))?;
            ids.push(entry.id);
            if entry.subject_id != subject_id {
                diags.push(Diagnostic::new(
                    DiagnosticKind::FieldDisagreement,
                    "study.subject_id",
                ));
            }
            for (i, _) in self.study_fields.differing(&p.hashes.study, &entry.hashes) {
                diags.push(disagreement(
                    DiagnosticKind::FieldDisagreement,
                    &self.study_fields,
                    i,
                    x,
                ));
            }
            fill(
                self.registry.store(),
                &self.study_fields,
                &p.hashes.study,
                &mut entry.hashes,
                entry.id,
                x,
            )?;
        }
        self.note(tally, diags);
        Ok(ids)
    }

    /// Series: a row per series UID the registry does not hold, its detail
    /// row beside it; a known row's study and fields compared.
    fn series(
        &mut self,
        parsed: &[&ParsedFile],
        study_ids: &[i64],
        subject_ids: &[i64],
        tally: &mut Counts,
    ) -> Result<Vec<i64>, HomeError> {
        let t = table("series");
        let mut rows = Vec::new();
        // series UID → the first file of the batch in it
        let mut pending: HashMap<String, usize> = HashMap::new();
        for (i, p) in parsed.iter().enumerate() {
            let x = &p.extracted;
            if self.series.contains(&x.series_uid) || pending.contains_key(&x.series_uid) {
                continue;
            }
            let mut row = vec![
                Param::from(x.series_uid.as_str()),
                Param::Int(study_ids[i]),
                Param::Int(subject_ids[i]),
            ];
            row.extend(x.row(Level::Series).map(|(_, v)| Param::from(v)));
            row.extend([Param::Int(0), Param::Int(0), Param::Int(self.batch_id)]);
            rows.push(row);
            pending.insert(x.series_uid.clone(), i);
        }
        if !rows.is_empty() {
            let spec = Insert::all(t)
                .on_conflict(Conflict::Nothing(&["series_instance_uid"]))
                .returning(&["id", "series_instance_uid"]);
            let returned = self.registry.store().insert(&spec, &rows)?;
            let mut detail: BTreeMap<Level, Vec<Vec<Param>>> = BTreeMap::new();
            for r in &returned {
                let uid = r.text(1)?;
                let id = r.int(0)?;
                let Some(i) = pending.remove(uid) else {
                    continue;
                };
                let p = parsed[i];
                let x = &p.extracted;
                self.series.put(
                    uid.to_string(),
                    SeriesEntry {
                        id,
                        study_id: study_ids[i],
                        level: detail_level(&x.modality),
                        hashes: p.hashes.series.clone(),
                    },
                );
                if let Some(level) = detail_level(&x.modality) {
                    let mut row = vec![Param::Int(id)];
                    row.extend(x.row(level).map(|(_, v)| Param::from(v)));
                    detail.entry(level).or_default().push(row);
                }
            }
            self.written.series_created += returned.len() as u64;
            for (level, rows) in &detail {
                let spec =
                    Insert::all(table(level.name())).on_conflict(Conflict::Nothing(&["series_id"]));
                self.registry.store().insert(&spec, rows)?;
            }
            if !pending.is_empty() {
                let levels: HashMap<String, Option<Level>> = pending
                    .iter()
                    .map(|(uid, &i)| (uid.clone(), detail_level(&parsed[i].extracted.modality)))
                    .collect();
                self.fetch_series(levels)?;
            }
        }
        let mut ids = Vec::with_capacity(parsed.len());
        let mut diags = Vec::new();
        for (i, p) in parsed.iter().enumerate() {
            let x = &p.extracted;
            let entry = self
                .series
                .get_mut(&x.series_uid)
                .ok_or_else(|| missing_row("series"))?;
            ids.push(entry.id);
            if entry.study_id != study_ids[i] {
                diags.push(Diagnostic::new(
                    DiagnosticKind::SeriesMultiStudy,
                    "series.study_id",
                ));
            }
            let fields = self
                .series_detail
                .entry(x.modality.clone())
                .or_insert_with(|| Fields::series(&x.modality));
            // a file of another modality than the row's is compared on the
            // series columns only; its detail table is not the row's
            let width = if entry.level == detail_level(&x.modality) {
                fields.len()
            } else {
                self.series_fields.len()
            };
            let mine = &p.hashes.series[..width];
            let theirs = &mut entry.hashes[..width];
            for (j, _) in fields.differing(mine, theirs) {
                diags.push(disagreement(
                    DiagnosticKind::FieldDisagreement,
                    fields,
                    j,
                    x,
                ));
            }
            fill(self.registry.store(), fields, mine, theirs, entry.id, x)?;
        }
        self.note(tally, diags);
        Ok(ids)
    }

    /// Read the series rows the batch met that the cache did not hold, with
    /// their detail rows, and cache them; `levels` says which detail table
    /// each file expects.
    fn fetch_series(&mut self, levels: HashMap<String, Option<Level>>) -> Result<(), HomeError> {
        let t = table("series");
        let missing: Vec<String> = levels.keys().cloned().collect();
        let cols = columns(
            t,
            &["id", "series_instance_uid", "study_id"],
            &self.series_fields,
        );
        let found =
            self.registry
                .store()
                .select_by_keys(t, &cols, "series_instance_uid", &missing)?;
        struct Base {
            uid: String,
            id: i64,
            study_id: i64,
            hashes: Vec<u32>,
            level: Option<Level>,
        }
        let mut bases = Vec::with_capacity(found.len());
        let mut by_level: BTreeMap<Level, Vec<i64>> = BTreeMap::new();
        for r in &found {
            let uid = r.text(1)?.to_string();
            let id = r.int(0)?;
            let level = levels.get(&uid).copied().flatten();
            if let Some(level) = level {
                by_level.entry(level).or_default().push(id);
            }
            bases.push(Base {
                uid,
                id,
                study_id: r.int(2)?,
                hashes: self.series_fields.hash_cells(&r.0[3..]).into_vec(),
                level,
            });
        }
        let mut details: HashMap<(Level, i64), Box<[u32]>> = HashMap::new();
        let mut widths: BTreeMap<Level, usize> = BTreeMap::new();
        for (level, ids) in &by_level {
            let fields = Fields::of(&[*level]);
            let dt = table(level.name());
            let cols = columns(dt, &["series_id"], &fields);
            let rows = self
                .registry
                .store()
                .select_by_ids(dt, &cols, "series_id", ids)?;
            for r in &rows {
                details.insert((*level, r.int(0)?), fields.hash_cells(&r.0[1..]));
            }
            widths.insert(*level, fields.len());
        }
        for mut b in bases {
            if let Some(level) = b.level {
                match details.remove(&(level, b.id)) {
                    Some(h) => b.hashes.extend(h.iter()),
                    // no detail row: every detail field reads as null
                    None => b
                        .hashes
                        .extend(std::iter::repeat_n(hash32(None), widths[&level])),
                }
            }
            self.series.put(
                b.uid,
                SeriesEntry {
                    id: b.id,
                    study_id: b.study_id,
                    level: b.level,
                    hashes: b.hashes.into_boxed_slice(),
                },
            );
        }
        Ok(())
    }

    /// Instances: a row per SOP instance UID the registry does not hold; the
    /// status of every file follows from whether its instance is new, its
    /// own from an earlier run, or another file's (§5.3).
    fn instances(
        &mut self,
        parsed: &[&ParsedFile],
        series_ids: &[i64],
        tally: &mut Counts,
    ) -> Result<Vec<Filed>, HomeError> {
        let t = table("instance");
        // SOP instance UID → the first file of the batch with it
        let mut first: HashMap<&str, usize> = HashMap::with_capacity(parsed.len());
        let mut rows = Vec::with_capacity(parsed.len());
        for (i, p) in parsed.iter().enumerate() {
            let x = &p.extracted;
            if first.contains_key(x.sop_uid.as_str()) {
                continue;
            }
            first.insert(&x.sop_uid, i);
            let mut row = vec![
                Param::from(x.sop_uid.as_str()),
                Param::Int(series_ids[i]),
                Param::Null,
            ];
            row.extend(x.row(Level::Instance).map(|(_, v)| Param::from(v)));
            row.extend([Param::Null, Param::Int(self.batch_id)]);
            rows.push(row);
        }
        let spec = Insert::all(t)
            .on_conflict(Conflict::Nothing(&["sop_instance_uid"]))
            .returning(&["id", "sop_instance_uid"]);
        let returned = self.registry.store().insert(&spec, &rows)?;
        // SOP instance UID → (id, created this batch)
        let mut ids: HashMap<String, (i64, bool)> = HashMap::with_capacity(rows.len());
        for r in &returned {
            ids.insert(r.text(1)?.to_string(), (r.int(0)?, true));
        }
        let missing: Vec<String> = first
            .keys()
            .filter(|uid| !ids.contains_key(**uid))
            .map(|uid| uid.to_string())
            .collect();
        if !missing.is_empty() {
            let cols = columns(t, &["id", "sop_instance_uid"], &Fields::of(&[]));
            let found =
                self.registry
                    .store()
                    .select_by_keys(t, &cols, "sop_instance_uid", &missing)?;
            for r in &found {
                ids.insert(r.text(1)?.to_string(), (r.int(0)?, false));
            }
        }
        let mut filed = Vec::with_capacity(parsed.len());
        let mut per_series: BTreeMap<i64, i64> = BTreeMap::new();
        let mut diags = Vec::new();
        for (i, p) in parsed.iter().enumerate() {
            let x = &p.extracted;
            let &(id, created) = ids
                .get(x.sop_uid.as_str())
                .ok_or_else(|| missing_row("instance"))?;
            let creator = created && first[x.sop_uid.as_str()] == i;
            let same = p.prior.is_some_and(|prior| prior.instance_id == Some(id));
            let (st, own) = if creator {
                *per_series.entry(series_ids[i]).or_default() += 1;
                (status::INGESTED, true)
            } else if same {
                (status::INGESTED, false)
            } else {
                (status::DUPLICATE, false)
            };
            if p.prior.is_some_and(|prior| prior.changed) {
                let subject = if creator {
                    "new_sop"
                } else if same {
                    "same_sop"
                } else {
                    "other_sop"
                };
                diags.push(Diagnostic::new(DiagnosticKind::FileChanged, subject));
            }
            filed.push(Filed {
                status: st,
                instance_id: id,
                own,
                orphan: p
                    .prior
                    .and_then(|prior| prior.instance_id)
                    .filter(|&old| old != id),
            });
        }
        let pairs: Vec<(i64, i64)> = per_series.into_iter().collect();
        if !pairs.is_empty() {
            self.registry.store().update_from_values(
                table("series"),
                "n_instances = n_instances + v.val",
                "id",
                &pairs,
            )?;
        }
        self.note(tally, diags);
        Ok(filed)
    }

    /// Source files: every item's row, upserted on `(source_id, path)`; then
    /// the new instances' `source_file_id`, and the instance an earlier run
    /// filed a changed path under let go of it.
    fn source_files(
        &mut self,
        batch: &Batch,
        filed: &[Filed],
        now: &str,
        progress: &Progress,
    ) -> Result<(), HomeError> {
        let t = table("source_file");
        let full = Insert::new(
            t,
            &[
                "source_id",
                "batch_id",
                "dir",
                "path",
                "size",
                "mtime_ns",
                "status",
                "reason",
                "detail",
                "instance_id",
                "seen_at",
            ],
        )
        .on_conflict(Conflict::Update {
            target: &["source_id", "path"],
            set: &[
                "batch_id",
                "size",
                "mtime_ns",
                "status",
                "reason",
                "detail",
                "instance_id",
                "seen_at",
            ],
        })
        .returning(&["id", "path"]);
        let light = Insert::new(
            t,
            &[
                "source_id",
                "batch_id",
                "dir",
                "path",
                "size",
                "mtime_ns",
                "status",
                "seen_at",
            ],
        )
        .on_conflict(Conflict::Update {
            target: &["source_id", "path"],
            set: &["batch_id", "seen_at"],
        });
        let row = |path: &str,
                   dir: &str,
                   size: u64,
                   mtime_ns: i64,
                   st: &str,
                   reason: Option<&str>,
                   detail: Option<&str>,
                   instance_id: Option<i64>| {
            vec![
                Param::Int(self.source_id),
                Param::Int(self.batch_id),
                Param::from(dir),
                Param::from(path),
                Param::Int(size as i64),
                Param::Int(mtime_ns),
                Param::from(st),
                Param::from(reason),
                Param::from(detail),
                Param::from(instance_id),
                Param::from(now),
            ]
        };
        let mut rows = Vec::with_capacity(batch.items.len());
        let mut light_rows = Vec::new();
        // path → (instance id, the file is its own, the instance it left)
        let mut wanted: HashMap<&str, (i64, bool, Option<i64>)> = HashMap::new();
        let mut ingested = 0;
        let mut next = filed.iter();
        for item in &batch.items {
            match item {
                Item::Parsed(p) => {
                    let f = next.next().ok_or_else(|| missing_row("instance"))?;
                    rows.push(row(
                        &p.path,
                        &p.dir,
                        p.size,
                        p.mtime_ns,
                        f.status,
                        None,
                        None,
                        Some(f.instance_id),
                    ));
                    if f.own || f.orphan.is_some() {
                        wanted.insert(&p.path, (f.instance_id, f.own, f.orphan));
                    }
                    if f.status == status::INGESTED {
                        ingested += 1;
                    } else {
                        self.written.duplicate += 1;
                    }
                    if p.prior.is_some_and(|prior| prior.changed) {
                        self.written.changed += 1;
                    }
                }
                Item::Refused {
                    path,
                    dir,
                    size,
                    mtime_ns,
                    refusal,
                } => rows.push(row(
                    path,
                    dir,
                    *size,
                    *mtime_ns,
                    status::QUARANTINED,
                    Some(refusal.class.name()),
                    refusal.detail.as_deref(),
                    None,
                )),
                Item::Skipped {
                    path,
                    dir,
                    size,
                    mtime_ns,
                    reason,
                } => rows.push(row(
                    path,
                    dir,
                    *size,
                    *mtime_ns,
                    status::SKIPPED,
                    Some(reason.name()),
                    None,
                    None,
                )),
                Item::Unchanged {
                    path,
                    dir,
                    size,
                    mtime_ns,
                    status,
                    quarantined,
                } => {
                    light_rows.push(vec![
                        Param::Int(self.source_id),
                        Param::Int(self.batch_id),
                        Param::from(dir.as_str()),
                        Param::from(path.as_str()),
                        Param::Int(*size as i64),
                        Param::Int(*mtime_ns),
                        Param::from(*status),
                        Param::from(now),
                    ]);
                    if *quarantined {
                        self.written.quarantine_kept += 1;
                    }
                }
                Item::WalkError { .. } => {}
            }
        }
        self.written.ingested += ingested;
        let store = self.registry.store();
        let returned = store.insert(&full, &rows)?;
        let mut pairs = Vec::new();
        let mut orphans = Vec::new();
        for r in &returned {
            if let Some(&(instance_id, own, orphan)) = wanted.get(r.text(1)?) {
                let file_id = r.int(0)?;
                if own {
                    pairs.push((instance_id, file_id));
                }
                if let Some(old) = orphan {
                    orphans.push((old, file_id));
                }
            }
        }
        store.insert(&light, &light_rows)?;
        if !pairs.is_empty() {
            store.update_from_values(table("instance"), "source_file_id = v.val", "id", &pairs)?;
        }
        if !orphans.is_empty() {
            let d = store.dialect();
            let sql = format!(
                "UPDATE {} SET source_file_id = NULL WHERE id = {} AND source_file_id = {}",
                store.qualified("instance"),
                d.param(1, Type::Int),
                d.param(2, Type::Int)
            );
            for (old, file_id) in orphans {
                store.execute(&sql, &[Param::Int(old), Param::Int(file_id)])?;
            }
        }
        progress.ingested(ingested);
        Ok(())
    }

    /// The batch's diagnostics: one row per kind, with its samples.
    fn diagnostics(&mut self, tally: &Counts, now: &str) -> Result<(), HomeError> {
        let rows: Vec<Vec<Param>> = tally
            .diagnostic_rows()
            .map(|(kind, count, samples)| {
                let samples: Vec<&String> = samples.iter().collect();
                vec![
                    Param::Int(self.batch_id),
                    Param::from(kind.name()),
                    Param::from("batch"),
                    Param::Null,
                    Param::Int(count as i64),
                    Param::from(serde_json::to_string(&samples).unwrap_or_default()),
                    Param::from(now),
                ]
            })
            .collect();
        if !rows.is_empty() {
            self.registry
                .store()
                .insert(&Insert::all(table("diagnostic")), &rows)?;
        }
        Ok(())
    }

    /// The writer's own diagnostics go to the batch's table and the report.
    fn note(&mut self, tally: &mut Counts, diags: Vec<Diagnostic>) {
        for d in diags {
            tally.diagnostic(&d);
            self.counts.diagnostic(&d);
        }
    }

    /// The job's heartbeat (§10): every ten seconds unless forced, with the
    /// progress counters beside it.
    pub fn heartbeat(&mut self, progress: &Progress, force: bool) -> Result<(), HomeError> {
        let Some(job_id) = self.job_id else {
            return Ok(());
        };
        if !force && self.last_heartbeat.elapsed() < PROGRESS_EVERY {
            return Ok(());
        }
        let mut json = progress.json();
        json["batch_id"] = self.batch_id.into();
        json["epoch"] = self.written.epoch.into();
        json["writes"] = self.written.writes.into();
        self.registry.store().update_by_id(
            table("job"),
            &[
                ("heartbeat_at", Param::from(now_iso())),
                ("progress", Param::from(json.to_string())),
            ],
            "id",
            job_id,
        )?;
        self.last_heartbeat = Instant::now();
        Ok(())
    }
}

/// The writer's loop: every batch the parsers send, a heartbeat between them
/// while they are quiet, until the last parser is done.
pub fn run(
    writer: &mut Writer<'_>,
    rx: &Receiver<Batch>,
    progress: &Progress,
) -> Result<(), HomeError> {
    loop {
        match rx.recv_timeout(PROGRESS_EVERY) {
            Ok(batch) => {
                writer.write(&batch, progress)?;
                writer.heartbeat(progress, false)?;
            }
            Err(RecvTimeoutError::Timeout) => writer.heartbeat(progress, false)?,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

/// The fixed columns, then the catalogue columns of `fields`, of `t`.
fn columns<'t>(t: &'t Table, fixed: &[&str], fields: &Fields) -> Vec<&'t Column> {
    fixed
        .iter()
        .copied()
        .chain(fields.names.iter().copied())
        .map(|n| {
            t.column(n)
                .unwrap_or_else(|| panic!("{}.{n} is not a column", t.name))
        })
        .collect()
}

/// A field of a known row that a file disagrees with: `table.field`, with
/// the shape of the file's value.
fn disagreement(
    kind: DiagnosticKind,
    fields: &Fields,
    i: usize,
    x: &nils_dicom::Extracted,
) -> Diagnostic {
    let d = Diagnostic::new(kind, fields.label(i));
    match x.value(fields.levels[i], fields.names[i]) {
        Some(v) => d.with_shape(&text_of(v)),
        None => d,
    }
}

fn text_of(v: &Value) -> String {
    v.to_string()
}

/// Fill the stored nulls of a row that a later file carries values for
/// (§9.1: a null is no value, so the first value is the first file that has
/// one), and note the values in the cached hashes.
fn fill(
    store: &mut Store,
    fields: &Fields,
    mine: &[u32],
    theirs: &mut [u32],
    id: i64,
    x: &nils_dicom::Extracted,
) -> Result<(), HomeError> {
    for i in fields.fillable(mine, theirs) {
        let level = fields.levels[i];
        let (t, key) = match level {
            Level::Subject | Level::Study | Level::Series => (table(level.name()), "id"),
            _ => (table(level.name()), "series_id"),
        };
        let value = x.value(level, fields.names[i]);
        store.update_by_id(t, &[(fields.names[i], Param::from(value))], key, id)?;
        theirs[i] = mine[i];
    }
    Ok(())
}

fn missing_row(what: &str) -> HomeError {
    HomeError::Message(format!(
        "a {what} row was neither inserted nor found; the store changed under the writer"
    ))
}
