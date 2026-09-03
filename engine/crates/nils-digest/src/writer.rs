// SPDX-License-Identifier: AGPL-3.0-only

//! The writer (§9.1): one thread, one transaction per batch, rows in the
//! order subjects, studies, series with their detail rows, stacks, instances,
//! source files, diagnostics, then the epoch. The caches keep the rows a batch
//! is likely to meet again; a miss costs one keyed select for the whole batch,
//! never a query per file.
//!
//! A row that exists is not replaced, and its fields do not depend on the
//! order the files reached the writer: a null is filled by the first file
//! that carries a value, and a field two files disagree on keeps the smaller
//! value in the canonical text order (§9.1). A disagreement over a field the
//! catalogue compares also raises a `field_disagreement` naming it.
//!
//! Subjects are resolved through the linkage store (§7.4): the lookup of a
//! file's identifier names its subject when the store has met it; otherwise
//! the code is derived and the subject created, or found by its code and the
//! identity attached, or refused as a collision. The identity rows are filed
//! after the registry's transaction commits (§9.3).

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::time::Instant;

use crossbeam_channel::{Receiver, RecvTimeoutError};
use lru::LruCache;
use nils_dicom::{Diagnostic, DiagnosticKind, Level, Value};
use nils_registry::dialect::Conflict;
use nils_registry::linkage::{self, NewIdentity, Subkeys};
use nils_registry::schema::{Column, Table, Type, table};
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::now_iso;
use nils_registry::{HomeError, Registry, Scheme, pseudonym};

use crate::batch::{
    Batch, Fields, Item, ParsedFile, canonical_cell, canonical_value, detail_level, hash_value,
    hash32,
};
use crate::cancel::{Cancel, Scripted};
use crate::progress::{PROGRESS_EVERY, Progress};
use crate::report::{Counts, Written};
use crate::resume::status;
use crate::rule::{FALLBACK_ID_TYPE, Rule};

/// Rows each of the writer's caches holds (§9.1).
pub const CACHE_ROWS: usize = 200_000;

/// The kind of the review item a collision opens (§7.1).
pub const COLLISION_KIND: &str = "identity.collision";
/// The review item that groups the files a batch quarantined into one class
/// (§5.3): one per batch and class, the count as evidence, no path in it.
pub const QUARANTINE_KIND: &str = "ingest.quarantine";

/// The error a batch ends with when an abort is asked while it is in flight
/// (§10): its transaction rolls back, and the run ends as aborted.
pub const ABORTED: &str = "aborted: the batch in flight rolled back";

struct SubjectEntry {
    hashes: Box<[u32]>,
    kept: Kept,
}

/// The canonical text of the fields of a cached row that instances have
/// disagreed on, read back from the row once and kept with it, so that
/// deciding a field (§9.1) costs one read per row and field however many
/// instances disagree on it. A row nobody disagrees about carries a null
/// pointer and nothing more, which is the common case.
#[derive(Default)]
// the box is the point: eight bytes on a row nobody disagrees about, against
// the map's forty-eight
#[allow(clippy::box_collection)]
struct Kept(Option<Box<HashMap<u16, Box<str>>>>);

impl Kept {
    fn get(&self, i: usize) -> Option<&str> {
        self.0.as_ref()?.get(&(i as u16)).map(|s| &**s)
    }

    fn set(&mut self, i: usize, text: &str) {
        self.0
            .get_or_insert_with(Default::default)
            .insert(i as u16, text.into());
    }
}

/// An id type the rule files under, with its row in the linkage store.
struct IdType {
    name: String,
    id: i64,
}

/// A collision met in a batch (§7.4 step 5): the review item to open once
/// the batch is rolled back.
struct Collision {
    code: String,
    subject_id: Option<i64>,
    id_type: String,
    /// `identity`: the subject holds another identifier of the type;
    /// `display-code`: its digest is another one's (blake2b-32, §7.1);
    /// `batch`: two identifiers of this batch derive the one code.
    reason: &'static str,
}

struct StudyEntry {
    id: i64,
    subject_id: i64,
    hashes: Box<[u32]>,
    kept: Kept,
}

struct SeriesEntry {
    id: i64,
    study_id: i64,
    /// The detail table the hashes past the series row belong to.
    level: Option<Level>,
    /// The series row's hashes, then the detail row's.
    hashes: Box<[u32]>,
    kept: Kept,
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
    /// The linkage store, on its own connection (§9.3).
    linkage: Store,
    /// The pseudonym key: read from the key store, written nowhere (§7.2).
    key: Vec<u8>,
    /// The subkeys of the linkage store, derived once (§7.2).
    keys: Subkeys,
    scheme: Scheme,
    display_length: usize,
    /// The rule's id type and the fallback's.
    id_type: IdType,
    fallback: IdType,
    source_id: i64,
    batch_id: i64,
    job_id: Option<i64>,
    /// Lookup → subject, for the identities met (§7.4 step 3).
    identities: LruCache<Vec<u8>, i64>,
    /// Subject id → the row's field hashes.
    subjects: LruCache<i64, SubjectEntry>,
    studies: LruCache<String, StudyEntry>,
    series: LruCache<String, SeriesEntry>,
    /// `(series id, stack key)` → stack id (§8).
    stacks: LruCache<(i64, String), i64>,
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
    /// The identity rows of the batch in flight, filed once it commits.
    pending: Vec<NewIdentity>,
    collision: Option<Collision>,
    /// The run's stop token (§10), checked between the tables of a batch.
    cancel: Cancel,
    /// The stop the tests script, acting on the count of commits.
    script: Option<Scripted>,
}

impl Drop for Writer<'_> {
    fn drop(&mut self) {
        self.key.fill(0);
    }
}

impl<'a> Writer<'a> {
    pub fn new(
        registry: &'a mut Registry,
        rule: &Rule,
        source_id: i64,
        batch_id: i64,
        job_id: Option<i64>,
    ) -> Result<Writer<'a>, HomeError> {
        let key = registry.pseudonym_key()?;
        let keys = Subkeys::derive(&key);
        let mut linkage = registry.open_linkage()?;
        let id_type = id_type_of(&mut linkage, &rule.id_type)?;
        let fallback = id_type_of(&mut linkage, FALLBACK_ID_TYPE)?;
        let meta = registry.meta();
        let scheme = meta.pseudonym_scheme;
        let display_length = meta.display_length;
        let cap = NonZeroUsize::new(CACHE_ROWS).unwrap_or(NonZeroUsize::MIN);
        Ok(Writer {
            registry,
            linkage,
            key,
            keys,
            scheme,
            display_length,
            id_type,
            fallback,
            source_id,
            batch_id,
            job_id,
            identities: LruCache::new(cap),
            subjects: LruCache::new(cap),
            studies: LruCache::new(cap),
            series: LruCache::new(cap),
            stacks: LruCache::new(cap),
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
            pending: Vec::new(),
            collision: None,
            cancel: Cancel::new(),
            script: None,
        })
    }

    /// The run's stop token, and the scripted stop of a test if there is one.
    pub fn cancelled_by(mut self, cancel: Cancel, script: Option<Scripted>) -> Writer<'a> {
        self.cancel = cancel;
        self.script = script;
        self
    }

    /// Write one batch in one transaction; nothing of it lands on an error.
    /// The identity rows follow in the linkage store once the registry has
    /// committed (§9.3); a collision opens its review item after the
    /// rollback, in a transaction of its own, and the error stands. An abort
    /// asked while the batch is in flight ends it with [`ABORTED`] at the
    /// next table.
    pub fn write(&mut self, batch: &Batch, progress: &Progress) -> Result<(), HomeError> {
        self.registry.store().begin()?;
        match self.write_rows(batch, progress) {
            Ok(()) => {
                if let Some(s) = self.script {
                    s.inside_transaction(self.written.writes);
                }
                self.registry.store().commit()?;
                self.written.writes += 1;
                if let Some(s) = self.script {
                    s.after_commit(self.written.writes, &self.cancel);
                }
                self.file_identities()
            }
            Err(e) => {
                let _ = self.registry.store().rollback();
                self.pending.clear();
                if let Some(c) = self.collision.take() {
                    let id = self.open_review(&c)?;
                    return Err(HomeError::Message(collision_message(&c, id)));
                }
                Err(e)
            }
        }
    }

    /// The identity rows of the batch just committed (§9.3).
    fn file_identities(&mut self) -> Result<(), HomeError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let rows = std::mem::take(&mut self.pending);
        self.linkage.begin()?;
        match linkage::insert_identities(&mut self.linkage, &rows) {
            Ok(_) => {
                self.linkage.commit()?;
                Ok(())
            }
            Err(e) => {
                let _ = self.linkage.rollback();
                Err(e.into())
            }
        }
    }

    /// The review item of a collision (§7.1): the subject and the type, never
    /// an identifier.
    fn open_review(&mut self, c: &Collision) -> Result<i64, HomeError> {
        let now = now_iso();
        let reference = serde_json::json!({ "subject_id": c.subject_id, "code": c.code });
        let evidence = serde_json::json!({
            "id_type": c.id_type,
            "reason": c.reason,
            "scheme": self.scheme.to_string(),
            "display_length": self.display_length,
            "batch_id": self.batch_id,
        });
        let store = self.registry.store();
        store.begin()?;
        let result = store.insert(
            &Insert::new(
                table("review_item"),
                &["kind", "scope", "ref", "evidence", "status", "created_at"],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(COLLISION_KIND),
                Param::from("subject"),
                Param::from(reference.to_string()),
                Param::from(evidence.to_string()),
                Param::from("open"),
                Param::from(now.as_str()),
            ]],
        );
        match result {
            Ok(rows) => {
                store.commit()?;
                Ok(rows.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
            }
            Err(e) => {
                let _ = store.rollback();
                Err(e.into())
            }
        }
    }

    /// The id type a file's identifier is filed under.
    fn type_of(&self, p: &ParsedFile) -> &IdType {
        if p.ident.fell_back {
            &self.fallback
        } else {
            &self.id_type
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
        self.checkpoint()?;
        let subject_ids = self.subjects(&parsed, &now, &mut tally)?;
        self.checkpoint()?;
        let study_ids = self.studies(&parsed, &subject_ids, &mut tally)?;
        self.checkpoint()?;
        let series_ids = self.series(&parsed, &study_ids, &subject_ids, &mut tally)?;
        self.checkpoint()?;
        let stack_ids = self.stacks(&parsed, &series_ids, &mut tally)?;
        self.checkpoint()?;
        let filed = self.instances(&parsed, &series_ids, &stack_ids, &mut tally)?;
        self.checkpoint()?;
        self.source_files(batch, &filed, &now, progress)?;
        self.checkpoint()?;
        self.diagnostics(&tally, &now)?;
        self.written.epoch = self.registry.next_epoch()?;
        Ok(())
    }

    /// Between two tables of a batch: the place an abort takes effect.
    fn checkpoint(&self) -> Result<(), HomeError> {
        if self.cancel.abort() {
            Err(HomeError::Message(ABORTED.into()))
        } else {
            Ok(())
        }
    }

    /// Subjects (§7.4): every file's identifier resolved through the linkage
    /// store, a row for each identifier no subject holds, the fields of a
    /// known row compared.
    fn subjects(
        &mut self,
        parsed: &[&ParsedFile],
        now: &str,
        tally: &mut Counts,
    ) -> Result<Vec<i64>, HomeError> {
        let ids = self.resolve(parsed, now)?;
        // the field hashes of every subject met, read for those not cached
        let mut fetch: Vec<i64> = Vec::new();
        for &id in &ids {
            if !self.subjects.contains(&id) && !fetch.contains(&id) {
                fetch.push(id);
            }
        }
        if !fetch.is_empty() {
            let t = table("subject");
            let cols = columns(t, &["id"], &self.subject_fields);
            let found = self
                .registry
                .store()
                .select_by_ids(t, &cols, "id", &fetch)?;
            for r in &found {
                self.subjects.put(
                    r.int(0)?,
                    SubjectEntry {
                        hashes: self.subject_fields.hash_cells(&r.0[1..]),
                        kept: Kept::default(),
                    },
                );
            }
        }
        let mut diags = Vec::new();
        for (p, &id) in parsed.iter().zip(&ids) {
            let x = &p.extracted;
            let h: Box<[u32]> = x.row(Level::Subject).map(|(_, v)| hash_value(v)).collect();
            let entry = self
                .subjects
                .get_mut(&id)
                .ok_or_else(|| missing_row("subject"))?;
            for (i, _) in self.subject_fields.differing(&h, &entry.hashes) {
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
                &h,
                &mut entry.hashes,
                id,
                &p.extracted,
            )?;
            resolve(
                self.registry.store(),
                &self.subject_fields,
                &h,
                &mut entry.hashes,
                &mut entry.kept,
                id,
                &p.extracted,
            )?;
        }
        self.note(tally, diags);
        Ok(ids)
    }

    /// The subject of every file (§7.4): by the lookup of its identifier
    /// when the linkage store has met it (step 3); else by the code, created
    /// (step 4), or found and the identity attached, or a collision (step 5).
    fn resolve(&mut self, parsed: &[&ParsedFile], now: &str) -> Result<Vec<i64>, HomeError> {
        let t = table("subject");
        let n = parsed.len();
        let mut ids: Vec<Option<i64>> = vec![None; n];
        // the lookups the cache does not hold, with the first file of each
        let mut misses: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut lookups: Vec<Vec<u8>> = Vec::with_capacity(n);
        for (i, p) in parsed.iter().enumerate() {
            let lookup = self.keys.lookup(&self.type_of(p).name, &p.ident.value);
            match self.identities.get(&lookup) {
                Some(&id) => ids[i] = Some(id),
                None => {
                    misses.entry(lookup.clone()).or_insert(i);
                }
            }
            lookups.push(lookup);
        }
        if !misses.is_empty() {
            let keys: Vec<Vec<u8>> = misses.keys().cloned().collect();
            for row in linkage::identities_by_lookup(&mut self.linkage, &keys)? {
                self.identities.put(row.lookup.clone(), row.subject_id);
                self.written.subjects_matched += 1;
                misses.remove(&row.lookup);
            }
        }
        // the rest are identifiers no subject holds: by code, deduplicated
        // in the batch; two identifiers of one type on one code collide
        struct Group {
            digest: Vec<u8>,
            /// The misses on the code: (lookup, first file).
            members: Vec<(Vec<u8>, usize)>,
        }
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for (lookup, &i) in &misses {
            let p = parsed[i];
            let code = pseudonym::code(self.scheme, &self.key, &p.ident.value, self.display_length);
            let g = groups.entry(code.code).or_insert_with(|| Group {
                digest: code.digest,
                members: Vec::new(),
            });
            g.members.push((lookup.clone(), i));
        }
        for (code, g) in &mut groups {
            g.members.sort_by_key(|(_, i)| *i);
            for (k, (_, i)) in g.members.iter().enumerate() {
                let ty = &self.type_of(parsed[*i]).name;
                if g.members[..k]
                    .iter()
                    .any(|(_, j)| &self.type_of(parsed[*j]).name == ty)
                {
                    self.collision = Some(Collision {
                        code: code.clone(),
                        subject_id: None,
                        id_type: ty.clone(),
                        reason: "batch",
                    });
                    return Err(HomeError::Message("identity collision".into()));
                }
            }
        }
        if !groups.is_empty() {
            let mut rows = Vec::with_capacity(groups.len());
            for (code, g) in &groups {
                let x = &parsed[g.members[0].1].extracted;
                let mut row = vec![Param::from(code.as_str()), Param::Bytes(g.digest.clone())];
                row.extend(x.row(Level::Subject).map(|(_, v)| Param::from(v)));
                row.push(Param::Int(self.batch_id));
                row.push(Param::from(now));
                rows.push(row);
            }
            let spec = Insert::all(t)
                .on_conflict(Conflict::Nothing(&["code"]))
                .returning(&["id", "code"]);
            let returned = self.registry.store().insert(&spec, &rows)?;
            // code → subject id, for the created and then the found
            let mut by_code: HashMap<String, i64> = HashMap::with_capacity(groups.len());
            for r in &returned {
                let id = r.int(0)?;
                let code = r.text(1)?;
                by_code.insert(code.to_string(), id);
                if let Some(g) = groups.get(code) {
                    let x = &parsed[g.members[0].1].extracted;
                    self.subjects.put(
                        id,
                        SubjectEntry {
                            hashes: x.row(Level::Subject).map(|(_, v)| hash_value(v)).collect(),
                            kept: Kept::default(),
                        },
                    );
                    for (lookup, i) in &g.members {
                        self.attach(id, parsed[*i], lookup.clone());
                    }
                }
            }
            self.written.subjects_created += returned.len() as u64;
            let existing: Vec<String> = groups
                .keys()
                .filter(|c| !by_code.contains_key(*c))
                .cloned()
                .collect();
            if !existing.is_empty() {
                let cols = columns(t, &["id", "code", "code_digest"], &Fields::of(&[]));
                let found = self
                    .registry
                    .store()
                    .select_by_keys(t, &cols, "code", &existing)?;
                let mut subject_ids = Vec::with_capacity(found.len());
                let mut digests: HashMap<i64, Option<Vec<u8>>> = HashMap::new();
                for r in &found {
                    let id = r.int(0)?;
                    by_code.insert(r.text(1)?.to_string(), id);
                    digests.insert(id, r.opt_bytes(2)?.map(<[u8]>::to_vec));
                    subject_ids.push(id);
                }
                let held = linkage::identities_of_subjects(&mut self.linkage, &subject_ids)?;
                for code in &existing {
                    let g = &groups[code];
                    let id = *by_code.get(code).ok_or_else(|| missing_row("subject"))?;
                    for (lookup, i) in &g.members {
                        let p = parsed[*i];
                        let ty = self.type_of(p);
                        let other_digest = digests
                            .get(&id)
                            .and_then(|d| d.as_ref())
                            .is_some_and(|d| *d != g.digest);
                        let other_identity = held
                            .iter()
                            .any(|h| h.subject_id == id && h.id_type_id == ty.id);
                        if other_digest || other_identity {
                            self.collision = Some(Collision {
                                code: code.clone(),
                                subject_id: Some(id),
                                id_type: ty.name.clone(),
                                reason: if other_digest {
                                    "display-code"
                                } else {
                                    "identity"
                                },
                            });
                            return Err(HomeError::Message("identity collision".into()));
                        }
                        self.attach(id, p, lookup.clone());
                        self.written.identities_attached += 1;
                    }
                }
            }
        }
        for (i, lookup) in lookups.iter().enumerate() {
            if ids[i].is_none() {
                ids[i] = self.identities.get(lookup).copied();
            }
        }
        ids.into_iter()
            .map(|id| id.ok_or_else(|| missing_row("subject")))
            .collect()
    }

    /// An identity row for the subject, filed after the commit (§9.3), and
    /// the lookup cached.
    fn attach(&mut self, subject_id: i64, p: &ParsedFile, lookup: Vec<u8>) {
        let ty = self.type_of(p);
        self.pending.push(NewIdentity {
            subject_id,
            id_type_id: ty.id,
            lookup: lookup.clone(),
            ciphertext: self.keys.seal(&p.ident.value),
            source: "dicom",
            first_batch_id: Some(self.batch_id),
        });
        self.identities.put(lookup, subject_id);
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
                        kept: Kept::default(),
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
                            kept: Kept::default(),
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
            resolve(
                self.registry.store(),
                &self.study_fields,
                &p.hashes.study,
                &mut entry.hashes,
                &mut entry.kept,
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
                        kept: Kept::default(),
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
            resolve(
                self.registry.store(),
                fields,
                mine,
                theirs,
                &mut entry.kept,
                entry.id,
                x,
            )?;
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
                    kept: Kept::default(),
                },
            );
        }
        Ok(())
    }

    /// Stacks (§8): a row per `(series, stack key)` the registry does not
    /// hold, its index the next of its series; every file's stack id comes
    /// back, for its instance. The stacks of a series the cache misses are
    /// read in one select, so the next index is the registry's, not the
    /// cache's.
    fn stacks(
        &mut self,
        parsed: &[&ParsedFile],
        series_ids: &[i64],
        tally: &mut Counts,
    ) -> Result<Vec<i64>, HomeError> {
        let t = table("stack");
        let missed: Vec<i64> = {
            let mut ids: Vec<i64> = parsed
                .iter()
                .zip(series_ids)
                .filter(|(p, sid)| !self.stacks.contains(&(**sid, p.signature.key.clone())))
                .map(|(_, sid)| *sid)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        // series id → the index the next stack of the series takes
        let mut next: HashMap<i64, i64> = HashMap::new();
        if !missed.is_empty() {
            let cols = columns(
                t,
                &["id", "series_id", "stack_key", "stack_index"],
                &Fields::of(&[]),
            );
            let found = self
                .registry
                .store()
                .select_by_ids(t, &cols, "series_id", &missed)?;
            for r in &found {
                let sid = r.int(1)?;
                self.stacks.put((sid, r.text(2)?.to_string()), r.int(0)?);
                let n = next.entry(sid).or_default();
                *n = (*n).max(r.int(3)? + 1);
            }
        }
        // (series id, key) → the first file of the batch in the stack
        let mut pending: HashMap<(i64, String), usize> = HashMap::new();
        let mut rows = Vec::new();
        let mut per_series: BTreeMap<i64, i64> = BTreeMap::new();
        for (i, p) in parsed.iter().enumerate() {
            let sid = series_ids[i];
            let key = (sid, p.signature.key.clone());
            if self.stacks.contains(&key) || pending.contains_key(&key) {
                continue;
            }
            let x = &p.extracted;
            let index = next.entry(sid).or_default();
            let mut row = vec![
                Param::Int(sid),
                Param::Int(*index),
                Param::from(p.signature.key.as_str()),
                Param::from(x.modality.as_str()),
                Param::from(p.signature.orientation.class.name()),
            ];
            row.extend(x.row(Level::Stack).map(|(_, v)| Param::from(v)));
            row.extend([
                Param::Double(p.signature.orientation.confidence),
                Param::Int(0),
                Param::Int(self.batch_id),
            ]);
            rows.push(row);
            *index += 1;
            *per_series.entry(sid).or_default() += 1;
            pending.insert(key, i);
        }
        let mut diags = Vec::new();
        if !rows.is_empty() {
            let spec = Insert::all(t)
                .on_conflict(Conflict::Nothing(&["series_id", "stack_key"]))
                .returning(&["id", "series_id", "stack_key"]);
            let returned = self.registry.store().insert(&spec, &rows)?;
            for r in &returned {
                let key = (r.int(1)?, r.text(2)?.to_string());
                let Some(i) = pending.remove(&key) else {
                    continue;
                };
                self.stacks.put(key, r.int(0)?);
                let o = &parsed[i].signature.orientation;
                if o.oblique() {
                    diags.push(Diagnostic::new(
                        DiagnosticKind::OrientationOblique,
                        format!("{} {:.2}", o.class.name(), o.confidence),
                    ));
                }
            }
            self.written.stacks_created += returned.len() as u64;
            // a row the insert did not return exists: another batch's
            for (sid, _) in pending.keys() {
                if let Some(n) = per_series.get_mut(sid) {
                    *n -= 1;
                }
            }
            if !pending.is_empty() {
                let mut again: Vec<i64> = pending.keys().map(|(sid, _)| *sid).collect();
                again.sort_unstable();
                again.dedup();
                let cols = columns(t, &["id", "series_id", "stack_key"], &Fields::of(&[]));
                let found = self
                    .registry
                    .store()
                    .select_by_ids(t, &cols, "series_id", &again)?;
                for r in &found {
                    self.stacks
                        .put((r.int(1)?, r.text(2)?.to_string()), r.int(0)?);
                }
            }
            let pairs: Vec<(i64, i64)> = per_series.into_iter().filter(|(_, n)| *n > 0).collect();
            if !pairs.is_empty() {
                self.registry.store().update_from_values(
                    table("series"),
                    "n_stacks = n_stacks + v.val",
                    "id",
                    &pairs,
                )?;
            }
        }
        let mut ids = Vec::with_capacity(parsed.len());
        for (i, p) in parsed.iter().enumerate() {
            let id = self
                .stacks
                .get(&(series_ids[i], p.signature.key.clone()))
                .copied()
                .ok_or_else(|| missing_row("stack"))?;
            ids.push(id);
        }
        self.note(tally, diags);
        Ok(ids)
    }

    /// Instances: a row per SOP instance UID the registry does not hold, in
    /// its file's stack; the status of every file follows from whether its
    /// instance is new, its own from an earlier run, or another file's (§5.3).
    fn instances(
        &mut self,
        parsed: &[&ParsedFile],
        series_ids: &[i64],
        stack_ids: &[i64],
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
                Param::Int(stack_ids[i]),
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
        let mut per_stack: BTreeMap<i64, i64> = BTreeMap::new();
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
                *per_stack.entry(stack_ids[i]).or_default() += 1;
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
        let pairs: Vec<(i64, i64)> = per_stack.into_iter().collect();
        if !pairs.is_empty() {
            self.registry.store().update_from_values(
                table("stack"),
                "n_instances = n_instances + v.val",
                "id",
                &pairs,
            )?;
        }
        self.note(tally, diags);
        Ok(filed)
    }

    /// Source files: every read item's row, upserted on `(source_id, path)`,
    /// and the unchanged files' rows touched by id; then the new instances'
    /// `source_file_id`, and the instance an earlier run filed a changed path
    /// under let go of it.
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
        let mut unchanged = Vec::new();
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
                Item::Unchanged { id, quarantined } => {
                    unchanged.push(*id);
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
        store.update_by_ids(
            t,
            &[
                ("batch_id", Param::Int(self.batch_id)),
                ("seen_at", Param::from(now)),
            ],
            "id",
            &unchanged,
        )?;
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
/// while they are quiet, until the last parser is done. A stop changes
/// nothing here: the parsers send what they have read, and it is written.
/// An abort lets every batch from then on go, the one in flight included.
pub fn run(
    writer: &mut Writer<'_>,
    rx: &Receiver<Batch>,
    progress: &Progress,
) -> Result<(), HomeError> {
    loop {
        match rx.recv_timeout(PROGRESS_EVERY) {
            Ok(batch) => {
                if writer.cancel.abort() {
                    continue;
                }
                match writer.write(&batch, progress) {
                    Ok(()) => {}
                    Err(HomeError::Message(m)) if m == ABORTED => continue,
                    Err(e) => return Err(e),
                }
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


/// The table and key column a field of `level` is updated by.
fn field_table(level: Level) -> (&'static Table, &'static str) {
    match level {
        Level::Subject | Level::Study | Level::Series => (table(level.name()), "id"),
        _ => (table(level.name()), "series_id"),
    }
}

/// Decide the fields a file and a stored row disagree on (§9.1): the row
/// keeps the smaller value in the canonical text order, whichever file
/// brought it, so that the row is the same however the walk and the workers
/// ordered the instances. The stored value is read back the first time a
/// field is decided and kept with the cached row.
fn resolve(
    store: &mut Store,
    fields: &Fields,
    mine: &[u32],
    theirs: &mut [u32],
    kept: &mut Kept,
    id: i64,
    x: &nils_dicom::Extracted,
) -> Result<(), HomeError> {
    for i in fields.resolvable(mine, theirs) {
        let level = fields.levels[i];
        let name = fields.names[i];
        let Some(value) = x.value(level, name) else {
            continue;
        };
        let (t, key) = field_table(level);
        let ours = canonical_value(value).into_owned();
        let stored = match kept.get(i) {
            Some(text) => text.to_string(),
            None => {
                let column = t
                    .column(name)
                    .unwrap_or_else(|| panic!("{}.{name} is not a column", t.name));
                let rows = store.select_by_ids(t, &[column], key, &[id])?;
                match rows.first().and_then(|r| r.0.first()) {
                    Some(cell) => match canonical_cell(fields.converters[i], cell) {
                        Some(text) => text.into_owned(),
                        None => continue,
                    },
                    None => continue,
                }
            }
        };
        if ours < stored {
            store.update_by_id(t, &[(name, Param::from(Some(value)))], key, id)?;
            theirs[i] = mine[i];
            kept.set(i, &ours);
        } else {
            kept.set(i, &stored);
        }
    }
    Ok(())
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
        let (t, key) = field_table(level);
        let value = x.value(level, fields.names[i]);
        store.update_by_id(t, &[(fields.names[i], Param::from(value))], key, id)?;
        theirs[i] = mine[i];
    }
    Ok(())
}

/// The id type of a name, which the linkage store must hold.
fn id_type_of(linkage: &mut Store, name: &str) -> Result<IdType, HomeError> {
    match linkage::id_type_id(linkage, name)? {
        Some(id) => Ok(IdType {
            name: name.to_string(),
            id,
        }),
        None => Err(HomeError::Message(format!(
            "no id type named {name}; nils linkage id-type list shows them, id-type add creates one"
        ))),
    }
}

/// The error a collision ends the job with: the code, the type and the item,
/// never an identifier.
fn collision_message(c: &Collision, item: i64) -> String {
    let what = match c.reason {
        "batch" => format!(
            "two identifiers of this batch derive the one code {}",
            c.code
        ),
        "display-code" => format!(
            "code {} is another identifier's (its subject holds a different digest)",
            c.code
        ),
        _ => format!(
            "the subject with code {} already holds another identifier of type {}",
            c.code, c.id_type
        ),
    };
    format!(
        "identity collision under {}: {what}; review item {item} is open. A blake2b-32 registry takes a longer display length (re-create it with --display-length); a blake2b-8 one has two identifiers on one code, which the review decides",
        c.id_type
    )
}

fn missing_row(what: &str) -> HomeError {
    HomeError::Message(format!(
        "a {what} row was neither inserted nor found; the store changed under the writer"
    ))
}
