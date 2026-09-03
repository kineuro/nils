// SPDX-License-Identifier: AGPL-3.0-only

//! The fingerprint over a digested registry, on SQLite and, when
//! `NILS_TEST_POSTGRES_DSN` is set, on Postgres: one row per stack, the text
//! folded and not rewritten, the geometry from the stack's first instance by
//! SOP Instance UID, a second run that writes nothing, and a stack that gained
//! an instance derived again.

use std::env;
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_classify::{Settings, run};
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Row, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_test";

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => None,
    }
}

struct Lab {
    name: &'static str,
    home: Home,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Lab {
    fn new(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
        let dir = TempDir::new("classify-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-fixture-key")
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

fn rows(reg: &mut Registry, sql: &str) -> Vec<Row> {
    let mut text = sql.to_string();
    for t in ["stack_fingerprint", "instance", "stack", "series", "job"] {
        text = text.replace(&format!("{{{t}}}"), &reg.store().qualified(t));
    }
    reg.store().query(&text, &[]).unwrap()
}

fn one(reg: &mut Registry, sql: &str) -> i64 {
    rows(reg, sql)[0].int(0).unwrap()
}

fn text_at(reg: &mut Registry, sql: &str) -> Option<String> {
    rows(reg, sql)[0].opt_text(0).unwrap().map(str::to_string)
}

fn elem(tag: dicom_core::Tag, vr: VR, v: &str) -> synth::Elem {
    synth::text(tag, vr, v)
}

fn mr(study: &str, series: &str, sop: &str, patient: &str, extra: &[synth::Elem]) -> Vec<u8> {
    let mut e = synth::minimal_mr(study, series, sop);
    e.push(elem(tags::PATIENT_ID, VR::LO, patient));
    e.extend(extra.iter().cloned());
    synth::part10(&MetaFields::mr(sop), &e, true)
}

fn settings(dir: &TempDir) -> nils_digest::Settings {
    let mut s = nils_digest::Settings::new(dir.path());
    s.name = "t".to_string();
    s.workers = 2;
    s.walk_threads = 2;
    s.batch_rows = 2;
    s
}

/// A tree whose one series carries text worth folding and two instances whose
/// SOP Instance UIDs order the opposite way to their file names.
fn tree() -> TempDir {
    let dir = TempDir::new("fingerprint");
    let common = [
        elem(tags::SERIES_DESCRIPTION, VR::LO, "  Ax   T2  FLAIR \n"),
        elem(tags::PROTOCOL_NAME, VR::LO, "RÖR PÅ DXSIN SE PÄRM"),
        elem(tags::SEQUENCE_NAME, VR::SH, "*tse2d1_5"),
        elem(tags::BODY_PART_EXAMINED, VR::CS, "BRAIN"),
        elem(tags::SCANNING_SEQUENCE, VR::CS, "SE\\IR"),
        elem(tags::SEQUENCE_VARIANT, VR::CS, "SK"),
        elem(tags::SCAN_OPTIONS, VR::CS, "FS"),
        elem(tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
        elem(tags::INVERSION_TIME, VR::DS, "2500"),
        elem(tags::ECHO_TIME, VR::DS, "92"),
        elem(tags::REPETITION_TIME, VR::DS, "9000"),
        elem(tags::FLIP_ANGLE, VR::DS, "150"),
        elem(tags::MAGNETIC_FIELD_STRENGTH, VR::DS, "3"),
        elem(tags::CONTRAST_BOLUS_AGENT, VR::LO, "Dotarem"),
        elem(tags::CONTRAST_BOLUS_TOTAL_DOSE, VR::DS, "15"),
        synth::num(tags::ROWS, VR::US, 256.0),
        synth::num(tags::COLUMNS, VR::US, 256.0),
    ];
    // The file named 1 has the later UID, so a fingerprint that took the first
    // row inserted would read its geometry and not the other's.
    let mut a = common.to_vec();
    a.push(elem(tags::PIXEL_SPACING, VR::DS, "0.5\\0.5"));
    let mut b = common.to_vec();
    b.push(elem(tags::PIXEL_SPACING, VR::DS, "0.25\\0.25"));
    dir.file("s/1", &mr("A", "A.1", "A.1.9", "P1", &a));
    dir.file("s/2", &mr("A", "A.1", "A.1.1", "P1", &b));
    dir
}

#[test]
fn one_row_per_stack_with_the_text_folded_and_not_rewritten() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap();

        let report = run(&mut reg, &Settings::default(), &Cancel::new()).unwrap();
        assert_eq!(report.read, 1, "{name}: one stack");
        assert_eq!(report.written, 1, "{name}");
        assert_eq!(report.skipped, 0, "{name}");

        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {stack_fingerprint}"),
            1,
            "{name}"
        );
        // folded: the runs of whitespace are gone, the case is not
        assert_eq!(
            text_at(
                &mut reg,
                "SELECT text_series_description FROM {stack_fingerprint}"
            )
            .as_deref(),
            Some("Ax T2 FLAIR"),
            "{name}"
        );
        // the phrase a pack removes case-sensitively survives the fingerprint
        assert_eq!(
            text_at(
                &mut reg,
                "SELECT text_protocol_name FROM {stack_fingerprint}"
            )
            .as_deref(),
            Some("RÖR PÅ DXSIN SE PÄRM"),
            "{name}"
        );
        // and nothing is rewritten: no star, no expansion, no token dropped
        let all = text_at(&mut reg, "SELECT text_all FROM {stack_fingerprint}").unwrap();
        assert!(all.starts_with("Ax T2 FLAIR RÖR"), "{name}: {all}");
        assert!(all.contains("*tse2d1_5"), "{name}: {all}");

        // the physics come across typed
        assert_eq!(
            one(
                &mut reg,
                "SELECT CAST(inversion_time AS INTEGER) FROM {stack_fingerprint}"
            ),
            2500,
            "{name}"
        );
        assert_eq!(
            text_at(
                &mut reg,
                "SELECT scanning_sequence FROM {stack_fingerprint}"
            )
            .as_deref(),
            Some("SE\\IR"),
            "{name}"
        );
        assert_eq!(
            text_at(&mut reg, "SELECT text_contrast FROM {stack_fingerprint}").as_deref(),
            Some("Dotarem dose 15"),
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT stacks_in_series FROM {stack_fingerprint}"),
            1,
            "{name}"
        );
    }
}

#[test]
fn the_geometry_comes_from_the_first_instance_by_uid_not_by_row() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap();
        run(&mut reg, &Settings::default(), &Cancel::new()).unwrap();

        // A.1.1 sorts before A.1.9 and carries 0.25 spacing.
        assert_eq!(
            text_at(&mut reg, "SELECT pixel_spacing FROM {stack_fingerprint}").as_deref(),
            Some("0.25\\0.25"),
            "{name}: the lowest SOP Instance UID decides, not the walk"
        );
    }
}

#[test]
fn a_second_run_writes_nothing_and_force_writes_again() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap();

        run(&mut reg, &Settings::default(), &Cancel::new()).unwrap();
        let again = run(&mut reg, &Settings::default(), &Cancel::new()).unwrap();
        assert_eq!(again.read, 1, "{name}");
        assert_eq!(again.skipped, 1, "{name}");
        assert_eq!(again.written, 0, "{name}");

        let forced = run(
            &mut reg,
            &Settings {
                force: true,
                ..Settings::default()
            },
            &Cancel::new(),
        )
        .unwrap();
        assert_eq!(forced.written, 1, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {stack_fingerprint}"),
            1,
            "{name}: force overwrites, it does not duplicate"
        );
    }
}

#[test]
fn a_stack_that_gained_an_instance_is_derived_again() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap();
        run(&mut reg, &Settings::default(), &Cancel::new()).unwrap();

        // a third instance of the same stack, with the lowest UID of all
        let mut c = vec![
            elem(tags::SERIES_DESCRIPTION, VR::LO, "  Ax   T2  FLAIR \n"),
            elem(tags::SCANNING_SEQUENCE, VR::CS, "SE\\IR"),
            elem(tags::SEQUENCE_VARIANT, VR::CS, "SK"),
            elem(tags::INVERSION_TIME, VR::DS, "2500"),
            elem(tags::ECHO_TIME, VR::DS, "92"),
            elem(tags::REPETITION_TIME, VR::DS, "9000"),
            elem(tags::FLIP_ANGLE, VR::DS, "150"),
        ];
        c.push(synth::num(tags::ROWS, VR::US, 256.0));
        c.push(synth::num(tags::COLUMNS, VR::US, 256.0));
        c.push(elem(tags::PIXEL_SPACING, VR::DS, "0.125\\0.125"));
        dir.file("s/3", &mr("A", "A.1", "A.1.0", "P1", &c));
        digest(&settings(&dir), &mut reg).unwrap();

        let after = run(&mut reg, &Settings::default(), &Cancel::new()).unwrap();
        assert_eq!(
            after.written, 1,
            "{name}: the instance count moved, so the fingerprint is stale"
        );
        assert_eq!(after.skipped, 0, "{name}");
        assert_eq!(
            text_at(&mut reg, "SELECT pixel_spacing FROM {stack_fingerprint}").as_deref(),
            Some("0.125\\0.125"),
            "{name}"
        );
    }
}

#[test]
fn the_run_is_a_job_the_registry_records() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap();
        let report = run(&mut reg, &Settings::default(), &Cancel::new()).unwrap();
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {job} WHERE kind = 'fingerprint' AND state = 'done'"
            ),
            1,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT job_id FROM {stack_fingerprint}"),
            report.job_id,
            "{name}: the row says which job made it"
        );
    }
}
