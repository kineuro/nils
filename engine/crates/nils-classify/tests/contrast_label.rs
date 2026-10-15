// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 F2 and F5 (spec §12.3, gate test T27), end to end on both
//! backends: a classification labels every MR stack of every session; once
//! a session holds certified T1 answers, every stack in it carries the
//! post-contrast state, the minutes since injection, the label's source and
//! the 72-hour flag, with the checks run; and the subject step makes a
//! session after an earlier injection undetermined before its first post
//! T1. Synthetic headers only.

mod common;

use std::env;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, Elem, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Row, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_contrast";

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
        let dir = TempDir::new("classify-contrast-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-contrast-key")
            .unwrap();
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
}

fn drop_schema(dsn: &str) {
    let mut store = Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop");
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            drop_schema(&dsn);
        }
    }
}

fn labs() -> Vec<Lab> {
    let mut out = vec![Lab::new("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        drop_schema(&dsn);
        let mut lab = Lab::new("postgres", Backend::Postgres, Some(dsn));
        lab._guard = Some(guard);
        out.push(lab);
    }
    out
}

fn packs() -> PathBuf {
    common::shipped_mri()
}

fn rows(reg: &mut Registry, sql: &str) -> Vec<Row> {
    let mut text = sql.to_string();
    for t in ["contrast_label", "decision", "stack", "series"] {
        text = text.replace(&format!("{{{t}}}"), &reg.store().qualified(t));
    }
    reg.store().query(&text, &[]).unwrap()
}

/// What a series is, by its physics and its name.
#[derive(Clone, Copy)]
enum Kind {
    T1,
    Flair,
    Dwi,
}

/// One series of two images of patient `patient`, study `study` on `day`
/// (YYYYMMDD) at `hhmmss`, the second image 30 seconds later.
fn series(
    patient: &str,
    study: &str,
    day: &str,
    series: &str,
    number: i64,
    kind: Kind,
    hhmmss: u32,
) -> Vec<(String, Vec<Elem>)> {
    let (name, scanning, variant, acq, tr, te, ti) = match kind {
        Kind::T1 => (
            "t1_mprage_sag",
            "GR\\IR",
            "SP\\MP",
            "3D",
            "2300",
            "3",
            Some("900"),
        ),
        Kind::Flair => (
            "AX FLAIR",
            "SE\\IR",
            "SK\\SP",
            "2D",
            "9000",
            "90",
            Some("2500"),
        ),
        Kind::Dwi => ("ep2d_diff_tra", "EP", "SK\\SP", "2D", "5000", "90", None),
    };
    let clock = |s: u32| format!("{:02}{:02}{:02}", s / 10_000, (s / 100) % 100, s % 100);
    (0..2)
        .map(|i| {
            let sop = format!("{series}.{}", i + 1);
            let mut e = synth::minimal_mr(study, series, &sop);
            let at = clock(hhmmss + 30 * i);
            let mut fields = vec![
                (tags::PATIENT_ID, VR::LO, patient.to_string()),
                (tags::STUDY_DATE, VR::DA, day.to_string()),
                (tags::STUDY_TIME, VR::TM, "075000".to_string()),
                (tags::SERIES_DATE, VR::DA, day.to_string()),
                (tags::ACQUISITION_DATE, VR::DA, day.to_string()),
                (tags::ACQUISITION_TIME, VR::TM, at.clone()),
                (tags::CONTENT_DATE, VR::DA, day.to_string()),
                (tags::CONTENT_TIME, VR::TM, at),
                (tags::MANUFACTURER, VR::LO, "SIEMENS".into()),
                (tags::MAGNETIC_FIELD_STRENGTH, VR::DS, "3".into()),
                (tags::BODY_PART_EXAMINED, VR::CS, "HEAD".into()),
                (tags::SERIES_DESCRIPTION, VR::LO, name.into()),
                (tags::PROTOCOL_NAME, VR::LO, name.into()),
                (tags::SERIES_NUMBER, VR::IS, number.to_string()),
                (tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND".into()),
                (tags::SCANNING_SEQUENCE, VR::CS, scanning.into()),
                (tags::SEQUENCE_VARIANT, VR::CS, variant.into()),
                (tags::MR_ACQUISITION_TYPE, VR::CS, acq.into()),
                (tags::REPETITION_TIME, VR::DS, tr.into()),
                (tags::ECHO_TIME, VR::DS, te.into()),
                (tags::SLICE_THICKNESS, VR::DS, "1".into()),
                (tags::PIXEL_SPACING, VR::DS, "1\\1".into()),
                (
                    tags::IMAGE_ORIENTATION_PATIENT,
                    VR::DS,
                    "1\\0\\0\\0\\1\\0".into(),
                ),
                (tags::IMAGE_POSITION_PATIENT, VR::DS, format!("0\\0\\{}", i)),
                (tags::SLICE_LOCATION, VR::DS, format!("{i}")),
                (tags::FRAME_OF_REFERENCE_UID, VR::UI, format!("{study}.0")),
            ];
            if let Some(ti) = ti {
                fields.push((tags::INVERSION_TIME, VR::DS, ti.into()));
            }
            for (tag, vr, v) in fields {
                e.push(synth::text(tag, vr, &v));
            }
            e.push(synth::num(tags::ROWS, VR::US, 64.0));
            e.push(synth::num(tags::COLUMNS, VR::US, 64.0));
            e.push(synth::text(
                tags::INSTANCE_NUMBER,
                VR::IS,
                &(i + 1).to_string(),
            ));
            (sop, e)
        })
        .collect()
}

fn write(dir: &TempDir, files: Vec<(String, Vec<Elem>)>) {
    for (sop, e) in files {
        dir.file(
            &format!("f/{sop}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
}

/// Subject A, one session (study 8.1) on 2024-03-10: a FLAIR at 09:55, the
/// pre T1 at 10:00, a FLAIR at 10:05, the post T1 at 10:20, a FLAIR at
/// 10:30 and a DWI at 10:40, in series order.
///
/// Subject B, two sessions: study 8.2 on 2024-03-09 with a post T1, and
/// study 8.3 the next day like A's, pre T1 and post T1 with a FLAIR before
/// and after.
fn tree() -> TempDir {
    let dir = TempDir::new("classify-contrast");
    let a = |s, n, k, t| series("PA", "8.1", "20240310", s, n, k, t);
    write(&dir, a("8.1.1", 1, Kind::Flair, 95_500));
    write(&dir, a("8.1.2", 2, Kind::T1, 100_000));
    write(&dir, a("8.1.3", 3, Kind::Flair, 100_500));
    write(&dir, a("8.1.5", 5, Kind::T1, 102_000));
    write(&dir, a("8.1.6", 6, Kind::Flair, 103_000));
    write(&dir, a("8.1.7", 7, Kind::Dwi, 104_000));
    let b0 = |s, n, k, t| series("PB", "8.2", "20240309", s, n, k, t);
    write(&dir, b0("8.2.1", 1, Kind::Flair, 90_000));
    write(&dir, b0("8.2.2", 2, Kind::T1, 91_000));
    let b = |s, n, k, t| series("PB", "8.3", "20240310", s, n, k, t);
    write(&dir, b("8.3.1", 1, Kind::Flair, 95_500));
    write(&dir, b("8.3.2", 2, Kind::T1, 100_000));
    write(&dir, b("8.3.5", 5, Kind::T1, 102_000));
    write(&dir, b("8.3.6", 6, Kind::Flair, 103_000));
    dir
}

fn stack_of(reg: &mut Registry, series: &str) -> i64 {
    rows(
        reg,
        &format!(
            "SELECT k.id FROM {{stack}} k JOIN {{series}} s ON s.id = k.series_id \
             WHERE s.series_instance_uid = '{series}'"
        ),
    )[0]
    .int(0)
    .unwrap()
}

/// A person's settled read of a stack's post-contrast state.
fn read(reg: &mut Registry, stack: i64, given: bool) {
    let value = if given { "given" } else { "not_given" };
    let d = reg.store().qualified("decision");
    reg.store()
        .batch(&format!(
            "INSERT INTO {d} (scope, ref, axis, value, actor, author_kind, decided_at) \
             VALUES ('stack', '{stack}', 'post_contrast', '{value}', 'reader', 'person', '2026-10-13T10:00:00Z')"
        ))
        .unwrap();
}

/// A stack's label: state, minutes, source, 72-hour flag, reason, anchored.
type Got = (String, Option<f64>, String, String, Option<String>, bool);

fn label(reg: &mut Registry, stack: i64) -> Got {
    let r = rows(
        reg,
        &format!(
            "SELECT contrast_state, minutes_since_injection, contrast_source, contrast_prior_72h, reason, anchored \
             FROM {{contrast_label}} WHERE stack_id = {stack}"
        ),
    );
    let r = r
        .first()
        .unwrap_or_else(|| panic!("stack {stack} carries a label"));
    (
        r.text(0).unwrap().to_string(),
        r.opt_double(1).unwrap(),
        r.text(2).unwrap().to_string(),
        r.text(3).unwrap().to_string(),
        r.opt_text(4).unwrap().map(str::to_string),
        r.int(5).unwrap() != 0,
    )
}

#[test]
fn every_stack_of_an_anchored_session_carries_the_label_t27() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.home.open().unwrap();
        let mut s = nils_digest::Settings::new(dir.path());
        s.name = "t".into();
        s.workers = 2;
        s.walk_threads = 2;
        digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        nils_classify::run(
            &mut reg,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap_or_else(|e| panic!("{name}: {e}"));

        // the classification labelled every MR stack; with no T1 answer yet
        // none is anchored, and nothing is pre
        let c = report.contrast.clone().expect("the classification labels");
        assert_eq!(c["stacks"], 12, "{name}: {c}");
        assert_eq!(c["anchored_sessions"], 0, "{name}: {c}");
        let flair = stack_of(&mut reg, "8.1.1");
        let got = label(&mut reg, flair);
        assert_eq!(got.0, "undetermined", "{name}");
        assert_eq!(got.4.as_deref(), Some("C7_no_t1"), "{name}");
        assert!(!got.5, "{name}");

        // the reads: A's T1s, B's earlier post T1 and B's T1s the next day
        for (series, given) in [
            ("8.1.2", false),
            ("8.1.5", true),
            ("8.2.2", true),
            ("8.3.2", false),
            ("8.3.5", true),
        ] {
            let st = stack_of(&mut reg, series);
            read(&mut reg, st, given);
        }
        let done = nils_classify::contrast::label(reg.store(), &pack, None)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(done.anchored_sessions, 3, "{name}: {done:?}");

        // T27: every stack of a session with a T1 anchor carries the four
        // fields, and the minutes on each post stack
        let missing = rows(
            &mut reg,
            "SELECT COUNT(*) FROM {contrast_label} WHERE anchored = 1 AND (contrast_state IS NULL \
             OR contrast_source IS NULL OR contrast_prior_72h IS NULL \
             OR (contrast_state IN ('post', 'by_definition') AND minutes_since_injection IS NULL) \
             OR (contrast_state = 'undetermined' AND reason IS NULL))",
        );
        assert_eq!(missing[0].int(0).unwrap(), 0, "{name}");
        let anchored = rows(
            &mut reg,
            "SELECT COUNT(*) FROM {contrast_label} WHERE anchored = 1",
        );
        assert_eq!(anchored[0].int(0).unwrap(), 12, "{name}");

        // A: the time rule over the session's clock
        let want = |state: &str, minutes: Option<f64>, source: &str, reason: Option<&str>| -> Got {
            (
                state.to_string(),
                minutes,
                source.to_string(),
                "no".to_string(),
                reason.map(str::to_string),
                true,
            )
        };
        let a = |reg: &mut Registry, s: &str| {
            let st = stack_of(reg, s);
            label(reg, st)
        };
        assert_eq!(
            a(&mut reg, "8.1.1"),
            want("pre", None, "t1_anchor_time_rule", None),
            "{name}"
        );
        assert_eq!(
            a(&mut reg, "8.1.2"),
            want("pre", None, "reader", None),
            "{name}"
        );
        assert_eq!(
            a(&mut reg, "8.1.3"),
            want(
                "undetermined",
                None,
                "t1_anchor_time_rule",
                Some("R_window")
            ),
            "{name}"
        );
        assert_eq!(
            a(&mut reg, "8.1.5"),
            want("post", Some(0.0), "reader", None),
            "{name}"
        );
        assert_eq!(
            a(&mut reg, "8.1.6"),
            want("post", Some(10.0), "t1_anchor_time_rule", None),
            "{name}"
        );
        assert_eq!(
            a(&mut reg, "8.1.7"),
            want("post", Some(20.0), "t1_anchor_time_rule", None),
            "{name}"
        );

        // B: the post T1 of the day before makes the next session's pre
        // labels undetermined (C1) and flags every stack of it
        let early = a(&mut reg, "8.3.1");
        assert_eq!(early.0, "undetermined", "{name}");
        assert_eq!(early.4.as_deref(), Some("C1_prior_72h"), "{name}");
        assert_eq!(early.3, "yes", "{name}");
        let own_pre = a(&mut reg, "8.3.2");
        assert_eq!(own_pre.4.as_deref(), Some("C1_prior_72h"), "{name}");
        let late = a(&mut reg, "8.3.6");
        assert_eq!(
            (late.0.as_str(), late.1, late.3.as_str()),
            ("post", Some(10.0), "yes"),
            "{name}"
        );
        assert_eq!(
            a(&mut reg, "8.2.1").3,
            "no",
            "{name}: nothing before the first session"
        );

        // labelling again gives the same rows
        let before = rows(
            &mut reg,
            "SELECT stack_id, contrast_state, reason FROM {contrast_label} ORDER BY stack_id",
        )
        .iter()
        .map(|r| {
            format!(
                "{} {} {:?}",
                r.int(0).unwrap(),
                r.text(1).unwrap(),
                r.opt_text(2).unwrap()
            )
        })
        .collect::<Vec<_>>();
        nils_classify::contrast::label(reg.store(), &pack, None).unwrap();
        let after = rows(
            &mut reg,
            "SELECT stack_id, contrast_state, reason FROM {contrast_label} ORDER BY stack_id",
        )
        .iter()
        .map(|r| {
            format!(
                "{} {} {:?}",
                r.int(0).unwrap(),
                r.text(1).unwrap(),
                r.opt_text(2).unwrap()
            )
        })
        .collect::<Vec<_>>();
        assert_eq!(before, after, "{name}");
    }
}
