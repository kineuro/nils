// SPDX-License-Identifier: AGPL-3.0-only

//! Record 53 S1, end to end: the MR Pulse Sequence module's mechanism
//! attributes, the MR Modifier group's spoiling and inversion recovery, and
//! the SOP class reach a pack under their names. An enhanced MR object writes
//! the module at the top level and the modifier in its shared functional
//! groups; an MR spectroscopy object is told from an image by its SOP class
//! alone; a classic image has none of them, and every one reads empty.
//! Synthetic headers only.

use std::env;
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::read::EXPLICIT_VR_LE;
use nils_dicom::synth::{self, Elem, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_pack::Evaluated;
use nils_pack::stack::field_index;
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_mechanism_fields";

const ENHANCED: &str = "1.2.840.10008.5.1.4.1.1.4.1";
const SPECTROSCOPY: &str = "1.2.840.10008.5.1.4.1.1.4.2";
const CLASSIC: &str = "1.2.840.10008.5.1.4.1.1.4";

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
        let dir = TempDir::new("classify-mechanism-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-mechanism-key")
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

/// A pack whose flags read the new fields (a text is read lower-cased), as MRI pack 0.19 does: the
/// deep research's case 22 table in miniature, and case 16's SOP class.
fn pack_dir() -> TempDir {
    let d = TempDir::new("classify-mechanism-pack");
    let file = |name: &str, body: &str| {
        d.file(name, body.as_bytes());
    };
    file(
        "pack.yml",
        "pack: t\nversion: 0.1.0\ncontract: 1\nmodality: MR\nflags: [flags.yml]\naxes: [axes/kind.yml]\norder: [kind]\nreview:\n  low_confidence: {default: 0.5}\n",
    );
    file(
        "flags.yml",
        "flags:
  spectroscopy_object: {text: sop_class_uid, equals: '1.2.840.10008.5.1.4.1.1.4.2'}
  spin: {text: echo_pulse_sequence, equals: spin}
  train: {text: multiple_spin_echo, equals: 'yes'}
  epi: {text: echo_planar_pulse_sequence, equals: 'yes'}
  full: {text: segmented_k_space_traversal, equals: full}
  tse: {all: [spin, train, {not: epi}, {not: full}]}
  inverted: {text: inversion_recovery, equals: 'yes'}
  unspoiled: {text: spoiling, equals: none}
  radial: {text: geometry_of_k_space_traversal, equals: radial}
  tof: {text: time_of_flight_contrast, equals: 'yes'}
  pc: {text: phase_contrast, equals: 'yes'}
  asl: {field: arterial_spin_labeling_contrast, present: true}
  bssfp: {text: steady_state_pulse_sequence, equals: free_precession}
",
    );
    file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nvalues:\n  'MRS': {detection: {exclusive: spectroscopy_object}}\n  'TSE': {detection: {exclusive: tse}}\n",
    );
    file(
        "corpus/cases.yml",
        "cases:
  - name: an MR spectroscopy object is told by its SOP class
    stack: {sop_class_uid: '1.2.840.10008.5.1.4.1.1.4.2'}
    flags: {spectroscopy_object: true}
    axes: {kind: MRS}
  - name: a spin echo train without EPI is a TSE
    stack: {echo_pulse_sequence: SPIN, multiple_spin_echo: 'YES', echo_planar_pulse_sequence: 'NO', segmented_k_space_traversal: PARTIAL}
    flags: {tse: true}
    axes: {kind: TSE}
",
    );
    d
}

/// An enhanced 3D inversion-recovery TSE with no ScanningSequence: the
/// module at the top level, the modifier in the shared groups.
fn enhanced(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    let shared = vec![synth::fg(
        tags::MR_MODIFIER_SEQUENCE,
        vec![
            synth::text(tags::INVERSION_RECOVERY, VR::CS, "YES"),
            synth::text(tags::SPOILING, VR::CS, "NONE"),
        ],
    )];
    let frames = vec![
        vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")],
        vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")],
    ];
    let mut e = synth::enhanced_mr(study, series, sop, shared, frames);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
    e.push(synth::text(tags::MANUFACTURER, VR::LO, "Philips"));
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\M\\NONE",
    ));
    e.push(synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"));
    e.push(synth::text(tags::PULSE_SEQUENCE_NAME, VR::SH, "TSE"));
    for (tag, value) in [
        (tags::ECHO_PULSE_SEQUENCE, "SPIN"),
        (tags::MULTIPLE_SPIN_ECHO, "YES"),
        (tags::ECHO_PLANAR_PULSE_SEQUENCE, "NO"),
        (tags::STEADY_STATE_PULSE_SEQUENCE, "NONE"),
        (tags::PHASE_CONTRAST, "NO"),
        (tags::TIME_OF_FLIGHT_CONTRAST, "NO"),
        (tags::GEOMETRY_OF_K_SPACE_TRAVERSAL, "RECTILINEAR"),
        (tags::SEGMENTED_K_SPACE_TRAVERSAL, "PARTIAL"),
    ] {
        e.push(synth::text(tag, VR::CS, value));
    }
    e
}

/// A single-voxel spectroscopy object: no pixels, one frame, the module's
/// attributes as the MR Spectroscopy Pulse Sequence module writes them.
fn spectroscopy(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    let mut e = vec![
        synth::text(tags::SOP_CLASS_UID, VR::UI, SPECTROSCOPY),
        synth::text(tags::SOP_INSTANCE_UID, VR::UI, sop),
        synth::text(tags::STUDY_INSTANCE_UID, VR::UI, study),
        synth::text(tags::SERIES_INSTANCE_UID, VR::UI, series),
        synth::text(tags::MODALITY, VR::CS, "MR"),
        synth::text(tags::PATIENT_ID, VR::LO, "P1"),
        synth::text(tags::MANUFACTURER, VR::LO, "GE MEDICAL SYSTEMS"),
        synth::text(
            tags::IMAGE_TYPE,
            VR::CS,
            "ORIGINAL\\PRIMARY\\SPECTROSCOPY\\NONE",
        ),
        synth::text(tags::NUMBER_OF_FRAMES, VR::IS, "1"),
        synth::text(tags::ECHO_PULSE_SEQUENCE, VR::CS, "SPIN"),
        synth::text(tags::MULTIPLE_SPIN_ECHO, VR::CS, "NO"),
    ];
    e.push(synth::seq(
        tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
        vec![vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")]],
    ));
    e
}

/// A classic image, which writes none of the module.
fn classic(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    let mut e = synth::minimal_mr(study, series, sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
    e.push(synth::text(tags::MANUFACTURER, VR::LO, "SIEMENS"));
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\M\\ND",
    ));
    e.push(synth::text(tags::SCANNING_SEQUENCE, VR::CS, "SE"));
    e
}

fn tree() -> TempDir {
    let dir = TempDir::new("classify-mechanism");
    dir.file(
        "enhanced/1",
        &synth::part10(
            &MetaFields::with(EXPLICIT_VR_LE, ENHANCED, "B.1.1"),
            &enhanced("B", "B.1", "B.1.1"),
            true,
        ),
    );
    dir.file(
        "mrs/1",
        &synth::part10(
            &MetaFields::with(EXPLICIT_VR_LE, SPECTROSCOPY, "B.2.1"),
            &spectroscopy("B", "B.2", "B.2.1"),
            true,
        ),
    );
    dir.file(
        "classic/1",
        &synth::part10(
            &MetaFields::with(EXPLICIT_VR_LE, CLASSIC, "B.3.1"),
            &classic("B", "B.3", "B.3.1"),
            true,
        ),
    );
    dir
}

fn stack_of_series(reg: &mut Registry, series: &str) -> i64 {
    let store = reg.store();
    let sql = format!(
        "SELECT k.id FROM {} k JOIN {} s ON s.id = k.series_id WHERE s.series_instance_uid = '{series}'",
        store.qualified("stack"),
        store.qualified("series"),
    );
    let rows = store.query(&sql, &[]).unwrap();
    assert!(!rows.is_empty(), "no stack for series {series}");
    rows[0].int(0).unwrap()
}

const MECHANISM: &[&str] = &[
    "echo_pulse_sequence",
    "multiple_spin_echo",
    "echo_planar_pulse_sequence",
    "steady_state_pulse_sequence",
    "phase_contrast",
    "time_of_flight_contrast",
    "arterial_spin_labeling_contrast",
    "geometry_of_k_space_traversal",
    "segmented_k_space_traversal",
    "spoiling",
    "inversion_recovery",
];

#[test]
fn the_mechanism_and_the_sop_class_reach_the_pack_under_their_names() {
    let packs = pack_dir();
    let pack = nils_pack::load(packs.path(), None).unwrap_or_else(|e| panic!("{e}"));
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.home.open().unwrap();
        let mut s = nils_digest::Settings::new(dir.path());
        s.name = "t".into();
        s.workers = 2;
        s.walk_threads = 2;
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 3, "{name}");
        nils_classify::run(
            &mut reg,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));

        let text = |stack: &nils_pack::Stack, f: &str| {
            stack
                .text(field_index(f).unwrap_or_else(|| panic!("{f} is a field")))
                .to_string()
        };
        let stack = |reg: &mut Registry, series: &str| {
            let id = stack_of_series(reg, series);
            nils_classify::classify::stack_of(reg.store(), &pack, id)
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: no fingerprint for {series}"))
                .0
        };

        // The enhanced object: every attribute as written, the modifier's
        // two read from the shared functional groups.
        let e = stack(&mut reg, "B.1");
        assert_eq!(text(&e, "sop_class_uid"), ENHANCED, "{name}");
        for (f, want) in [
            ("echo_pulse_sequence", "SPIN"),
            ("multiple_spin_echo", "YES"),
            ("echo_planar_pulse_sequence", "NO"),
            ("steady_state_pulse_sequence", "NONE"),
            ("phase_contrast", "NO"),
            ("time_of_flight_contrast", "NO"),
            ("arterial_spin_labeling_contrast", ""),
            ("geometry_of_k_space_traversal", "RECTILINEAR"),
            ("segmented_k_space_traversal", "PARTIAL"),
            ("spoiling", "NONE"),
            ("inversion_recovery", "YES"),
            ("mr_acquisition_type", "3D"),
            ("pulse_sequence_name", "TSE"),
        ] {
            assert_eq!(text(&e, f), want, "{name}: {f}");
        }
        let v = Evaluated::new(&pack, &e);
        assert_eq!(v.flag("tse"), Some(true), "{name}: the table reads a TSE");
        assert_eq!(v.flag("inverted"), Some(true), "{name}");
        assert_eq!(v.flag("unspoiled"), Some(true), "{name}");
        assert_eq!(v.flag("spectroscopy_object"), Some(false), "{name}");
        assert_eq!(v.classify().stored("kind"), "TSE", "{name}");

        // The spectroscopy object: its SOP class decides, and its module
        // is read as an image's is.
        let m = stack(&mut reg, "B.2");
        assert_eq!(text(&m, "sop_class_uid"), SPECTROSCOPY, "{name}");
        assert_eq!(text(&m, "echo_pulse_sequence"), "SPIN", "{name}");
        let v = Evaluated::new(&pack, &m);
        assert_eq!(v.flag("spectroscopy_object"), Some(true), "{name}");
        assert_eq!(v.classify().stored("kind"), "MRS", "{name}");

        // The classic image: the SOP class, and nothing of the module.
        let c = stack(&mut reg, "B.3");
        assert_eq!(text(&c, "sop_class_uid"), CLASSIC, "{name}");
        for f in MECHANISM {
            assert_eq!(text(&c, f), "", "{name}: {f} on a classic image");
        }
        let v = Evaluated::new(&pack, &c);
        assert_eq!(v.classify().stored("kind"), "", "{name}");

        // And the fingerprint stores them, at the revision this build writes.
        let store = reg.store();
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE fingerprint_revision = {} AND sop_class_uid IS NOT NULL",
            store.qualified("stack_fingerprint"),
            nils_classify::fingerprint::REVISION
        );
        assert_eq!(
            store.query(&sql, &[]).unwrap()[0].int(0).unwrap(),
            3,
            "{name}"
        );
    }
}
