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
use nils_registry::schema::{Column, Table, Type, table};
use nils_registry::store::{Cell, Insert, Param, Store};
use nils_registry::time::now_iso;
use nils_registry::{HomeError, Registry};

use crate::batch::{
    Batch, Fields, Item, ParsedFile, canonical_cell, canonical_value, detail_level, hash_value,
    hash32,
};
use crate::cancel::{Cancel, Scripted};
use crate::date;
use crate::knobs::Unmapped;
use crate::progress::{PROGRESS_EVERY, Progress};
use crate::report::{Counts, Written};
use crate::resolve::{
    Collision, Found, Make, ResolveError, Resolved, Resolver, Who, collision_message, missing_row,
};
use crate::resume::status;
use crate::rule::Rule;

/// Rows each of the writer's caches holds (§9.1).
pub use crate::resolve::CACHE_ROWS;

/// The kind of the review item a collision opens (§7.1).
pub const COLLISION_KIND: &str = "identity.collision";
/// The review item that groups the files a batch quarantined into one class
/// (§5.3): one per batch and class, the count as evidence, no path in it.
pub const QUARANTINE_KIND: &str = "ingest.quarantine";

/// The error a batch ends with when an abort is asked while it is in flight
/// (§10): its transaction rolls back, and the run ends as aborted.
pub const ABORTED: &str = "aborted: the batch in flight rolled back";

/// Record 26 §1 and §4: the dataset a run reads in place, whose held files
/// are the digest's own to record. A dataset that arrives identified has
/// none here: the pseudonymiser held or coded every file of its originals
/// before it wrote the tree a digest reads, and those rows are its own.
#[derive(Debug, Clone)]
pub struct Dataset {
    pub id: i64,
    pub name: String,
}

/// What an earlier run recorded of a file it held, and what has been said
/// about it since: the lookup a map released it under, where the map named
/// the value as another type, and whether a person asked for it to be coded
/// anyway.
#[derive(Debug, Clone, Default)]
struct PriorHeld {
    released: Option<Vec<u8>>,
    code_anyway: bool,
}

/// A subject this run coded from an identifier no map named (record 26 §4):
/// the shape of that identifier and how many of the run's files are about
/// the person, which is what the `identity.provisional` item carries.
#[derive(Debug, Clone, Default)]
pub struct Provisional {
    pub shape: String,
    pub files: i64,
}

/// The state of a `pseudonym_file` row whose file waits for a map, as the
/// pseudonymiser writes it.
const HELD: &str = "held";

/// The table both verbs record a held file in.
const HELD_TABLE: &str = "pseudonym_file";

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
    /// The private elements the series holds (Wave 4a §5.2), read once from
    /// its row and kept with it. Absent on a series no file of this run
    /// brought a private element for, which with no pack is every series.
    private: Option<Box<PrivateState>>,
}

/// What `series_private` holds for one series, as the writer decides it
/// (Wave 4a §5.2): the value under each address, the addresses two files
/// disagreed on, whether the row has been read, whether it needs writing.
#[derive(Default)]
struct PrivateState {
    values: BTreeMap<String, String>,
    varied: std::collections::BTreeSet<String>,
    loaded: bool,
    dirty: bool,
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
    /// Who each file is about (§7.4), through the linkage store.
    resolver: Resolver,
    /// The years a recovered date must fall in to be believable (§4.2). A knob
    /// with a default, because reading eight digits out of a UID is a guess
    /// and the range is what makes it a reasonable one.
    date_range: date::Range,
    /// One ballot per study the run has seen, kept across batches. A study's
    /// files do not all arrive together, and a date decided from whichever of
    /// them shared a batch with the row insert would be an answer about the
    /// batch rather than about the study, which is the fault of C14 in another
    /// place. The verdicts are written once, when the run ends.
    ballots: HashMap<String, date::Ballot>,
    source_id: i64,
    batch_id: i64,
    job_id: Option<i64>,
    /// Record 26 §4: what the run does with a file whose identifier the
    /// linkage store does not know.
    unmapped: Unmapped,
    /// Record 26 §4: the dataset read in place, whose held files this run
    /// records in `pseudonym_file` as the pseudonymiser records its own, so
    /// that the map, the held doors and the counts are one path whichever
    /// verb held the file.
    dataset: Option<Dataset>,
    /// Those rows for the files of the batch in hand.
    prior_held: HashMap<String, PriorHeld>,
    /// Whether the dataset holds any such row at all, asked once: a dataset
    /// that has never held a file asks nothing per batch.
    any_held: Option<bool>,
    /// The subjects this run coded from an identifier no map named, for the
    /// items it raises when it ends.
    pub provisional: BTreeMap<i64, Provisional>,
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
    /// The address of each slot of a file's ingested private elements, in
    /// the order the files were extracted with (Wave 4a §5.2).
    ingest: Vec<String>,
    /// The series row and the detail row, per modality.
    series_detail: HashMap<String, Fields>,
    /// The writer's own diagnostics, for the report.
    pub counts: Counts,
    pub written: Written,
    last_heartbeat: Instant,
    collision: Option<Collision>,
    /// The run's stop token (§10), checked between the tables of a batch.
    cancel: Cancel,
    /// The stop the tests script, acting on the count of commits.
    script: Option<Scripted>,
}

impl<'a> Writer<'a> {
    pub fn new(
        registry: &'a mut Registry,
        rule: &Rule,
        source_id: i64,
        batch_id: i64,
        job_id: Option<i64>,
    ) -> Result<Writer<'a>, HomeError> {
        let resolver = Resolver::new(registry, rule, batch_id)?;
        let cap = NonZeroUsize::new(CACHE_ROWS).unwrap_or(NonZeroUsize::MIN);
        Ok(Writer {
            registry,
            resolver,
            date_range: date::Range::default(),
            ballots: HashMap::new(),
            source_id,
            batch_id,
            job_id,
            unmapped: Unmapped::Subject,
            dataset: None,
            prior_held: HashMap::new(),
            any_held: None,
            provisional: BTreeMap::new(),
            subjects: LruCache::new(cap),
            studies: LruCache::new(cap),
            series: LruCache::new(cap),
            stacks: LruCache::new(cap),
            subject_fields: Fields::subject(),
            study_fields: Fields::study(),
            series_fields: Fields::of(&[Level::Series]),
            ingest: Vec::new(),
            series_detail: HashMap::new(),
            counts: Counts::default(),
            written: Written {
                batch_id,
                ..Written::default()
            },
            last_heartbeat: Instant::now(),
            collision: None,
            cancel: Cancel::new(),
            script: None,
        })
    }

    /// The run's stop token, and the scripted stop of a test if there is one.
    /// The private elements the files were extracted with, in order, so the
    /// writer knows the address of each slot (Wave 4a §5.2).
    /// Record 26 §4: what the dataset says of a file whose identifier the
    /// linkage store does not know, and the dataset itself where the tree is
    /// one read in place, whose held files this run records.
    pub fn holding(mut self, unmapped: Unmapped, dataset: Option<Dataset>) -> Writer<'a> {
        self.unmapped = unmapped;
        self.dataset = dataset;
        self
    }

    pub fn with_ingest(mut self, ingest: &[nils_dicom::private::Ingest]) -> Writer<'a> {
        self.ingest = ingest.iter().map(|i| i.address()).collect();
        self
    }

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
                self.resolver.file_identities()
            }
            Err(e) => {
                let _ = self.registry.store().rollback();
                self.resolver.abandon();
                if let Some(c) = self.collision.take() {
                    let id = self.open_review(&c)?;
                    return Err(HomeError::Message(collision_message(&c, id)));
                }
                Err(e)
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
            "scheme": self.resolver.scheme().to_string(),
            "display_length": self.resolver.display_length(),
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
        let (subject_ids, held) = self.subjects(&parsed, &now, &mut tally)?;
        // record 26 §4: a file the dataset holds for want of a map is no
        // part of what this batch writes; its `source_file` row says why
        let parsed: Vec<&ParsedFile> = match held.iter().any(|h| *h) {
            false => parsed,
            true => parsed
                .into_iter()
                .zip(&held)
                .filter(|(_, h)| !**h)
                .map(|(p, _)| p)
                .collect(),
        };
        self.checkpoint()?;
        let study_ids = self.studies(&parsed, &subject_ids, &mut tally)?;
        self.checkpoint()?;
        let series_ids = self.series(&parsed, &study_ids, &subject_ids, &mut tally)?;
        self.checkpoint()?;
        let stack_ids = self.stacks(&parsed, &series_ids, &mut tally)?;
        self.checkpoint()?;
        let filed = self.instances(&parsed, &series_ids, &stack_ids, &mut tally)?;
        self.checkpoint()?;
        self.instance_frames(&parsed, &stack_ids, &filed)?;
        self.checkpoint()?;
        self.source_files(batch, &filed, &held, &now, progress)?;
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
    ) -> Result<(Vec<i64>, Vec<bool>), HomeError> {
        // record 26 §4: what an earlier run held of these files, and what a
        // map or a person has said about them since
        self.prior_held = self.held_prior(parsed)?;
        let mut who: Vec<Who<'_>> = Vec::with_capacity(parsed.len());
        for p in parsed {
            who.push(Who {
                ident: &p.ident,
                subject: p
                    .extracted
                    .row(Level::Subject)
                    .map(|(_, v)| Param::from(v))
                    .collect(),
                // a map that named the value as another type released the
                // row under its own lookup, and the identity is found there
                lookup: self
                    .prior_held
                    .get(&p.path)
                    .and_then(|h| h.released.clone()),
            });
        }
        // record 26 §4: the dataset says what an identifier no subject holds
        // does, and a run that holds makes nothing for one
        let make = match self.unmapped {
            Unmapped::Subject => Make::Subject,
            Unmapped::Hold => Make::Nothing,
            Unmapped::Code => Make::Provisional,
        };
        let mut resolved = self.resolve_who(&who, now, make)?;
        // the files coded from an identifier no map named, whose subjects are
        // provisional: every one of them where the dataset says `code`, and
        // where it holds, the ones a person asked to be coded anyway
        let mut coded = vec![make == Make::Provisional; parsed.len()];
        if make == Make::Nothing && !self.prior_held.is_empty() {
            let anyway: Vec<usize> = (0..parsed.len())
                .filter(|&i| {
                    resolved.found[i].id().is_none()
                        && self
                            .prior_held
                            .get(&parsed[i].path)
                            .is_some_and(|h| h.code_anyway)
                })
                .collect();
            if !anyway.is_empty() {
                let mut asked: Vec<Who<'_>> = Vec::with_capacity(anyway.len());
                for &i in &anyway {
                    asked.push(Who {
                        ident: &parsed[i].ident,
                        subject: parsed[i]
                            .extracted
                            .row(Level::Subject)
                            .map(|(_, v)| Param::from(v))
                            .collect(),
                        lookup: None,
                    });
                }
                let second = self.resolve_who(&asked, now, Make::Provisional)?;
                resolved.matched += second.matched;
                resolved.created += second.created;
                resolved.attached += second.attached;
                for (k, &i) in anyway.iter().enumerate() {
                    resolved.found[i] = second.found[k];
                    coded[i] = true;
                }
            }
        }
        self.written.subjects_matched += resolved.matched;
        self.written.subjects_created += resolved.created;
        self.written.identities_attached += resolved.attached;
        let mut ids = Vec::with_capacity(parsed.len());
        // the files the dataset holds for want of a map, in the order the
        // batch parsed them: no subject, and no row of any other table
        let mut held = vec![false; parsed.len()];
        for (i, (p, f)) in parsed.iter().zip(&resolved.found).enumerate() {
            let Some(id) = f.id() else {
                held[i] = true;
                continue;
            };
            if let Found::Created(_) = f {
                let x = &p.extracted;
                self.subjects.put(
                    id,
                    SubjectEntry {
                        hashes: x.row(Level::Subject).map(|(_, v)| hash_value(v)).collect(),
                        kept: Kept::default(),
                    },
                );
            }
            if coded[i] {
                self.note_provisional(id, matches!(f, Found::Created(_)), &p.ident.value);
            }
            ids.push(id);
        }
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
        let filed: Vec<&ParsedFile> = match held.iter().any(|h| *h) {
            false => parsed.to_vec(),
            true => parsed
                .iter()
                .zip(&held)
                .filter(|(_, h)| !**h)
                .map(|(p, _)| *p)
                .collect(),
        };
        for (p, &id) in filed.iter().zip(&ids) {
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
        Ok((ids, held))
    }

    /// One call to the resolver, with a collision kept for the rollback that
    /// follows, as the batch's own error is not the collision itself.
    fn resolve_who(
        &mut self,
        who: &[Who<'_>],
        now: &str,
        make: Make,
    ) -> Result<Resolved, HomeError> {
        match self.resolver.resolve(self.registry.store(), who, now, make) {
            Ok(r) => Ok(r),
            Err(ResolveError::Collision(c)) => {
                self.collision = Some(c);
                Err(HomeError::Message("identity collision".into()))
            }
            Err(ResolveError::Home(e)) => Err(e),
        }
    }

    /// Record 26 §4: the subject a file was coded into and the shape of the
    /// identifier it was coded from, so the run can ask about the person when
    /// it ends. Counted per file, as the pseudonymiser counts them.
    fn note_provisional(&mut self, id: i64, created: bool, value: &str) {
        if created {
            self.provisional.insert(
                id,
                Provisional {
                    shape: nils_dicom::diagnostic::shape(value),
                    files: 1,
                },
            );
        } else if let Some(p) = self.provisional.get_mut(&id) {
            p.files += 1;
        }
    }

    /// Record 26 §4: what `pseudonym_file` holds of these files, where the
    /// tree is a dataset read in place. Nothing at all for a dataset that has
    /// never held one, which costs one question for the run and none per
    /// batch.
    fn held_prior(
        &mut self,
        parsed: &[&ParsedFile],
    ) -> Result<HashMap<String, PriorHeld>, HomeError> {
        let Some(place_id) = self.dataset.as_ref().map(|d| d.id) else {
            return Ok(HashMap::new());
        };
        if self.any_held.is_none() {
            let store = self.registry.store();
            let sql = format!(
                "SELECT COUNT(*) FROM {} WHERE place_id = {}",
                store.qualified(HELD_TABLE),
                store.dialect().param(1, Type::Int)
            );
            let any = store.query(&sql, &[Param::Int(place_id)])?[0].int(0)? > 0;
            self.any_held = Some(any);
        }
        if self.any_held != Some(true) || parsed.is_empty() {
            return Ok(HashMap::new());
        }
        let mut out = HashMap::new();
        let paths: Vec<&str> = parsed.iter().map(|p| p.path.as_str()).collect();
        for chunk in paths.chunks(nils_registry::store::SQLITE_KEY_CHUNK) {
            let store = self.registry.store();
            let d = store.dialect();
            let released = d.text_of(
                table(HELD_TABLE)
                    .column("released_at")
                    .expect("pseudonym_file.released_at"),
            );
            let marks: Vec<String> = (0..chunk.len())
                .map(|i| d.param(i + 2, Type::Text))
                .collect();
            let sql = format!(
                "SELECT path, lookup, {released} IS NOT NULL, code_anyway FROM {} \
                 WHERE place_id = {} AND state = '{HELD}' AND path IN ({})",
                store.qualified(HELD_TABLE),
                d.param(1, Type::Int),
                marks.join(", ")
            );
            let mut params: Vec<Param> = Vec::with_capacity(chunk.len() + 1);
            params.push(Param::Int(place_id));
            params.extend(chunk.iter().map(|p| Param::from(*p)));
            for r in &store.query(&sql, &params)? {
                let released = match r.get(2) {
                    Cell::Bool(b) => *b,
                    Cell::Int(n) => *n != 0,
                    _ => false,
                };
                out.insert(
                    r.text(0)?.to_string(),
                    PriorHeld {
                        released: match released {
                            true => r.opt_bytes(1)?.map(<[u8]>::to_vec),
                            false => None,
                        },
                        code_anyway: r.int(3)? != 0,
                    },
                );
            }
        }
        Ok(out)
    }

    /// Record 26 §4: what a dataset read in place holds waits for a map in
    /// `pseudonym_file`, beside what the pseudonymiser holds of an identified
    /// one, so that the map, the held doors and the counts read one table
    /// whichever verb held the file. A file this batch filed is no longer
    /// held, and its row goes.
    ///
    /// A held row carries no digest of the original (lab 26d, finding 2):
    /// the file has no copy anywhere, a purge of the dataset is refused for
    /// as long as one waits for a map, and what is never destroyed needs no
    /// proof. A row that stands for a copy carries one.
    fn record_held(
        &mut self,
        place_id: i64,
        held: &[Vec<Param>],
        filed: &[&str],
    ) -> Result<(), HomeError> {
        let store = self.registry.store();
        if !held.is_empty() {
            store.insert(
                &Insert::new(
                    table(HELD_TABLE),
                    &[
                        "place_id",
                        "path",
                        "dir",
                        "size",
                        "mtime",
                        "state",
                        "shape",
                        "lookup",
                        "sealed",
                        "id_type",
                        "batch_id",
                        "first_seen",
                        "code_anyway",
                    ],
                )
                .on_conflict(Conflict::Update {
                    // `released_at` and `code_anyway` stay as they are, as
                    // they do for the pseudonymiser: a file held again after
                    // a release keeps its release
                    target: &["place_id", "path"],
                    set: &[
                        "dir", "size", "mtime", "state", "shape", "lookup", "sealed", "id_type",
                        "batch_id",
                    ],
                }),
                held,
            )?;
        }
        for chunk in filed.chunks(nils_registry::store::SQLITE_KEY_CHUNK) {
            let d = store.dialect();
            let marks: Vec<String> = (0..chunk.len())
                .map(|i| d.param(i + 2, Type::Text))
                .collect();
            let sql = format!(
                "DELETE FROM {} WHERE place_id = {} AND path IN ({})",
                store.qualified(HELD_TABLE),
                d.param(1, Type::Int),
                marks.join(", ")
            );
            let mut params: Vec<Param> = Vec::with_capacity(chunk.len() + 1);
            params.push(Param::Int(place_id));
            params.extend(chunk.iter().map(|p| Param::from(*p)));
            store.execute(&sql, &params)?;
        }
        Ok(())
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
        // The date is decided per study rather than per file, because the
        // point of a vote is that several files can agree (§4.2). Every file
        // of a study in this batch gets a say before the row is written.
        for p in parsed {
            self.ballots
                .entry(p.extracted.study_uid.clone())
                .or_default()
                .cast(p, self.date_range);
        }
        for (p, &subject_id) in parsed.iter().zip(subject_ids) {
            let x = &p.extracted;
            if self.studies.contains(&x.study_uid) || pending.contains_key(&x.study_uid) {
                continue;
            }
            let mut row = vec![Param::from(x.study_uid.as_str()), Param::Int(subject_id)];
            row.extend(x.row(Level::Study).map(|(_, v)| Param::from(v)));
            row.push(Param::Int(self.batch_id));
            // What is left is what a later step decides: the date this study
            // was given (`settle_dates`, when the run ends, because the
            // study's other files may still be coming) and whether it holds a
            // primary (`nils fingerprint`, §6). Padded to the table's width
            // rather than by a count, so that adding a column to `study` does
            // not silently unbalance this insert.
            while row.len() < t.data_columns().count() {
                row.push(Param::Null);
            }
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
                        private: None,
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
        if !self.ingest.is_empty() {
            self.merge_private(parsed, &ids)?;
        }
        self.note(tally, diags);
        Ok(ids)
    }

    /// Fold each file's ingested private elements into its series' row
    /// (Wave 4a §5.2), under the rule the catalogue's columns follow: a value
    /// the row lacks is filled by the first file that has one, and where two
    /// files disagree the smaller in text order stays and the address is
    /// listed as varied. The row is the same however the walk and the
    /// workers ordered the instances, and a reader can see that the series
    /// was not of one mind.
    fn merge_private(&mut self, parsed: &[&ParsedFile], ids: &[i64]) -> Result<(), HomeError> {
        // The rows to read first: series cached from the registry whose
        // private state this run has not seen, and that a file of this batch
        // brings a value for. One select for the batch.
        let mut need: Vec<(String, i64)> = Vec::new();
        for (p, &id) in parsed.iter().zip(ids) {
            if p.extracted.private.iter().all(Option::is_none) {
                continue;
            }
            let entry = self
                .series
                .get_mut(&p.extracted.series_uid)
                .ok_or_else(|| missing_row("series"))?;
            if entry.private.as_ref().is_none_or(|st| !st.loaded) {
                need.push((p.extracted.series_uid.clone(), id));
            }
        }
        need.sort();
        need.dedup();
        if !need.is_empty() {
            self.load_private(&need)?;
        }
        let mut touched: Vec<String> = Vec::new();
        for p in parsed {
            let x = &p.extracted;
            if x.private.iter().all(Option::is_none) {
                continue;
            }
            let entry = self
                .series
                .get_mut(&x.series_uid)
                .ok_or_else(|| missing_row("series"))?;
            let state = entry.private.get_or_insert_with(Default::default);
            state.loaded = true;
            for (i, value) in x.private.iter().enumerate() {
                let Some(v) = value else { continue };
                let key = &self.ingest[i];
                match state.values.get(key) {
                    None => {
                        state.values.insert(key.clone(), v.clone());
                        state.dirty = true;
                    }
                    Some(old) if old == v => {}
                    Some(old) => {
                        if v < old {
                            state.values.insert(key.clone(), v.clone());
                        }
                        state.varied.insert(key.clone());
                        state.dirty = true;
                    }
                }
            }
            if state.dirty && !touched.contains(&x.series_uid) {
                touched.push(x.series_uid.clone());
            }
        }
        let mut rows: Vec<Vec<Param>> = Vec::new();
        for uid in &touched {
            let Some(entry) = self.series.get_mut(uid) else {
                continue;
            };
            let Some(state) = entry.private.as_mut() else {
                continue;
            };
            if !state.dirty {
                continue;
            }
            state.dirty = false;
            rows.push(vec![
                Param::Int(entry.id),
                Param::from(serde_json::to_string(&state.values).unwrap_or_else(|_| "{}".into())),
                match state.varied.is_empty() {
                    true => Param::Null,
                    false => {
                        Param::from(state.varied.iter().cloned().collect::<Vec<_>>().join(","))
                    }
                },
            ]);
        }
        if !rows.is_empty() {
            let spec = Insert::new(
                table("series_private"),
                &["series_id", "elements", "varied"],
            )
            .on_conflict(Conflict::Update {
                target: &["series_id"],
                set: &["elements", "varied"],
            });
            self.registry.store().insert(&spec, &rows)?;
        }
        Ok(())
    }

    /// Read the `series_private` rows of the series named, into their cached
    /// entries; a series with no row is marked read all the same, so it is
    /// not asked for again.
    fn load_private(&mut self, need: &[(String, i64)]) -> Result<(), HomeError> {
        let t = table("series_private");
        let cols = [
            t.column("series_id").expect("series_private.series_id"),
            t.column("elements").expect("series_private.elements"),
            t.column("varied").expect("series_private.varied"),
        ];
        let ids: Vec<i64> = need.iter().map(|(_, id)| *id).collect();
        let rows = self
            .registry
            .store()
            .select_by_ids(t, &cols, "series_id", &ids)?;
        let mut found: HashMap<i64, PrivateState> = HashMap::new();
        for r in &rows {
            let id = r.int(0)?;
            let values: BTreeMap<String, String> =
                serde_json::from_str(r.text(1)?).unwrap_or_default();
            let varied = r
                .opt_text(2)?
                .map(|v| v.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            found.insert(
                id,
                PrivateState {
                    values,
                    varied,
                    loaded: true,
                    dirty: false,
                },
            );
        }
        for (uid, id) in need {
            let Some(entry) = self.series.get_mut(uid) else {
                continue;
            };
            let state = found.remove(id).unwrap_or(PrivateState {
                loaded: true,
                ..Default::default()
            });
            entry.private = Some(Box::new(state));
        }
        Ok(())
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
                    private: None,
                },
            );
        }
        Ok(())
    }

    /// Stacks (§8, record 37 S8): a row per `(series, stack key)` the registry
    /// does not hold, its index the next of its series; every stack of every
    /// file comes back, in the file's order, the first for its instance. A
    /// classic instance holds one; an enhanced object whose frames state more
    /// than one holds one per group of frames. The stacks of a series the
    /// cache misses are read in one select, so the next index is the
    /// registry's, not the cache's.
    fn stacks(
        &mut self,
        parsed: &[&ParsedFile],
        series_ids: &[i64],
        tally: &mut Counts,
    ) -> Result<Vec<Vec<i64>>, HomeError> {
        let t = table("stack");
        let missed: Vec<i64> = {
            let mut ids: Vec<i64> = parsed
                .iter()
                .zip(series_ids)
                .filter(|(p, sid)| {
                    p.stacks
                        .iter()
                        .any(|s| !self.stacks.contains(&(**sid, s.signature.key.clone())))
                })
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
        // (series id, key) → the first stack of the batch with it
        let mut pending: HashMap<(i64, String), (usize, usize)> = HashMap::new();
        let mut rows = Vec::new();
        let mut per_series: BTreeMap<i64, i64> = BTreeMap::new();
        for (i, p) in parsed.iter().enumerate() {
            let sid = series_ids[i];
            for (n, s) in p.stacks.iter().enumerate() {
                let key = (sid, s.signature.key.clone());
                if self.stacks.contains(&key) || pending.contains_key(&key) {
                    continue;
                }
                let x = &p.extracted;
                let index = next.entry(sid).or_default();
                let mut row = vec![
                    Param::Int(sid),
                    Param::Int(*index),
                    Param::from(s.signature.key.as_str()),
                    Param::from(x.modality.as_str()),
                    Param::from(s.signature.orientation.class.name()),
                ];
                match &s.values {
                    // a stack made of some of a file's frames says what those
                    // frames said, not what the file's first frame said
                    Some(values) => row.extend(values.iter().map(|v| Param::from(v.as_ref()))),
                    None => row.extend(x.row(Level::Stack).map(|(_, v)| Param::from(v))),
                }
                row.extend([
                    Param::Double(s.signature.orientation.confidence),
                    Param::Int(0),
                    Param::Int(self.batch_id),
                ]);
                rows.push(row);
                *index += 1;
                *per_series.entry(sid).or_default() += 1;
                pending.insert(key, (i, n));
            }
        }
        let mut diags = Vec::new();
        if !rows.is_empty() {
            let spec = Insert::all(t)
                .on_conflict(Conflict::Nothing(&["series_id", "stack_key"]))
                .returning(&["id", "series_id", "stack_key"]);
            let returned = self.registry.store().insert(&spec, &rows)?;
            for r in &returned {
                let key = (r.int(1)?, r.text(2)?.to_string());
                let Some((i, n)) = pending.remove(&key) else {
                    continue;
                };
                self.stacks.put(key, r.int(0)?);
                let o = &parsed[i].stacks[n].signature.orientation;
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
            let mut per_file = Vec::with_capacity(p.stacks.len());
            for s in &p.stacks {
                let id = self
                    .stacks
                    .get(&(series_ids[i], s.signature.key.clone()))
                    .copied()
                    .ok_or_else(|| missing_row("stack"))?;
                per_file.push(id);
            }
            ids.push(per_file);
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
        stack_ids: &[Vec<i64>],
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
                // the stack of the instance is its first, which for an
                // enhanced object is the stack of its first frame
                Param::Int(stack_ids[i][0]),
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
                // an instance counts once in every stack its frames reach
                for id in &stack_ids[i] {
                    *per_stack.entry(*id).or_default() += 1;
                }
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

    /// Which frames of an instance are in which stack (record 37, S8): one
    /// row per stack of a file whose frames made more than one, so that a
    /// reader of the second stack can find the frames it is made of. A file
    /// whose frames are all in one stack writes nothing here: its instance
    /// row already names that stack, and `number_of_frames` says how many.
    fn instance_frames(
        &mut self,
        parsed: &[&ParsedFile],
        stack_ids: &[Vec<i64>],
        filed: &[Filed],
    ) -> Result<(), HomeError> {
        let mut rows = Vec::new();
        for (i, p) in parsed.iter().enumerate() {
            // the instance's own file, whether this run created it or read it
            // again, so a registry brought up to date by a re-digest gets the
            // rows too; a duplicate path writes nothing
            if p.stacks.len() < 2 || filed[i].status != status::INGESTED {
                continue;
            }
            for (n, s) in p.stacks.iter().enumerate() {
                rows.push(vec![
                    Param::Int(filed[i].instance_id),
                    Param::Int(stack_ids[i][n]),
                    Param::Int(i64::from(s.frames)),
                    Param::Int(i64::from(s.first_frame())),
                    Param::from(s.list()),
                    Param::Int(self.batch_id),
                ]);
            }
        }
        if rows.is_empty() {
            return Ok(());
        }
        let t = table("instance_frame");
        let spec = Insert::all(t)
            .on_conflict(Conflict::Nothing(&["instance_id", "stack_id"]))
            .returning(&["id"]);
        let written = self.registry.store().insert(&spec, &rows)?;
        self.written.frame_groups += written.len() as u64;
        Ok(())
    }

    /// Source files: every read item's row, upserted on `(source_id, path)`,
    /// and the unchanged files' rows touched by id; then the new instances'
    /// `source_file_id`, and the instance an earlier run filed a changed path
    /// under let go of it.
    fn source_files(
        &mut self,
        batch: &Batch,
        filed: &[Filed],
        held: &[bool],
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
        // record 26 §4: the dataset read in place, whose held files are
        // recorded beside the pseudonymiser's, and the rows of files this
        // batch filed, which wait for a map no longer
        let place_id = self.dataset.as_ref().map(|d| d.id);
        let mut now_held: Vec<Vec<Param>> = Vec::new();
        let mut now_filed: Vec<&str> = Vec::new();
        // path → (instance id, the file is its own, the instance it left)
        let mut wanted: HashMap<&str, (i64, bool, Option<i64>)> = HashMap::new();
        let mut ingested = 0;
        let mut next = filed.iter();
        // the parsed files in the order the batch holds them, which is the
        // order the held flags are in
        let mut nth = 0;
        for item in &batch.items {
            match item {
                Item::Parsed(p) => {
                    let holds = held.get(nth).copied().unwrap_or(false);
                    nth += 1;
                    if holds {
                        // record 26 §4: no instance and no row of its own, a
                        // quarantined row under `identity.unmapped` whose
                        // detail is the shape of the identifier, which is
                        // what the review item asks about; never the
                        // identifier itself
                        let shape = nils_dicom::diagnostic::shape(&p.ident.value);
                        rows.push(row(
                            &p.path,
                            &p.dir,
                            p.size,
                            p.mtime_ns,
                            status::QUARANTINED,
                            Some(nils_registry::review::UNMAPPED_KIND),
                            Some(shape.as_str()),
                            None,
                        ));
                        // and, for a dataset read in place, the row a map
                        // releases and the held doors answer: the shape, the
                        // keyed lookup and the identifier sealed, never the
                        // identifier itself
                        if let Some(place_id) = place_id {
                            now_held.push(vec![
                                Param::Int(place_id),
                                Param::from(p.path.as_str()),
                                Param::from(p.dir.as_str()),
                                Param::Int(p.size as i64),
                                Param::Int(p.mtime_ns),
                                Param::from(HELD),
                                Param::from(shape.as_str()),
                                Param::Bytes(self.resolver.lookup(&p.ident)),
                                Param::Bytes(self.resolver.seal(&p.ident.value)),
                                Param::from(self.resolver.type_of(&p.ident).name.as_str()),
                                Param::Int(self.batch_id),
                                Param::from(now),
                                Param::Int(0),
                            ]);
                        }
                        self.written.held += 1;
                        continue;
                    }
                    if place_id.is_some() && self.prior_held.contains_key(&p.path) {
                        now_filed.push(p.path.as_str());
                    }
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
        if let Some(place_id) = place_id {
            self.record_held(place_id, &now_held, &now_filed)?;
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
        // Wave 4a §9.1: a cancel asked through `nils jobs cancel` arrives
        // here, and stops the run the way the first signal does.
        let asked = nils_registry::job::beat(self.registry.store(), job_id, Some(&json))
            .map_err(|e| HomeError::Store(nils_registry::store::Error::Message(e.to_string())))?;
        if asked == nils_registry::job::Asked::Cancel && !self.cancel.stop() {
            self.cancel.request();
        }
        self.last_heartbeat = Instant::now();
        Ok(())
    }
}

/// The writer's loop: every batch the parsers send, a heartbeat between them
/// while they are quiet, until the last parser is done. A stop changes
/// nothing here: the parsers send what they have read, and it is written.
/// An abort lets every batch from then on go, the one in flight included.
impl Writer<'_> {
    /// The date of every study the run touched, decided once, when every file
    /// of it has been seen (§4.2). A study whose `study_date` said something
    /// keeps it: the vote fills a gap and never writes over a measurement.
    pub fn settle_dates(&mut self) -> Result<u64, HomeError> {
        if self.ballots.is_empty() {
            return Ok(0);
        }
        let ballots = std::mem::take(&mut self.ballots);
        let t = table("study");
        let store = self.registry.store();
        let mut settled: u64 = 0;
        for (uid, ballot) in ballots {
            let Some(v) = ballot.verdict() else { continue };
            let sql = format!(
                // A measured value that is a placeholder is no measurement:
                // `19000101` is a real day, so the reader keeps it and the
                // column still says what the file said, but the vote is
                // allowed to answer over it (§4.2).
                "UPDATE {} SET date_filled = {}, date_source = {}, date_weight = {}, \
                 date_runner_up = {} WHERE study_instance_uid = {} \
                 AND (study_date IS NULL OR study_date IN ('1900-01-01', '1901-01-01'))",
                store.qualified(t.name),
                store.dialect().param(1, Type::Date),
                store.dialect().param(2, Type::Text),
                store.dialect().param(3, Type::Int),
                store.dialect().param(4, Type::Int),
                store.dialect().param(5, Type::Text),
            );
            settled += store.execute(
                &sql,
                &[
                    Param::from(v.date.as_str()),
                    Param::from(v.source.name()),
                    Param::Int(v.weight as i64),
                    Param::Int(v.runner_up as i64),
                    Param::from(uid.as_str()),
                ],
            )?;
        }
        Ok(settled)
    }
}

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
    // Every file has been seen, so every ballot is complete.
    if !writer.cancel.abort() {
        writer.settle_dates()?;
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
