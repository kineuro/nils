// SPDX-License-Identifier: AGPL-3.0-only
//! What the pack crate's tests share: the MRI pack the repository ships, and
//! the version its manifest states, so no test names a version of its own.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The MRI pack as the repository ships it.
pub fn shipped_mri() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

/// The version of the shipped MRI pack, as the loader reads it.
pub fn shipped_mri_version() -> String {
    nils_pack::load(&shipped_mri(), None)
        .expect("the MRI pack loads")
        .version
        .to_string()
}
