// SPDX-License-Identifier: AGPL-3.0-only

//! The ImageType Philips writes per frame of an enhanced MR object, end to
//! end. A Philips enhanced Dixon object names its parts (W, F, IP, OP) only in
//! the ImageType of each frame's (2005,140F) item: its frames are a stack per
//! part, each stack's fingerprint carries its own part, and a pack reads it
//! as `private_frame_image_type`. An enhanced object whose frames all name
//! one part is one stack with the key it always had; a classic image has the
//! column empty; and a Siemens enhanced object, whose (0021,1201) items carry
//! an ImageType too, is not split by it. Synthetic headers only.

use std::env;
use std::sync::{Mutex, MutexGuard};

use dicom_core::{Tag, VR};
use dicom_dictionary_std::tags;
use nils_dicom::read::EXPLICIT_VR_LE;
use nils_dicom::synth::{self, Elem, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_pack::Evaluated;
use nils_pack::stack::field_index;
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_frame_image_type";

const ENHANCED: &str = "1.2.840.10008.5.1.4.1.1.4.1";
const CLASSIC: &str = "1.2.840.10008.5.1.4.1.1.4";
const PHILIPS: Tag = Tag(0x2005, 0x140F);
const SIEMENS: Tag = Tag(0x0021, 0x1201);
const AXIAL: &str = "1\\0\\0\\0\\1\\0";
/// The fourteen values of an axial frame of [`enhanced`], as every build
/// before the Philips per-frame ImageType wrote them.
const AXIAL_14: &str = "||||||||||||Axial|DERIVED\\\\PRIMARY\\\\DIXON\\\\NONE";

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
        let dir = TempDir::new("classify-frame-type-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-frame-type-key")
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

/// A pack whose flags read the new field (a text is read lower-cased), and
/// an axis that names the Dixon part from it.
fn pack_dir() -> TempDir {
    let d = TempDir::new("classify-frame-type-pack");
    let file = |name: &str, body: &str| {
        d.file(name, body.as_bytes());
    };
    file(
        "pack.yml",
        "pack: t\nversion: 0.1.0\ncontract: 1\nmodality: MR\nflags: [flags.yml]\naxes: [axes/part.yml]\norder: [part]\nreview:\n  low_confidence: {default: 0.5}\n",
    );
    file(
        "flags.yml",
        "flags:
  water: {text: private_frame_image_type, equals: 'derived\\primary\\w\\w\\derived'}
  fat: {text: private_frame_image_type, equals: 'derived\\primary\\f\\f\\derived'}
  in_phase: {text: private_frame_image_type, equals: 'derived\\primary\\ip\\ip\\derived'}
  out_phase: {text: private_frame_image_type, equals: 'derived\\primary\\op\\op\\derived'}
",
    );
    file(
        "axes/part.yml",
        "axis: part\nkind: single\nvalues:\n  'W': {detection: {exclusive: water}}\n  'F': {detection: {exclusive: fat}}\n  'IP': {detection: {exclusive: in_phase}}\n  'OP': {detection: {exclusive: out_phase}}\n",
    );
    file(
        "corpus/cases.yml",
        "cases:
  - name: a water image is told by the ImageType Philips writes per frame
    stack: {private_frame_image_type: 'DERIVED\\PRIMARY\\W\\W\\DERIVED'}
    flags: {water: true, fat: false}
    axes: {part: W}
",
    );
    d
}

/// An enhanced MR object whose frames carry, each, an orientation and, in
/// `private`'s first item, the given ImageType. The top-level ImageType names
/// no part, as a Philips enhanced Dixon object's does not.
fn enhanced(
    study: &str,
    series: &str,
    sop: &str,
    manufacturer: &str,
    private: Tag,
    frames: &[&str],
) -> Vec<Elem> {
    let per_frame: Vec<Vec<Elem>> = frames
        .iter()
        .map(|t| {
            vec![
                synth::fg_orientation(AXIAL),
                synth::seq(
                    private,
                    vec![vec![synth::text(tags::IMAGE_TYPE, VR::CS, t)]],
                ),
            ]
        })
        .collect();
    let mut e = synth::enhanced_mr(study, series, sop, Vec::new(), per_frame);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
    e.push(synth::text(tags::MANUFACTURER, VR::LO, manufacturer));
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "DERIVED\\PRIMARY\\DIXON\\NONE",
    ));
    e
}

/// A classic image.
fn classic(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    let mut e = synth::minimal_mr(study, series, sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
    e.push(synth::text(tags::MANUFACTURER, VR::LO, "Philips"));
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\M_FFE\\M\\FFE",
    ));
    e
}

const PARTS: [&str; 4] = ["W", "F", "IP", "OP"];

fn part(p: &str) -> String {
    format!("DERIVED\\PRIMARY\\{p}\\{p}\\DERIVED")
}

fn tree() -> TempDir {
    let dir = TempDir::new("classify-frame-type");
    let enhanced_file = |name: &str, series: &str, manufacturer: &str, private, frames: &[&str]| {
        let sop = format!("{series}.1");
        dir.file(
            name,
            &synth::part10(
                &MetaFields::with(EXPLICIT_VR_LE, ENHANCED, &sop),
                &enhanced("B", series, &sop, manufacturer, private, frames),
                true,
            ),
        );
    };
    // four parts, interleaved frame by frame
    let dixon: Vec<String> = (0..8).map(|i| part(PARTS[i % 4])).collect();
    let dixon: Vec<&str> = dixon.iter().map(String::as_str).collect();
    enhanced_file("dixon/1", "B.1", "Philips", PHILIPS, &dixon);
    // every frame the water image
    let water = part("W");
    enhanced_file("water/1", "B.2", "Philips", PHILIPS, &[water.as_str(); 4]);
    // a Siemens object whose private items disagree
    enhanced_file(
        "siemens/1",
        "B.3",
        "SIEMENS",
        SIEMENS,
        &[
            "ORIGINAL\\PRIMARY\\M\\NORM",
            "ORIGINAL\\PRIMARY\\P\\NORM",
            "ORIGINAL\\PRIMARY\\M\\NORM",
            "ORIGINAL\\PRIMARY\\P\\NORM",
        ],
    );
    dir.file(
        "classic/1",
        &synth::part10(
            &MetaFields::with(EXPLICIT_VR_LE, CLASSIC, "B.4.1"),
            &classic("B", "B.4", "B.4.1"),
            true,
        ),
    );
    dir
}

/// The stacks of a series, by stack index: id, key and the stack row's own
/// `private_frame_image_type`.
fn stacks_of_series(reg: &mut Registry, series: &str) -> Vec<(i64, String, Option<String>)> {
    let store = reg.store();
    let sql = format!(
        "SELECT k.id, k.stack_key, k.private_frame_image_type FROM {} k JOIN {} s ON s.id = k.series_id \
         WHERE s.series_instance_uid = '{series}' ORDER BY k.stack_index",
        store.qualified("stack"),
        store.qualified("series"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.int(0).unwrap(),
                r.text(1).unwrap().to_string(),
                r.opt_text(2).unwrap().map(str::to_string),
            )
        })
        .collect()
}

fn fingerprint_of(reg: &mut Registry, stack: i64) -> (Option<String>, Option<String>) {
    let store = reg.store();
    let sql = format!(
        "SELECT private_frame_image_type, split_reason FROM {} WHERE stack_id = {stack}",
        store.qualified("stack_fingerprint"),
    );
    let rows = store.query(&sql, &[]).unwrap();
    assert_eq!(rows.len(), 1, "one fingerprint for stack {stack}");
    (
        rows[0].opt_text(0).unwrap().map(str::to_string),
        rows[0].opt_text(1).unwrap().map(str::to_string),
    )
}

#[test]
fn the_parts_of_a_philips_dixon_object_are_stacks_a_pack_reads_by_name() {
    let packs = pack_dir();
    let pack = nils_pack::load(packs.path(), None).unwrap_or_else(|e| panic!("{e}"));
    let index = field_index("private_frame_image_type").expect("a pack field");
    assert!(index >= nils_pack::stack::FIRST_TEXT, "a text field");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.home.open().unwrap();
        let mut s = nils_digest::Settings::new(dir.path());
        s.name = "t".into();
        s.workers = 2;
        s.walk_threads = 2;
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 4, "{name}");
        nils_classify::run(
            &mut reg,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));

        let read = |reg: &mut Registry, id: i64| {
            nils_classify::classify::stack_of(reg.store(), &pack, id)
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: no fingerprint for stack {id}"))
                .0
        };

        // The Dixon object: four stacks, one per part, each its own value on
        // the stack row and on the fingerprint, and the pack names the part.
        let dixon = stacks_of_series(&mut reg, "B.1");
        assert_eq!(dixon.len(), 4, "{name}: {dixon:?}");
        let keys: std::collections::BTreeSet<&str> =
            dixon.iter().map(|(_, k, _)| k.as_str()).collect();
        assert_eq!(keys.len(), 4, "{name}: four keys");
        for ((id, key, own), p) in dixon.iter().zip(PARTS) {
            let want = part(p);
            assert_eq!(own.as_deref(), Some(want.as_str()), "{name}: {p}");
            assert_eq!(
                key,
                &nils_digest::stack::key_of(&format!("{AXIAL_14}|{p}")),
                "{name}: {p}"
            );
            let (fp, reason) = fingerprint_of(&mut reg, *id);
            assert_eq!(fp.as_deref(), Some(want.as_str()), "{name}: {p}");
            assert_eq!(
                reason.as_deref(),
                Some("image_type_variation"),
                "{name}: {p}"
            );
            let stack = read(&mut reg, *id);
            assert_eq!(stack.text(index), want, "{name}: {p}");
            let v = Evaluated::new(&pack, &stack);
            assert_eq!(v.classify().stored("part"), p, "{name}: {p}");
        }

        // Every frame the water image: one stack, the key of its fourteen
        // values as before, and the part still read.
        let water = stacks_of_series(&mut reg, "B.2");
        assert_eq!(water.len(), 1, "{name}: {water:?}");
        assert_eq!(
            water[0].1,
            nils_digest::stack::key_of(AXIAL_14),
            "{name}: the key is unchanged"
        );
        assert_eq!(water[0].2.as_deref(), Some(part("W").as_str()), "{name}");
        let (fp, reason) = fingerprint_of(&mut reg, water[0].0);
        assert_eq!(fp.as_deref(), Some(part("W").as_str()), "{name}");
        assert_eq!(reason, None, "{name}");
        let w = read(&mut reg, water[0].0);
        let v = Evaluated::new(&pack, &w);
        assert_eq!(v.classify().stored("part"), "W", "{name}");

        // The Siemens object: its private ImageType is not the Philips one,
        // so it splits nothing and reads empty.
        let siemens = stacks_of_series(&mut reg, "B.3");
        assert_eq!(siemens.len(), 1, "{name}: {siemens:?}");
        assert_eq!(
            siemens[0].1,
            nils_digest::stack::key_of(AXIAL_14),
            "{name}: the key is unchanged"
        );
        assert_eq!(siemens[0].2, None, "{name}");
        assert_eq!(fingerprint_of(&mut reg, siemens[0].0).0, None, "{name}");
        assert_eq!(read(&mut reg, siemens[0].0).text(index), "", "{name}");

        // The classic image: empty everywhere.
        let classic = stacks_of_series(&mut reg, "B.4");
        assert_eq!(classic.len(), 1, "{name}");
        assert_eq!(classic[0].2, None, "{name}");
        assert_eq!(fingerprint_of(&mut reg, classic[0].0).0, None, "{name}");
        let c = read(&mut reg, classic[0].0);
        assert_eq!(c.text(index), "", "{name}");
        assert_eq!(
            Evaluated::new(&pack, &c).classify().stored("part"),
            "",
            "{name}"
        );

        // And the fingerprint is a revision that writes it (6 or later).
        let revision = nils_classify::fingerprint::REVISION;
        assert!(revision >= 6);
        let store = reg.store();
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE fingerprint_revision = {revision}",
            store.qualified("stack_fingerprint"),
        );
        assert_eq!(
            store.query(&sql, &[]).unwrap()[0].int(0).unwrap(),
            7,
            "{name}"
        );
    }
}
