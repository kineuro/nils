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
        "excludes",
        "hints",
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
fn every_overlay_key_the_loader_reads_is_on_the_overlay_schema() {
    // Pack contract 5 writes the overlay document down. The keys
    // `nils_pack::Overlay::parse` reads, and nothing else: an overlay naming
    // anything more is refused by the loader and by the schema alike.
    const READ: &[&str] = &[
        "overlay", "version", "pack", "scope", "buckets", "lists", "cases",
    ];
    let version: u32 = std::fs::read_to_string(contracts().join("pack/VERSION"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(version >= 5, "the overlay schema is contract 5's");
    let text =
        std::fs::read_to_string(contracts().join(format!("pack/v{version}/overlay.schema.json")))
            .expect("the overlay schema of the published version exists");
    let schema: serde_json::Value = serde_json::from_str(&text).expect("the schema is JSON");
    let properties = schema["properties"].as_object().expect("properties");
    for key in READ {
        assert!(
            properties.contains_key(*key),
            "{key} is read by the loader and not on the overlay schema"
        );
    }
    for key in properties.keys() {
        assert!(
            READ.contains(&key.as_str()),
            "{key} is on the overlay schema and the loader never reads it"
        );
    }
    assert_eq!(schema["additionalProperties"], false);
    let scopes: Vec<&str> = schema["properties"]["scope"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        scopes,
        nils_pack::overlay::SCOPES,
        "an overlay is keyed on an origin"
    );
    let edit = &schema["$defs"]["edit"];
    assert_eq!(
        edit["additionalProperties"], false,
        "an edit adds and removes, nothing else"
    );
    for (key, p) in properties {
        assert!(
            p.get("description")
                .and_then(|d| d.as_str())
                .is_some_and(|d| !d.is_empty()),
            "{key} has no description"
        );
    }
}

/// The MRI pack as a contract-5 pack: copied, with the keys contract 6
/// added taken out of its manifest and the contract it declares set to 5.
fn mri_at_contract_5() -> std::path::PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
    let mri = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    let to = std::env::temp_dir().join(format!("nils-contract-5-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&to);
    copy(&mri, &to);
    let manifest = std::fs::read_to_string(to.join("pack.yml")).unwrap();
    let mut out = String::new();
    let mut skipping = false;
    for line in manifest.lines() {
        if line.starts_with("excludes:") || line.starts_with("hints:") {
            skipping = true;
            continue;
        }
        if skipping && line.starts_with("  - ") {
            continue;
        }
        skipping = false;
        if line.starts_with("contract:") {
            out.push_str("contract: 5\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(to.join("pack.yml"), out).unwrap();
    to
}

#[test]
fn a_pack_of_an_earlier_contract_loads_under_this_one() {
    // Version 6 added two optional keys and changed none, so a contract-5
    // pack, which is what every pack written before 6 is, loads unchanged:
    // the MRI pack without its exclusions and hints is one.
    let dir = mri_at_contract_5();
    let pack = nils_pack::load(&dir, None).expect("the MRI pack at contract 5 loads");
    assert_eq!(pack.contract, 5);
    assert!(pack.contract < nils_pack::CONTRACT);
    assert!(pack.excludes.is_empty() && pack.hints.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
    let mri = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    let pack = nils_pack::load(&mri, None).expect("the MRI pack loads");
    assert_eq!(
        pack.contract,
        nils_pack::CONTRACT,
        "the shipped pack writes exclusions and hints"
    );
    assert!(!pack.excludes.is_empty() && !pack.hints.is_empty());
    assert!(
        pack.lists.len() > 100,
        "every axis value's word list is a site's to amend: {}",
        pack.lists.len()
    );
    assert!(
        pack.lists.iter().any(|l| l == "technique.TSE"),
        "{:?}",
        pack.lists
    );
    assert!(
        pack.lists.iter().any(|l| l == "base.T1w"),
        "a longhand rule's words too"
    );
    assert!(
        !pack.lists.iter().any(|l| l == "provenance.RawRecon"),
        "the default is reached by no word"
    );
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
