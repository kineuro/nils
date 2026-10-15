// SPDX-License-Identifier: AGPL-3.0-only
//! What the engine's integration tests share about the tree they test.
#![allow(dead_code)]

/// The MRI pack the tree ships, as the loader names it (`mri@<version>`), so
/// a test follows a rules release instead of naming one.
pub fn shipped_mri() -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    nils_pack::load(&dir, None)
        .expect("the MRI pack the tree ships loads")
        .id()
}
