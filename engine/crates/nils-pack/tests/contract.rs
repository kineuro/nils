// SPDX-License-Identifier: AGPL-3.0-only

//! The pack contract (`contracts/pack/`), kept honest by the loader
//! (`docs/specs/wave4a-engine-completes.md`, §6.2): the version the engine
//! implements is the version the contract publishes, and every manifest key
//! the loader reads is a property the schema declares.

use std::path::Path;

fn contracts() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts")
}

#[test]
fn the_engine_implements_the_contract_version_it_publishes() {
    let version: u32 = std::fs::read_to_string(contracts().join("pack/VERSION"))
        .expect("contracts/pack/VERSION")
        .trim()
        .parse()
        .expect("a version number");
    assert_eq!(nils_pack::CONTRACT, version);
    assert!(
        contracts()
            .join(format!("pack/v{version}/pack.schema.json"))
            .is_file(),
        "the schema of the published version exists"
    );
}

#[test]
fn every_manifest_key_the_loader_reads_is_on_the_schema() {
    // The keys `nils_pack::load` reads off pack.yml, and nothing else: a key
    // the loader gained without the schema gaining it is a contract change
    // nobody wrote down.
    const READ: &[&str] = &[
        "pack",
        "version",
        "contract",
        "modality",
        "parsers",
        "flags",
        "normalize",
        "axes",
        "rules",
        "order",
        "passes",
        "picks",
        "private",
        "dictionary",
        "bids",
        "review",
        "buckets",
        "fields",
        "levels",
        "mcp",
    ];
    let version: u32 = std::fs::read_to_string(contracts().join("pack/VERSION"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let text =
        std::fs::read_to_string(contracts().join(format!("pack/v{version}/pack.schema.json")))
            .unwrap();
    let schema: serde_json::Value = serde_json::from_str(&text).expect("the schema is JSON");
    let properties = schema["properties"].as_object().expect("properties");
    for key in READ {
        assert!(
            properties.contains_key(*key),
            "{key} is read by the loader and not on the schema"
        );
    }
    for key in properties.keys() {
        assert!(
            READ.contains(&key.as_str()),
            "{key} is on the schema and the loader never reads it"
        );
    }
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(required, ["pack", "version", "contract", "modality"]);
    assert_eq!(schema["additionalProperties"], false);
    // Every property says what it is for: the description is what a reader
    // and an agent get.
    for (key, p) in properties {
        assert!(
            p.get("description")
                .and_then(|d| d.as_str())
                .is_some_and(|d| !d.is_empty()),
            "{key} has no description"
        );
    }
}

#[test]
fn the_mri_pack_s_manifest_keeps_to_the_contract() {
    // The shipped pack's manifest, key by key, against the schema's
    // properties and required keys, without a validator: a key the schema
    // does not know is a contract violation.
    let manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri/pack.yml"),
    )
    .unwrap();
    // Against the version the pack declares, which is what the loader holds
    // it to.
    let declared: u32 = manifest
        .lines()
        .find_map(|l| l.strip_prefix("contract:"))
        .and_then(|v| v.trim().parse().ok())
        .expect("the pack declares its contract");
    let text =
        std::fs::read_to_string(contracts().join(format!("pack/v{declared}/pack.schema.json")))
            .unwrap();
    let schema: serde_json::Value = serde_json::from_str(&text).unwrap();
    let properties = schema["properties"].as_object().unwrap();
    let mut keys = Vec::new();
    for line in manifest.lines() {
        if line.starts_with('#') || line.starts_with(' ') || line.trim().is_empty() {
            continue;
        }
        if let Some((key, _)) = line.split_once(':') {
            keys.push(key.trim().to_string());
        }
    }
    for key in &keys {
        assert!(
            properties.contains_key(key),
            "packs/mri/pack.yml has {key}, which the contract does not"
        );
    }
    for key in ["pack", "version", "contract", "modality"] {
        assert!(
            keys.iter().any(|k| k == key),
            "packs/mri/pack.yml lacks {key}"
        );
    }
}
