// SPDX-License-Identifier: AGPL-3.0-only

//! Classifying a digested registry: the verdict, the evidence that made it,
//! and the decision that outranks it.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::store::{Insert, Param};
use nils_registry::{Backend, Registry, Row, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_run";

fn postgres_dsn() -> Option<String> {
    env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
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
        home.keys(None).add("k", b"nils-classify-run-key").unwrap();
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
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
            store
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
                ))
                .expect("drop");
        }
    }
}

fn labs() -> Vec<Lab> {
    let mut out = vec![Lab::new("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .expect("drop");
        let mut lab = Lab::new("postgres", Backend::Postgres, Some(dsn));
        lab._guard = Some(guard);
        out.push(lab);
    }
    out
}

fn rows(reg: &mut Registry, sql: &str) -> Vec<Row> {
    let mut text = sql.to_string();
    for t in [
        "classification_evidence",
        "classification_axis",
        "classification",
        "stack_fingerprint",
        "review_item",
        "decision",
        "stack",
    ] {
        text = text.replace(&format!("{{{t}}}"), &reg.store().qualified(t));
    }
    reg.store().query(&text, &[]).unwrap()
}

fn one(reg: &mut Registry, sql: &str) -> i64 {
    rows(reg, sql)[0].int(0).unwrap()
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

fn elem(tag: dicom_core::Tag, vr: VR, v: &str) -> synth::Elem {
    synth::text(tag, vr, v)
}

/// One unmistakable stack: a Siemens MPRAGE.
fn tree() -> TempDir {
    let dir = TempDir::new("classify");
    let mut e = synth::minimal_mr("A", "A.1", "A.1.1");
    e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
    e.extend([
        elem(tags::SERIES_DESCRIPTION, VR::LO, "sag T1 mprage"),
        elem(tags::SCANNING_SEQUENCE, VR::CS, "GR"),
        elem(tags::SEQUENCE_VARIANT, VR::CS, "SK\\SP\\MP"),
        elem(tags::SEQUENCE_NAME, VR::SH, "*tfl3d1_16"),
        elem(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
        elem(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND\\NORM"),
        elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
    ]);
    dir.file("s/1", &synth::part10(&MetaFields::mr("A.1.1"), &e, true));
    dir
}

fn prepare(lab: &Lab, dir: &TempDir) -> Registry {
    let mut reg = lab.home.open().unwrap();
    let mut s = nils_digest::Settings::new(dir.path());
    s.name = "t".into();
    s.workers = 2;
    s.walk_threads = 2;
    digest(&s, &mut reg).unwrap();
    nils_classify::run(
        &mut reg,
        &nils_classify::Settings::default(),
        &Cancel::new(),
    )
    .unwrap();
    reg
}

#[test]
fn a_verdict_is_written_with_the_evidence_that_made_it() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(report.written, 1, "{name}");
        assert_eq!(report.no_pack, 0, "{name}");
        assert!(report.evidence > 0, "{name}");

        let got: Vec<(String, String)> = rows(
            &mut reg,
            "SELECT axis, COALESCE(value, '') FROM {classification_axis} ORDER BY axis",
        )
        .iter()
        .map(|r| (r.text(0).unwrap().into(), r.text(1).unwrap().into()))
        .collect();
        let by = |a: &str| {
            got.iter()
                .find(|(x, _)| x == a)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(by("technique"), "MPRAGE", "{name}: {got:?}");
        assert_eq!(by("base"), "T1w", "{name}");
        assert_eq!(by("directory_type"), "anat", "{name}");

        // and the evidence says why, by rule set, rule and what matched
        let ev: Vec<(String, String, String)> = rows(
            &mut reg,
            "SELECT axis, rule_set, COALESCE(matched, '') FROM {classification_evidence} WHERE axis = 'technique'",
        )
        .iter()
        .map(|r| {
            (
                r.text(0).unwrap().into(),
                r.text(1).unwrap().into(),
                r.text(2).unwrap().into(),
            )
        })
        .collect();
        assert_eq!(ev.len(), 1, "{name}: {ev:?}");
        assert_eq!(ev[0].1, "technique", "{name}");
        assert!(
            !ev[0].2.is_empty(),
            "{name}: the evidence names what matched"
        );

        // the row says which pack judged it, which is what makes a
        // re-classification a diff rather than an overwrite
        assert_eq!(
            rows(&mut reg, "SELECT pack, pack_version FROM {classification}")[0]
                .text(1)
                .unwrap(),
            pack.version.to_string(),
            "{name}"
        );
    }
}

#[test]
fn a_decision_outranks_the_rule_and_survives_a_re_classification() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let stack = one(&mut reg, "SELECT stack_id FROM {classification}");

        // A person says it is a T2w, and disagrees with the rule.
        reg.store()
            .insert(
                &Insert::new(
                    nils_registry::schema::table("decision"),
                    &[
                        "scope",
                        "ref",
                        "axis",
                        "value",
                        "actor",
                        "why",
                        "decided_at",
                    ],
                ),
                &[vec![
                    Param::from("stack"),
                    Param::from(stack.to_string()),
                    Param::from("base"),
                    Param::from("T2w"),
                    Param::from("a person"),
                    Param::from("checked by eye"),
                    Param::from(nils_registry::time::now_iso()),
                ]],
            )
            .unwrap();

        let again =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(
            rows(
                &mut reg,
                "SELECT value, tier FROM {classification_axis} WHERE axis = 'base'"
            )[0]
            .text(0)
            .unwrap(),
            "T2w",
            "{name}: the decision wins"
        );
        assert_eq!(
            rows(
                &mut reg,
                "SELECT value, tier FROM {classification_axis} WHERE axis = 'base'"
            )[0]
            .text(1)
            .unwrap(),
            "decision",
            "{name}: and the row says so"
        );
        assert!(
            again.review_items > 0,
            "{name}: the rule still disagrees, so a person is told"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'base:decision'"
            ),
            1,
            "{name}"
        );

        // The evidence still records what the rule said, so the disagreement
        // is legible rather than lost.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_evidence} WHERE axis = 'base' AND value = 'T1w'"
            ),
            1,
            "{name}"
        );
    }
}

#[test]
fn a_modality_the_pack_does_not_judge_is_said_so_and_not_guessed() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("classify-ct");
        let mut e = synth::minimal_ct("B", "B.1", "B.1.1");
        e.push(elem(tags::PATIENT_ID, VR::LO, "P2"));
        dir.file("s/1", &synth::part10(&MetaFields::ct("B.1.1"), &e, true));
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(report.no_pack, 1, "{name}");
        assert_eq!(report.written, 0, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {review_item}"),
            0,
            "{name}: no pack is an outcome, not a question for a person"
        );
    }
}

#[test]
fn a_decision_about_a_provenance_governs_every_stack_of_it_until_one_is_looked_at() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        let stack = {
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
            one(&mut reg, "SELECT stack_id FROM {classification}")
        };
        let decide = |reg: &mut Registry, scope: &str, reference: &str, value: &str| {
            reg.store()
                .insert(
                    &Insert::new(
                        nils_registry::schema::table("decision"),
                        &["scope", "ref", "axis", "value", "actor", "decided_at"],
                    ),
                    &[vec![
                        Param::from(scope),
                        Param::from(reference),
                        Param::from("base"),
                        Param::from(value),
                        Param::from("a person"),
                        Param::from(nils_registry::time::now_iso()),
                    ]],
                )
                .unwrap();
        };
        let base = |reg: &mut Registry| -> String {
            rows(
                reg,
                "SELECT value FROM {classification_axis} WHERE axis = 'base'",
            )[0]
            .text(0)
            .unwrap()
            .to_string()
        };

        // The whole scanner is called wrong, and every stack it made follows.
        decide(&mut reg, "provenance", "manufacturer=synthetic", "PDw");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(base(&mut reg), "PDw", "{name}: the provenance decides");

        // Someone looks at one series of it and says otherwise, and the
        // narrower call wins where it applies.
        let series = one(&mut reg, "SELECT series_id FROM {stack} WHERE id = 1");
        decide(&mut reg, "series", &series.to_string(), "T2w");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(base(&mut reg), "T2w", "{name}: the series is closer");

        // And this one stack is closer still.
        decide(&mut reg, "stack", &stack.to_string(), "FLAIR");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(base(&mut reg), "FLAIR", "{name}: the stack is closest");
    }
}
