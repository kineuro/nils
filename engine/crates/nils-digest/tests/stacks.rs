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

fn echo_number(n: &str) -> synth::Elem {
    synth::text(tags::ECHO_NUMBERS, VR::IS, n)
}

fn instance_number(n: &str) -> synth::Elem {
    synth::text(tags::INSTANCE_NUMBER, VR::IS, n)
}

fn ipp(position: &str) -> synth::Elem {
    synth::text(tags::IMAGE_POSITION_PATIENT, VR::DS, position)
}

fn repetition_time(ms: &str) -> synth::Elem {
    synth::text(tags::REPETITION_TIME, VR::DS, ms)
}

fn inversion_time(ms: &str) -> synth::Elem {
    synth::text(tags::INVERSION_TIME, VR::DS, ms)
}

fn flip_angle(degrees: &str) -> synth::Elem {
    synth::text(tags::FLIP_ANGLE, VR::DS, degrees)
}

fn image_type(t: &str) -> synth::Elem {
    synth::text(tags::IMAGE_TYPE, VR::CS, t)
}

const AXIAL: &str = "1\\0\\0\\0\\1\\0";
const SAGITTAL: &str = "0\\1\\0\\0\\0\\-1";
const CORONAL: &str = "1\\0\\0\\0\\0\\-1";

/// A phase-contrast cine as one vendor writes it: `frames` images at one
/// position in one plane, each frame's place in the cycle written as its
/// EchoNumbers and its InstanceNumber, the echo time on the first two frames
/// and zero on the rest, no trigger time and no temporal position; and a
/// scout of another plane, with timing of its own, filed in the same series.
fn cine(dir: &TempDir, study: &str, series: &str, frames: u32) {
    for n in 1..=frames {
        let te = if n <= 2 { "11.6" } else { "0" };
        dir.file(
            &format!("{series}/{n}"),
            &mr(
                study,
                series,
                &format!("{series}.{n}"),
                "P1",
                &[
                    echo_number(&n.to_string()),
                    instance_number(&n.to_string()),
                    echo_time(te),
                    repetition_time("40.9"),
                    flip_angle("20"),
                    image_type("ORIGINAL\\PRIMARY\\M"),
                    iop(SAGITTAL),
                    ipp("-90\\-76.8\\-32.8"),
                ],
            ),
        );
    }
    dir.file(
        &format!("{series}/0"),
        &mr(
            study,
            series,
            &format!("{series}.0"),
            "P1",
            &[
                echo_number("1"),
                instance_number("0"),
                echo_time("20"),
                repetition_time("150"),
                flip_angle("90"),
                image_type("ORIGINAL\\PRIMARY\\SCOUT"),
                iop(CORONAL),
                ipp("0\\-170\\170"),
            ],
        ),
    );
}

/// One volume whose slices write EchoNumbers 1 and 2 in turn under one echo
/// time, as another vendor's 3D EPI does: `slices` positions, each holding
/// one image.
fn alternating(dir: &TempDir, study: &str, series: &str, slices: u32) {
    for n in 1..=slices {
        let z = 2.0 * f64::from(n);
        dir.file(
            &format!("{series}/{n}"),
            &mr(
                study,
                series,
                &format!("{series}.{n}"),
                "P1",
                &[
                    echo_number(if n % 2 == 1 { "1" } else { "2" }),
                    instance_number(&n.to_string()),
                    echo_time("25.5"),
                    repetition_time("60"),
                    iop(AXIAL),
                    ipp(&format!("-120\\-110\\{z}")),
                ],
            ),
        );
    }
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

/// Record 37 S8: an enhanced multi-frame file is read whole. Candidate Q's
/// shape, synthesized: one object of eight frames, four axial and four
/// sagittal, beside an ordinary single-frame instance of the same series.
#[test]
fn an_enhanced_file_whose_frames_hold_two_orientations_becomes_two_stacks() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-enhanced");
        let per_frame: Vec<Vec<synth::Elem>> = (0..8)
            .map(|i| {
                vec![synth::fg_orientation(match i < 4 {
                    true => "1\\0\\0\\0\\1\\0",
                    false => "0\\1\\0\\0\\0\\-1",
                })]
            })
            .collect();
        dir.file(
            "a/1",
            &enhanced("A", "A.1", "A.1.1", "P1", Vec::new(), per_frame),
        );
        // an enhanced object whose frames agree is one stack, as before
        dir.file(
            "a/2",
            &enhanced(
                "A",
                "A.2",
                "A.2.1",
                "P1",
                vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")],
                (0..4).map(|_| Vec::new()).collect(),
            ),
        );
        let s = settings(&dir);
        let mut reg = lab.open();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 2, "{name}");
        assert_eq!(report.series, 2, "{name}");
        // two files, three stacks: the split one is two
        assert_eq!(report.stacks, 3, "{name}");
        assert_eq!(report.frames, 12, "{name}");
        assert_eq!(report.multi_stack_files, 1, "{name}");
        let w = report.written.clone().unwrap();
        assert_eq!(w.stacks_created, 3, "{name}");
        assert_eq!(w.frame_groups, 2, "{name}");
        let split: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.kind == "frames_multi_stack")
            .collect();
        assert_eq!(split.len(), 1, "{name}: {:?}", report.diagnostics);
        assert_eq!(split[0].count, 1, "{name}");
        assert_eq!(split[0].samples, ["2 stacks in one file"], "{name}");

        // the split series holds two stacks, one per orientation, each
        // counting the one instance its frames came from
        assert_eq!(
            texts(
                &mut reg,
                "SELECT k.orientation FROM {stack} k JOIN {series} s ON s.id = k.series_id \
                 WHERE s.series_instance_uid = 'A.1' ORDER BY k.stack_index"
            ),
            ["Axial", "Sagittal"],
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT k.n_instances FROM {stack} k JOIN {series} s ON s.id = k.series_id \
                 WHERE s.series_instance_uid = 'A.1' ORDER BY k.stack_index"
            ),
            [1, 1],
            "{name}"
        );
        // a stack made of frames says what those frames said, not what the
        // file's first frame said
        assert_eq!(
            texts(
                &mut reg,
                "SELECT k.image_orientation_patient FROM {stack} k \
                 JOIN {series} s ON s.id = k.series_id \
                 WHERE s.series_instance_uid = 'A.1' ORDER BY k.stack_index"
            ),
            ["1\\0\\0\\0\\1\\0", "0\\1\\0\\0\\0\\-1"],
            "{name}"
        );
        // the instance is filed under the stack of its first frame
        assert_eq!(
            texts(
                &mut reg,
                "SELECT k.orientation FROM {instance} i JOIN {stack} k ON k.id = i.stack_id \
                 JOIN {series} s ON s.id = i.series_id WHERE s.series_instance_uid = 'A.1'"
            ),
            ["Axial"],
            "{name}"
        );
        // and which frames are in which stack is written down
        assert_eq!(
            rows(
                &mut reg,
                "SELECT f.frames, f.n_frames, f.first_frame FROM {instance_frame} f \
                 JOIN {stack} k ON k.id = f.stack_id ORDER BY k.stack_index"
            )
            .iter()
            .map(|r| (
                r.text(0).unwrap().to_string(),
                r.int(1).unwrap(),
                r.int(2).unwrap()
            ))
            .collect::<Vec<_>>(),
            [("1-4".to_string(), 4, 1), ("5-8".to_string(), 4, 5)],
            "{name}"
        );
        // the file whose frames agree writes none of those rows
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {instance_frame} f JOIN {instance} i ON i.id = f.instance_id \
                 WHERE i.sop_instance_uid = 'A.2.1'"
            ),
            0,
            "{name}"
        );

        // a second run reads the same files again and creates nothing
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.unchanged, 2, "{name}");
        let w = report.written.unwrap();
        assert_eq!(w.stacks_created, 0, "{name}");
        assert_eq!(w.frame_groups, 0, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance_frame}"),
            2,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT CAST(SUM(n_instances) AS BIGINT) FROM {stack}"
            ),
            3,
            "{name}"
        );
    }
}

/// A file whose instance another file holds under another series is filed
/// as a duplicate, and the series and stacks it alone made are gone at the
/// end of the run (a real registry held 203 such stacks): every stack and
/// series left holds an instance, and the report counts what went.
#[test]
fn a_series_made_only_of_duplicates_leaves_no_empty_stack_or_series() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-dup");
        dir.file("a/1", &mr("A", "A.1", "A.1.1", "P1", &[echo_time("10")]));
        dir.file("a/2", &mr("A", "A.1", "A.1.2", "P1", &[echo_time("10")]));
        let s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        // the same two instances under a second series, which splits them
        // into two stacks of its own
        dir.file("b/1", &mr("A", "A.2", "A.1.1", "P1", &[echo_time("10")]));
        dir.file("b/2", &mr("A", "A.2", "A.1.2", "P1", &[echo_time("30")]));
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate), (0, 2), "{name}");
        assert_eq!((w.series_created, w.stacks_created), (0, 0), "{name}");
        assert_eq!(
            (w.empty_series_removed, w.empty_stacks_removed),
            (1, 2),
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {stack} s WHERE NOT EXISTS \
                 (SELECT 1 FROM {instance} i WHERE i.stack_id = s.id)"
            ),
            0,
            "{name}: an empty stack is left"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {series} se WHERE NOT EXISTS \
                 (SELECT 1 FROM {instance} i WHERE i.series_id = se.id)"
            ),
            0,
            "{name}: an empty series is left"
        );
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {series}"), 1, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 1, "{name}");
        assert_eq!(one(&mut reg, "SELECT n_stacks FROM {series}"), 1, "{name}");
        // a third run over the same tree makes nothing and removes nothing
        let again = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = again.written.unwrap();
        assert_eq!((w.stacks_created, w.empty_stacks_removed), (0, 0), "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 1, "{name}");
    }
}

/// The stacks of each series, by series UID, as `(uid, n_instances)` in the
/// order of their size and then their echo number.
fn stacks_by_series(reg: &mut nils_registry::Registry) -> Vec<(String, i64)> {
    rows(
        reg,
        "SELECT s.series_instance_uid, k.n_instances FROM {stack} k \
         JOIN {series} s ON s.id = k.series_id \
         ORDER BY s.series_instance_uid, k.n_instances DESC, k.echo_numbers",
    )
    .iter()
    .map(|r| (r.text(0).unwrap().to_string(), r.int(1).unwrap()))
    .collect()
}

/// Every instance is in a stack of its own series, and the stacks' counts
/// add up to their series'.
fn counts_add_up(reg: &mut nils_registry::Registry, name: &str) {
    assert_eq!(
        one(
            reg,
            "SELECT COUNT(*) FROM {instance} i JOIN {stack} k ON k.id = i.stack_id \
             WHERE k.series_id <> i.series_id"
        ),
        0,
        "{name}"
    );
    assert_eq!(
        one(
            reg,
            "SELECT COUNT(*) FROM {series} s WHERE s.n_instances <> \
             (SELECT CAST(SUM(k.n_instances) AS BIGINT) FROM {stack} k WHERE k.series_id = s.id) \
             OR s.n_stacks <> (SELECT COUNT(*) FROM {stack} k WHERE k.series_id = s.id)"
        ),
        0,
        "{name}"
    );
    assert_eq!(
        one(
            reg,
            "SELECT COUNT(*) FROM {stack} k WHERE k.n_instances <> \
             (SELECT COUNT(*) FROM {instance} i WHERE i.stack_id = k.id)"
        ),
        0,
        "{name}"
    );
}

/// A cine whose EchoNumbers counts its frames (one vendor's phase-contrast
/// flow study, 32 frames at one position, the echo time stated on two of
/// them) was 32 stacks of one image and a scout. The echo number is no echo
/// there, since no two of its frames state different echo times: the frames
/// are one stack, the scout of another plane its own, and the frames' stack
/// keeps the echo time the series states.
#[test]
fn a_cine_whose_echo_number_counts_its_frames_is_one_stack_beside_its_scout() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-cine");
        cine(&dir, "C", "C.1", 32);
        let s = settings(&dir);
        let mut reg = lab.open();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 33, "{name}");
        assert_eq!(report.stacks, 2, "{name}");
        let w = report.written.clone().unwrap();
        assert_eq!(w.stacks_created, 2, "{name}");
        assert_eq!(w.echo_stacks_folded, 31, "{name}");
        assert!(report.to_string().contains("folded"), "{name}: {report}");
        assert_eq!(
            stacks_by_series(&mut reg),
            [("C.1".to_string(), 32), ("C.1".to_string(), 1)],
            "{name}"
        );
        counts_add_up(&mut reg, name);
        // the frames' stack: the first frame's row, which states the echo
        // time; the scout: its own plane and timing
        assert_eq!(
            rows(
                &mut reg,
                "SELECT k.echo_numbers, k.echo_time, k.orientation FROM {stack} k \
                 ORDER BY k.n_instances DESC"
            )
            .iter()
            .map(|r| (
                r.text(0).unwrap().to_string(),
                r.double(1).unwrap(),
                r.text(2).unwrap().to_string()
            ))
            .collect::<Vec<_>>(),
            [
                ("1".to_string(), 11.6, "Sagittal".to_string()),
                ("1".to_string(), 20.0, "Coronal".to_string())
            ],
            "{name}"
        );
        // the frames' stack has the first index any frame had: the series'
        // first, or its second where the scout came first
        let index = one(
            &mut reg,
            "SELECT stack_index FROM {stack} WHERE n_instances = 32",
        );
        assert!(index <= 1, "{name}: {index}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(DISTINCT stack_index) FROM {stack}"),
            2,
            "{name}"
        );

        // a second run reads nothing again, creates and folds nothing
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.unchanged, 33, "{name}");
        let w = report.written.unwrap();
        assert_eq!((w.stacks_created, w.echo_stacks_folded), (0, 0), "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 2, "{name}");

        // a run that reads every file again meets the frames' own keys,
        // which the registry no longer holds, and ends where it began
        let mut again = settings(&dir);
        again.restart = true;
        let report = digest(&again, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 33, "{name}");
        assert_eq!(report.written.unwrap().stacks_created, 0, "{name}");
        assert_eq!(
            stacks_by_series(&mut reg),
            [("C.1".to_string(), 32), ("C.1".to_string(), 1)],
            "{name}"
        );
        counts_add_up(&mut reg, name);
    }
}

/// A volume whose slices write EchoNumbers 1 and 2 in turn under one echo
/// time (another vendor's 3D EPI) was two half volumes at twice the spacing.
/// It is one stack, one image at each position.
#[test]
fn a_volume_whose_slices_take_turns_at_the_echo_number_is_one_stack() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-alternating");
        alternating(&dir, "G", "G.1", 8);
        let s = settings(&dir);
        let mut reg = lab.open();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.stacks, 1, "{name}");
        let w = report.written.unwrap();
        assert_eq!((w.stacks_created, w.echo_stacks_folded), (1, 1), "{name}");
        assert_eq!(
            stacks_by_series(&mut reg),
            [("G.1".to_string(), 8)],
            "{name}"
        );
        counts_add_up(&mut reg, name);
    }
}

/// Where the echo number is an echo, each echo states its echo time, and the
/// split stands: a dual echo, three echo numbers of which two state times
/// and one states none, SyMRI's echoes under each saturation delay, a
/// multi-echo series with a magnitude and a phase. A diffusion series and a
/// functional time series, whose echo number never moves, keep the stacks
/// they always had.
#[test]
fn stacks_the_echo_time_tells_apart_keep_their_split() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-echoes");
        let file = |series: &str, n: u32, extra: &[synth::Elem]| {
            let mut e = vec![
                instance_number(&n.to_string()),
                iop(AXIAL),
                ipp(&format!("0\\0\\{}", n % 3)),
            ];
            e.extend(extra.iter().cloned());
            dir.file(
                &format!("{series}/{n}"),
                &mr("E", series, &format!("{series}.{n}"), "P1", &e),
            );
        };
        let mut n = 0;
        // a dual echo, three slices
        for (en, te) in [("1", "10"), ("2", "80")] {
            for _ in 0..3 {
                n += 1;
                file("E.1", n, &[echo_number(en), echo_time(te)]);
            }
        }
        // three echo numbers, two stated echo times and a zero
        for (en, te) in [("1", "10"), ("2", "20"), ("3", "0")] {
            n += 1;
            file("E.2", n, &[echo_number(en), echo_time(te)]);
        }
        // SyMRI: two echoes under each of two saturation delays
        for ti in ["150", "580"] {
            for (en, te) in [("1", "22"), ("2", "99")] {
                for _ in 0..2 {
                    n += 1;
                    file(
                        "M.1",
                        n,
                        &[echo_number(en), echo_time(te), inversion_time(ti)],
                    );
                }
            }
        }
        // a multi-echo series written as a magnitude and a phase per echo
        for t in [
            "ORIGINAL\\PRIMARY\\M_SE\\M\\SE",
            "ORIGINAL\\PRIMARY\\PHASE MAP\\P\\SE",
        ] {
            for (en, te) in [("1", "17.5"), ("2", "35"), ("3", "52.5")] {
                n += 1;
                file("M.2", n, &[echo_number(en), echo_time(te), image_type(t)]);
            }
        }
        // diffusion: the acquired images and a derived map, one echo
        for t in [
            "ORIGINAL\\PRIMARY\\DIFFUSION\\NONE",
            "ORIGINAL\\PRIMARY\\DIFFUSION\\NONE",
            "DERIVED\\PRIMARY\\DIFFUSION\\ADC",
        ] {
            for _ in 0..2 {
                n += 1;
                file(
                    "D.1",
                    n,
                    &[echo_number("1"), echo_time("90"), image_type(t)],
                );
            }
        }
        // a functional time series: two slices, three time points
        for t in 1..=3 {
            for _ in 0..2 {
                n += 1;
                file(
                    "F.1",
                    n,
                    &[
                        echo_number("1"),
                        echo_time("30"),
                        synth::text(tags::TEMPORAL_POSITION_IDENTIFIER, VR::IS, &t.to_string()),
                    ],
                );
            }
        }
        let s = settings(&dir);
        let mut reg = lab.open();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.clone().unwrap();
        assert_eq!(w.echo_stacks_folded, 0, "{name}");
        let per_series = |reg: &mut nils_registry::Registry| {
            rows(
                reg,
                "SELECT s.series_instance_uid, COUNT(*) FROM {stack} k \
                 JOIN {series} s ON s.id = k.series_id \
                 GROUP BY s.series_instance_uid ORDER BY s.series_instance_uid",
            )
            .iter()
            .map(|r| (r.text(0).unwrap().to_string(), r.int(1).unwrap()))
            .collect::<Vec<_>>()
        };
        let want: Vec<(String, i64)> = [
            ("D.1", 2),
            ("E.1", 2),
            ("E.2", 3),
            ("F.1", 1),
            ("M.1", 4),
            ("M.2", 6),
        ]
        .iter()
        .map(|(s, n)| (s.to_string(), *n))
        .collect();
        assert_eq!(per_series(&mut reg), want, "{name}");
        assert_eq!(report.stacks, 18, "{name}");
        assert_eq!(w.stacks_created, 18, "{name}");
        counts_add_up(&mut reg, name);
    }
}

/// The dry run counts the stacks the digest would leave: a cine whose echo
/// number counts its frames as one stack beside its scout, a dual echo as
/// two.
#[test]
fn the_dry_run_counts_what_only_the_echo_number_split_as_one_stack() {
    let dir = TempDir::new("stacks-dry-echo");
    cine(&dir, "C", "C.1", 12);
    alternating(&dir, "G", "G.1", 6);
    for (n, (en, te)) in [("1", "10"), ("2", "80")].into_iter().enumerate() {
        dir.file(
            &format!("e/{n}"),
            &mr(
                "E",
                "E.1",
                &format!("E.1.{n}"),
                "P1",
                &[echo_number(en), echo_time(te)],
            ),
        );
    }
    let mut s = settings(&dir);
    s.dry_run = true;
    let report = dry_run(&s).unwrap();
    assert_eq!(report.parsed, 21);
    assert_eq!(report.series, 3);
    assert_eq!(report.stacks, 2 + 1 + 2);
    assert!(report.written.is_none());
}

/// When the rows a test writes by hand were made.
const NOW: &str = "2026-10-09T12:00:00Z";

/// A statement with `{table}` placeholders for the qualified table names.
fn exec(reg: &mut nils_registry::Registry, sql: &str) {
    let sql = rows_sql(reg, sql);
    reg.store()
        .execute(&sql, &[])
        .unwrap_or_else(|e| panic!("{e}: {sql}"));
}

/// An insert with `{table}` placeholders, answering the new row's id.
fn insert(reg: &mut nils_registry::Registry, sql: &str) -> i64 {
    one(reg, &format!("{sql} RETURNING id"))
}

/// A re-read of one file of every MR series the registry holds.
fn reread(dir: &TempDir) -> nils_digest::Settings {
    let mut s = settings(dir);
    s.reread_every = true;
    s.reread_one = true;
    s
}

/// A digested cine of [`cine`] put back as the rule before the fold left
/// it: the first frame's stack, which the digest kept, and one for each
/// other frame, keyed as no file keys it. Answers the frames' stacks, the
/// first frame's first.
fn old_shape(reg: &mut nils_registry::Registry, frames: u32) -> Vec<i64> {
    let first = one(
        reg,
        &format!("SELECT id FROM {{stack}} WHERE n_instances = {frames} AND echo_numbers = '1'"),
    );
    let mut out = vec![first];
    for n in 2..=frames {
        let te = if n <= 2 { "11.6" } else { "0" };
        let id = insert(
            reg,
            &format!(
                "INSERT INTO {{stack}} (series_id, stack_index, stack_key, modality, orientation, \
                 image_orientation_patient, image_type, echo_numbers, echo_time, \
                 repetition_time, flip_angle, orientation_confidence, n_instances, first_batch_id) \
                 SELECT series_id, {index}, '{n:016x}', modality, orientation, \
                 image_orientation_patient, image_type, '{n}', {te}, \
                 repetition_time, flip_angle, orientation_confidence, 1, first_batch_id \
                 FROM {{stack}} WHERE id = {first}",
                index = 100 + n
            ),
        );
        exec(
            reg,
            &format!("UPDATE {{instance}} SET stack_id = {id} WHERE sop_instance_uid = 'C.1.{n}'"),
        );
        out.push(id);
    }
    exec(
        reg,
        &format!("UPDATE {{stack}} SET n_instances = 1 WHERE id = {first}"),
    );
    exec(
        reg,
        &format!("UPDATE {{series}} SET n_stacks = {}", frames + 1),
    );
    counts_add_up(reg, "the old shape");
    out
}

/// A grouped question of `kind` and `status` over `stacks`.
fn grouped(reg: &mut nils_registry::Registry, kind: &str, status: &str, stacks: &[i64]) -> i64 {
    let item = insert(
        reg,
        &format!(
            "INSERT INTO {{review_item}} (kind, scope, status, created_at, members) \
             VALUES ('{kind}', 'group', '{status}', '{NOW}', {})",
            stacks.len()
        ),
    );
    for s in stacks {
        exec(
            reg,
            &format!("INSERT INTO {{review_member}} (item_id, stack_id) VALUES ({item}, {s})"),
        );
    }
    item
}

/// A stack's own question of `kind` and `status`, answered by `decision`.
fn question(
    reg: &mut nils_registry::Registry,
    kind: &str,
    status: &str,
    stack: i64,
    decision: Option<i64>,
) -> i64 {
    insert(
        reg,
        &format!(
            "INSERT INTO {{review_item}} (kind, scope, ref, status, created_at, decision_id) \
             VALUES ('{kind}', 'stack', '{{\"stack_id\": {stack}}}', '{status}', '{NOW}', {})",
            decision.map_or("NULL".to_string(), |d| d.to_string())
        ),
    )
}

/// A decision on a stack's axis, by an author of `kind`.
fn decision(reg: &mut nils_registry::Registry, stack: i64, kind: &str) -> i64 {
    insert(
        reg,
        &format!(
            "INSERT INTO {{decision}} (scope, ref, axis, value, actor, author_kind, decided_at) \
             VALUES ('stack', '{stack}', 'base', 'PC', 'tester', '{kind}', '{NOW}')"
        ),
    )
}

/// A registry digested before the rule holds a cine as one stack per frame.
/// Every value the rule reads is on the stack rows, so a run that reads one
/// file of each series again folds them; until then, and while a person's
/// decision names a stack that would go, the series stays as it is.
#[test]
fn a_registry_digested_before_takes_the_fold_from_a_re_read_of_its_series() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-before");
        cine(&dir, "C", "C.1", 8);
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));

        let frames = old_shape(&mut reg, 8);
        counts_add_up(&mut reg, name);
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 9, "{name}");

        // an ordinary run reads nothing again and leaves it
        let report = digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.written.unwrap().echo_stacks_folded, 0, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 9, "{name}");

        // a person decided something of the fifth frame's stack: a re-read
        // of one file of the series keeps the group as it is
        let fifth = frames[4];
        exec(
            &mut reg,
            &format!(
                "INSERT INTO {{decision}} (scope, ref, axis, value, actor, author_kind, decided_at) \
                 VALUES ('stack', '{fifth}', 'base', 'PC', 'tester', 'person', '{NOW}')"
            ),
        );
        let again = reread(&dir);
        let report = digest(&again, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 1, "{name}");
        let w = report.written.unwrap();
        assert_eq!((w.echo_stacks_folded, w.echo_groups_kept), (0, 1), "{name}");
        assert_eq!(w.stacks_created, 0, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 9, "{name}");
        counts_add_up(&mut reg, name);

        // without it, the same re-read folds the frames into one stack
        exec(
            &mut reg,
            &format!("DELETE FROM {{decision}} WHERE ref = '{fifth}'"),
        );
        let report = digest(&again, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.unwrap();
        assert_eq!((w.echo_stacks_folded, w.echo_groups_kept), (7, 0), "{name}");
        assert_eq!(w.stacks_created, 0, "{name}");
        assert_eq!(
            stacks_by_series(&mut reg),
            [("C.1".to_string(), 8), ("C.1".to_string(), 1)],
            "{name}"
        );
        counts_add_up(&mut reg, name);
        // the stack that stays is the first frame's
        assert_eq!(
            one(
                &mut reg,
                &format!(
                    "SELECT COUNT(*) FROM {{stack}} WHERE id = {} AND n_instances = 8",
                    frames[0]
                )
            ),
            1,
            "{name}"
        );
    }
}

/// The rows the machine made do not hold a group (the ruling of 2026-10-09
/// for the fold): questions no person answered, grouped or a stack's own,
/// open or superseded, go with the stacks that go, a grouped one keeping its
/// other members; and what the sort said of the stack that stays goes too,
/// so the next sort judges it anew.
#[test]
fn a_group_held_only_by_questions_no_person_answered_is_folded() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-unanswered");
        cine(&dir, "C", "C.1", 8);
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let frames = old_shape(&mut reg, 8);
        let scout = one(
            &mut reg,
            "SELECT id FROM {stack} WHERE orientation = 'Coronal'",
        );
        // the split asked about every frame; a superseded question over the
        // frames and the scout; a frame's own open question
        let split = grouped(&mut reg, "split:one_image_per_stack", "open", &frames);
        let mut wider = frames.clone();
        wider.push(scout);
        let missing = grouped(&mut reg, "post_contrast:missing", "superseded", &wider);
        question(&mut reg, "base:missing", "open", frames[2], None);
        // what the sort said of the first frame's stack and of another
        for s in [frames[0], frames[3]] {
            exec(
                &mut reg,
                &format!(
                    "INSERT INTO {{classification}} (stack_id, pack, pack_version, contract, \
                     job_id, epoch, review_items) VALUES ({s}, 'mri', '1', '1', 0, 0, 0)"
                ),
            );
        }

        let report = digest(&reread(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.unwrap();
        assert_eq!((w.echo_stacks_folded, w.echo_groups_kept), (7, 0), "{name}");
        assert_eq!(
            stacks_by_series(&mut reg),
            [("C.1".to_string(), 8), ("C.1".to_string(), 1)],
            "{name}"
        );
        counts_add_up(&mut reg, name);
        // the split's question held only the frames, and is gone; the
        // superseded one keeps the scout
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {{review_item}} WHERE id = {split}")
            ),
            0,
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                &format!("SELECT stack_id FROM {{review_member}} WHERE item_id = {missing}")
            ),
            [scout],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT members FROM {{review_item}} WHERE id = {missing}")
            ),
            1,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'base:missing'"
            ),
            0,
            "{name}"
        );
        // the stack that stays is the first frame's, and nothing the sort
        // said of it is left: the next sort judges it anew
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT n_instances FROM {{stack}} WHERE id = {}", frames[0])
            ),
            8,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {classification}"),
            0,
            "{name}"
        );
    }
}

/// What a person did holds the group as it is: a question about a stack
/// that would go that a person answered, and a seal, on a stack that would
/// go or on the one that would stay (a sealed sample is not changed by a
/// fold). An answer a model gave and no person put in force holds nothing,
/// and goes with its question.
#[test]
fn a_group_a_person_answered_about_or_a_seal_holds_stays_as_it_was() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-held");
        cine(&dir, "C", "C.1", 8);
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let frames = old_shape(&mut reg, 8);
        let kept = |reg: &mut nils_registry::Registry, why: &str| {
            let report = digest(&reread(&dir), reg).unwrap_or_else(|e| panic!("{name}: {e}"));
            let w = report.written.unwrap();
            assert_eq!(
                (w.echo_stacks_folded, w.echo_groups_kept),
                (0, 1),
                "{name}: {why}"
            );
            assert_eq!(one(reg, "SELECT COUNT(*) FROM {stack}"), 9, "{name}: {why}");
        };

        // a person answered a question about the fifth frame's stack
        let answer = decision(&mut reg, frames[4], "person");
        let item = question(
            &mut reg,
            "base:low_confidence",
            "accepted",
            frames[4],
            Some(answer),
        );
        kept(&mut reg, "a person's answer");

        // the same answer a model's, staged and never put in force
        exec(
            &mut reg,
            &format!("UPDATE {{decision}} SET author_kind = 'model' WHERE id = {answer}"),
        );
        exec(
            &mut reg,
            &format!("UPDATE {{review_item}} SET status = 'staged' WHERE id = {item}"),
        );
        // a seal on the stack that would stay, then on one that would go
        let subject = one(&mut reg, "SELECT subject_id FROM {series}");
        exec(
            &mut reg,
            &format!(
                "INSERT INTO {{sealed_stack}} (sample, stack_id, subject_id, sealed_by, sealed_at) \
                 VALUES ('sample-1', {}, {subject}, 'tester', '{NOW}')",
                frames[0]
            ),
        );
        kept(&mut reg, "a seal on the stack that stays");
        exec(
            &mut reg,
            &format!("UPDATE {{sealed_stack}} SET stack_id = {}", frames[5]),
        );
        kept(&mut reg, "a seal on a stack that goes");

        // nothing a person did is left: the group folds, and the model's
        // answer goes with its question
        exec(&mut reg, "DELETE FROM {sealed_stack}");
        let report = digest(&reread(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.unwrap();
        assert_eq!((w.echo_stacks_folded, w.echo_groups_kept), (7, 0), "{name}");
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {{review_item}} WHERE id = {item}")
            ),
            0,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {{decision}} WHERE id = {answer}")
            ),
            0,
            "{name}"
        );
        counts_add_up(&mut reg, name);
    }
}

/// What a model made of the stacks goes with them: the score tables a run of
/// an operation's model wrote (its pipeline proposes values for an axis),
/// the scores read from them, its staged answer over the frames and the
/// decision that answers it. The stack that stays keeps none of it, so the
/// operation reads as not run for it. A pipeline's own derivative holds the
/// group.
#[test]
fn model_outputs_go_with_the_group_and_a_pipeline_s_own_derivative_holds_it() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("stacks-model");
        cine(&dir, "C", "C.1", 8);
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let frames = old_shape(&mut reg, 8);
        let place = insert(
            &mut reg,
            &format!(
                "INSERT INTO {{place}} (name, role, path, guarantees, created_at) \
                 VALUES ('work', 'working', '/nowhere', '{{}}', '{NOW}')"
            ),
        );
        let pipeline = |reg: &mut nils_registry::Registry, pipe: &str, descriptor: &str| {
            let id = insert(
                reg,
                &format!(
                    "INSERT INTO {{pipeline}} (name, version, tool_version, descriptor, \
                     descriptor_digest, image, image_digest, layout, level, state, added_by, added_at) \
                     VALUES ('{pipe}', '1', '1', '{descriptor}', 'sha256:{pipe}', 'img', \
                     'sha256:img', 'stack', 'stack', 'active', 'tester', '{NOW}')"
                ),
            );
            let run = insert(
                reg,
                &format!(
                    "INSERT INTO {{pipeline_run}} (pipeline_id, params, runtime, runtime_version, \
                     host, device, model_ids, status, started_at, principal) \
                     VALUES ({id}, '{{}}', 'podman', '1', 'h', 'cpu', '[]', 'done', '{NOW}', 'tester')"
                ),
            );
            (id, run)
        };
        let (model, model_run) = pipeline(
            &mut reg,
            "bodypart-infer",
            r#"{"x-nils": {"proposals": [{"axis": "body_part"}]}}"#,
        );
        let (_, seg_run) = pipeline(&mut reg, "seg", "{}");
        let derivative = |reg: &mut nils_registry::Registry, stack: i64, kind: &str, run: i64| {
            insert(
                reg,
                &format!(
                    "INSERT INTO {{derivative}} (kind, scope, stack_id, place_id, path, bytes, \
                     sha256, media_type, run_id, created_at) VALUES ('{kind}', 'stack', {stack}, \
                     {place}, 'd/{stack}-{kind}', 1, 'x', 'text/csv', {run}, '{NOW}')"
                ),
            )
        };
        for s in &frames {
            let table = derivative(&mut reg, *s, "table", model_run);
            exec(
                &mut reg,
                &format!(
                    "INSERT INTO {{measure}} (run_id, pipeline_id, pipeline, derivative_id, source, \
                     scope, stack_id, unit_id, name, type, number, created_at) VALUES ({model_run}, \
                     {model}, 'bodypart-infer', {table}, 'scores', 'stack', {s}, 0, 'fine_brain', \
                     'number', 0.9, '{NOW}')"
                ),
            );
        }
        let staged = grouped(&mut reg, "body_part:model", "staged", &frames);
        let answer = insert(
            &mut reg,
            &format!(
                "INSERT INTO {{decision}} (scope, ref, axis, value, actor, author_kind, decided_at, \
                 staged_at) VALUES ('group', '{staged}', 'body_part', 'head', 'bodypart-infer', \
                 'model', '{NOW}', '{NOW}')"
            ),
        );
        exec(
            &mut reg,
            &format!("UPDATE {{review_item}} SET decision_id = {answer} WHERE id = {staged}"),
        );
        // a pipeline's own output on the seventh frame holds the group
        let own = derivative(&mut reg, frames[6], "output", seg_run);
        let report = digest(&reread(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.unwrap();
        assert_eq!((w.echo_stacks_folded, w.echo_groups_kept), (0, 1), "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {stack}"), 9, "{name}");

        // without it the group folds, and what the model made goes with it
        exec(
            &mut reg,
            &format!("DELETE FROM {{derivative}} WHERE id = {own}"),
        );
        let report = digest(&reread(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.unwrap();
        assert_eq!((w.echo_stacks_folded, w.echo_groups_kept), (7, 0), "{name}");
        counts_add_up(&mut reg, name);
        for (what, sql) in [
            ("tables", "SELECT COUNT(*) FROM {derivative}"),
            ("scores", "SELECT COUNT(*) FROM {measure}"),
            (
                "staged answers",
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'body_part:model'",
            ),
            ("places in them", "SELECT COUNT(*) FROM {review_member}"),
            (
                "a model's decisions",
                "SELECT COUNT(*) FROM {decision} WHERE author_kind = 'model'",
            ),
        ] {
            assert_eq!(one(&mut reg, sql), 0, "{name}: {what}");
        }
        // the stack that stays reads as not answered by the model
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT n_instances FROM {{stack}} WHERE id = {}", frames[0])
            ),
            8,
            "{name}"
        );
    }
}
