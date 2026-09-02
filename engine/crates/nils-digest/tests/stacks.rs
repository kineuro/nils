// SPDX-License-Identifier: AGPL-3.0-only

//! Stacks (§8) over synthetic trees, on SQLite and, when
//! `NILS_TEST_POSTGRES_DSN` is set, on Postgres: a series splits on its
//! signature, a CT series holds null MR columns, the index continues across
//! batches and runs, the orientation lands with its diagnostic, and the dry
//! run counts what the digest would create.

mod common;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, TempDir};
use nils_digest::{digest, dry_run};

use common::*;

fn echo_time(ms: &str) -> synth::Elem {
    synth::text(tags::ECHO_TIME, VR::DS, ms)
}

fn iop(cosines: &str) -> synth::Elem {
    synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, cosines)
}

fn kvp(v: &str) -> synth::Elem {
    synth::text(tags::KVP, VR::DS, v)
}

fn tube_current(v: &str) -> synth::Elem {
    synth::text(tags::X_RAY_TUBE_CURRENT, VR::DS, v)
}

#[test]
fn a_series_splits_on_its_signature_and_a_ct_series_holds_null_mr_columns() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks");
        // one MR series, two echo times; one CT series whose KVPs round alike
        // and whose tube currents do not
        dir.file("a/1", &mr("A", "A.1", "A.1.1", "P1", &[echo_time("10")]));
        dir.file(
            "a/2",
            &mr("A", "A.1", "A.1.2", "P1", &[echo_time("10.004")]),
        );
        dir.file("a/3", &mr("A", "A.1", "A.1.3", "P1", &[echo_time("20")]));
        dir.file("a/4", &mr("A", "A.1", "A.1.4", "P1", &[echo_time("20")]));
        dir.file("a/5", &mr("A", "A.1", "A.1.5", "P1", &[echo_time("20")]));
        dir.file(
            "b/1",
            &ct(
                "B",
                "B.1",
                "B.1.1",
                "P2",
                &[kvp("120"), tube_current("300")],
            ),
        );
        dir.file(
            "b/2",
            &ct(
                "B",
                "B.1",
                "B.1.2",
                "P2",
                &[kvp("120.4"), tube_current("300.4")],
            ),
        );
        dir.file(
            "b/3",
            &ct(
                "B",
                "B.1",
                "B.1.3",
                "P2",
                &[kvp("120"), tube_current("300.6")],
            ),
        );
        let s = settings(&dir);
        let mut reg = lab.open();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 8, "{name}");
        assert_eq!(report.series, 2, "{name}");
        assert_eq!(report.stacks, 4, "{name}");
        let w = report.written.clone().unwrap();
        assert_eq!(w.stacks_created, 4, "{name}");
        assert!(
            report.diagnostics.is_empty(),
            "{name}: {:?}",
            report.diagnostics
        );

        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 4, "{name}");
        assert_eq!(
            ints(
                &mut reg,
                "SELECT n_stacks FROM {series} ORDER BY series_instance_uid"
            ),
            [2, 2],
            "{name}"
        );
        // the MR series: two stacks, by echo time, indexes 0 and 1 in the
        // order the files came
        assert_eq!(
            ints(
                &mut reg,
                "SELECT k.stack_index FROM {stack} k JOIN {series} s ON s.id = k.series_id \
                 WHERE s.series_instance_uid = 'A.1' ORDER BY k.stack_index"
            ),
            [0, 1],
            "{name}"
        );
        assert_eq!(
            rows(
                &mut reg,
                "SELECT k.echo_time, k.n_instances FROM {stack} k JOIN {series} s ON s.id = k.series_id \
                 WHERE s.series_instance_uid = 'A.1' ORDER BY k.echo_time"
            )
            .iter()
            .map(|r| (r.double(0).unwrap().round() as i64, r.int(1).unwrap()))
            .collect::<Vec<_>>(),
            [(10, 2), (20, 3)],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {stack} k JOIN {series} s ON s.id = k.series_id \
                 WHERE s.series_instance_uid = 'A.1' AND k.modality = 'MR' AND k.kvp IS NULL \
                 AND k.orientation = 'Axial' AND k.orientation_confidence = 0.5"
            ),
            2,
            "{name}"
        );
        // the CT series: the KVPs round to one value, the tube currents to two
        assert_eq!(
            ints(
                &mut reg,
                "SELECT k.n_instances FROM {stack} k JOIN {series} s ON s.id = k.series_id \
                 WHERE s.series_instance_uid = 'B.1' AND k.echo_time IS NULL AND k.kvp IS NOT NULL \
                 ORDER BY k.n_instances"
            ),
            [1, 2],
            "{name}"
        );
        // every instance is in a stack of its own series, and the stacks'
        // counts add up to the series'
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {instance} i JOIN {stack} k ON k.id = i.stack_id \
                 WHERE k.series_id = i.series_id"
            ),
            8,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {series} s WHERE s.n_instances <> \
                 (SELECT CAST(SUM(k.n_instances) AS BIGINT) FROM {stack} k WHERE k.series_id = s.id)"
            ),
            0,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {stack} WHERE LENGTH(stack_key) <> 16"
            ),
            0,
            "{name}"
        );

        // a second run creates nothing and moves no count
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.unchanged, 8, "{name}");
        assert_eq!(report.written.unwrap().stacks_created, 0, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 4, "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT CAST(SUM(n_instances) AS BIGINT) FROM {stack}"
            ),
            8,
            "{name}"
        );
    }
}

#[test]
fn the_index_continues_across_batches_and_runs() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-index");
        dir.file("a/1", &mr("A", "A.1", "A.1.1", "P1", &[echo_time("10")]));
        let mut s = settings(&dir);
        s.workers = 1;
        s.batch_rows = 1;
        let mut reg = lab.open();

        digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            ints(&mut reg, "SELECT stack_index FROM {stack}"),
            [0],
            "{name}"
        );

        // two new signatures in a later run, one file per batch: the
        // indexes go on from the registry's, in the order the files came
        dir.file("a/2", &mr("A", "A.1", "A.1.2", "P1", &[echo_time("20")]));
        dir.file("a/3", &mr("A", "A.1", "A.1.3", "P1", &[echo_time("30")]));
        dir.file("a/4", &mr("A", "A.1", "A.1.4", "P1", &[echo_time("10")]));
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.written.unwrap().stacks_created, 2, "{name}");
        assert_eq!(
            ints(
                &mut reg,
                "SELECT stack_index FROM {stack} ORDER BY stack_index"
            ),
            [0, 1, 2],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {stack} WHERE stack_index = 0"
            ),
            2,
            "{name}"
        );
        assert_eq!(one(&mut reg, "SELECT n_stacks FROM {series}"), 3, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT n_instances FROM {series}"),
            4,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(DISTINCT stack_id) FROM {instance} WHERE stack_id IS NOT NULL"
            ),
            3,
            "{name}"
        );
    }
}

#[test]
fn the_orientation_lands_with_its_diagnostic() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-orientation");
        // an axial stack, a sagittal one, and one tilted halfway between Y
        // and Z, which the tie gives to Coronal and the confidence flags
        dir.file(
            "a/1",
            &mr("A", "A.1", "A.1.1", "P1", &[iop("1\\0\\0\\0\\1\\0")]),
        );
        dir.file(
            "a/2",
            &mr("A", "A.1", "A.1.2", "P1", &[iop("1\\0\\0\\0\\1\\0")]),
        );
        dir.file(
            "a/3",
            &mr("A", "A.2", "A.2.1", "P1", &[iop("0\\1\\0\\0\\0\\-1")]),
        );
        dir.file(
            "a/4",
            &mr(
                "A",
                "A.3",
                "A.3.1",
                "P1",
                &[iop("1\\0\\0\\0\\0.70710678\\0.70710678")],
            ),
        );
        dir.file(
            "a/5",
            &mr(
                "A",
                "A.3",
                "A.3.2",
                "P1",
                &[iop("1\\0\\0\\0\\0.70710678\\0.70710678")],
            ),
        );
        // no orientation at all: unknown, not oblique
        dir.file("a/6", &mr("A", "A.4", "A.4.1", "P1", &[]));
        let s = settings(&dir);
        let mut reg = lab.open();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.stacks, 4, "{name}");
        let oblique: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.kind == "orientation_oblique")
            .collect();
        assert_eq!(oblique.len(), 1, "{name}: {:?}", report.diagnostics);
        assert_eq!(oblique[0].count, 1, "{name}");
        assert_eq!(oblique[0].samples, ["Coronal 0.71"], "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT k.orientation FROM {stack} k JOIN {series} s ON s.id = k.series_id \
                 ORDER BY s.series_instance_uid"
            ),
            ["Axial", "Sagittal", "Coronal", "Axial"],
            "{name}"
        );
        let confidences: Vec<f64> = rows(
            &mut reg,
            "SELECT k.orientation_confidence FROM {stack} k JOIN {series} s ON s.id = k.series_id \
             ORDER BY s.series_instance_uid",
        )
        .iter()
        .map(|r| r.double(0).unwrap())
        .collect();
        assert_eq!(confidences[0], 1.0, "{name}");
        assert_eq!(confidences[1], 1.0, "{name}");
        assert!(
            (confidences[2] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "{name}"
        );
        assert_eq!(confidences[3], 0.5, "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT count FROM {diagnostic} WHERE kind = 'orientation_oblique'"
            ),
            1,
            "{name}"
        );
        // the orientation text is stored as read, the class beside it
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {stack} WHERE image_orientation_patient = '0\\1\\0\\0\\0\\-1' \
                 AND orientation = 'Sagittal'"
            ),
            1,
            "{name}"
        );
    }
}

#[test]
fn the_dry_run_counts_the_stacks() {
    let dir = TempDir::new("stacks-dry");
    dir.file("a/1", &mr("A", "A.1", "A.1.1", "P1", &[echo_time("10")]));
    dir.file("a/2", &mr("A", "A.1", "A.1.2", "P1", &[echo_time("20")]));
    dir.file(
        "a/3",
        &mr("A", "A.1", "A.1.3", "P1", &[echo_time("20.001")]),
    );
    dir.file("b/1", &ct("B", "B.1", "B.1.1", "P2", &[]));
    let mut s = settings(&dir);
    s.dry_run = true;
    let report = dry_run(&s).unwrap();
    assert_eq!(report.parsed, 4);
    assert_eq!(report.series, 2);
    assert_eq!(report.stacks, 3);
    assert!(report.written.is_none());
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["stacks"], 3);
    assert!(report.to_string().contains("stacks 3"));
}
