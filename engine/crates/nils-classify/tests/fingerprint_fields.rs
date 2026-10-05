// SPDX-License-Identifier: AGPL-3.0-only

//! The 2026-10-03 fingerprint fields, end to end, on SQLite and, when
//! `NILS_TEST_POSTGRES_DSN` is set, on Postgres. A registry digested before
//! the fields existed is fingerprinted, which fills what it already holds
//! (the angiography flag, the temporal resolution, the diffusion
//! directionality), then read again one file a series for the series columns
//! and the Philips prepulse, after which a pack reads every field by name.
//! Synthetic headers only.
//!
//! - A Philips classic 3D T1 TFE in implicit VR, its Imaging DD 001 block at
//!   11 behind another creator, Prepulse Type `INV ` as bytes and a delay.
//! - A Philips classic TFE in explicit VR with Prepulse Type NO.
//! - A dynamic of three slices and four time points, 2.5 s apart, with no
//!   TemporalResolution: the frame interval answers.
//! - A GE series whose TemporalResolution says 59192: the header answers.
//! - An enhanced isotropic diffusion image, its AcquisitionContrast in the
//!   shared MR Image Frame Type group and its directionality in the shared
//!   MR Diffusion group.
//! - A colour display composite saved as RGB.

use std::env;
use std::sync::{Mutex, MutexGuard};

use dicom_core::{Tag, VR};
use dicom_dictionary_std::tags;
use nils_dicom::read::{EXPLICIT_VR_LE, IMPLICIT_VR_LE};
use nils_dicom::synth::{self, Elem, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_pack::Evaluated;
use nils_pack::stack::field_index;
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_fingerprint_fields";

const CLASSIC: &str = "1.2.840.10008.5.1.4.1.1.4";
const ENHANCED: &str = "1.2.840.10008.5.1.4.1.1.4.1";

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
        let dir = TempDir::new("classify-fp-fields-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-fp-fields-key")
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

/// A pack that ingests the Philips prepulse and whose flags read every new
/// field (a text is read lower-cased).
fn pack_dir() -> TempDir {
    let d = TempDir::new("classify-fp-fields-pack");
    let file = |name: &str, body: &str| {
        d.file(name, body.as_bytes());
    };
    file(
        "pack.yml",
        "pack: t\nversion: 0.1.0\ncontract: 8\nmodality: MR\nflags: [flags.yml]\naxes: [axes/kind.yml]\norder: [kind]\nprivate: [private.yml]\ndictionary: [dictionary.tsv]\nreview:\n  low_confidence: {default: 0.5}\n",
    );
    file(
        "private.yml",
        "private:
  coverage: [a test]
  ingest:
    - creator: Philips Imaging DD 001
      group: 0x2001
      element: 0x1C
      name: philips_prepulse_type
      kind: parameter
      why: the TFE prepulse
    - creator: Philips Imaging DD 001
      group: 0x2001
      element: 0x1B
      name: philips_prepulse_delay
      kind: timing
      why: the delay after it
  shown:
    - name: philips_prepulse_type
      why: the TFE prepulse
    - name: philips_prepulse_delay
      why: the delay after it
  release: []
",
    );
    file(
        "dictionary.tsv",
        "Philips Imaging DD 001\t2001\t1B\tFL\t1\tPrepulse Delay\nPhilips Imaging DD 001\t2001\t1C\tCS\t1\tPrepulse Type\n",
    );
    file(
        "flags.yml",
        "flags:
  inversion_prepulse: {text: philips_prepulse_type, equals: inv}
  no_prepulse: {text: philips_prepulse_type, equals: 'no'}
  colour: {text: photometric_interpretation, equals: rgb}
  three_samples: {field: samples_per_pixel, ge: 3}
  isotropic: {text: diffusion_directionality, equals: isotropic}
  diffusion_contrast: {text: acquisition_contrast, equals: diffusion}
  fast_dynamic: {field: temporal_resolution, lt: 10000}
  angio: {text: angio_flag, equals: 'y'}
",
    );
    file(
        "corpus/cases.yml",
        "cases:
  - name: an inversion prepulse is an MPRAGE
    stack: {philips_prepulse_type: INV}
    flags: {inversion_prepulse: true}
    axes: {kind: MPRAGE}
  - name: an RGB composite is no map
    stack: {photometric_interpretation: RGB, samples_per_pixel: 3}
    flags: {colour: true, three_samples: true}
    axes: {kind: Composite}
",
    );
    file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nvalues:\n  'MPRAGE': {detection: {exclusive: inversion_prepulse}}\n  'TurboFLASH': {detection: {exclusive: no_prepulse}}\n  'Composite': {detection: {exclusive: colour}}\n",
    );
    d
}

fn base(series: &str, sop: &str, maker: &str) -> Vec<Elem> {
    let mut e = synth::minimal_mr("F", series, sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
    e.push(synth::text(tags::MANUFACTURER, VR::LO, maker));
    e.push(synth::text(tags::SERIES_NUMBER, VR::IS, &series[2..]));
    e
}

fn image(e: &mut Vec<Elem>, photometric: &str, samples: u16) {
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\M\\ND",
    ));
    e.push(synth::us(tags::SAMPLES_PER_PIXEL, samples));
    e.push(synth::text(
        tags::PHOTOMETRIC_INTERPRETATION,
        VR::CS,
        photometric,
    ));
    e.push(synth::text(
        tags::IMAGE_ORIENTATION_PATIENT,
        VR::DS,
        "1\\0\\0\\0\\1\\0",
    ));
}

fn at(e: &mut Vec<Elem>, z: f64, time: &str, number: usize) {
    e.push(synth::text(
        tags::IMAGE_POSITION_PATIENT,
        VR::DS,
        &format!("0\\0\\{z}"),
    ));
    e.push(synth::text(tags::SLICE_LOCATION, VR::DS, &format!("{z}")));
    e.push(synth::text(tags::ACQUISITION_DATE, VR::DA, "20240101"));
    e.push(synth::text(tags::ACQUISITION_TIME, VR::TM, time));
    e.push(synth::text(
        tags::INSTANCE_NUMBER,
        VR::IS,
        &number.to_string(),
    ));
}

fn tree() -> TempDir {
    let dir = TempDir::new("classify-fp-fields");
    let classic = |name: &str, syntax: &str, sop: &str, e: &[Elem]| {
        dir.file(
            name,
            &synth::part10(&MetaFields::with(syntax, CLASSIC, sop), e, true),
        );
    };

    // F.1: Philips, implicit VR, its Imaging DD 001 block at 11, a decoy at
    // 10 under another creator.
    for n in 1..=2 {
        let sop = format!("F.1.{n}");
        let mut e = base("F.1", &sop, "Philips Medical Systems");
        image(&mut e, "MONOCHROME2", 1);
        at(&mut e, n as f64, "100000", n);
        e.push(synth::text(tags::ANGIO_FLAG, VR::CS, "N"));
        e.push(synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"));
        e.push(synth::text(
            Tag(0x2001, 0x0010),
            VR::LO,
            "Philips MR Imaging DD 001",
        ));
        e.push(synth::text(
            Tag(0x2001, 0x0011),
            VR::LO,
            "Philips Imaging DD 001",
        ));
        e.push(synth::bytes(Tag(0x2001, 0x101C), VR::UN, b"NO".to_vec()));
        e.push(synth::bytes(Tag(0x2001, 0x111C), VR::UN, b"INV ".to_vec()));
        e.push(synth::bytes(
            Tag(0x2001, 0x111B),
            VR::UN,
            1100.0f32.to_le_bytes().to_vec(),
        ));
        classic(&format!("f1/{n}"), IMPLICIT_VR_LE, &sop, &e);
    }

    // F.2: Philips, explicit VR, no prepulse.
    {
        let mut e = base("F.2", "F.2.1", "Philips Medical Systems");
        image(&mut e, "MONOCHROME2", 1);
        at(&mut e, 0.0, "101000", 1);
        e.push(synth::text(
            Tag(0x2001, 0x0010),
            VR::LO,
            "Philips Imaging DD 001",
        ));
        e.push(synth::text(Tag(0x2001, 0x101C), VR::CS, "NO"));
        e.push(synth::num(Tag(0x2001, 0x101B), VR::FL, 0.0));
        classic("f2/1", EXPLICIT_VR_LE, "F.2.1", &e);
    }

    // F.3: a dynamic of three slices and four time points, 2.5 s apart,
    // each slice 0.1 s after the one before.
    let mut n = 0;
    for frame in 0..4 {
        for slice in 0..3 {
            n += 1;
            let sop = format!("F.3.{n}");
            let mut e = base("F.3", &sop, "SIEMENS");
            image(&mut e, "MONOCHROME2", 1);
            let seconds = 2.5 * frame as f64 + 0.1 * slice as f64;
            let time = format!(
                "1020{:02}.{:06}",
                seconds as u32,
                ((seconds.fract()) * 1e6).round() as u32
            );
            at(&mut e, slice as f64 * 5.0, &time, n);
            e.push(synth::text(tags::ANGIO_FLAG, VR::CS, "N"));
            classic(&format!("f3/{n}"), EXPLICIT_VR_LE, &sop, &e);
        }
    }

    // F.4: GE writes its TemporalResolution, and one time for every image.
    for n in 1..=2 {
        let sop = format!("F.4.{n}");
        let mut e = base("F.4", &sop, "GE MEDICAL SYSTEMS");
        image(&mut e, "MONOCHROME2", 1);
        at(&mut e, 0.0, "103000", n);
        e.push(synth::text(tags::TEMPORAL_RESOLUTION, VR::DS, "59192"));
        classic(&format!("f4/{n}"), EXPLICIT_VR_LE, &sop, &e);
    }

    // F.5: an enhanced isotropic diffusion image.
    {
        let shared = vec![
            synth::fg(
                tags::MR_IMAGE_FRAME_TYPE_SEQUENCE,
                vec![synth::text(tags::ACQUISITION_CONTRAST, VR::CS, "DIFFUSION")],
            ),
            synth::fg(
                tags::MR_DIFFUSION_SEQUENCE,
                vec![
                    synth::text(tags::DIFFUSION_DIRECTIONALITY, VR::CS, "ISOTROPIC"),
                    synth::num(tags::DIFFUSION_B_VALUE, VR::FD, 1000.0),
                ],
            ),
        ];
        let frames = vec![
            vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")],
            vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")],
        ];
        let mut e = synth::enhanced_mr("F", "F.5", "F.5.1", shared, frames);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::MANUFACTURER, VR::LO, "Philips"));
        e.push(synth::text(
            tags::IMAGE_TYPE,
            VR::CS,
            "DERIVED\\PRIMARY\\DIFFUSION\\NONE",
        ));
        e.push(synth::us(tags::SAMPLES_PER_PIXEL, 1));
        e.push(synth::text(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            "MONOCHROME2",
        ));
        dir.file(
            "f5/1",
            &synth::part10(
                &MetaFields::with(EXPLICIT_VR_LE, ENHANCED, "F.5.1"),
                &e,
                true,
            ),
        );
    }

    // F.6: a colour display composite, saved as RGB.
    {
        let mut e = base("F.6", "F.6.1", "SyntheticMR");
        image(&mut e, "RGB", 3);
        at(&mut e, 0.0, "104000", 1);
        e.push(synth::text(tags::ANGIO_FLAG, VR::CS, "Y"));
        classic("f6/1", EXPLICIT_VR_LE, "F.6.1", &e);
    }
    dir
}

fn stack_of_series(reg: &mut Registry, series: &str) -> i64 {
    let store = reg.store();
    let sql = format!(
        "SELECT k.id FROM {} k JOIN {} s ON s.id = k.series_id WHERE s.series_instance_uid = '{series}' ORDER BY k.id",
        store.qualified("stack"),
        store.qualified("series"),
    );
    let rows = store.query(&sql, &[]).unwrap();
    assert!(!rows.is_empty(), "no stack for series {series}");
    rows[0].int(0).unwrap()
}

fn settings(dir: &TempDir) -> nils_digest::Settings {
    let mut s = nils_digest::Settings::new(dir.path());
    s.name = "t".into();
    s.workers = 2;
    s.walk_threads = 2;
    s
}

#[test]
fn the_new_fields_fill_by_a_fingerprint_and_a_one_file_reread_and_reach_the_pack() {
    let packs = pack_dir();
    let pack = nils_pack::load(packs.path(), None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(pack.ingest.len(), 2);
    assert_eq!(
        pack.ingest[0].vr.as_deref(),
        Some("CS"),
        "the dictionary's VR"
    );
    let ingest: Vec<nils_dicom::private::Ingest> = pack
        .ingest
        .iter()
        .map(|i| nils_dicom::private::Ingest {
            creator: i.creator.clone(),
            group: i.group,
            element: i.element,
            vr: i.vr.clone(),
        })
        .collect();

    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.home.open().unwrap();

        // A digest by a build that read no prepulse, and a registry from
        // before schema 79: the new series columns empty.
        let first = digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(first.parsed, 19, "{name}");
        {
            let store = reg.store();
            let sql = format!(
                "UPDATE {} SET photometric_interpretation = NULL, samples_per_pixel = NULL;\n\
                 UPDATE {} SET acquisition_contrast = NULL",
                store.qualified("series"),
                store.qualified("series_mr"),
            );
            store.batch(&sql).unwrap();
        }
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
        let num = |stack: &nils_pack::Stack, f: &str| {
            stack.num(field_index(f).unwrap_or_else(|| panic!("{f} is a field")))
        };
        let stack = |reg: &mut Registry, series: &str| {
            let id = stack_of_series(reg, series);
            nils_classify::classify::stack_of(reg.store(), &pack, id)
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: no fingerprint for {series}"))
        };

        // What the registry already held fills at once.
        let (s, _) = stack(&mut reg, "F.3");
        assert_eq!(num(&s, "temporal_resolution"), Some(2500.0), "{name}");
        assert_eq!(
            text(&s, "temporal_resolution_source"),
            "acquisition_times",
            "{name}"
        );
        assert_eq!(text(&s, "angio_flag"), "N", "{name}");
        // MRI pack 1.0.1: images a slice position, worked out from the
        // fingerprint's counts as the select reads them, on both backends
        assert_eq!(num(&s, "images_per_position"), Some(4.0), "{name}");
        {
            let id = stack_of_series(&mut reg, "F.3");
            let store = reg.store();
            let sql = format!(
                "SELECT {} FROM {} f WHERE f.stack_id = {id}",
                nils_classify::classify::field_sql(store, Some("f"), ("images_per_position", "")),
                store.qualified("stack_fingerprint"),
            );
            let row = &store.query(&sql, &[]).unwrap()[0];
            let v = match row.get(0) {
                nils_registry::store::Cell::Double(d) => *d,
                nils_registry::store::Cell::Int(i) => *i as f64,
                nils_registry::store::Cell::Text(t) => t.parse().unwrap(),
                other => panic!("{name}: {other:?}"),
            };
            assert_eq!(v, 4.0, "{name}");
        }
        let (s, _) = stack(&mut reg, "F.4");
        assert_eq!(num(&s, "temporal_resolution"), Some(59192.0), "{name}");
        assert_eq!(text(&s, "temporal_resolution_source"), "header", "{name}");
        let (s, _) = stack(&mut reg, "F.5");
        assert_eq!(text(&s, "diffusion_directionality"), "ISOTROPIC", "{name}");
        // one image per position, no time repeated: no temporal resolution
        let (s, _) = stack(&mut reg, "F.1");
        assert_eq!(num(&s, "temporal_resolution"), None, "{name}");
        assert_eq!(text(&s, "temporal_resolution_source"), "", "{name}");
        // and nothing yet of what only the files hold
        assert_eq!(text(&s, "photometric_interpretation"), "", "{name}");
        let (s, _) = stack(&mut reg, "F.6");
        assert_eq!(text(&s, "angio_flag"), "Y", "{name}");
        assert_eq!(text(&s, "photometric_interpretation"), "", "{name}");

        // The re-read: every MR series, one file each, only those missing
        // the new columns, with the prepulse ingested.
        let mut again = settings(&dir);
        again.reread_every = true;
        again.reread_one = true;
        again.reread_missing = true;
        again.ingest = ingest.clone();
        let report = digest(&again, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 6, "{name}: one file of each of six series");
        // run again, nothing is missing any more
        let report = digest(&again, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 0, "{name}: nothing left to fill");
        // without --reread-missing, one file of each series again
        again.reread_missing = false;
        let report = digest(&again, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 6, "{name}");

        nils_classify::run(
            &mut reg,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));

        let prepulse = |p: &[String]| (p[0].clone(), p[1].clone());
        let (s, p) = stack(&mut reg, "F.1");
        assert_eq!(
            prepulse(&p),
            ("INV".to_string(), "1100".to_string()),
            "{name}: the block at 11, the bytes in upper case"
        );
        assert_eq!(
            text(&s, "photometric_interpretation"),
            "MONOCHROME2",
            "{name}"
        );
        assert_eq!(num(&s, "samples_per_pixel"), Some(1.0), "{name}");
        let v = Evaluated::with_private(&pack, &s, p);
        assert_eq!(v.flag("inversion_prepulse"), Some(true), "{name}");
        assert_eq!(v.classify().stored("kind"), "MPRAGE", "{name}");

        let (s, p) = stack(&mut reg, "F.2");
        assert_eq!(prepulse(&p), ("NO".to_string(), "0".to_string()), "{name}");
        let v = Evaluated::with_private(&pack, &s, p);
        assert_eq!(v.classify().stored("kind"), "TurboFLASH", "{name}");

        let (s, p) = stack(&mut reg, "F.3");
        assert_eq!(p, vec![String::new(), String::new()], "{name}");
        let v = Evaluated::with_private(&pack, &s, p);
        assert_eq!(v.flag("fast_dynamic"), Some(true), "{name}");
        assert_eq!(v.classify().stored("kind"), "", "{name}");

        let (s, p) = stack(&mut reg, "F.5");
        assert_eq!(text(&s, "acquisition_contrast"), "DIFFUSION", "{name}");
        let v = Evaluated::with_private(&pack, &s, p);
        assert_eq!(v.flag("isotropic"), Some(true), "{name}");
        assert_eq!(v.flag("diffusion_contrast"), Some(true), "{name}");

        let (s, p) = stack(&mut reg, "F.6");
        assert_eq!(text(&s, "photometric_interpretation"), "RGB", "{name}");
        assert_eq!(num(&s, "samples_per_pixel"), Some(3.0), "{name}");
        let v = Evaluated::with_private(&pack, &s, p);
        assert_eq!(v.flag("colour"), Some(true), "{name}");
        assert_eq!(v.flag("three_samples"), Some(true), "{name}");
        assert_eq!(v.flag("angio"), Some(true), "{name}");
        assert_eq!(v.classify().stored("kind"), "Composite", "{name}");

        // And the fingerprint stores them at the revision this build writes.
        let store = reg.store();
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE fingerprint_revision = {} AND photometric_interpretation IS NOT NULL",
            store.qualified("stack_fingerprint"),
            nils_classify::fingerprint::REVISION
        );
        let stacks = store.query(&sql, &[]).unwrap()[0].int(0).unwrap();
        let all = store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {}",
                    store.qualified("stack_fingerprint")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(stacks, all, "{name}");
    }
}
