// SPDX-License-Identifier: AGPL-3.0-only

//! The MRI pack in the repository loads, and its own corpus is what says so.

use std::path::PathBuf;

fn packs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

#[test]
fn the_mri_pack_loads_and_its_corpus_holds() {
    let pack = match nils_pack::load(&packs().join("mri"), None) {
        Ok(p) => p,
        Err(e) => panic!("the MRI pack does not load:\n{e}"),
    };
    assert_eq!(pack.name, "mri");
    assert_eq!(pack.id(), "mri@0.2.0");
    assert_eq!(pack.modality, "MR");
    assert_eq!(
        pack.parsers.len(),
        5,
        "v0 has five parsers and so does the pack"
    );
    assert_eq!(
        pack.parsers.iter().map(|p| p.preds.len()).sum::<usize>(),
        222,
        "v0's 220 predicates, all of them, and the two record 37 added"
    );
    assert_eq!(
        pack.flags.len(),
        145,
        "v0's 138 flags and the seven helpers it keeps as context methods: \
         record 37 removed four that said the Dixon part twice and added \
         four that say what is wrong with an image"
    );
    assert!(pack.cases >= 15, "{} cases", pack.cases);
    assert!(pack.overlay.is_none());
}

/// Record 37, slice S5. The archive states an identity in `ImageType` on
/// 17,065 stacks that no axis of the pack read, in thirteen flags that were
/// parsed and never looked at again. Every spelling the 2026-09-19 survey
/// counted is below, with the axis it now reaches: the table is the breadth
/// of the archive's vocabulary, where the pack's own corpus carries the
/// headline claims one at a time.
#[test]
fn every_image_type_identity_record_37_named_reaches_an_axis() {
    let pack = nils_pack::load(&packs().join("mri"), None).expect("the MRI pack loads");
    // image type, axis, what the row should store.
    let want: &[(&str, &str, &str)] = &[
        // Composed: the largest single discriminator in the archive, 2,792
        // stacks over six spellings, parsed into a dead `is_composite`.
        ("DERIVED\\PRIMARY\\M\\COMPOSED", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_SP", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_AD", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_AN", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_AD_N", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_MIP", "construct", "Composed"),
        // The echo-combined image, 1,154 stacks, in a dead `is_mean`. It
        // reaches the axis through the susceptibility route as well, which
        // is where 1,084 of those stacks are.
        ("ORIGINAL\\PRIMARY\\M\\MEAN", "construct", "EchoCombined"),
        (
            "DERIVED\\PRIMARY\\SWI\\MEAN",
            "construct",
            "EchoCombined,SWI",
        ),
        (
            "DERIVED\\PRIMARY\\MINIP\\MEAN",
            "construct",
            "EchoCombined,MinIP",
        ),
        // The Dixon parts, 2,773 stacks. These have been on the construct
        // axis since it was written; what was dead was a second spelling of
        // them, and the case here is that the part survived its removal.
        ("ORIGINAL\\PRIMARY\\W\\WATER", "construct", "Water"),
        ("ORIGINAL\\PRIMARY\\M\\FAT", "construct", "Fat"),
        ("ORIGINAL\\PRIMARY\\M\\IN_PHASE", "construct", "InPhase"),
        ("ORIGINAL\\PRIMARY\\M\\IP", "construct", "InPhase"),
        ("ORIGINAL\\PRIMARY\\M\\OUT_PHASE", "construct", "OutPhase"),
        ("ORIGINAL\\PRIMARY\\M\\OPP_PHASE", "construct", "OutPhase"),
        // The quantitative maps, 3,633 stacks. `QMAP` said which quantity it
        // was of in a predicate the axis never read, so every one of them
        // came out as a T1 map and a T2 map at once.
        ("DERIVED\\PRIMARY\\QMAP\\T1", "construct", "T1map"),
        ("DERIVED\\PRIMARY\\QMAP\\T2", "construct", "T2map"),
        ("DERIVED\\PRIMARY\\QMAP\\PD", "construct", "PDmap"),
        ("DERIVED\\PRIMARY\\QMAP", "construct", "Qmap"),
        ("DERIVED\\PRIMARY\\T1 MAP", "construct", "T1map"),
        ("DERIVED\\PRIMARY\\R1", "construct", "R1map"),
        ("DERIVED\\PRIMARY\\R2", "construct", "R2map"),
        ("DERIVED\\PRIMARY\\FLIP ANGLE MAP", "construct", "B1map"),
        // The projection spellings with no predicate at all, 549 stacks, and
        // the truncated value on 326 more.
        ("DERIVED\\PRIMARY\\MINIMUM", "construct", "MinIP"),
        ("DERIVED\\PRIMARY\\MAXIMUM", "construct", "MIP"),
        ("DERIVED\\PRIMARY\\HD MIP", "construct", "MIP"),
        ("DERIVED\\PRIMARY\\MAX_IP", "construct", "MIP"),
        ("DERIVED\\PRIMARY\\MIPT", "construct", "MIP"),
        ("DERIVED\\PRIMARY\\CPR", "construct", "MPR"),
        (
            "DERIVED\\PRIMARY\\PROJECTION IMAG",
            "provenance",
            "ProjectionDerived",
        ),
        (
            "DERIVED\\PRIMARY\\COLLAPSE",
            "provenance",
            "ProjectionDerived",
        ),
        ("DERIVED\\PRIMARY\\PJN", "provenance", "ProjectionDerived"),
        // A statistic over a series, 357 stacks, telling 209 colliding names
        // apart.
        ("DERIVED\\PRIMARY\\TTEST", "construct", "TTestMap"),
        // The metal artefact technique, 16 stacks in two spellings, in a
        // dead `is_mavric`. The composite it also writes is a composed image.
        ("ORIGINAL\\PRIMARY\\M\\MAVRIC", "technique", "MAVRIC"),
        ("DERIVED\\PRIMARY\\MAVRIC_COMPOSITE", "technique", "MAVRIC"),
        (
            "DERIVED\\PRIMARY\\MAVRIC_COMPOSITE",
            "construct",
            "Composed",
        ),
        // And what the file says is wrong with its own image.
        (
            "DERIVED\\PRIMARY\\SWI\\NAVAIL",
            "quality",
            "InputUnavailable",
        ),
        ("ORIGINAL\\PRIMARY\\M\\DISTORTED", "quality", "Distorted"),
        ("DERIVED\\PRIMARY\\ENCRYPTED", "quality", "Encrypted"),
        // An ordinary stack says none of it.
        ("ORIGINAL\\PRIMARY\\M\\ND\\NORM", "quality", ""),
    ];
    let mut wrong = Vec::new();
    for (image_type, axis, expected) in want {
        let mut stack = nils_pack::Stack::new();
        stack
            .set(
                "image_type",
                nils_pack::stack::Value::Text(Some(image_type)),
            )
            .expect("image_type is a field");
        let got = nils_pack::Evaluated::new(&pack, &stack)
            .classify()
            .stored(axis);
        if got != *expected {
            wrong.push(format!(
                "  {image_type}: {axis} is {got:?}, not {expected:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} identities do not reach their axis:\n{}",
        wrong.len(),
        want.len(),
        wrong.join("\n")
    );
}
