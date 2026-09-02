// SPDX-License-Identifier: AGPL-3.0-only

//! The dry run over a synthetic tree: every form the reader accepts, every
//! quarantine class, the skips, the filters, the diagnostics, the counts.

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{Filter, Settings, dry_run};

const MR: &str = "1.2.840.10008.5.1.4.1.1.4";
const PET: &str = "1.2.840.10008.5.1.4.1.1.128";
const SECONDARY_CAPTURE: &str = "1.2.840.10008.5.1.4.1.1.7";
const EXPLICIT: &str = "1.2.840.10008.1.2.1";

fn with_patient(mut elems: Vec<synth::Elem>, id: &str) -> Vec<synth::Elem> {
    elems.push(synth::text(tags::PATIENT_ID, VR::LO, id));
    elems
}

/// The tree every test walks: five accepted files, eight refused, one link.
fn tree() -> TempDir {
    let dir = TempDir::new("dryrun");

    // accepted: Part 10 with the preamble, an invalid EchoTime
    let mut a1 = with_patient(
        synth::minimal_mr("1.2.3.A", "1.2.3.A.1", "1.2.3.A.1.1"),
        "P1",
    );
    a1.push(synth::text(
        tags::SPECIFIC_CHARACTER_SET,
        VR::CS,
        "ISO_IR 100",
    ));
    a1.push(synth::text(tags::ECHO_TIME, VR::DS, "9a"));
    dir.file(
        "sub1/IM_0001",
        &synth::part10(&MetaFields::mr("1.2.3.A.1.1"), &a1, true),
    );
    // accepted: Part 10 without the preamble
    let a2 = with_patient(
        synth::minimal_mr("1.2.3.A", "1.2.3.A.1", "1.2.3.A.1.2"),
        "P1",
    );
    dir.file(
        "sub1/IM_0002",
        &synth::part10(&MetaFields::mr("1.2.3.A.1.2"), &a2, false),
    );
    // accepted: a bare implicit VR data set
    let a3 = synth::minimal_mr("1.2.3.A", "1.2.3.A.2", "1.2.3.A.2.1");
    dir.file("sub1/IM_0003.dcm", &synth::bare(&a3, false));
    // accepted: CT with an unknown character set
    let mut b1 = with_patient(
        synth::minimal_ct("1.2.3.B", "1.2.3.B.1", "1.2.3.B.1.1"),
        "P2",
    );
    b1.push(synth::text(
        tags::SPECIFIC_CHARACTER_SET,
        VR::CS,
        "ISO_IR 999",
    ));
    dir.file(
        "sub2/ct/1.dcm",
        &synth::part10(&MetaFields::ct("1.2.3.B.1.1"), &b1, true),
    );
    // accepted: PET written as `PET`, no PatientID
    let mut b2 = synth::minimal_pet("1.2.3.B", "1.2.3.B.2", "1.2.3.B.2.1");
    b2.retain(|e| e.tag != tags::MODALITY);
    b2.push(synth::text(tags::MODALITY, VR::CS, "PET"));
    dir.file(
        "sub2/pet/1.dcm",
        &synth::part10(&MetaFields::pet("1.2.3.B.2.1"), &b2, true),
    );

    // refused: cut inside the data set
    let whole = synth::part10(
        &MetaFields::mr("1.2.3.C.1.1"),
        &synth::minimal_mr("1.2.3.C", "1.2.3.C.1", "1.2.3.C.1.1"),
        true,
    );
    dir.file("bad/trunc.dcm", &whole[..whole.len() - 5]);
    // refused: no SOPInstanceUID in the data set
    let mut nosop = synth::minimal_mr("1.2.3.C", "1.2.3.C.1", "1.2.3.C.1.2");
    nosop.retain(|e| e.tag != tags::SOP_INSTANCE_UID);
    dir.file(
        "bad/nosop.dcm",
        &synth::part10(&MetaFields::mr("1.2.3.C.1.2"), &nosop, true),
    );
    // refused: not DICOM at all
    dir.file("bad/readme.txt", b"this is not a DICOM file\n");
    dir.file("bad/empty", b"");
    dir.file(
        "bad/.DS_Store",
        &[0, 0, 0, 1, b'B', b'u', b'd', b'1', 0, 0, 16, 0],
    );
    // refused: a SOP class outside the nine
    let mut sc = synth::minimal_mr("1.2.3.C", "1.2.3.C.2", "1.2.3.C.2.1");
    sc.retain(|e| e.tag != tags::SOP_CLASS_UID);
    sc.push(synth::text(tags::SOP_CLASS_UID, VR::UI, SECONDARY_CAPTURE));
    dir.file(
        "bad/sc.dcm",
        &synth::part10(
            &MetaFields::with(EXPLICIT, SECONDARY_CAPTURE, "1.2.3.C.2.1"),
            &sc,
            true,
        ),
    );
    // refused: a modality outside the three
    let mut us = synth::minimal_mr("1.2.3.C", "1.2.3.C.3", "1.2.3.C.3.1");
    us.retain(|e| e.tag != tags::MODALITY);
    us.push(synth::text(tags::MODALITY, VR::CS, "US"));
    dir.file(
        "bad/us.dcm",
        &synth::part10(&MetaFields::mr("1.2.3.C.3.1"), &us, true),
    );
    // refused: no modality at all
    let mut nomod = synth::minimal_mr("1.2.3.C", "1.2.3.C.4", "1.2.3.C.4.1");
    nomod.retain(|e| e.tag != tags::MODALITY);
    dir.file(
        "bad/nomod.dcm",
        &synth::part10(&MetaFields::mr("1.2.3.C.4.1"), &nomod, true),
    );

    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("sub1"), dir.path().join("link")).unwrap();
    dir
}

fn settings(dir: &TempDir) -> Settings {
    let mut s = Settings::new(dir.path());
    s.name = "test".into();
    s.workers = 3;
    s.walk_threads = 2;
    s.dry_run = true;
    s
}

#[test]
fn the_dry_run_counts_every_class() {
    let dir = tree();
    let report = dry_run(&settings(&dir)).unwrap();

    assert_eq!(report.seen, 13);
    assert_eq!(report.parsed, 5);
    assert_eq!(report.quarantined, 8);
    assert_eq!(report.filtered, 0);
    assert_eq!(report.walk_errors, 0);
    assert_eq!(report.skipped.symlink, u64::from(cfg!(unix)));
    assert_eq!(report.skipped.special, 0);
    assert!(report.bytes > 0);

    assert_eq!(report.class("not_dicom"), 3);
    assert_eq!(report.class("unreadable"), 0);
    assert_eq!(report.class("parse_error"), 1);
    assert_eq!(report.class("missing_uid"), 1);
    assert_eq!(report.class("unsupported_sop_class"), 1);
    assert_eq!(report.class("missing_modality"), 1);
    assert_eq!(report.class("unsupported_modality"), 1);
    let breakdown = |class: &str| -> Vec<(String, u64)> {
        report
            .quarantine
            .iter()
            .find(|c| c.class == class)
            .unwrap()
            .breakdown
            .iter()
            .map(|k| (k.key.clone(), k.count))
            .collect()
    };
    assert_eq!(breakdown("parse_error"), vec![("truncated".to_string(), 1)]);
    assert_eq!(
        breakdown("missing_uid"),
        vec![("SOPInstanceUID".to_string(), 1)]
    );
    assert_eq!(
        breakdown("unsupported_sop_class"),
        vec![(SECONDARY_CAPTURE.to_string(), 1)]
    );
    assert_eq!(
        breakdown("unsupported_modality"),
        vec![("US".to_string(), 1)]
    );
    assert_eq!(breakdown("not_dicom"), vec![]);

    assert_eq!(report.studies, 2);
    assert_eq!(report.series, 4);
    assert_eq!(report.subjects, 2);
    let keyed = |items: &[nils_digest::report::Keyed]| -> Vec<(String, u64)> {
        items.iter().map(|k| (k.key.clone(), k.count)).collect()
    };
    assert_eq!(
        keyed(&report.modalities),
        vec![
            ("MR".to_string(), 3),
            ("CT".to_string(), 1),
            ("PT".to_string(), 1)
        ]
    );
    assert_eq!(report.sop_classes[0].uid, MR);
    assert_eq!(report.sop_classes[0].name, Some("MR Image Storage"));
    assert_eq!(report.sop_classes[0].count, 3);
    assert!(
        report
            .sop_classes
            .iter()
            .any(|c| c.uid == PET && c.count == 1)
    );
    assert_eq!(
        keyed(&report.forms),
        vec![("part10".to_string(), 4), ("bare-implicit".to_string(), 1)]
    );
    assert_eq!(
        keyed(&report.transfer_syntaxes),
        vec![
            (EXPLICIT.to_string(), 4),
            ("1.2.840.10008.1.2".to_string(), 1)
        ]
    );
    assert_eq!(
        keyed(&report.charsets),
        vec![
            ("(none)".to_string(), 3),
            ("ISO_IR 100".to_string(), 1),
            ("ISO_IR 999".to_string(), 1)
        ]
    );

    // EchoTime feeds a stack column and a series_mr column: one diagnostic each
    assert_eq!(report.kind("value_invalid"), 2);
    assert_eq!(report.kind("charset_unknown"), 1);
    assert_eq!(report.kind("walk_error"), 0);
    let samples = |kind: &str| -> Vec<String> {
        report
            .diagnostics
            .iter()
            .find(|d| d.kind == kind)
            .map(|d| d.samples.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        samples("value_invalid"),
        vec![
            "series_mr.echo_time=9a".to_string(),
            "stack.echo_time=9a".to_string()
        ]
    );
    assert_eq!(samples("charset_unknown"), vec!["ISO_IR 999".to_string()]);

    assert_eq!(report.setup.name, "test");
    assert_eq!(report.setup.workers, 3);
    assert!(report.elapsed_s >= 0.0);
    let text = report.to_string();
    assert!(text.starts_with("nils digest (dry run)   name test   root "));
    assert!(text.contains("13 seen   5 parsed   8 quarantined"));
    assert!(text.contains("MR Image Storage 3"));
    // nothing from a file's content but codes and UIDs
    assert!(!text.contains("P1") && !text.contains("P2"));
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["parsed"], 5);
    assert_eq!(json["quarantine"][0]["class"], "not_dicom");
    assert_eq!(json["quarantine"][0]["count"], 3);
}

#[test]
fn the_files_knob_selects_candidates() {
    let dir = tree();
    let mut s = settings(&dir);

    s.filter = Filter::Dcm;
    let report = dry_run(&s).unwrap();
    assert_eq!(report.seen, 8);
    assert_eq!(report.filtered, 5);
    assert_eq!(report.parsed, 3);
    assert_eq!(report.setup.files, "dcm");

    s.filter = Filter::NoExt;
    let report = dry_run(&s).unwrap();
    assert_eq!(report.seen, 4);
    assert_eq!(report.parsed, 2);
    assert_eq!(report.class("not_dicom"), 2);

    s.filter = Filter::parse("IM_*").unwrap();
    let report = dry_run(&s).unwrap();
    assert_eq!(report.seen, 3);
    assert_eq!(report.parsed, 3);
    assert_eq!(report.filtered, 10);
}

#[test]
fn a_missing_root_fails_the_job() {
    let dir = TempDir::new("dryrun-missing");
    let mut s = Settings::new(dir.path().join("nope"));
    s.dry_run = true;
    let err = dry_run(&s).unwrap_err();
    assert!(err.to_string().starts_with("cannot list "));
}

#[test]
fn an_empty_root_is_an_empty_report() {
    let dir = TempDir::new("dryrun-empty");
    let report = dry_run(&settings(&dir)).unwrap();
    assert_eq!(report.seen, 0);
    assert_eq!(report.files_per_s, 0.0);
    assert!(report.diagnostics.is_empty());
    assert!(report.to_string().contains("  none\n"));
}
