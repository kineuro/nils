// SPDX-License-Identifier: AGPL-3.0-only
//! What the pack crate's tests share: the MRI pack the repository ships, and
//! the version its manifest states, so no test names a version of its own.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The MRI pack as the repository ships it.
pub fn shipped_mri() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

/// The version the shipped MRI pack's `pack.yml` states.
pub fn shipped_mri_version() -> String {
    let text = std::fs::read_to_string(shipped_mri().join("pack.yml"))
        .expect("the MRI pack has a manifest");
    text.lines()
        .find_map(|l| l.strip_prefix("version:"))
        .map(|v| {
            v.split('#')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(['"', '\''])
                .to_string()
        })
        .expect("the manifest states its version")
}
