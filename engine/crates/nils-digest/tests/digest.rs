// SPDX-License-Identifier: AGPL-3.0-only

//! The digest over a synthetic tree, into a registry on SQLite and, when
//! `NILS_TEST_POSTGRES_DSN` is set, on Postgres: the rows a first run writes
//! and what a second run leaves alone (§5.2), duplicates (§5.3), changed and
//! gone files, the retry of quarantine, a restart, the disagreements the
//! writer raises as diagnostics (§9.1), and the job that holds the registry
//! (§10). Every test runs on each backend and expects the same numbers.

use std::env;
use std::fs;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{DigestError, Report, Settings, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::table;
use nils_registry::store::Cell;
use nils_registry::time::{iso_of, now_iso, now_secs};
use nils_registry::{Backend, Insert, Param, Registry, Row, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_digest_test";

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            None
        }
    }
}

/// A registry home on one backend, fresh for one test.
struct Lab {
    name: &'static str,
    home: Home,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Lab {
    fn new(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
        let dir = TempDir::new("digest-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-digest-fixture-key")
            .unwrap();
        home.init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: Scheme::DEFAULT,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
        Lab {
            name,
            home,
            _dir: dir,
            _guard: None,
        }
    }

    fn open(&self) -> Registry {
        self.home.open().unwrap()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            drop_schemas(&dsn);
        }
    }
}

fn drop_schemas(dsn: &str) {
    let mut store = Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop the test schemas");
}

/// SQLite always; Postgres when the DSN is set, one test at a time.
fn labs() -> Vec<Lab> {
    let mut out = vec![Lab::new("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        drop_schemas(&dsn);
        let mut lab = Lab::new("postgres", Backend::Postgres, Some(dsn));
        lab._guard = Some(guard);
        out.push(lab);
    }
    out
}

/// A query with `{table}` placeholders for the qualified table names.
fn rows(reg: &mut Registry, sql: &str) -> Vec<Row> {
    let mut text = sql.to_string();
    for t in [
        "source_file",
        "instance",
        "series_mr",
        "series_ct",
        "series_pet",
        "series",
        "study",
        "subject",
        "diagnostic",
        "ingest_batch",
        "job",
        "source",
    ] {
        text = text.replace(&format!("{{{t}}}"), &reg.store().qualified(t));
    }
    reg.store().query(&text, &[]).unwrap()
}

fn one(reg: &mut Registry, sql: &str) -> i64 {
    rows(reg, sql)[0].int(0).unwrap()
}

fn ints(reg: &mut Registry, sql: &str) -> Vec<i64> {
    rows(reg, sql).iter().map(|r| r.int(0).unwrap()).collect()
}

fn texts(reg: &mut Registry, sql: &str) -> Vec<String> {
    rows(reg, sql)
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect()
}

fn opt_int(reg: &mut Registry, sql: &str) -> Option<i64> {
    rows(reg, sql)[0].opt_int(0).unwrap()
}

fn settings(dir: &TempDir) -> Settings {
    let mut s = Settings::new(dir.path());
    s.name = "t".to_string();
    s.workers = 2;
    s.walk_threads = 2;
    // several batches for six files
    s.batch_rows = 2;
    s
}

fn with_patient(mut elems: Vec<synth::Elem>, id: &str) -> Vec<synth::Elem> {
    elems.push(synth::text(tags::PATIENT_ID, VR::LO, id));
    elems
}

fn mr(study: &str, series: &str, sop: &str, patient: &str, extra: &[synth::Elem]) -> Vec<u8> {
    let mut e = with_patient(synth::minimal_mr(study, series, sop), patient);
    e.extend(extra.iter().cloned());
    synth::part10(&MetaFields::mr(sop), &e, true)
}

fn ct(study: &str, series: &str, sop: &str, patient: &str, extra: &[synth::Elem]) -> Vec<u8> {
    let mut e = with_patient(synth::minimal_ct(study, series, sop), patient);
    e.extend(extra.iter().cloned());
    synth::part10(&MetaFields::ct(sop), &e, true)
}

fn pet(study: &str, series: &str, sop: &str, patient: &str) -> Vec<u8> {
    let e = with_patient(synth::minimal_pet(study, series, sop), patient);
    synth::part10(&MetaFields::pet(sop), &e, true)
}

fn birth(date: &str) -> synth::Elem {
    synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, date)
}

fn sex(s: &str) -> synth::Elem {
    synth::text(tags::PATIENT_SEX, VR::CS, s)
}

fn description(s: &str) -> synth::Elem {
    synth::text(tags::STUDY_DESCRIPTION, VR::LO, s)
}

/// Two subjects, two studies, four series, five instances, one refused file.
fn tree() -> TempDir {
    let dir = TempDir::new("digest");
    let p1 = [birth("19800101"), sex("M"), description("Brain")];
    let p2 = [birth("19900202"), sex("F"), description("Chest")];
    dir.file("sub1/IM_0001", &mr("A", "A.1", "A.1.1", "P1", &p1));
    dir.file("sub1/IM_0002", &mr("A", "A.1", "A.1.2", "P1", &p1));
    dir.file("sub1/IM_0003", &mr("A", "A.2", "A.2.1", "P1", &p1));
    dir.file("sub2/IM_0001", &ct("B", "B.1", "B.1.1", "P2", &p2));
    dir.file("sub2/IM_0002", &pet("B", "B.2", "B.2.1", "P2"));
    dir.file("bad/readme.txt", b"not a dicom file at all");
    dir
}

/// The counts a report carries, without the timings, the root, and the
/// number of batches (how the parsers split the files is a matter of timing).
fn shape(report: &Report) -> serde_json::Value {
    let mut v = serde_json::to_value(report).unwrap();
    let o = v.as_object_mut().unwrap();
    for k in [
        "elapsed_s",
        "files_per_s",
        "mb_per_s",
        "peak_rss_bytes",
        "setup",
        "root",
    ] {
        o.remove(k);
    }
    if let Some(w) = o.get_mut("written").and_then(|w| w.as_object_mut()) {
        w.remove("writes");
        w.remove("epoch");
    }
    v
}

#[test]
fn a_first_run_writes_the_rows_and_a_second_leaves_them_alone() {
    let mut shapes = Vec::new();
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.seen, 6, "{name}");
        assert_eq!(report.parsed, 5, "{name}");
        assert_eq!(report.quarantined, 1, "{name}");
        assert_eq!(report.unchanged, 0, "{name}");
        assert_eq!(report.subjects, 2, "{name}");
        assert_eq!(report.studies, 2, "{name}");
        assert_eq!(report.series, 4, "{name}");
        assert!(
            report.diagnostics.is_empty(),
            "{name}: {:?}",
            report.diagnostics
        );
        let w = report.written.clone().unwrap();
        assert_eq!(w.batch_id, 1, "{name}");
        // the epoch moves once per batch written (§4.2)
        assert!(w.writes >= 1, "{name}");
        assert_eq!(w.epoch, w.writes as i64, "{name}");
        let epoch = w.epoch;
        assert_eq!(w.ingested, 5, "{name}");
        assert_eq!(w.duplicate, 0, "{name}");
        assert_eq!(w.changed, 0, "{name}");
        assert_eq!(w.quarantine_kept, 0, "{name}");
        assert_eq!(w.gone, 0, "{name}");
        assert_eq!(w.subjects_created, 2, "{name}");
        assert_eq!(w.studies_created, 2, "{name}");
        assert_eq!(w.series_created, 4, "{name}");

        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 2, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {study}"), 2, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {series}"), 4, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {series_mr}"),
            2,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {series_ct}"),
            1,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {series_pet}"),
            1,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {source_file}"),
            6,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {diagnostic}"),
            0,
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT n_instances FROM {series} ORDER BY series_instance_uid"
            ),
            [2, 1, 1, 1],
            "{name}"
        );
        // every instance points at its file and every ingested file at its instance
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {instance} i JOIN {source_file} f ON f.id = i.source_file_id AND f.instance_id = i.id"
            ),
            5,
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT status FROM {source_file} ORDER BY path"),
            [
                "quarantined",
                "ingested",
                "ingested",
                "ingested",
                "ingested",
                "ingested"
            ],
            "{name}"
        );
        assert_eq!(
            texts(
                &mut reg,
                "SELECT reason FROM {source_file} WHERE status = 'quarantined'"
            ),
            ["not_dicom"],
            "{name}"
        );
        // the subject rows carry the catalogue values and the pseudonym, not the identifier
        assert_eq!(
            texts(&mut reg, "SELECT sex FROM {subject} ORDER BY sex"),
            ["F", "M"],
            "{name}"
        );
        let codes = texts(&mut reg, "SELECT code FROM {subject}");
        assert!(
            codes
                .iter()
                .all(|c| c.len() == 12 && c != "P1" && c != "P2"),
            "{name}: {codes:?}"
        );
        assert_eq!(
            texts(
                &mut reg,
                "SELECT study_description FROM {study} ORDER BY study_description"
            ),
            ["Brain", "Chest"],
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT state FROM {ingest_batch}"),
            ["done"],
            "{name}"
        );
        assert_eq!(
            ints(&mut reg, "SELECT epoch_after FROM {ingest_batch}"),
            [epoch],
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT state FROM {job}"),
            ["done"],
            "{name}"
        );
        reg.refresh_meta().unwrap();
        assert_eq!(reg.meta().epoch, epoch, "{name}");

        // the second run reads nothing and moves every row to the new batch
        let again = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(again.seen, 6, "{name}");
        assert_eq!(again.parsed, 0, "{name}");
        assert_eq!(again.quarantined, 0, "{name}");
        assert_eq!(again.unchanged, 6, "{name}");
        let w = again.written.clone().unwrap();
        assert_eq!(w.batch_id, 2, "{name}");
        // unchanged files are rows too: their batches move the epoch
        assert!(w.writes >= 1, "{name}");
        assert_eq!(w.epoch, epoch + w.writes as i64, "{name}");
        assert_eq!(w.ingested, 0, "{name}");
        assert_eq!(w.quarantine_kept, 1, "{name}");
        assert_eq!(w.subjects_created, 0, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {source_file} WHERE batch_id = 2"
            ),
            6,
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT epoch_after FROM {ingest_batch} ORDER BY id"
            ),
            [epoch, w.epoch],
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT state FROM {job} ORDER BY id"),
            ["done", "done"],
            "{name}"
        );
        shapes.push((name, shape(&report), shape(&again)));
    }
    // both backends report the same numbers
    for pair in shapes.windows(2) {
        assert_eq!(pair[0].1, pair[1].1, "{} vs {}", pair[0].0, pair[1].0);
        assert_eq!(pair[0].2, pair[1].2, "{} vs {}", pair[0].0, pair[1].0);
    }
}

#[test]
fn duplicates_changed_and_gone_files_are_filed_as_the_spec_says() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap();
        let original = dir.path().join("sub1/IM_0001");
        let original_bytes = fs::read(&original).unwrap();
        let original_mtime = fs::metadata(&original).unwrap().modified().unwrap();
        let first_instance = one(
            &mut reg,
            "SELECT id FROM {instance} WHERE sop_instance_uid = 'A.1.1'",
        );

        // the same instance under a second path is a duplicate (§5.3)
        let copy = dir.file("sub1/copy/IM_0001", &original_bytes);
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (7, 1, 6), "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate, w.changed), (0, 1, 0), "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'sub1/copy/IM_0001'"
            ),
            ["duplicate"],
            "{name}"
        );
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT instance_id FROM {source_file} WHERE path = 'sub1/copy/IM_0001'"
            ),
            Some(first_instance),
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'A.1'"
            ),
            2,
            "{name}"
        );

        // a file rewritten with another SOP instance: file_changed, the old
        // instance loses its file, the new one is ingested
        let changed = dir.path().join("sub1/IM_0002");
        let old_mtime = fs::metadata(&changed).unwrap().modified().unwrap();
        let p1 = [birth("19800101"), sex("M"), description("Brain")];
        fs::write(&changed, mr("A", "A.1", "A.1.9", "P1", &p1)).unwrap();
        fs::File::options()
            .write(true)
            .open(&changed)
            .unwrap()
            .set_modified(old_mtime + Duration::from_secs(10))
            .unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (7, 1, 6), "{name}");
        assert_eq!(r.kind("file_changed"), 1, "{name}");
        let sample = r
            .diagnostics
            .iter()
            .find(|d| d.kind == "file_changed")
            .map(|d| d.samples.clone())
            .unwrap();
        assert_eq!(sample, ["new_sop"], "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate, w.changed), (1, 0, 1), "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            6,
            "{name}"
        );
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT source_file_id FROM {instance} WHERE sop_instance_uid = 'A.1.2'"
            ),
            None,
            "{name}: the replaced instance has no file"
        );
        let new_instance = one(
            &mut reg,
            "SELECT id FROM {instance} WHERE sop_instance_uid = 'A.1.9'",
        );
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT instance_id FROM {source_file} WHERE path = 'sub1/IM_0002'"
            ),
            Some(new_instance),
            "{name}"
        );
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT source_file_id FROM {instance} WHERE sop_instance_uid = 'A.1.9'"
            ),
            Some(one(
                &mut reg,
                "SELECT id FROM {source_file} WHERE path = 'sub1/IM_0002'"
            )),
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'A.1'"
            ),
            3,
            "{name}"
        );

        // the duplicate goes: gone; back as it was: a duplicate again
        let copy_mtime = fs::metadata(&copy).unwrap().modified().unwrap();
        fs::remove_file(&copy).unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (6, 0, 6), "{name}");
        assert_eq!(r.written.as_ref().unwrap().gone, 1, "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'sub1/copy/IM_0001'"
            ),
            ["gone"],
            "{name}"
        );
        dir.file("sub1/copy/IM_0001", &original_bytes);
        fs::File::options()
            .write(true)
            .open(&copy)
            .unwrap()
            .set_modified(copy_mtime)
            .unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (7, 1, 6), "{name}");
        assert_eq!(r.kind("file_changed"), 0, "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate, w.gone), (0, 1, 0), "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'sub1/copy/IM_0001'"
            ),
            ["duplicate"],
            "{name}"
        );

        // the instance's own file goes and comes back: its own again, not a
        // duplicate of itself
        fs::remove_file(&original).unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!(r.written.as_ref().unwrap().gone, 1, "{name}");
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT source_file_id FROM {instance} WHERE sop_instance_uid = 'A.1.1'"
            ),
            Some(one(
                &mut reg,
                "SELECT id FROM {source_file} WHERE path = 'sub1/IM_0001'"
            )),
            "{name}: a gone file keeps its instance"
        );
        dir.file("sub1/IM_0001", &original_bytes);
        fs::File::options()
            .write(true)
            .open(&original)
            .unwrap()
            .set_modified(original_mtime)
            .unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (7, 1, 6), "{name}");
        assert_eq!(r.kind("file_changed"), 0, "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate, w.gone), (1, 0, 0), "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'sub1/IM_0001'"
            ),
            ["ingested"],
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            6,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'A.1'"
            ),
            3,
            "{name}"
        );
    }
}

#[test]
fn the_quarantine_is_kept_until_retried_or_the_file_changes() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap();

        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.quarantined, r.unchanged), (0, 6), "{name}");
        assert_eq!(r.written.as_ref().unwrap().quarantine_kept, 1, "{name}");

        s.retry_quarantine = true;
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.quarantined, r.unchanged), (1, 5), "{name}");
        assert_eq!(r.class("not_dicom"), 1, "{name}");
        assert_eq!(r.written.as_ref().unwrap().quarantine_kept, 0, "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'bad/readme.txt'"
            ),
            ["quarantined"],
            "{name}"
        );
        s.retry_quarantine = false;

        // the file becomes a DICOM file: read again without the flag
        dir.file("bad/readme.txt", &pet("B", "B.2", "B.2.2", "P2"));
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.parsed, r.quarantined, r.unchanged), (1, 0, 5), "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.quarantine_kept), (1, 0), "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'bad/readme.txt'"
            ),
            ["ingested"],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'B.2'"
            ),
            2,
            "{name}"
        );
    }
}

#[test]
fn a_restart_reads_everything_and_agrees_with_what_it_wrote() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap();

        s.restart = true;
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!(
            (r.seen, r.parsed, r.quarantined, r.unchanged),
            (6, 5, 1, 0),
            "{name}"
        );
        // the rows read back hash as the rows written: no disagreement on either backend
        assert!(r.diagnostics.is_empty(), "{name}: {:?}", r.diagnostics);
        let w = r.written.clone().unwrap();
        assert_eq!(w.ingested, 5, "{name}");
        assert_eq!(w.duplicate, 0, "{name}");
        assert_eq!(w.changed, 0, "{name}");
        assert_eq!(
            w.subjects_created + w.studies_created + w.series_created,
            0,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT n_instances FROM {series} ORDER BY series_instance_uid"
            ),
            [2, 1, 1, 1],
            "{name}: a restart does not count instances twice"
        );
        assert_eq!(
            texts(&mut reg, "SELECT status FROM {source_file} ORDER BY path"),
            [
                "quarantined",
                "ingested",
                "ingested",
                "ingested",
                "ingested",
                "ingested"
            ],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {source_file} WHERE batch_id = 2"
            ),
            6,
            "{name}"
        );
    }
}

#[test]
fn disagreements_are_diagnostics_not_errors() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("digest-disagree");
        // each disagreement is between two files, so the count is one
        // whichever file the walk brings first; a null (a/2's birth date,
        // a/3's description, c/1's both) is no value and disagrees with nothing
        // one study described two ways
        dir.file(
            "a/1",
            &mr(
                "A",
                "A.1",
                "A.1.1",
                "P1",
                &[birth("19800101"), description("Brain")],
            ),
        );
        dir.file(
            "a/2",
            &mr("A", "A.2", "A.2.1", "P1", &[description("Head")]),
        );
        // one subject born twice
        dir.file("a/3", &mr("A", "A.3", "A.3.1", "P1", &[birth("19800102")]));
        // one series under two studies
        dir.file("c/1", &mr("C", "A.1", "C.1.1", "P1", &[]));
        let s = settings(&dir);
        let mut reg = lab.open();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed), (4, 4), "{name}");
        assert_eq!(r.written.as_ref().unwrap().ingested, 4, "{name}");
        assert_eq!(
            r.kind("field_disagreement"),
            1,
            "{name}: {:?}",
            r.diagnostics
        );
        assert_eq!(
            r.kind("subject_field_disagreement"),
            1,
            "{name}: {:?}",
            r.diagnostics
        );
        assert_eq!(
            r.kind("series_multi_study"),
            1,
            "{name}: {:?}",
            r.diagnostics
        );
        let samples = |kind: &str| {
            r.diagnostics
                .iter()
                .find(|d| d.kind == kind)
                .map(|d| d.samples.clone())
                .unwrap_or_default()
        };
        let field = samples("field_disagreement");
        assert!(
            field == ["study.study_description=Aaaaa"] || field == ["study.study_description=Aaaa"],
            "{name}: {field:?}"
        );
        assert_eq!(
            samples("subject_field_disagreement"),
            ["subject.birth_date=9999-99-99"],
            "{name}"
        );
        assert_eq!(samples("series_multi_study"), ["series.study_id"], "{name}");
        // the first values stand: the study has one description, the subject
        // one birth date, the series one study
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 1, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {study}"), 2, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {series}"), 3, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            4,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'A.1'"
            ),
            2,
            "{name}"
        );
        let described = texts(
            &mut reg,
            "SELECT study_description FROM {study} WHERE study_instance_uid = 'A'",
        );
        assert!(
            described == ["Brain"] || described == ["Head"],
            "{name}: {described:?}"
        );
        let born = texts(&mut reg, "SELECT CAST(birth_date AS TEXT) FROM {subject}");
        assert!(
            born == ["1980-01-01"] || born == ["1980-01-02"],
            "{name}: {born:?}"
        );
        // the diagnostics are rows of the batch
        assert_eq!(
            texts(
                &mut reg,
                "SELECT kind FROM {diagnostic} WHERE batch_id = 1 ORDER BY kind"
            ),
            [
                "field_disagreement",
                "series_multi_study",
                "subject_field_disagreement"
            ],
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT count FROM {diagnostic} WHERE batch_id = 1 ORDER BY kind"
            ),
            [1, 1, 1],
            "{name}"
        );
    }
}

#[test]
fn a_fresh_job_holds_the_registry_and_a_stale_one_is_taken_over() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        let job = table("job");
        let insert = Insert::new(
            job,
            &[
                "kind",
                "name",
                "args",
                "state",
                "pid",
                "host",
                "started_at",
                "heartbeat_at",
            ],
        )
        .returning(&["id"]);
        let now = now_iso();
        let inserted = reg
            .store()
            .insert(
                &insert,
                &[vec![
                    Param::from("digest"),
                    Param::from("other"),
                    Param::from("{}"),
                    Param::from("running"),
                    Param::from(4242_i64),
                    Param::from("elsewhere"),
                    Param::from(now.as_str()),
                    Param::from(now.as_str()),
                ]],
            )
            .unwrap();
        let other = inserted[0].int(0).unwrap();

        let err = digest(&s, &mut reg).unwrap_err();
        assert!(
            matches!(err, DigestError::Busy { job_id, .. } if job_id == other),
            "{name}: {err}"
        );
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {job}"), 1, "{name}");

        // a heartbeat two minutes old: the job is marked failed and the run goes on
        reg.store()
            .update_by_id(
                job,
                &[("heartbeat_at", Param::from(iso_of(now_secs() - 120)))],
                "id",
                other,
            )
            .unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!(r.written.as_ref().unwrap().ingested, 5, "{name}");
        let states = rows(&mut reg, "SELECT state, error FROM {job} ORDER BY id");
        assert_eq!(states.len(), 2, "{name}");
        assert_eq!(states[0].text(0).unwrap(), "failed", "{name}");
        assert!(
            states[0]
                .text(1)
                .unwrap()
                .starts_with("stale: no heartbeat since "),
            "{name}: {:?}",
            states[0].get(1)
        );
        assert_eq!(states[1].text(0).unwrap(), "done", "{name}");
        assert!(matches!(states[1].get(1), Cell::Null), "{name}");
    }
}
