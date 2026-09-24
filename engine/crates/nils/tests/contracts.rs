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
    assert_eq!(v, 3);
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
    assert_eq!(grants.len(), 28, "{grants:?}");
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

/// Record 42 S2: the model contract (`contracts/model`), the card and the
/// lifecycle the engine's registry and Kvasir's both keep, held to what the
/// engine implements: the kinds, the states and transitions, the digest's
/// shape, the slots, and what a card and a check require.
#[test]
fn the_model_contract_is_the_engine_s_registry() {
    let v = version("model");
    assert_eq!(v, 1);
    let card = json(&format!("model/v{v}/card.schema.json"));
    let life = json(&format!("model/v{v}/lifecycle.schema.json"));
    for doc in [&card, &life] {
        assert_eq!(
            doc["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
    }
    assert_eq!(
        strings(&card["$defs"]["kind"]["enum"]),
        nils_registry::model::KINDS
    );
    assert_eq!(
        strings(&card["required"]),
        ["name", "version", "kind", "digest", "task"]
    );
    assert_eq!(card["$defs"]["digest"]["pattern"], "^sha256:[0-9a-f]{64}$");
    let slot = card["$defs"]["slot"]["pattern"].as_str().unwrap();
    assert!(slot.contains("site") && slot.contains("cohort"), "{slot}");
    assert_eq!(
        strings(&life["$defs"]["state"]["enum"]),
        nils_registry::model::STATES
    );
    assert_eq!(
        strings(&life["$defs"]["transition"]["enum"]),
        [
            "registered",
            "admitted",
            "admission_failed",
            "promoted",
            "retired"
        ]
    );
    assert_eq!(
        strings(&life["$defs"]["check"]["required"]),
        ["suite", "passed", "checks"]
    );
    for key in ["id", "digest", "state", "card", "registered_by"] {
        assert!(
            strings(&life["required"]).contains(&key.to_string()),
            "{key}"
        );
    }
}

/// Record 43 S1: the job contract (`contracts/job`), the descriptor, the
/// stack manifest and the results, held to the runner that reads them: the
/// levels, layouts, GPU needs, parameter types and output kinds (the
/// registry's derivative kinds), the image pinned by a manifest digest in
/// the schema's pattern as in the runner's check, and v0's N4 descriptor,
/// re-pinned in this repository, valid under both.
#[test]
fn the_job_contract_is_the_runner_s() {
    use nils_pipeline::descriptor;
    let v = version("job");
    assert_eq!(v, 1);
    let job = json(&format!("job/v{v}/nils.job.schema.json"));
    let results = json(&format!("job/v{v}/results.schema.json"));
    let stacks = json(&format!("job/v{v}/stacks.schema.json"));
    for doc in [&job, &results, &stacks] {
        assert_eq!(
            doc["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
    }
    let x = &job["properties"]["x-nils"]["properties"];
    assert_eq!(strings(&x["analysis-level"]["enum"]), descriptor::LEVELS);
    assert_eq!(
        strings(&x["input"]["properties"]["layout"]["enum"]),
        descriptor::LAYOUTS
    );
    assert_eq!(
        strings(&job["$defs"]["needs"]["properties"]["gpu"]["enum"]),
        descriptor::GPU
    );
    assert_eq!(
        strings(&job["$defs"]["parameter"]["properties"]["type"]["enum"]),
        descriptor::PARAM_TYPES
    );
    assert_eq!(
        strings(&job["$defs"]["output"]["properties"]["kind"]["enum"]),
        descriptor::OUTPUT_KINDS
    );
    for kind in descriptor::OUTPUT_KINDS {
        assert!(nils_registry::derivative::KINDS.contains(&kind), "{kind}");
    }
    // the registry's kinds a run writes itself, and only a run
    for kind in nils_registry::derivative::KINDS {
        assert!(
            descriptor::OUTPUT_KINDS.contains(&kind)
                || nils_registry::derivative::RUN_KINDS.contains(&kind),
            "{kind}"
        );
    }
    assert_eq!(
        strings(&job["$defs"]["output"]["properties"]["level"]["enum"]),
        descriptor::OUTPUT_LEVELS
    );
    // record 49: how units meet their containers, what a unit needs of the
    // lane and the card, and the secrets a pipeline reads
    assert_eq!(strings(&x["units"]["enum"]), descriptor::UNITS);
    for key in ["cores", "memory-gb", "gpu-memory-gb"] {
        assert!(
            job["$defs"]["needs"]["properties"].get(key).is_some(),
            "{key}"
        );
    }
    for key in ["id", "mount", "env", "optional"] {
        assert!(
            job["$defs"]["secret"]["properties"].get(key).is_some(),
            "{key}"
        );
    }
    assert_eq!(x["secrets"]["items"]["$ref"], "#/$defs/secret");
    assert_eq!(
        strings(&job["required"]),
        [
            "name",
            "schema-version",
            "tool-version",
            "container-image",
            "x-nils"
        ]
    );
    let described = job["$defs"]["value_key"]["description"].as_str().unwrap();
    for key in descriptor::RESERVED_KEYS {
        assert!(described.contains(key), "{key} is the engine's own");
    }
    // the image: the schema's pattern and the runner's check agree
    let pattern = regex::Regex::new(
        job["properties"]["container-image"]["properties"]["image"]["pattern"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let hex = "59c45f54a1f1dc69134f63bec91a726e41c71c64a16cc21cda0b54526910a3c3";
    for (image, pinned) in [
        (format!("docker.io/antsx/ants@sha256:{hex}"), true),
        (format!("registry.local:5000/a/b@sha256:{hex}"), true),
        ("antsx/ants:latest".to_string(), false),
        (format!("antsx/ants@sha256:{}", &hex[..12]), false),
        (format!("antsx/ants@sha256:{}", hex.to_uppercase()), false),
    ] {
        assert_eq!(pattern.is_match(&image), pinned, "{image}");
        assert_eq!(descriptor::pinned(&image).is_ok(), pinned, "{image}");
    }
    // the results and the manifest the runner reads and writes
    assert_eq!(
        strings(&results["properties"]["units"]["items"]["properties"]["status"]["enum"]),
        ["succeeded", "failed", "skipped"]
    );
    assert_eq!(
        stacks["properties"]["contract"]["const"],
        nils_pipeline::CONTRACT
    );
    // record 43's rulings: what a stack carries for an image that seeds and
    // picks slices, and what results carry beside the proposals
    let stack = &stacks["properties"]["stacks"]["items"]["properties"];
    for key in ["orientation", "body_part", "technique", "slices"] {
        assert!(stack.get(key).is_some(), "stacks.json names {key}");
    }
    assert_eq!(
        results["properties"]["proposals"]["$ref"],
        "proposals.schema.json"
    );
    for key in ["models", "seeds", "selection"] {
        assert!(results["properties"].get(key).is_some(), "{key}");
    }
    // the image's ENTRYPOINT, when it has one, runs before each command line
    let dockerfile =
        std::fs::read_to_string(contracts().join("../pipelines/nils-bodypart/Dockerfile")).unwrap();
    let entrypoint: Vec<String> = dockerfile
        .lines()
        .filter_map(|l| l.strip_prefix("ENTRYPOINT"))
        .map(|rest| serde_json::from_str(rest.trim()).unwrap())
        .next_back()
        .unwrap_or_default();
    // every descriptor of the body-part image checks, as the catalog takes it
    for entry in ["embed", "seed", "train", "infer"] {
        let text = std::fs::read_to_string(contracts().join(format!(
            "../pipelines/nils-bodypart/bodypart-{entry}/nils.job.yml"
        )))
        .unwrap();
        let d = descriptor::parse(&text).unwrap_or_else(|e| panic!("bodypart-{entry}: {e}"));
        assert_eq!(d.name, format!("bodypart-{entry}"));
        assert_eq!(d.layout, descriptor::Layout::Stacks);
        // wave 43's proof: the image's ENTRYPOINT and a command line that
        // named the program too ran `nils-bodypart nils-bodypart embed`;
        // what the container runs names the program once, then the entry
        let argv = d.argv(&d.resolve(&[]).unwrap(), &[]).unwrap();
        let runs: Vec<&str> = entrypoint.iter().chain(&argv).map(String::as_str).collect();
        assert_eq!(runs[..2], ["nils-bodypart", entry], "{runs:?}");
        assert_eq!(
            runs.iter().filter(|w| **w == "nils-bodypart").count(),
            1,
            "{runs:?}"
        );
        if entry == "train" {
            let model = d.outputs.iter().find(|o| o.kind == "model").unwrap();
            assert!(model.run_level && model.card.is_some());
        }
    }
    // v0's N4, re-pinned: it checks, and its image is the schema's
    let text =
        std::fs::read_to_string(contracts().join("../pipelines/n4-bias-correction/nils.job.yml"))
            .unwrap();
    let n4 = descriptor::parse(&text).unwrap();
    assert_eq!(n4.name, "n4-bias-correction");
    assert!(pattern.is_match(&n4.image.reference));
    assert_eq!(n4.level, descriptor::Level::Session);
    assert_eq!(n4.layout, descriptor::Layout::Bids);
    assert_eq!(
        serde_json::Value::Object(n4.resolve(&[]).unwrap()),
        serde_json::json!({"dimension": 3, "shrink_factor": 4})
    );
}

/// A small reader of the JSON Schema keywords the job contract uses
/// (`$ref`, `type`, `enum`, `const`, `not`, `pattern`, `required`,
/// `properties`, `additionalProperties: false`, `items`, `minItems`,
/// `minimum`, `exclusiveMinimum`, `oneOf`), enough to hold a descriptor to
/// the schema itself and not only to the runner's reading of it. Answers
/// every place the value breaks the schema.
fn breaks(
    schema: &serde_json::Value,
    root: &serde_json::Value,
    v: &serde_json::Value,
    at: &str,
) -> Vec<String> {
    use serde_json::Value;
    let mut out = Vec::new();
    if let Some(r) = schema["$ref"].as_str() {
        let name = r.trim_start_matches("#/$defs/");
        return breaks(&root["$defs"][name], root, v, at);
    }
    if let Some(ty) = schema.get("type") {
        let types: Vec<&str> = match ty {
            Value::String(t) => vec![t.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let fits = types.iter().any(|t| match *t {
            "string" => v.is_string(),
            "number" => v.is_number(),
            "integer" => v.as_f64().is_some_and(|n| n.fract() == 0.0),
            "boolean" => v.is_boolean(),
            "object" => v.is_object(),
            "array" => v.is_array(),
            _ => false,
        });
        if !fits {
            out.push(format!("{at}: not {types:?}: {v}"));
            return out;
        }
    }
    if let Some(e) = schema["enum"].as_array()
        && !e.contains(v)
    {
        out.push(format!("{at}: {v} is not one of {e:?}"));
    }
    if let Some(c) = schema.get("const")
        && c != v
        && !(c.as_str().is_some() && v.as_f64().map(|n| n.to_string()).as_deref() == c.as_str())
    {
        out.push(format!("{at}: {v} is not {c}"));
    }
    if let Some(n) = schema.get("not")
        && breaks(n, root, v, at).is_empty()
    {
        out.push(format!("{at}: {v} is what it may not be"));
    }
    if let (Some(p), Some(t)) = (schema["pattern"].as_str(), v.as_str())
        && !regex::Regex::new(p).unwrap().is_match(t)
    {
        out.push(format!("{at}: {t} does not match {p}"));
    }
    if let Some(n) = v.as_f64() {
        if let Some(m) = schema["minimum"].as_f64()
            && n < m
        {
            out.push(format!("{at}: {n} is below {m}"));
        }
        if let Some(m) = schema["exclusiveMinimum"].as_f64()
            && n <= m
        {
            out.push(format!("{at}: {n} is not above {m}"));
        }
    }
    if let Some(o) = v.as_object() {
        for r in schema["required"].as_array().into_iter().flatten() {
            let k = r.as_str().unwrap();
            if !o.contains_key(k) {
                out.push(format!("{at}: {k} is required"));
            }
        }
        let props = schema["properties"].as_object();
        for (k, value) in o {
            match props.and_then(|p| p.get(k)) {
                Some(s) => out.extend(breaks(s, root, value, &format!("{at}.{k}"))),
                None if schema["additionalProperties"] == false => {
                    out.push(format!("{at}.{k} is not a key"));
                }
                None => {}
            }
        }
    }
    if let Some(a) = v.as_array() {
        if let Some(m) = schema["minItems"].as_u64()
            && (a.len() as u64) < m
        {
            out.push(format!("{at}: fewer than {m} items"));
        }
        if let Some(items) = schema.get("items") {
            for (i, item) in a.iter().enumerate() {
                out.extend(breaks(items, root, item, &format!("{at}[{i}]")));
            }
        }
    }
    if let Some(one) = schema["oneOf"].as_array() {
        let fit = one
            .iter()
            .filter(|s| breaks(s, root, v, at).is_empty())
            .count();
        if fit != 1 {
            out.push(format!("{at}: {v} fits {fit} of oneOf, not one"));
        }
    }
    out
}

/// Record 49 A3 and A4: the table kind, the checks and the roles are the
/// runner's words as the schema names them, and every descriptor this
/// repository ships, the starter catalog first, checks against the schema
/// itself and against the runner.
#[test]
fn every_descriptor_the_repository_ships_validates_against_the_job_contract() {
    use nils_pipeline::descriptor;
    let job = json("job/v1/nils.job.schema.json");
    let out = &job["$defs"]["output"]["properties"];
    assert_eq!(strings(&out["format"]["enum"]), descriptor::TABLE_FORMATS);
    assert_eq!(
        strings(&job["$defs"]["column"]["properties"]["type"]["enum"]),
        descriptor::COLUMN_TYPES
    );
    assert_eq!(
        strings(&job["$defs"]["check"]["oneOf"][1]["properties"]["op"]["enum"]),
        descriptor::CHECK_OPS
    );
    let mut files: Vec<PathBuf> = Vec::new();
    let pipelines = contracts().join("../pipelines");
    for entry in std::fs::read_dir(&pipelines).unwrap().flatten() {
        let dir = entry.path();
        if dir.join("nils.job.yml").is_file() {
            files.push(dir.join("nils.job.yml"));
        }
        for sub in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if sub.path().join("nils.job.yml").is_file() {
                files.push(sub.path().join("nils.job.yml"));
            }
        }
    }
    files.sort();
    assert!(files.len() >= 10, "{files:?}");
    let starters = [
        "n4-bias-correction",
        "synthstrip",
        "synthseg",
        "samseg-lesions",
        "mriqc",
        "freesurfer-recon-all",
    ];
    for s in starters {
        assert!(
            files.iter().any(|f| f.parent().unwrap().ends_with(s)),
            "the starter {s} is in pipelines/"
        );
    }
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let value: serde_json::Value = serde_saphyr::from_str(&text).unwrap();
        let broken = breaks(&job, &job, &value, "descriptor");
        assert!(broken.is_empty(), "{}: {broken:#?}", f.display());
        let d = descriptor::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        d.resolve(&[])
            .unwrap_or_else(|e| panic!("{}: its defaults: {e}", f.display()));
    }
    // and the schema refuses what the runner refuses
    let bad: serde_json::Value = serde_saphyr::from_str(
        "name: x\nschema-version: \"0.5\"\ntool-version: \"1\"\ncontainer-image: {type: docker, image: \"a/b:latest\"}\nx-nils: {analysis-level: session, input: {layout: bids}, outputs: [{id: t, kind: table, path-template: \"sub-{subject}/ses-{session}/t.csv\", columns: [{name: run}]}], qc: [\"snr => 8\"]}\n",
    )
    .unwrap();
    let broken = breaks(&job, &job, &bad, "descriptor");
    for words in ["does not match", "what it may not be", "fits 0 of oneOf"] {
        assert!(
            broken.iter().any(|b| b.contains(words)),
            "{words}: {broken:#?}"
        );
    }
}
