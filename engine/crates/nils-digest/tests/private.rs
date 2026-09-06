// SPDX-License-Identifier: AGPL-3.0-only

//! The private elements a pack asks for are read into the registry
//! (`docs/specs/wave4a-engine-completes.md`, §5.2), per series, under the
//! rule every other series field follows.

mod common;

use dicom_core::{Tag, VR};
use nils_dicom::private::Ingest;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;

use common::{labs, one, settings, texts, with_patient};

fn ingest() -> Vec<Ingest> {
    vec![
        Ingest {
            creator: "SIEMENS MR HEADER".into(),
            group: 0x0019,
            element: 0x0C,
            vr: Some("IS".into()),
        },
        Ingest {
            creator: "SIEMENS MR HEADER".into(),
            group: 0x0051,
            element: 0x0C,
            vr: Some("SH".into()),
        },
    ]
}

/// Two series: one whose files agree on their private elements, one whose
/// files do not, and a third file with no private element at all.
fn tree() -> TempDir {
    let dir = TempDir::new("private");
    let file = |sop: &str, series: &str, b: &str, fov: Option<&str>| {
        let mut e = with_patient(synth::minimal_mr("1.2.3.A", series, sop), "P1");
        // The block sits at slot 0x11 in one file and 0x10 in the others,
        // which is what addressing by creator is for.
        let slot: u16 = if sop.ends_with('2') { 0x0011 } else { 0x0010 };
        e.push(synth::text(Tag(0x0019, slot), VR::LO, "SIEMENS MR HEADER"));
        e.push(synth::text(Tag(0x0019, (slot << 8) | 0x0C), VR::IS, b));
        if let Some(fov) = fov {
            e.push(synth::text(
                Tag(0x0051, 0x0010),
                VR::LO,
                "SIEMENS MR HEADER",
            ));
            e.push(synth::text(Tag(0x0051, 0x100C), VR::SH, fov));
        }
        synth::part10(&MetaFields::mr(sop), &e, true)
    };
    dir.file(
        "a/1",
        &file("1.2.3.A.1.1", "1.2.3.A.1", "1000", Some("FoV 230*230")),
    );
    dir.file(
        "a/2",
        &file("1.2.3.A.1.2", "1.2.3.A.1", "1000", Some("FoV 230*230")),
    );
    dir.file("b/1", &file("1.2.3.A.2.1", "1.2.3.A.2", "1000", None));
    dir.file(
        "b/2",
        &file("1.2.3.A.2.2", "1.2.3.A.2", "0", Some("FoV 250*250")),
    );
    let plain = with_patient(
        synth::minimal_mr("1.2.3.A", "1.2.3.A.3", "1.2.3.A.3.1"),
        "P1",
    );
    dir.file(
        "c/1",
        &synth::part10(&MetaFields::mr("1.2.3.A.3.1"), &plain, true),
    );
    dir
}

#[test]
fn the_elements_a_pack_asks_for_are_kept_per_series_under_their_address() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut s = settings(&dir);
        s.ingest = ingest();
        s.ingest_from = Some("test@0.0.0".into());
        let mut reg = lab.open();
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 5, "{name}");
        assert_eq!(
            report.setup.private, "2 element(s) from test@0.0.0",
            "{name}"
        );

        // One row per series that had a private element; the plain series
        // has none.
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM series_private"),
            2,
            "{name}"
        );
        let agreed = texts(
            &mut reg,
            "SELECT CAST(p.elements AS TEXT) FROM series_private p JOIN series s ON s.id = p.series_id \
             WHERE s.series_instance_uid = '1.2.3.A.1'",
        );
        let v: serde_json::Value = serde_json::from_str(&agreed[0]).unwrap();
        assert_eq!(v["0019xx0C SIEMENS MR HEADER"], "1000", "{name}: {v}");
        assert_eq!(
            v["0051xx0C SIEMENS MR HEADER"], "FoV 230*230",
            "{name}: {v}"
        );
        assert!(
            texts(
                &mut reg,
                "SELECT COALESCE(p.varied, '') FROM series_private p JOIN series s ON s.id = p.series_id \
                 WHERE s.series_instance_uid = '1.2.3.A.1'"
            )[0]
                .is_empty(),
            "{name}: files that agree leave nothing varied"
        );

        // Where the files disagree the smaller value in text order stays,
        // whichever file came first, and the address is listed as varied;
        // a value only one file carried is filled in.
        let disagreed = texts(
            &mut reg,
            "SELECT CAST(p.elements AS TEXT) FROM series_private p \
             JOIN series s ON s.id = p.series_id WHERE s.series_instance_uid = '1.2.3.A.2'",
        );
        let v: serde_json::Value = serde_json::from_str(&disagreed[0]).unwrap();
        assert_eq!(v["0019xx0C SIEMENS MR HEADER"], "0", "{name}: {v}");
        assert_eq!(
            v["0051xx0C SIEMENS MR HEADER"], "FoV 250*250",
            "{name}: {v}"
        );
        let varied = texts(
            &mut reg,
            "SELECT COALESCE(p.varied, '') FROM series_private p JOIN series s ON s.id = p.series_id \
             WHERE s.series_instance_uid = '1.2.3.A.2'",
        );
        assert_eq!(varied[0], "0019xx0C SIEMENS MR HEADER", "{name}");

        // A second run reads nothing new and changes nothing.
        let again = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(again.unchanged, 5, "{name}");
        let after = texts(
            &mut reg,
            "SELECT CAST(p.elements AS TEXT) FROM series_private p JOIN series s ON s.id = p.series_id \
             WHERE s.series_instance_uid = '1.2.3.A.2'",
        );
        assert_eq!(after[0], disagreed[0], "{name}");

        // A file that arrives later for a series the registry already holds
        // is folded into the row the registry has, under the same rule: the
        // row is read back, a smaller value replaces the kept one, and a
        // value the series did not have yet is filled in.
        let mut e = with_patient(
            synth::minimal_mr("1.2.3.A", "1.2.3.A.1", "1.2.3.A.1.9"),
            "P1",
        );
        e.push(synth::text(
            Tag(0x0019, 0x0010),
            VR::LO,
            "SIEMENS MR HEADER",
        ));
        e.push(synth::text(Tag(0x0019, 0x100C), VR::IS, "0"));
        dir.file(
            "a/9",
            &synth::part10(&MetaFields::mr("1.2.3.A.1.9"), &e, true),
        );
        let later = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(later.parsed, 1, "{name}");
        let merged = texts(
            &mut reg,
            "SELECT CAST(p.elements AS TEXT) FROM series_private p JOIN series s ON s.id = p.series_id \
             WHERE s.series_instance_uid = '1.2.3.A.1'",
        );
        let v: serde_json::Value = serde_json::from_str(&merged[0]).unwrap();
        assert_eq!(v["0019xx0C SIEMENS MR HEADER"], "0", "{name}: {v}");
        assert_eq!(
            v["0051xx0C SIEMENS MR HEADER"], "FoV 230*230",
            "{name}: {v}"
        );
        let varied = texts(
            &mut reg,
            "SELECT COALESCE(p.varied, '') FROM series_private p JOIN series s ON s.id = p.series_id \
             WHERE s.series_instance_uid = '1.2.3.A.1'",
        );
        assert_eq!(varied[0], "0019xx0C SIEMENS MR HEADER", "{name}");
    }
}

#[test]
fn with_no_pack_the_digest_reads_no_private_element() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.setup.private, "none", "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM series_private"),
            0,
            "{name}"
        );
    }
}
