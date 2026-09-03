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
    assert_eq!(pack.id(), "mri@0.1.0");
    assert_eq!(pack.modality, "MR");
    assert_eq!(
        pack.parsers.len(),
        5,
        "v0 has five parsers and so does the pack"
    );
    assert_eq!(
        pack.parsers.iter().map(|p| p.preds.len()).sum::<usize>(),
        220,
        "v0's 220 predicates, all of them"
    );
    assert_eq!(
        pack.flags.len(),
        145,
        "v0's 138 flags and the seven helpers it keeps as context methods"
    );
    assert!(pack.cases >= 15, "{} cases", pack.cases);
    assert!(pack.overlay.is_none());
}
