// SPDX-License-Identifier: AGPL-3.0-only
//! What the release's integration tests share: the MRI pack the repository
//! ships, so no test spells its path.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The MRI pack as the repository ships it.
pub fn shipped_mri() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}
