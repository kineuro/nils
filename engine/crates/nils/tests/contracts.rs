// SPDX-License-Identifier: AGPL-3.0-only

//! The suite and MCP contracts (`contracts/suite`, `contracts/mcp`; Wave 4c
//! §4.5 and §6.7), held to the engine's own vocabulary: the grants and the
//! detail with its order, the ladder a ceiling still names, the proposal
//! kinds and terminal reasons the spec names, the MCP operations the pack
//! loader admits and the door each calls. A live server's documents are
//! held to them in `serve.rs` and `mcp.rs`, which also run the vectors.

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
    assert_eq!(v, 2);
    let dir = format!("suite/v{v}");
    for file in [
        "grants.schema.json",
        "headers.schema.json",
        "purpose.schema.json",
        "capabilities.schema.json",
        "app.schema.json",
        "station.schema.json",
        "vectors/trust-list.json",
        "vectors/grants.json",
    ] {
        let doc = json(&format!("{dir}/{file}"));
        assert_eq!(
            doc["$schema"], "https://json-schema.org/draft/2020-12/schema",
            "{file}"
        );
    }
    // version 1 stays beside it, with the entitlements it fixed; version 2
    // has grants in their place
    assert!(
        contracts()
            .join("suite/v1/entitlements.schema.json")
            .is_file()
    );
    assert!(
        !contracts()
            .join(&dir)
            .join("entitlements.schema.json")
            .exists()
    );
    // the grants: a page and how far a caller goes there, see or work, which
    // has its see beside it, and the assistant's use; sorted by code point
    let g = json(&format!("{dir}/grants.schema.json"));
    let grants = strings(&g["$defs"]["grant"]["enum"]);
    assert_eq!(grants.len(), 24, "{grants:?}");
    let mut sorted = grants.clone();
    sorted.sort();
    assert_eq!(sorted, grants, "sorted by code point");
    for grant in &grants {
        let (page, how) = grant.split_once(':').expect("page:how");
        match how {
            "use" => assert_eq!(page, "assistant"),
            "see" => {}
            "work" => assert!(grants.contains(&format!("{page}:see")), "{grant}"),
            other => panic!("{grant}: {other} is not see, work or use"),
        }
    }
    let details = ["plain", "quasi", "sensitive"];
    assert_eq!(strings(&g["$defs"]["detail"]["enum"]), details);
    assert_eq!(strings(&g["order"]), details);
    let ladder = ["reader", "reviewer", "operator", "admin"];
    assert_eq!(strings(&g["$defs"]["step"]["enum"]), ladder);
    // the headers: the ceiling is still a ladder step, the actor's kinds are the four
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
    // the deployment document: the engine's caller and the person carry
    // grants and detail, and a policy row names its grant, not a role
    let c = json(&format!("{dir}/capabilities.schema.json"));
    assert_eq!(strings(&c["required"]), ["engine", "person", "desk"]);
    assert_eq!(
        strings(&c["properties"]["desk"]["properties"]["mode"]["enum"]),
        ["off", "local", "oidc"]
    );
    let engine = strings(&c["properties"]["engine"]["required"]);
    for key in ["principal", "grants", "detail", "roles", "policy"] {
        assert!(engine.contains(&key.to_string()), "the engine's {key}");
    }
    assert_eq!(
        strings(&c["properties"]["engine"]["properties"]["roles"]["items"]["enum"]),
        ["reader", "reviewer", "operator"],
        "never admin"
    );
    let person = strings(&c["properties"]["person"]["required"]);
    for key in ["subject", "grants", "detail"] {
        assert!(person.contains(&key.to_string()), "the person's {key}");
    }
    let row = strings(&c["$defs"]["policy_row"]["required"]);
    assert!(row.contains(&"grant".to_string()), "{row:?}");
    assert!(!row.contains(&"role".to_string()), "{row:?}");
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
    // the grants vectors name grants of the vocabulary only, sorted, and
    // hold a case in every group the engine runs
    let gv = json(&format!("{dir}/vectors/grants.json"));
    assert_eq!(strings(&gv["everything"]["grants"]), grants);
    for (name, set) in gv["sets"].as_object().unwrap() {
        let held = strings(&set["grants"]);
        assert!(held.iter().all(|x| grants.contains(x)), "{name}: {held:?}");
        let mut sorted = held.clone();
        sorted.sort();
        assert_eq!(sorted, held, "{name}: sorted");
        assert!(details.contains(&set["detail"].as_str().unwrap()), "{name}");
    }
    for group in ["claims", "ceilings", "named", "principals"] {
        assert!(!gv[group].as_array().unwrap().is_empty(), "{group}");
    }
}

#[test]
fn the_mcp_contract_is_the_pack_loader_s_vocabulary_and_every_operation_names_its_door() {
    let v = version("mcp");
    assert_eq!(v, 2);
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
    // version 2: the policy fields name the grant a door needs, not a role
    let policy = strings(&m["$defs"]["policy"]["required"]);
    assert!(policy.contains(&"grant".to_string()), "{policy:?}");
    assert!(!policy.contains(&"role".to_string()), "{policy:?}");
}
