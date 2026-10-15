// SPDX-License-Identifier: AGPL-3.0-only
//! What the engine's integration tests share about the tree they test.
#![allow(dead_code)]

use std::sync::OnceLock;

/// The MRI pack the tree ships, named as the loader names it
/// (`mri@<version>`), from its manifest's version line, read once: a test
/// follows a rules release instead of naming one.
pub fn shipped_mri_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri/pack.yml");
        let text = std::fs::read_to_string(&manifest).expect("the MRI pack has a manifest");
        let version = text
            .lines()
            .find_map(|l| l.strip_prefix("version:"))
            .map(|v| {
                v.split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(['"', '\''])
            })
            .expect("the manifest states its version");
        format!("mri@{version}")
    })
}
