// SPDX-License-Identifier: AGPL-3.0-only

//! A rule reads an ingested private element the way it reads a standard
//! field (`docs/specs/wave4a-engine-completes.md`, §5.2), end to end: the
//! pack names it, the digest reads it, the classifier decides on it.

use std::env;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use dicom_core::{Tag, VR};
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_private";

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
        let dir = TempDir::new("classify-private-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-private-key")
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

/// A pack for MR with one ingested element and one axis decided on it.
fn pack_dir() -> TempDir {
    let d = TempDir::new("classify-private-pack");
    let file = |name: &str, body: &str| {
        d.file(name, body.as_bytes());
    };
    file(
        "pack.yml",
        "pack: t\nversion: 0.1.0\ncontract: 1\nmodality: MR\nparsers: [parsers.yml]\nflags: [flags.yml]\naxes: [axes/coil.yml]\norder: [coil]\nprivate: [private.yml]\ndictionary: [dictionary.tsv]\nreview:\n  low_confidence: {default: 0.5}\n",
    );
    file(
        "parsers.yml",
        "parsers:\n  image_type:\n    field: image_type\n    case: upper\n    tokenize: {split: '[\\\\\\\\/\\\\s]+'}\n    predicates:\n      is_original: {token: ORIGINAL}\n",
    );
    file(
        "flags.yml",
        "flags:\n  is_original: image_type.is_original\n  head_coil: {text: siemens_coil_string, substring: hea}\n  any_coil: {field: siemens_coil_string, present: true}\n",
    );
    file(
        "private.yml",
        "private:\n  coverage: [a test]\n  ingest:\n    - creator: SIEMENS MR HEADER\n      group: 0x0051\n      element: 0x0F\n      name: siemens_coil_string\n      why: which coil was on\n  release: []\n",
    );
    file(
        "dictionary.tsv",
        "SIEMENS MR HEADER\t0051\t0F\tLO\t1\tCoilString\n",
    );
    file(
        "axes/coil.yml",
        "axis: coil\nkind: single\nvalues:\n  'head': {detection: {exclusive: head_coil}}\n  'other': {detection: {exclusive: any_coil}}\n",
    );
    file(
        "corpus/cases.yml",
        "cases:\n  - name: a head coil is read from the private element\n    stack: {image_type: 'ORIGINAL\\\\PRIMARY', siemens_coil_string: 'HEA;HEP'}\n    flags: {head_coil: true}\n    axes: {coil: head}\n  - name: a body coil is not the head\n    stack: {image_type: 'ORIGINAL\\\\PRIMARY', siemens_coil_string: 'BO1'}\n    flags: {head_coil: false}\n    axes: {coil: other}\n",
    );
    d
}

fn tree() -> TempDir {
    let dir = TempDir::new("classify-private");
    let file = |sop: &str, series: &str, coil: &str| {
        let mut e = synth::minimal_mr("A", series, sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(
            tags::IMAGE_TYPE,
            VR::CS,
            "ORIGINAL\\PRIMARY\\M\\ND",
        ));
        e.push(synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"));
        e.push(synth::text(
            Tag(0x0051, 0x0010),
            VR::LO,
            "SIEMENS MR HEADER",
        ));
        e.push(synth::text(Tag(0x0051, 0x100F), VR::LO, coil));
        synth::part10(&MetaFields::mr(sop), &e, true)
    };
    dir.file("a/1", &file("A.1.1", "A.1", "HEA;HEP"));
    dir.file("b/1", &file("A.2.1", "A.2", "BO1"));
    dir
}

fn axis(reg: &mut Registry, series: &str, axis: &str) -> Option<String> {
    let store = reg.store();
    let sql = format!(
        "SELECT a.value FROM {} a JOIN {} k ON k.id = a.stack_id JOIN {} s ON s.id = k.series_id \
         WHERE s.series_instance_uid = '{series}' AND a.axis = '{axis}'",
        store.qualified("classification_axis"),
        store.qualified("stack"),
        store.qualified("series"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .first()
        .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
}

#[test]
fn a_rule_reads_a_private_element_the_pack_ingested() {
    let packs = pack_dir();
    let pack = nils_pack::load(packs.path(), None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(pack.ingest.len(), 1);
    assert_eq!(
        pack.ingest[0].vr.as_deref(),
        Some("LO"),
        "the dictionary's VR"
    );
    assert_eq!(
        pack.ingest[0].dictionary_name.as_deref(),
        Some("CoilString"),
        "and its name"
    );
    assert_eq!(pack.cases, 2, "the corpus set the field by the pack's name");

    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.home.open().unwrap();
        let mut s = nils_digest::Settings::new(dir.path());
        s.name = "t".into();
        s.workers = 2;
        s.walk_threads = 2;
        s.ingest = pack
            .ingest
            .iter()
            .map(|i| nils_dicom::private::Ingest {
                creator: i.creator.clone(),
                group: i.group,
                element: i.element,
                vr: i.vr.clone(),
            })
            .collect();
        s.ingest_from = Some(pack.id());
        digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        nils_classify::run(
            &mut reg,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let report = nils_classify::classify::classify(
            &mut reg,
            &pack,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.read, 2, "{name}");
        assert_eq!(
            axis(&mut reg, "A.1", "coil").as_deref(),
            Some("head"),
            "{name}"
        );
        assert_eq!(
            axis(&mut reg, "A.2", "coil").as_deref(),
            Some("other"),
            "{name}"
        );
        let _ = Path::new("");
    }
}
