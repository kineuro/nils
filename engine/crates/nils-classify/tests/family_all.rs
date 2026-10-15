// SPDX-License-Identifier: AGPL-3.0-only

//! The acquisition family on a call over every stack, on both backends: a
//! stack known only by its provenance (a display composite) has its family,
//! and of two origin decisions of one rank on a scanner and axis the newer
//! stands.

use std::env;
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_classify::family::{families_of, in_force};
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_family_all";

fn postgres_dsn() -> Option<String> {
    env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
}

fn drop_schema(dsn: &str) {
    let mut store = Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop");
}

struct Lab {
    name: &'static str,
    home: Home,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            drop_schema(&dsn);
        }
    }
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("family-all-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-family-all-key").unwrap();
    home.init(&InitOptions {
        backend,
        dsn,
        schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
        scheme: Scheme::Blake2b32,
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

fn labs() -> Vec<Lab> {
    let mut out = vec![lab("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        drop_schema(&dsn);
        let mut l = lab("postgres", Backend::Postgres, Some(dsn));
        l._guard = Some(guard);
        out.push(l);
    }
    out
}

/// A registry of one MR stack of two images from one scanner, digested and
/// fingerprinted, with no route evidence about it.
fn prepared(lab: &Lab) -> (Registry, i64) {
    let tree = TempDir::new("family-all-tree");
    for i in 0..2 {
        let sop = format!("9.1.{}", i + 1);
        let mut e = synth::minimal_mr("9", "9.1", &sop);
        for (tag, vr, v) in [
            (tags::PATIENT_ID, VR::LO, "PX".to_string()),
            (tags::MANUFACTURER, VR::LO, "SIEMENS".to_string()),
            (tags::SERIES_DESCRIPTION, VR::LO, "plain".to_string()),
            (tags::SERIES_NUMBER, VR::IS, "1".to_string()),
            (
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                "1\\0\\0\\0\\1\\0".to_string(),
            ),
            (tags::IMAGE_POSITION_PATIENT, VR::DS, format!("0\\0\\{i}")),
            (tags::FRAME_OF_REFERENCE_UID, VR::UI, "9.0".to_string()),
        ] {
            e.push(synth::text(tag, vr, &v));
        }
        e.push(synth::num(tags::ROWS, VR::US, 64.0));
        e.push(synth::num(tags::COLUMNS, VR::US, 64.0));
        e.push(synth::text(
            tags::INSTANCE_NUMBER,
            VR::IS,
            &(i + 1).to_string(),
        ));
        tree.file(
            &format!("f/{sop}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    let mut reg = lab.home.open().unwrap();
    let mut s = nils_digest::Settings::new(tree.path());
    s.name = "t".into();
    s.workers = 2;
    s.walk_threads = 2;
    digest(&s, &mut reg).unwrap_or_else(|e| panic!("{}: {e}", lab.name));
    nils_classify::run(
        &mut reg,
        &nils_classify::Settings::default(),
        &Cancel::new(),
    )
    .unwrap();
    let store = reg.store();
    let sql = format!("SELECT id FROM {}", store.qualified("stack"));
    let stack = store.query(&sql, &[]).unwrap()[0].int(0).unwrap();
    (reg, stack)
}

#[test]
fn an_all_stacks_call_gives_a_provenance_only_stack_its_family() {
    for lab in labs() {
        let name = lab.name;
        let (mut reg, stack) = prepared(&lab);
        let provenance = |s: i64| (s == stack).then(|| "SyMRI".to_string());
        let all = families_of(reg.store(), None, None, &provenance).unwrap();
        assert_eq!(all.get(&stack), Some(&"symri"), "{name}");
        // the call over the stack itself agrees
        let some = families_of(reg.store(), None, Some(&[stack]), &provenance).unwrap();
        assert_eq!(some, all, "{name}");
    }
}

#[test]
fn of_two_origin_decisions_of_one_rank_the_newer_stands() {
    for lab in labs() {
        let name = lab.name;
        let (mut reg, stack) = prepared(&lab);
        let d = reg.store().qualified("decision");
        for value in ["brain", "spine"] {
            reg.store()
                .batch(&format!(
                    "INSERT INTO {d} (scope, ref, axis, value, actor, author_kind, decided_at) \
                     VALUES ('origin', 'manufacturer=siemens', 'body_part', '{value}', 'reader', \
                     'person', '2026-10-13T10:00:00Z')"
                ))
                .unwrap();
        }
        for stacks in [None, Some(vec![stack])] {
            let got = in_force(reg.store(), stacks.as_deref()).unwrap();
            assert_eq!(
                got.get(&stack).and_then(|f| f.get("body_part")),
                Some(&Some("spine".to_string())),
                "{name}"
            );
        }
    }
}
