// SPDX-License-Identifier: AGPL-3.0-only

//! The lab the digest tests share: a registry home on SQLite and, when
//! `NILS_TEST_POSTGRES_DSN` is set, on Postgres; the queries; the synthetic
//! files.

#![allow(dead_code)]

use std::env;
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::Settings;
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Row, Scheme, Store};

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
pub struct Lab {
    pub name: &'static str,
    pub home: Home,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

/// The key every lab registry derives its pseudonyms from.
pub const KEY: &[u8] = b"nils-digest-fixture-key";

impl Lab {
    fn new(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
        Lab::with(name, backend, dsn, Scheme::DEFAULT, 12, KEY)
    }

    fn with(
        name: &'static str,
        backend: Backend,
        dsn: Option<String>,
        scheme: Scheme,
        display_length: usize,
        key: &[u8],
    ) -> Lab {
        let dir = TempDir::new("digest-home");
        let home = Home::new(dir.path());
        home.keys(None).add("k", key).unwrap();
        home.init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme,
            key: "k".to_string(),
            display_length,
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

    pub fn open(&self) -> Registry {
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
pub fn labs() -> Vec<Lab> {
    labs_with(Scheme::DEFAULT, 12)
}

/// The labs on a pseudonym scheme and display length of the test's choosing.
pub fn labs_with(scheme: Scheme, display_length: usize) -> Vec<Lab> {
    labs_keyed(scheme, display_length, KEY)
}

/// The labs on a scheme, a display length and a pseudonym key.
pub fn labs_keyed(scheme: Scheme, display_length: usize, key: &[u8]) -> Vec<Lab> {
    let mut out = vec![Lab::with(
        "sqlite",
        Backend::Sqlite,
        None,
        scheme,
        display_length,
        key,
    )];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        drop_schemas(&dsn);
        let mut lab = Lab::with(
            "postgres",
            Backend::Postgres,
            Some(dsn),
            scheme,
            display_length,
            key,
        );
        lab._guard = Some(guard);
        out.push(lab);
    }
    out
}

/// A query with `{table}` placeholders for the qualified table names.
pub fn rows(reg: &mut Registry, sql: &str) -> Vec<Row> {
    let mut text = sql.to_string();
    for t in [
        "source_file",
        "instance",
        "series_mr",
        "series_ct",
        "series_pet",
        "series",
        "stack",
        "study",
        "subject",
        "diagnostic",
        "ingest_batch",
        "job",
        "source",
        "review_item",
    ] {
        text = text.replace(&format!("{{{t}}}"), &reg.store().qualified(t));
    }
    reg.store().query(&text, &[]).unwrap()
}

pub fn one(reg: &mut Registry, sql: &str) -> i64 {
    rows(reg, sql)[0].int(0).unwrap()
}

pub fn ints(reg: &mut Registry, sql: &str) -> Vec<i64> {
    rows(reg, sql).iter().map(|r| r.int(0).unwrap()).collect()
}

pub fn texts(reg: &mut Registry, sql: &str) -> Vec<String> {
    rows(reg, sql)
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect()
}

pub fn opt_int(reg: &mut Registry, sql: &str) -> Option<i64> {
    rows(reg, sql)[0].opt_int(0).unwrap()
}

pub fn settings(dir: &TempDir) -> Settings {
    let mut s = Settings::new(dir.path());
    s.name = "t".to_string();
    s.workers = 2;
    s.walk_threads = 2;
    // several batches for six files
    s.batch_rows = 2;
    s
}

pub fn with_patient(mut elems: Vec<synth::Elem>, id: &str) -> Vec<synth::Elem> {
    elems.push(synth::text(tags::PATIENT_ID, VR::LO, id));
    elems
}

pub fn mr(study: &str, series: &str, sop: &str, patient: &str, extra: &[synth::Elem]) -> Vec<u8> {
    let mut e = with_patient(synth::minimal_mr(study, series, sop), patient);
    e.extend(extra.iter().cloned());
    synth::part10(&MetaFields::mr(sop), &e, true)
}

pub fn ct(study: &str, series: &str, sop: &str, patient: &str, extra: &[synth::Elem]) -> Vec<u8> {
    let mut e = with_patient(synth::minimal_ct(study, series, sop), patient);
    e.extend(extra.iter().cloned());
    synth::part10(&MetaFields::ct(sop), &e, true)
}

pub fn pet(study: &str, series: &str, sop: &str, patient: &str) -> Vec<u8> {
    let e = with_patient(synth::minimal_pet(study, series, sop), patient);
    synth::part10(&MetaFields::pet(sop), &e, true)
}

pub fn birth(date: &str) -> synth::Elem {
    synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, date)
}

pub fn sex(s: &str) -> synth::Elem {
    synth::text(tags::PATIENT_SEX, VR::CS, s)
}

pub fn description(s: &str) -> synth::Elem {
    synth::text(tags::STUDY_DESCRIPTION, VR::LO, s)
}

/// Two subjects, two studies, four series, five instances, one refused file.
pub fn tree() -> TempDir {
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
