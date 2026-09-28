// SPDX-License-Identifier: AGPL-3.0-only

//! The vendor sequence tags of the 2026-09-28 sequence research, end to end:
//! a pack declares the GE and Philips private elements by creator and
//! offset, the digest reads them in implicit and explicit VR files alike
//! (binary SS, multi-valued LO and IS included), and the classifier hands
//! them to the pack's stack under their ingest names. Beside them the
//! standard PulseSequenceName (0018,9005), which Siemens XA writes where it
//! leaves SequenceName empty, reaches the pack as `pulse_sequence_name`.

use std::env;
use std::sync::{Mutex, MutexGuard};

use dicom_core::{Tag, VR};
use dicom_dictionary_std::tags;
use nils_dicom::read::IMPLICIT_VR_LE;
use nils_dicom::synth::{self, Elem, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_pack::Evaluated;
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_sequence_tags";

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
        let dir = TempDir::new("classify-seqtags-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-seqtags-key")
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

/// The nine ingest entries the MRI pack declares for the sequence research,
/// with the names it gives them. `ge_multiband` has no dictionary entry, as
/// in the MRI pack's dictionary, so it is read as the string it is.
const INGEST: &str = "private:
  coverage: [a test]
  ingest:
    - {creator: GEMS_ACQU_01, group: 0x0019, element: 0x9C, name: ge_pulse_sequence_name, why: t}
    - {creator: GEMS_ACQU_01, group: 0x0019, element: 0x9E, name: ge_internal_pulse_sequence_name, why: t}
    - {creator: GEMS_PARM_01, group: 0x0043, element: 0xA3, name: ge_asl_contrast_technique, why: t}
    - {creator: GEMS_PARM_01, group: 0x0043, element: 0xA4, name: ge_asl_labeling_technique, why: t}
    - {creator: GEMS_PARM_01, group: 0x0043, element: 0xA5, name: ge_asl_label_duration, why: t}
    - {creator: GEMS_PARM_01, group: 0x0043, element: 0x2F, name: ge_private_image_type, why: t}
    - {creator: GEMS_PARM_01, group: 0x0043, element: 0xB6, name: ge_multiband, why: t}
    - {creator: Philips MR Imaging DD 005, group: 0x2005, element: 0x29, name: philips_label_type, vr: CS, why: t}
    - {creator: Philips Imaging DD 001, group: 0x2001, element: 0x81, name: philips_dynamic_scans, why: t}
  release: []
";

/// The MRI pack's dictionary lines for the same elements; it has none for
/// Philips' label type, whose VR the ingest entry gives.
const DICTIONARY: &str = "GEMS_ACQU_01\t0019\t9C\tLO\t1\tPulse Sequence Name
GEMS_ACQU_01\t0019\t9E\tLO\t1\tInternal Pulse Sequence Name
GEMS_PARM_01\t0043\t2F\tSS\t1\tImage Type (real, imaginary, phase, magnitude)
GEMS_PARM_01\t0043\tA3\tCS\t1\tASL Contrast technique
GEMS_PARM_01\t0043\tA4\tLO\t1\tDetailed text for ASL labeling technique
GEMS_PARM_01\t0043\tA5\tIS\t1\tDuration of the label or control pulse
Philips Imaging DD 001\t2001\t81\tIS\t1\tNumber of Dynamic Scans
";

/// A pack for MR whose flags read each of the fields by name.
fn pack_dir() -> TempDir {
    let d = TempDir::new("classify-seqtags-pack");
    let file = |name: &str, body: &str| {
        d.file(name, body.as_bytes());
    };
    file(
        "pack.yml",
        "pack: t\nversion: 0.1.0\ncontract: 1\nmodality: MR\nflags: [flags.yml]\naxes: [axes/kind.yml]\norder: [kind]\nprivate: [private.yml]\ndictionary: [dictionary.tsv]\nreview:\n  low_confidence: {default: 0.5}\n",
    );
    file(
        "flags.yml",
        "flags:
  pcasl: {text: ge_asl_contrast_technique, substring: pseudo}
  ge_3dasl: {text: ge_pulse_sequence_name, substring: 3dasl}
  ge_label_text: {field: ge_asl_labeling_technique, present: true}
  ge_label_duration: {field: ge_asl_label_duration, present: true}
  ge_internal: {field: ge_internal_pulse_sequence_name, present: true}
  ge_phase: {text: ge_private_image_type, substring: '1'}
  ge_multiband: {field: ge_multiband, present: true}
  philips_label: {text: philips_label_type, substring: label}
  philips_dynamics: {field: philips_dynamic_scans, present: true}
  xa_epfid: {text: pulse_sequence_name, substring: epfid}
",
    );
    file("private.yml", INGEST);
    file("dictionary.tsv", DICTIONARY);
    file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nvalues:\n  'asl': {detection: {exclusive: pcasl}}\n  'bold': {detection: {exclusive: xa_epfid}}\n",
    );
    file(
        "corpus/cases.yml",
        "cases:
  - name: a GE pseudo-continuous label is ASL
    stack: {ge_asl_contrast_technique: PSEUDOCONTINUOUS}
    flags: {pcasl: true}
    axes: {kind: asl}
  - name: an XA pulse sequence name is read where the sequence name is empty
    stack: {pulse_sequence_name: epfid2d1_64}
    flags: {xa_epfid: true}
    axes: {kind: bold}
",
    );
    d
}

fn creator(group: u16, name: &str) -> Elem {
    synth::text(Tag(group, 0x0010), VR::LO, name)
}

/// The elements of one file, with the private blocks at slot 0x10 unless
/// `slot` moves them.
fn ge(study: &str, series: &str, sop: &str, casl: &str, image_type: i16, slot: u16) -> Vec<Elem> {
    let mut e = synth::minimal_mr(study, series, sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
    e.push(synth::text(
        tags::MANUFACTURER,
        VR::LO,
        "GE MEDICAL SYSTEMS",
    ));
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\ASL",
    ));
    let at = |g: u16, off: u16| Tag(g, (slot << 8) | off);
    e.push(synth::text(Tag(0x0019, slot), VR::LO, "GEMS_ACQU_01"));
    e.push(synth::text(at(0x0019, 0x9C), VR::LO, "3dasl"));
    e.push(synth::text(at(0x0019, 0x9E), VR::LO, "3DASL_SPIRAL"));
    e.push(synth::text(Tag(0x0043, slot), VR::LO, "GEMS_PARM_01"));
    e.push(synth::num(at(0x0043, 0x2F), VR::SS, f64::from(image_type)));
    e.push(synth::text(at(0x0043, 0xA3), VR::CS, casl));
    e.push(synth::text(
        at(0x0043, 0xA4),
        VR::LO,
        "PCASL\\Background Suppressed",
    ));
    e.push(synth::text(at(0x0043, 0xA5), VR::IS, "1450"));
    e.push(synth::text(at(0x0043, 0xB6), VR::LO, "3\\1\\0"));
    e
}

fn philips(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    let mut e = synth::minimal_mr(study, series, sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
    e.push(synth::text(
        tags::MANUFACTURER,
        VR::LO,
        "Philips Medical Systems",
    ));
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\M_FFE\\M\\FFE",
    ));
    // Several creators in 2005; the one the pack names reserves block 0x14,
    // so the label type is (2005,1429).
    e.push(creator(0x2001, "Philips Imaging DD 001"));
    e.push(synth::text(Tag(0x2001, 0x1081), VR::IS, "30"));
    e.push(creator(0x2005, "Philips MR Imaging DD 001"));
    e.push(synth::text(
        Tag(0x2005, 0x0014),
        VR::LO,
        "Philips MR Imaging DD 005",
    ));
    e.push(synth::text(Tag(0x2005, 0x1029), VR::CS, "NOT THIS"));
    e.push(synth::text(Tag(0x2005, 0x1429), VR::CS, "LABEL"));
    e
}

fn xa(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    let mut e = synth::minimal_mr(study, series, sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
    e.push(synth::text(
        tags::MANUFACTURER,
        VR::LO,
        "Siemens Healthineers",
    ));
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\M\\ND",
    ));
    // SequenceName empty, as XA leaves it on about a quarter of its stacks.
    e.push(synth::text(tags::SEQUENCE_NAME, VR::SH, ""));
    e.push(synth::text(
        tags::PULSE_SEQUENCE_NAME,
        VR::SH,
        "epfid2d1_64",
    ));
    e
}

fn implicit(sop: &str) -> MetaFields {
    MetaFields::with(IMPLICIT_VR_LE, "1.2.840.10008.5.1.4.1.1.4", sop)
}

/// GE's ASL written twice, once implicit and once explicit VR, each series
/// holding its PD and PW passes; a Philips label scan; a Siemens XA EPI.
fn tree() -> TempDir {
    let dir = TempDir::new("classify-seqtags");
    // Explicit VR: the PW pass and the PD pass of one series.
    dir.file(
        "ge-explicit/1",
        &synth::part10(
            &MetaFields::mr("A.1.1"),
            &ge("A", "A.1", "A.1.1", "PSEUDOCONTINUOUS", 0, 0x0010),
            true,
        ),
    );
    dir.file(
        "ge-explicit/2",
        &synth::part10(
            &MetaFields::mr("A.1.2"),
            &ge("A", "A.1", "A.1.2", "CONTINUOUS", 0, 0x0011),
            true,
        ),
    );
    // Implicit VR: the private elements arrive as bytes, and the
    // dictionary's VR is what reads them.
    dir.file(
        "ge-implicit/1",
        &synth::part10(
            &implicit("A.2.1"),
            &ge("A", "A.2", "A.2.1", "PSEUDOCONTINUOUS", 1, 0x0010),
            true,
        ),
    );
    dir.file(
        "philips-implicit/1",
        &synth::part10(&implicit("A.3.1"), &philips("A", "A.3", "A.3.1"), true),
    );
    dir.file(
        "philips-explicit/1",
        &synth::part10(
            &MetaFields::mr("A.4.1"),
            &philips("A", "A.4", "A.4.1"),
            true,
        ),
    );
    dir.file(
        "xa/1",
        &synth::part10(&MetaFields::mr("A.5.1"), &xa("A", "A.5", "A.5.1"), true),
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
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

#[test]
fn the_sequence_tags_reach_the_pack_under_their_names() {
    let packs = pack_dir();
    let pack = nils_pack::load(packs.path(), None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(pack.ingest.len(), 9);
    let vr = |name: &str| {
        pack.ingest
            .iter()
            .find(|i| i.name == name)
            .and_then(|i| i.vr.clone())
    };
    assert_eq!(vr("ge_private_image_type").as_deref(), Some("SS"));
    assert_eq!(vr("ge_asl_label_duration").as_deref(), Some("IS"));
    assert_eq!(vr("ge_multiband"), None, "no dictionary entry");
    assert_eq!(
        vr("philips_label_type").as_deref(),
        Some("CS"),
        "the entry's"
    );

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
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 6, "{name}");
        nils_classify::run(
            &mut reg,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));

        let fields = |reg: &mut Registry, series: &str| {
            let id = stack_of_series(reg, series);
            let (stack, private) = nils_classify::classify::stack_of(reg.store(), &pack, id)
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: no fingerprint for {series}"));
            let named: std::collections::BTreeMap<String, String> = pack
                .ingest
                .iter()
                .map(|i| i.name.clone())
                .zip(private.iter().cloned())
                .collect();
            (stack, private, named)
        };

        // Explicit VR: every element read as its VR says. The PW and PD
        // passes disagree on the contrast technique, so the series keeps the
        // smaller in text order, as every series field does.
        let (_, _, v) = fields(&mut reg, "A.1");
        assert_eq!(v["ge_pulse_sequence_name"], "3dasl", "{name}");
        assert_eq!(
            v["ge_internal_pulse_sequence_name"], "3DASL_SPIRAL",
            "{name}"
        );
        assert_eq!(v["ge_asl_contrast_technique"], "CONTINUOUS", "{name}");
        assert_eq!(
            v["ge_asl_labeling_technique"], "PCASL\\Background Suppressed",
            "{name}"
        );
        assert_eq!(v["ge_asl_label_duration"], "1450", "{name}");
        assert_eq!(v["ge_private_image_type"], "0", "{name}");
        assert_eq!(v["ge_multiband"], "3\\1\\0", "{name}");

        // Implicit VR: the same values from bytes, the SS as a number.
        let (stack, private, v) = fields(&mut reg, "A.2");
        assert_eq!(v["ge_pulse_sequence_name"], "3dasl", "{name}");
        assert_eq!(
            v["ge_internal_pulse_sequence_name"], "3DASL_SPIRAL",
            "{name}"
        );
        assert_eq!(v["ge_asl_contrast_technique"], "PSEUDOCONTINUOUS", "{name}");
        assert_eq!(
            v["ge_asl_labeling_technique"], "PCASL\\Background Suppressed",
            "{name}"
        );
        assert_eq!(v["ge_asl_label_duration"], "1450", "{name}");
        assert_eq!(v["ge_private_image_type"], "1", "{name}");
        assert_eq!(v["ge_multiband"], "3\\1\\0", "{name}");
        let e = Evaluated::with_private(&pack, &stack, private);
        for flag in [
            "pcasl",
            "ge_3dasl",
            "ge_label_text",
            "ge_label_duration",
            "ge_internal",
            "ge_phase",
            "ge_multiband",
        ] {
            assert_eq!(e.flag(flag), Some(true), "{name}: {flag}");
        }
        assert_eq!(e.flag("philips_label"), Some(false), "{name}");
        assert_eq!(e.classify().axis("kind").unwrap().values, ["asl"], "{name}");

        // Philips, both ways, with the creator the pack names at block 0x14
        // of its group and another creator's element at the same offset.
        for series in ["A.3", "A.4"] {
            let (stack, private, v) = fields(&mut reg, series);
            assert_eq!(v["philips_label_type"], "LABEL", "{name} {series}");
            assert_eq!(v["philips_dynamic_scans"], "30", "{name} {series}");
            assert_eq!(v["ge_pulse_sequence_name"], "", "{name} {series}");
            let e = Evaluated::with_private(&pack, &stack, private);
            assert_eq!(e.flag("philips_label"), Some(true), "{name} {series}");
            assert_eq!(e.flag("philips_dynamics"), Some(true), "{name} {series}");
        }

        // Siemens XA: SequenceName empty, PulseSequenceName read.
        let (stack, private, _) = fields(&mut reg, "A.5");
        let at = nils_pack::stack::field_index("pulse_sequence_name").unwrap();
        assert_eq!(stack.text(at), "epfid2d1_64", "{name}");
        let at = nils_pack::stack::field_index("text_sequence_name").unwrap();
        assert_eq!(stack.text(at), "", "{name}");
        let e = Evaluated::with_private(&pack, &stack, private);
        assert_eq!(e.flag("xa_epfid"), Some(true), "{name}");
        assert_eq!(
            e.classify().axis("kind").unwrap().values,
            ["bold"],
            "{name}"
        );
    }
}

fn one(reg: &mut Registry, sql: &str) -> i64 {
    reg.store().query(sql, &[]).unwrap()[0].int(0).unwrap()
}

fn opt_text(reg: &mut Registry, sql: &str) -> Option<String> {
    reg.store()
        .query(sql, &[])
        .unwrap()
        .first()
        .and_then(|r| r.opt_text(0).unwrap().map(str::to_string))
}

fn settings(dir: &TempDir, pack: Option<&nils_pack::Pack>) -> nils_digest::Settings {
    let mut s = nils_digest::Settings::new(dir.path());
    s.name = "t".into();
    s.workers = 2;
    s.walk_threads = 2;
    if let Some(pack) = pack {
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
    }
    s
}

/// A registry digested before the sequence research is brought up to it by
/// reading again only the GE and Siemens XA MR files, found in the registry:
/// the new fields fill, nothing else is read, nothing is marked gone, and
/// the fingerprints of those series are derived again.
#[test]
fn a_reread_reads_only_the_named_manufacturers_mr_files() {
    let packs = pack_dir();
    let pack = nils_pack::load(packs.path(), None).unwrap_or_else(|e| panic!("{e}"));
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("classify-seqtags-reread");
        // One study per vendor: the manufacturer a re-read matches is the
        // study's. GE ASL, implicit VR, both passes in one series.
        for (i, casl) in [(1, "PSEUDOCONTINUOUS"), (2, "CONTINUOUS")] {
            let sop = format!("B.1.{i}");
            dir.file(
                &format!("ge/{i}"),
                &synth::part10(
                    &implicit(&sop),
                    &ge("B1", "B.1", &sop, casl, 0, 0x0010),
                    true,
                ),
            );
        }
        // A GE CT: the manufacturer matches, the modality does not.
        let mut ct = synth::minimal_ct("B2", "B.2", "B.2.1");
        ct.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        ct.push(synth::text(
            tags::MANUFACTURER,
            VR::LO,
            "GE MEDICAL SYSTEMS",
        ));
        dir.file("ct/1", &synth::part10(&MetaFields::ct("B.2.1"), &ct, true));
        dir.file(
            "xa/1",
            &synth::part10(&MetaFields::mr("B.3.1"), &xa("B3", "B.3", "B.3.1"), true),
        );
        dir.file(
            "philips/1",
            &synth::part10(
                &MetaFields::mr("B.4.1"),
                &philips("B4", "B.4", "B.4.1"),
                true,
            ),
        );

        // As an engine before the research left it: no private element
        // read, and no pulse sequence name, with fingerprints derived.
        let mut reg = lab.home.open().unwrap();
        let first =
            digest(&settings(&dir, None), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(first.parsed, 5, "{name}");
        let series_mr = reg.store().qualified("series_mr");
        reg.store()
            .batch(&format!(
                "UPDATE {series_mr} SET pulse_sequence_name = NULL"
            ))
            .unwrap();
        nils_classify::run(
            &mut reg,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let fp = reg.store().qualified("stack_fingerprint");
        let series = reg.store().qualified("series");
        let private = reg.store().qualified("series_private");
        let files = reg.store().qualified("source_file");
        let pulse = |reg: &mut Registry, uid: &str| {
            opt_text(
                reg,
                &format!(
                    "SELECT f.pulse_sequence_name FROM {fp} f JOIN {series} s ON s.id = f.series_id \
                     WHERE s.series_instance_uid = '{uid}'"
                ),
            )
        };
        assert_eq!(pulse(&mut reg, "B.3"), None, "{name}");
        assert_eq!(
            one(&mut reg, &format!("SELECT COUNT(*) FROM {private}")),
            0,
            "{name}"
        );

        // The re-read.
        let mut s = settings(&dir, Some(&pack));
        s.reread = vec!["GE MEDICAL SYSTEMS".into(), " siemens healthineers ".into()];
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            report.parsed, 3,
            "{name}: the GE and XA MR files and no other"
        );
        assert_eq!(report.unchanged, 0, "{name}: nothing walked");
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {files} WHERE status = 'gone'")
            ),
            0,
            "{name}: a re-read marks nothing gone"
        );
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {files} WHERE status = 'ingested'")
            ),
            5,
            "{name}"
        );
        assert_eq!(
            opt_text(
                &mut reg,
                &format!(
                    "SELECT m.pulse_sequence_name FROM {series_mr} m JOIN {series} s ON s.id = m.series_id \
                     WHERE s.series_instance_uid = 'B.3'"
                )
            )
            .as_deref(),
            Some("epfid2d1_64"),
            "{name}"
        );
        // Every file of the GE series was read, so the series knows its two
        // passes disagreed.
        let varied = opt_text(
            &mut reg,
            &format!(
                "SELECT p.varied FROM {private} p JOIN {series} s ON s.id = p.series_id \
                 WHERE s.series_instance_uid = 'B.1'"
            ),
        );
        assert_eq!(varied.as_deref(), Some("0043xxA3 GEMS_PARM_01"), "{name}");
        // Philips was not read again, so it has no private row yet.
        assert_eq!(
            one(
                &mut reg,
                &format!(
                    "SELECT COUNT(*) FROM {private} p JOIN {series} s ON s.id = p.series_id \
                     WHERE s.series_instance_uid = 'B.4'"
                )
            ),
            0,
            "{name}"
        );

        // The fingerprints of the series read again are derived again.
        nils_classify::run(
            &mut reg,
            &nils_classify::Settings::default(),
            &Cancel::new(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            pulse(&mut reg, "B.3").as_deref(),
            Some("epfid2d1_64"),
            "{name}"
        );
        let id = stack_of_series(&mut reg, "B.1");
        let (_, private) = nils_classify::classify::stack_of(reg.store(), &pack, id)
            .unwrap()
            .unwrap();
        let at = pack
            .ingest
            .iter()
            .position(|i| i.name == "ge_pulse_sequence_name")
            .unwrap();
        assert_eq!(private[at], "3dasl", "{name}");

        // A re-read has no dry run.
        let mut dry = s.clone();
        dry.dry_run = true;
        assert!(
            nils_digest::dry_run(&dry)
                .unwrap_err()
                .to_string()
                .contains("no dry run"),
            "{name}"
        );
    }
}
