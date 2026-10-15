// SPDX-License-Identifier: AGPL-3.0-only
//! What the engine's integration tests share about the tree they test.
#![allow(dead_code)]

use std::sync::OnceLock;

/// The MRI pack as the repository ships it.
pub fn shipped_mri() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

/// The MRI pack the tree ships, named as the loader names it
/// (`mri@<version>`), read once: a test follows a rules release instead of
/// naming one.
pub fn shipped_mri_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        nils_pack::load(&shipped_mri(), None)
            .expect("the MRI pack loads")
            .id()
    })
}
