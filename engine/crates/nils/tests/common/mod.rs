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

/// The identity rule these tests declare an identified dataset with. A
/// dataset whose originals are in play says what PatientID holds, and one
/// with no rule is refused (2026-11-11); here it holds the type `patient-id`,
/// said in so many words.
pub const PATIENT_ID_RULE: &str =
    "identity:\n  id_type: patient-id\n  from:\n    - field: PatientID\n";

/// [`PATIENT_ID_RULE`] as a file in `dir`, which is never the dataset's own
/// folder: the path `--identity` takes.
pub fn patient_id_rule(dir: &nils_dicom::synth::TempDir) -> String {
    dir.file("patient-id-rule.yml", PATIENT_ID_RULE.as_bytes())
        .display()
        .to_string()
}

/// [`PATIENT_ID_RULE`] as a door's body takes it, in `identity`.
pub fn patient_id_rule_json() -> serde_json::Value {
    serde_json::json!({"id_type": "patient-id", "from": [{"field": "PatientID"}]})
}
