// SPDX-License-Identifier: AGPL-3.0-only

//! Record 53 S2, end to end on both backends: the MRI pack's session pass
//! decides a GE susceptibility image whose own header is silent from a
//! computed output GE wrote beside it, flags every value it writes with tier
//! `session`, names the sibling in its evidence and asks about the answer;
//! and it leaves alone a stack with no such sibling, one whose sibling has
//! another frame of reference, and one whose sibling's series number is
//! unrelated. A second classification gives the same answer. Synthetic
//! headers only.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, Elem, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Row, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_session";

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
        let dir = TempDir::new("classify-session-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-session-key")
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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

fn rows(reg: &mut Registry, sql: &str) -> Vec<Row> {
    let mut text = sql.to_string();
    for t in [
        "classification_evidence",
        "classification_axis",
        "review_item",
        "stack",
        "series",
    ] {
        text = text.replace(&format!("{{{t}}}"), &reg.store().qualified(t));
    }
    reg.store().query(&text, &[]).unwrap()
}

/// One GE susceptibility series of two slices: ORIGINAL, no output token,
/// the geometry and frame of reference given.
fn ge_swi(
    study: &str,
    series: &str,
    number: i64,
    name: &str,
    frame: &str,
) -> Vec<(String, Vec<Elem>)> {
    (0..2)
        .map(|i| {
            let sop = format!("{series}.{}", i + 1);
            let mut e = synth::minimal_mr(study, series, &sop);
            for (tag, vr, v) in [
                (tags::PATIENT_ID, VR::LO, format!("P{study}")),
                (tags::MANUFACTURER, VR::LO, "GE MEDICAL SYSTEMS".into()),
                (tags::SERIES_DESCRIPTION, VR::LO, name.into()),
                (tags::SERIES_NUMBER, VR::IS, number.to_string()),
                (tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\OTHER".into()),
                (tags::SCANNING_SEQUENCE, VR::CS, "GR".into()),
                (tags::SEQUENCE_VARIANT, VR::CS, "SS\\SP".into()),
                (tags::MR_ACQUISITION_TYPE, VR::CS, "3D".into()),
                (tags::REPETITION_TIME, VR::DS, "40".into()),
                (tags::ECHO_TIME, VR::DS, "25".into()),
                (tags::FLIP_ANGLE, VR::DS, "15".into()),
                (tags::SLICE_THICKNESS, VR::DS, "2".into()),
                (tags::PIXEL_SPACING, VR::DS, "0.5\\0.5".into()),
                (
                    tags::IMAGE_ORIENTATION_PATIENT,
                    VR::DS,
                    "1\\0\\0\\0\\1\\0".into(),
                ),
                (
                    tags::IMAGE_POSITION_PATIENT,
                    VR::DS,
                    format!("0\\0\\{}", 2 * i),
                ),
                (tags::SLICE_LOCATION, VR::DS, format!("{}", 2 * i)),
                (tags::FRAME_OF_REFERENCE_UID, VR::UI, frame.into()),
            ] {
                e.push(synth::text(tag, vr, &v));
            }
            e.push(synth::num(tags::ROWS, VR::US, 256.0));
            e.push(synth::num(tags::COLUMNS, VR::US, 256.0));
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

/// Four sessions:
/// - S: the acquisition, series 5, and GE's computed `SWI:` output, series
///   500, of the same geometry and frame of reference. The pass decides 5.
/// - L: the acquisition alone. Nothing to read.
/// - F: the same pair in two frames of reference. Not the same acquisition.
/// - N: the same pair at series 5 and 9, unrelated numbers.
fn tree() -> TempDir {
    let dir = TempDir::new("classify-session");
    write(&dir, ge_swi("9.1", "9.1.1", 5, "Ax SWAN", "9.1.0"));
    write(&dir, ge_swi("9.1", "9.1.2", 500, "SWI: Ax SWAN", "9.1.0"));
    write(&dir, ge_swi("9.2", "9.2.1", 5, "Ax SWAN", "9.2.0"));
    write(&dir, ge_swi("9.3", "9.3.1", 5, "Ax SWAN", "9.3.0"));
    write(&dir, ge_swi("9.3", "9.3.2", 500, "SWI: Ax SWAN", "9.3.9"));
    write(&dir, ge_swi("9.4", "9.4.1", 5, "Ax SWAN", "9.4.0"));
    write(&dir, ge_swi("9.4", "9.4.2", 9, "SWI: Ax SWAN", "9.4.0"));
    dir
}

/// The axes in force of the one stack of a series: (axis, value, tier).
fn axes(reg: &mut Registry, series: &str) -> Vec<(String, String, String)> {
    rows(
        reg,
        &format!(
            "SELECT a.axis, COALESCE(a.value, ''), a.tier FROM {{classification_axis}} a \
             JOIN {{stack}} k ON k.id = a.stack_id JOIN {{series}} s ON s.id = k.series_id \
             WHERE s.series_instance_uid = '{series}' AND a.axis IN ('provenance', 'construct', 'base') \
             ORDER BY a.axis, a.value"
        ),
    )
    .iter()
    .map(|r| {
        (
            r.text(0).unwrap().to_string(),
            r.text(1).unwrap().to_string(),
            r.text(2).unwrap().to_string(),
        )
    })
    .collect()
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

fn classify(reg: &mut Registry, pack: &nils_pack::Pack) -> nils_classify::Classified {
    nils_classify::classify::classify(reg, pack, &Default::default(), &Cancel::new())
        .expect("classify")
}

#[test]
fn a_silent_ge_swi_is_decided_from_its_session_and_flagged() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    assert!(
        pack.passes.iter().any(|p| p.session().is_some()),
        "the MRI pack declares a session pass"
    );
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

        let report = classify(&mut reg, &pack);
        let ran = report
            .passes
            .iter()
            .find(|p| p.kind == "session_context")
            .unwrap_or_else(|| panic!("{name}: the session pass ran: {:?}", report.passes));
        assert_eq!(ran.targets, 4, "{name}: the four unprefixed acquisitions");
        assert_eq!(ran.decided, 1, "{name}: {ran:?}");
        assert_eq!(
            ran.by_method["ge_swan_beside_its_computed_output"], 1,
            "{name}"
        );
        assert_eq!(ran.by_method["no rule"], 3, "{name}");

        // S: decided from the session, every value flagged.
        let want = |v: &[(&str, &str, &str)]| -> Vec<(String, String, String)> {
            v.iter()
                .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
                .collect()
        };
        assert_eq!(
            axes(&mut reg, "9.1.1"),
            want(&[
                ("base", "T2*w", "session"),
                ("construct", "Magnitude", "session"),
                ("provenance", "RawRecon", "session"),
            ]),
            "{name}"
        );
        // The computed output is not a target and keeps the route's answer.
        assert!(
            axes(&mut reg, "9.1.2")
                .iter()
                .all(|(_, _, t)| t != "session"),
            "{name}"
        );
        // L, F and N keep the route's fallback: the processed image.
        for series in ["9.2.1", "9.3.1", "9.4.1"] {
            let a = axes(&mut reg, series);
            assert!(
                a.iter().all(|(_, _, t)| t != "session"),
                "{name}: {series}: {a:?}"
            );
            assert!(
                a.contains(&("construct".into(), "SWI".into(), "stated".into())),
                "{name}: {series}: {a:?}"
            );
        }

        // The evidence names the pass, the rule and the sibling by stack id.
        let target = stack_of(&mut reg, "9.1.1");
        let sibling = stack_of(&mut reg, "9.1.2");
        let ev = rows(
            &mut reg,
            &format!(
                "SELECT axis, tier, rule_set, rule, source, matched, pass, reference \
                 FROM {{classification_evidence}} WHERE stack_id = {target} AND tier = 'session' \
                 ORDER BY axis"
            ),
        );
        assert_eq!(ev.len(), 3, "{name}");
        for r in &ev {
            assert_eq!(r.text(2).unwrap(), "session_context", "{name}");
            assert_eq!(
                r.text(3).unwrap(),
                "ge_swan_beside_its_computed_output",
                "{name}"
            );
            assert_eq!(r.text(4).unwrap(), "session", "{name}");
            assert_eq!(
                r.text(5).unwrap(),
                format!("sibling stacks {sibling}"),
                "{name}"
            );
            assert_eq!(r.text(6).unwrap(), "session_context", "{name}");
            assert_eq!(r.text(7).unwrap(), "session", "{name}");
        }
        // Below the pass's threshold, so each answer is also a question
        // (one group per answer; the target is the only stack decided).
        let asked = rows(
            &mut reg,
            "SELECT kind FROM {review_item} WHERE kind LIKE '%:session' ORDER BY kind",
        );
        let kinds: Vec<&str> = asked.iter().map(|r| r.text(0).unwrap()).collect();
        assert_eq!(
            kinds,
            ["base:session", "construct:session", "provenance:session"],
            "{name}"
        );
        // A reader is told what decided it.
        assert_eq!(nils_pack::rules::basis_of("session"), "session");

        // Again: the same answer, and nothing more written.
        let again = classify(&mut reg, &pack);
        let ran = again
            .passes
            .iter()
            .find(|p| p.kind == "session_context")
            .expect("the session pass ran");
        assert_eq!(ran.decided, 1, "{name}");
        assert_eq!(
            axes(&mut reg, "9.1.1"),
            want(&[
                ("base", "T2*w", "session"),
                ("construct", "Magnitude", "session"),
                ("provenance", "RawRecon", "session"),
            ]),
            "{name}: a second run"
        );

        // A target that names no axis value outright is tried on every
        // stack, window by window, and decides the same.
        let full = full_scan_pack();
        let pack2 = nils_pack::load(&full, None).expect("the edited pack loads");
        assert!(
            pack2
                .passes
                .iter()
                .find(|p| p.session().is_some())
                .and_then(|p| p.target.as_ref())
                .is_some_and(|t| t.required_axes().is_empty())
        );
        let third = classify(&mut reg, &pack2);
        let ran = third
            .passes
            .iter()
            .find(|p| p.kind == "session_context")
            .expect("the session pass ran");
        assert_eq!((ran.targets, ran.decided), (4, 1), "{name}: {ran:?}");
        assert_eq!(
            axes(&mut reg, "9.1.1"),
            want(&[
                ("base", "T2*w", "session"),
                ("construct", "Magnitude", "session"),
                ("provenance", "RawRecon", "session"),
            ]),
            "{name}: a full scan"
        );
        let _ = std::fs::remove_dir_all(&full);
    }
}

/// The MRI pack, copied, with its session target's axis values written so
/// that none is required outright.
fn full_scan_pack() -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
    let to = std::env::temp_dir().join(format!("nils-session-full-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&to);
    copy(&packs(), &to);
    let p = to.join("passes/session.yml");
    let text = std::fs::read_to_string(&p).unwrap();
    let edited = text
        .replace(
            "      - {axis: provenance, is: SWIRecon}",
            "      - {any: [{axis: provenance, is: SWIRecon}]}",
        )
        .replace(
            "      - {axis: construct, is: SWI}",
            "      - {any: [{axis: construct, is: SWI}]}",
        );
    assert_ne!(edited, text);
    std::fs::write(&p, edited).unwrap();
    to
}
