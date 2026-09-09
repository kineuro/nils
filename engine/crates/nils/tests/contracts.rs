// SPDX-License-Identifier: AGPL-3.0-only

//! The suite and MCP contracts (`contracts/suite`, `contracts/mcp`; Wave 4c
//! §4.5 and §6.7), held to the engine's own vocabulary: the entitlements are
//! the engine's roles and `assist`, the ceiling is a role, the proposal
//! kinds and terminal reasons are the closed sets the spec names, the MCP
//! operations are the ones the pack loader admits and each names the door
//! it calls. A live server's documents are held to them in `serve.rs` and
//! `mcp.rs`.

use std::path::{Path, PathBuf};

fn contracts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts")
}

fn version(name: &str) -> u32 {
    std::fs::read_to_string(contracts().join(name).join("VERSION"))
        .unwrap_or_else(|_| panic!("contracts/{name}/VERSION"))
        .trim()
        .parse()
        .expect("a version number")
}

fn json(path: &str) -> serde_json::Value {
    let p = contracts().join(path);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("an array: {v}"))
        .iter()
        .map(|s| s.as_str().expect("a string").to_string())
        .collect()
}

#[test]
fn the_suite_contract_names_the_engine_s_own_vocabulary() {
    let v = version("suite");
    assert_eq!(v, 1);
    let dir = format!("suite/v{v}");
    for file in [
        "entitlements.schema.json",
        "headers.schema.json",
        "purpose.schema.json",
        "capabilities.schema.json",
        "app.schema.json",
        "station.schema.json",
        "vectors/trust-list.json",
    ] {
        let doc = json(&format!("{dir}/{file}"));
        assert_eq!(
            doc["$schema"], "https://json-schema.org/draft/2020-12/schema",
            "{file}"
        );
    }
    // the entitlements: the engine's ladder, then assist
    let e = json(&format!("{dir}/entitlements.schema.json"));
    let ladder: Vec<&str> = ["reader", "reviewer", "operator", "admin"].to_vec();
    assert_eq!(strings(&e["ladder"]), ladder);
    assert_eq!(strings(&e["$defs"]["role"]["enum"]), ladder);
    let mut all = ladder.clone();
    all.push("assist");
    assert_eq!(strings(&e["$defs"]["entitlement"]["enum"]), all);
    assert_eq!(strings(&e["orthogonal"]), ["assist"]);
    // the headers: the ceiling is a role, the actor's kinds are the four
    let h = json(&format!("{dir}/headers.schema.json"));
    assert_eq!(strings(&h["$defs"]["ceiling"]["enum"]), ladder);
    assert_eq!(
        strings(&h["$defs"]["actor"]["properties"]["kind"]["enum"]),
        ["person", "agent", "model", "absent"]
    );
    assert_eq!(h["$defs"]["idempotency_key"]["maxLength"], 256);
    // the purposes: three content classes, two localities
    let p = json(&format!("{dir}/purpose.schema.json"));
    assert_eq!(
        strings(&p["$defs"]["content_class"]["enum"]),
        ["catalog", "rows", "identifiers"]
    );
    assert_eq!(
        strings(&p["$defs"]["locality"]["enum"]),
        ["local", "remote"]
    );
    // the station: the five proposal kinds and the ten terminal reasons
    let s = json(&format!("{dir}/station.schema.json"));
    assert_eq!(
        strings(&s["$defs"]["proposal_kind"]["enum"]),
        [
            "document_version",
            "review_decision",
            "overlay",
            "identity_rule",
            "note"
        ]
    );
    assert_eq!(
        s["$defs"]["terminal_reason"]["enum"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    for field in [
        "id", "app", "purpose", "content", "ceiling", "grant", "brief", "result", "checks",
        "budget", "writes", "phases", "evals",
    ] {
        assert!(
            strings(&s["required"]).contains(&field.to_string()),
            "{field}"
        );
    }
    // the deployment document: the engine part names what the engine serves
    let c = json(&format!("{dir}/capabilities.schema.json"));
    assert_eq!(strings(&c["required"]), ["engine", "person", "desk"]);
    assert_eq!(
        strings(&c["properties"]["desk"]["properties"]["mode"]["enum"]),
        ["off", "local", "oidc"]
    );
    // the vectors: every key and JWKS they name is beside them
    let t = json(&format!("{dir}/vectors/trust-list.json"));
    for entry in t["trust"].as_array().unwrap() {
        let jwks = entry["jwks"].as_str().unwrap();
        assert!(
            contracts().join(&dir).join("vectors").join(jwks).is_file(),
            "{jwks}"
        );
    }
    for (kid, pem) in t["keys"].as_object().unwrap() {
        let pem = pem.as_str().unwrap();
        assert!(
            contracts().join(&dir).join("vectors").join(pem).is_file(),
            "{kid}: {pem}"
        );
    }
    assert!(t["cases"].as_array().unwrap().len() >= 8);
}

#[test]
fn the_mcp_contract_is_the_pack_loader_s_vocabulary_and_every_operation_names_its_door() {
    let v = version("mcp");
    assert_eq!(v, 1);
    let m = json(&format!("mcp/v{v}/mcp.schema.json"));
    let mut ops: Vec<String> = nils_pack::mcp::OPERATIONS
        .iter()
        .map(|s| s.to_string())
        .collect();
    ops.sort();
    assert_eq!(ops.len(), 16, "the vocabulary of sixteen");
    assert_eq!(strings(&m["operations"]), ops);
    assert_eq!(strings(&m["$defs"]["operation"]["enum"]), ops);
    for op in &ops {
        assert!(
            m["$defs"]["input"][op].is_object(),
            "{op} has an input schema"
        );
        assert_eq!(m["$defs"]["input"][op]["type"], "object", "{op}");
        let door = m["$defs"]["door_of"][op]
            .as_str()
            .unwrap_or_else(|| panic!("{op} names a door"));
        assert!(
            door.starts_with("GET /api/") || door.starts_with("POST /api/"),
            "{op}: {door}"
        );
    }
    assert_eq!(m["$defs"]["door_of"].as_object().unwrap().len(), ops.len());
    assert_eq!(
        strings(&m["$defs"]["result"]["required"]),
        ["content", "isError"]
    );
    assert!(m["$defs"]["paging"]["properties"]["next_offset"].is_object());
    assert_eq!(
        strings(&m["$defs"]["policy"]["properties"]["cost"]["enum"]),
        ["free", "bounded", "job", "stream"]
    );
}
