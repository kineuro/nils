// SPDX-License-Identifier: AGPL-3.0-only

//! Record 48, D1 of the move to the group's install: sealed means sealed on
//! every door. Two of four classified stacks are sealed, and an admin's
//! token, which holds every grant a ladder gives, knocks on every GET door
//! the engine lists and on the ask: nothing any system said of a sealed
//! stack comes back, while the file (its header and its pictures) still
//! opens. The certificate's grant, `sealed:see`, which no ladder set holds,
//! still reads it. At the keyboard the operator is refused the same, unless
//! `--unsealed-access` names a reason, which the audit keeps.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

/// The Postgres schema the second half runs in.
const SCHEMA: &str = "nils_sealed_doors";

fn drop_schema(dsn: &str) {
    let mut store = nils_registry::Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop");
}

/// A registry of four stacks of two people, classified, on SQLite or, with
/// a DSN, on Postgres.
fn registry(dsn: Option<&str>) -> TempDir {
    let home = TempDir::new("sealed-home");
    let dir = TempDir::new("sealed-src");
    for (patient, study, sop, description) in [
        ("P1", "1.2.3.A", "1.2.3.A.1.1", "t1 mprage"),
        ("P1", "1.2.3.B", "1.2.3.B.1.1", "flair axial"),
        ("P2", "1.2.3.C", "1.2.3.C.1.1", "t1 mprage"),
        ("P2", "1.2.3.D", "1.2.3.D.1.1", "t2 spine"),
    ] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
        if description == "flair axial" {
            e.push(synth::text(tags::SEQUENCE_NAME, VR::SH, "*tir2d1rr99"));
            e.push(synth::text(tags::ECHO_TIME, VR::DS, "100"));
            e.push(synth::text(tags::REPETITION_TIME, VR::DS, "9000"));
            e.push(synth::text(tags::INVERSION_TIME, VR::DS, "2500"));
        }
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    let run = |args: &[&str], stdin: Option<&str>| {
        let mut child = nils()
            .arg("--registry")
            .arg(home.path())
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        match stdin {
            Some(text) => child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap(),
            None => drop(child.stdin.take()),
        }
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["key", "add", "k"], Some("a sealed doors test key\n"));
    match dsn {
        Some(dsn) => {
            drop_schema(dsn);
            run(
                &[
                    "init",
                    "--backend",
                    "postgres",
                    "--dsn",
                    dsn,
                    "--schema",
                    SCHEMA,
                    "--key",
                    "k",
                ],
                None,
            );
        }
        None => run(&["init", "--key", "k"], None),
    }
    run(
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            dir.path().to_str().unwrap(),
        ],
        None,
    );
    run(&["fingerprint"], None);
    run(&["classify", "--pack-dir", packs().to_str().unwrap()], None);
    std::mem::forget(dir);
    home
}

/// An admin: every grant a ladder gives, at detail sensitive.
const ADMIN: &str = "admin-sweep-token-of-length";
/// The certificate's computation: an admin with the one grant more.
const CERT: &str = "certificate-sweep-token-of-length";
/// A rater of the reading campaign.
const RATER: &str = "rater-sweep-token-of-length";

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start(home: &TempDir) -> Server {
        let tokens = [
            format!("{ADMIN}=ada@lab:admin"),
            format!("{CERT}=cert@lab:admin,sealed:see"),
            format!("{RATER}=rita@lab:campaigns:work"),
        ]
        .join(",");
        let child = nils()
            .arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "1",
                "--auth",
                "token",
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .env("NILS_TOKENS", tokens)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // held from here, so that a panic below kills it too
        let mut held = Server { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        held
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n",
            body.len()
        );
        if !body.is_empty() {
            head.push_str("Content-Type: application/json\r\n");
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let response = String::from_utf8_lossy(&response).to_string();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let json = serde_json::from_str(text).unwrap_or(Value::String(text.to_string()));
        (status, json)
    }

    fn ok(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> Value {
        let (status, doc) = self.call(method, path, body, token);
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {doc}"
        );
        doc
    }
}

/// Run the command line in the registry: its status, stdout and stderr.
fn cli(home: &TempDir, args: &[&str]) -> (bool, String, String) {
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "cleo")
        .env("HOSTNAME", "lab")
        .env("NILS_PRINCIPAL", "cleo@lab")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// System 1's question on a stack, from the contract's fixture.
fn raise(home: &TempDir, stack: i64, dir: &Path) -> std::process::Output {
    let example: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../contracts/review-item/v4/classify.asked.example.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let evidence = dir.join("evidence.json");
    std::fs::write(&evidence, example["evidence"].to_string()).unwrap();
    nils()
        .arg("--registry")
        .arg(home.path())
        .args([
            "review",
            "asked",
            "--stack",
            &stack.to_string(),
            "--evidence",
            evidence.to_str().unwrap(),
            "--pack-dir",
            packs().to_str().unwrap(),
        ])
        .env("NILS_PRINCIPAL", "cleo@lab")
        .env("NILS_FIXTURES", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn every_stack() -> Value {
    json!({"document": {
        "ast_version": 1,
        "sets": {"s": {"grain": "stack"}},
        "out": {"set": "s", "level": "record"},
    }, "fresh": true})
}

fn keys_of(answer: &Value) -> BTreeSet<i64> {
    answer["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r[0].as_i64())
        .collect()
}

/// What the classifier said of a stack, as the certificate's grant reads
/// it: the rules that fired and the values they gave.
fn said(server: &Server, stack: i64) -> BTreeSet<String> {
    let doc = server.ok("GET", &format!("/api/explain/{stack}"), None, CERT);
    let mut out = BTreeSet::new();
    let mut axes = BTreeSet::new();
    for a in doc["axes"].as_array().into_iter().flatten() {
        axes.insert(a["axis"].as_str().unwrap_or_default().to_string());
        for e in a["evidence"].as_array().into_iter().flatten() {
            if let Some(r) = e["rule"].as_str().filter(|r| r.len() > 3) {
                out.insert(r.to_string());
            }
        }
    }
    // a rule named as its axis is a word of the vocabulary, no stack's
    out.retain(|r| !axes.contains(r));
    out
}

/// Every stack id a document names under `stack`, `stack_id` or `_key`.
fn stacks_in(v: &Value, out: &mut BTreeSet<i64>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if matches!(k.as_str(), "stack" | "stack_id")
                    && let Some(id) = x.as_i64()
                {
                    out.insert(id);
                }
                stacks_in(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| stacks_in(x, out)),
        _ => {}
    }
}

#[test]
fn sealed_means_sealed_on_every_door_but_the_certificate_s() {
    sweep(None);
}

#[test]
fn sealed_means_sealed_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    sweep(Some(&dsn));
    drop_schema(&dsn);
}

fn sweep(dsn: Option<&str>) {
    let home = registry(dsn);
    let out = TempDir::new("sealed-out");
    let server = Server::start(&home);
    server.ok(
        "POST",
        "/api/places",
        Some(json!({"name": "labels-out", "role": "export", "path": out.path().to_str().unwrap()})),
        ADMIN,
    );

    // ------------------------------------------------ before the seal
    let before = server.ok("POST", "/api/ask/run", Some(every_stack()), ADMIN);
    let all = keys_of(&before);
    assert_eq!(all.len(), 4, "{before}");
    let old_handle = before["handle"].as_i64().unwrap();
    // in the order of the files: a T1, the FLAIR, a T1, the spine, told
    // apart by the header text the evidence line shows
    let described = |s: &i64| {
        server
            .ok("GET", &format!("/api/stacks/{s}/why"), None, ADMIN)
            .to_string()
    };
    let t1: Vec<i64> = all
        .iter()
        .filter(|s| described(s).contains("t1 mprage"))
        .copied()
        .collect();
    let flair = *all
        .iter()
        .find(|s| described(s).contains("flair axial"))
        .unwrap();
    let spine = *all
        .iter()
        .find(|s| described(s).contains("t2 spine"))
        .unwrap();
    assert_eq!(t1.len(), 2);
    let ids: Vec<i64> = vec![t1[0], flair, t1[1], spine];
    // the FLAIR and the spine: two stacks of two people
    let sealed: BTreeSet<i64> = [ids[1], ids[3]].into_iter().collect();
    let open: BTreeSet<i64> = [ids[0], ids[2]].into_iter().collect();
    // a person's decision on a sealed stack, while it is not yet sealed
    let base = server.ok("GET", &format!("/api/explain/{}", ids[1]), None, ADMIN);
    let base_value = base["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == "base")
        .and_then(|a| a["value"].as_str())
        .unwrap_or("T2w")
        .to_string();
    // the canaries: what the rules said of a sealed stack and of no other
    let seen_open: BTreeSet<String> = open.iter().flat_map(|s| said(&server, *s)).collect();
    let canaries: BTreeSet<String> = sealed
        .iter()
        .flat_map(|s| said(&server, *s))
        .filter(|r| !seen_open.contains(r))
        .collect();
    // nor a word System 1's fixture question carries on every stack
    let fixture = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../contracts/review-item/v4/classify.asked.example.json"),
    )
    .unwrap();
    let canaries: BTreeSet<String> = canaries
        .into_iter()
        .filter(|c| !fixture.contains(&format!("\"{c}\"")))
        .collect();
    assert!(!canaries.is_empty(), "no rule is the sealed stacks' own");
    // System 1 asked about every stack, and a person answered it on one
    // stack of each kind: decisions, in force, on a sealed stack and an
    // open one
    let mut asked = BTreeMap::new();
    for s in &ids {
        let done = raise(&home, *s, out.path());
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
        let raised: Value = serde_json::from_slice(&done.stdout).unwrap();
        asked.insert(*s, raised["review_item"].as_i64().unwrap());
    }
    let answer = json!({"values": {"base": "T1w", "technique": "MPRAGE", "modifier": ["FatSat"]}});
    for s in [ids[1], ids[0]] {
        server.ok(
            "POST",
            &format!("/api/review/{}/apply", asked[&s]),
            Some(answer.clone()),
            ADMIN,
        );
    }
    // decisions staged, not yet in force, on an open stack and a sealed one
    let mut staged = BTreeMap::new();
    for s in [ids[2], ids[3]] {
        let done = server.ok(
            "POST",
            &format!("/api/review/{}/apply", asked[&s]),
            Some(json!({"values": answer["values"], "stage": true})),
            ADMIN,
        );
        let base = done["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["axis"] == "base")
            .unwrap()["decision"]
            .as_i64()
            .unwrap();
        staged.insert(s, base);
    }
    let review_item = Some(asked[&ids[3]]);
    let decided = server.ok(
        "POST",
        "/api/label-sets",
        Some(json!({"axis": "base", "place": "labels-out", "name": "before"})),
        ADMIN,
    );
    assert_eq!(decided["rows"].as_i64(), Some(2), "{decided}");

    // ------------------------------------------------ sealed
    server.ok(
        "PUT",
        "/api/ask/selections/sealed",
        Some(json!({"document": {
            "ast_version": 1,
            "params": {"ids": {"type": "list", "value": sealed.iter().collect::<Vec<_>>()}},
            "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
            "out": {"set": "s", "level": "record"},
        }})),
        ADMIN,
    );
    let (done, stdout, stderr) = cli(
        &home,
        &[
            "labels",
            "seal",
            "--select",
            "selection:sealed@1",
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
    );
    assert!(done, "{stderr}");
    let seal: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(seal["stacks"].as_i64(), Some(2), "{seal}");
    // classified again once sealed: the batch diagnostics and the review
    // queue leave the sealed stacks out
    let (done, _, stderr) = cli(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
    );
    assert!(done, "{stderr}");
    // a sealed sample still becomes a reading campaign: its keys are the
    // files' identities, never a system's answer
    let made = server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "read",
            "question": {"kind": "axis", "axis": "base"},
            "source": {"selection": "sealed@1"},
            "raters_per_item": 1,
            "raters": ["rita@lab"],
            "adjudication": {"when": "never"},
            "closes_into": "none",
        })),
        ADMIN,
    );
    let items = made["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 2, "{made}");
    let item = items[0]["id"].as_i64().unwrap();
    let stack = items[0]["stack_id"].as_i64().unwrap();
    assert!(sealed.contains(&stack), "{made}");
    let campaign_handle = made["handle_id"].as_i64().unwrap();

    // ------------------------------------------------ the ask
    let now = server.ok("POST", "/api/ask/run", Some(every_stack()), ADMIN);
    assert_eq!(keys_of(&now), open, "{now}");
    let certified = server.ok("POST", "/api/ask/run", Some(every_stack()), CERT);
    assert_eq!(keys_of(&certified), all, "{certified}");
    // a count, the cached answer and the value sampler leave them out too
    let mut counted = every_stack();
    counted["document"]["out"]["level"] = json!("count");
    let n = server.ok("POST", "/api/ask/run", Some(counted.clone()), ADMIN);
    assert_eq!(
        n["rows"][0][2].as_i64().or(n["rows"][0][0].as_i64()),
        Some(2),
        "{n}"
    );
    counted["fresh"] = json!(false);
    let cached = server.ok("POST", "/api/ask/run", Some(counted), ADMIN);
    assert_eq!(cached["rows"], n["rows"], "{cached}");
    // a handle answered before the seal reads without the sealed stacks,
    // and the campaign's frozen keys, which read them, open to no one else
    let rows = server.ok(
        "GET",
        &format!("/api/ask/handles/{old_handle}/rows"),
        None,
        ADMIN,
    );
    assert_eq!(keys_of(&rows), open, "{rows}");
    assert_eq!(rows["withheld_sealed"].as_i64(), Some(2), "{rows}");
    let (status, doc) = server.call(
        "GET",
        &format!("/api/ask/handles/{campaign_handle}/rows"),
        None,
        ADMIN,
    );
    assert_eq!(status, 403, "{doc}");
    let rows = server.ok(
        "GET",
        &format!("/api/ask/handles/{campaign_handle}/rows"),
        None,
        CERT,
    );
    assert_eq!(keys_of(&rows), sealed, "{rows}");
    // the preview, the funnel, a queued run: none reaches a sealed stack
    let preview = server.ok(
        "POST",
        "/api/ask/preview",
        Some(json!({"document": every_stack()["document"]})),
        ADMIN,
    );
    let mut named = BTreeSet::new();
    stacks_in(&preview, &mut named);
    assert!(named.is_disjoint(&sealed), "{preview}");
    assert!(
        !preview.to_string().contains(&format!("[{},", ids[1])),
        "{preview}"
    );

    // ------------------------------------------------ every GET door
    // a label set of decisions, made by the admin, and its door
    let set = server.ok(
        "POST",
        "/api/label-sets",
        Some(json!({"axis": "base", "place": "labels-out", "name": "bases"})),
        ADMIN,
    );
    let decisions = set["id"].as_i64().unwrap();
    let caps = server.ok("GET", "/api/capabilities", None, ADMIN);
    assert!(
        !caps["grants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g == "sealed:see"),
        "admin does not hold the certificate's grant: {caps}"
    );
    // what a vocabulary door names is the pack's, never a stack's
    let vocabulary = [
        "/api/packs",
        "/api/ask/schema",
        "/api/ask/guide",
        "/api/ask/catalog",
        "/api/capabilities",
        "/api/overlays",
        "/api/pipelines",
    ];
    let mut knocked = BTreeMap::new();
    for door in caps["doors"].as_array().unwrap() {
        let door = door.as_str().unwrap();
        let Some(path) = door.strip_prefix("GET ") else {
            continue;
        };
        if path == "/api/events" || path.starts_with("/api/supervise") {
            continue;
        }
        let mut paths = Vec::new();
        let level = if path.starts_with("/api/instances/") {
            "0"
        } else {
            "stack"
        };
        let name = if path.starts_with("/api/ask/selections/") {
            "sealed"
        } else if path.starts_with("/api/cohorts/") {
            "a"
        } else {
            "mri"
        };
        let p = path
            .replace("{stack}", &stack.to_string())
            .replace("{item}", &item.to_string())
            .replace("{level}", level)
            .replace("{field}", "modality")
            .replace("{name}", name)
            .replace("{z}", "0")
            .replace("{z0}-{z1}", "0-1");
        if p.starts_with("/api/timeline/{kind}") || p.starts_with("/api/depends/{kind}") {
            for s in &sealed {
                paths.push(p.replace("{kind}/{id}", &format!("stack/{s}")));
            }
            if let Some(r) = review_item {
                paths.push(p.replace("{kind}/{id}", &format!("review/{r}")));
            }
        } else if p.starts_with("/api/campaigns/") {
            paths.push(p.replace("{id}", "read"));
        } else if p.starts_with("/api/review/") {
            if let Some(r) = review_item {
                paths.push(p.replace("{id}", &r.to_string()));
            }
        } else if p.starts_with("/api/ask/handles/") {
            paths.push(p.replace("{id}", &old_handle.to_string()));
            paths.push(p.replace("{id}", &campaign_handle.to_string()));
        } else if p.starts_with("/api/label-sets/") {
            paths.push(p.replace("{id}", &decisions.to_string()));
            paths.push(p.replace("{id}", &decided["id"].to_string()));
        } else if p == "/api/classify/signals" {
            paths.push(format!("{p}?scope=batch:1"));
        } else {
            paths.push(p.replace("{id}", "1"));
        }
        for p in paths {
            let (status, doc) = server.call("GET", &p, None, ADMIN);
            let text = doc.to_string();
            if status == 200 && !vocabulary.iter().any(|v| p.starts_with(v)) {
                for c in &canaries {
                    assert!(
                        !text.contains(&format!("\"{c}\"")),
                        "GET {p} names {c}, which the rules said of a sealed stack alone: {text}"
                    );
                }
            }
            knocked.insert(p, status);
        }
    }
    assert!(knocked.len() > 60, "{knocked:?}");
    // the sweep would have seen a leak: the certificate's grant reads the
    // canaries where the admin read none
    let told: String = sealed
        .iter()
        .map(|s| {
            server
                .ok("GET", &format!("/api/explain/{s}"), None, CERT)
                .to_string()
        })
        .collect();
    assert!(
        canaries.iter().any(|c| told.contains(&format!("\"{c}\""))),
        "{told}"
    );

    // ------------------------------------------------ door by door
    // explain and the evidence line: blind for the admin, not for the
    // certificate's grant
    for s in &sealed {
        let e = server.ok("GET", &format!("/api/explain/{s}"), None, ADMIN);
        assert_eq!(e["sealed"], true, "{e}");
        assert!(e["axes"].as_array().unwrap().is_empty(), "{e}");
        let w = server.ok("GET", &format!("/api/stacks/{s}/why"), None, ADMIN);
        assert_eq!(w["blind"], true, "{w}");
        assert!(w.get("axes").is_none(), "{w}");
        // blind hides the systems' answers, never the file
        assert!(w["header"].is_object(), "{w}");
        let e = server.ok("GET", &format!("/api/explain/{s}"), None, CERT);
        assert!(!e["axes"].as_array().unwrap().is_empty(), "{e}");
    }
    for s in &open {
        let e = server.ok("GET", &format!("/api/explain/{s}"), None, ADMIN);
        assert!(!e["axes"].as_array().unwrap().is_empty(), "{e}");
    }
    // the review: not listed, not there, not answered
    for token in [ADMIN, CERT] {
        let listed = server.ok("GET", "/api/review", None, token);
        let mut named = BTreeSet::new();
        for i in listed["items"].as_array().unwrap() {
            if let Some(s) = i["ref"]["stack_id"].as_i64() {
                named.insert(s);
            }
        }
        if token == ADMIN {
            assert!(named.is_disjoint(&sealed), "{listed}");
            assert!(named.contains(&ids[2]), "{listed}");
        } else {
            assert!(named.contains(&ids[3]), "{listed}");
        }
    }
    if let Some(r) = review_item {
        let (status, doc) = server.call("GET", &format!("/api/review/{r}"), None, ADMIN);
        assert_eq!(status, 404, "{doc}");
        let (status, doc) = server.call(
            "POST",
            &format!("/api/review/{r}/apply"),
            Some(json!({"value": base_value})),
            ADMIN,
        );
        assert_eq!(status, 404, "{doc}");
        let (status, doc) = server.call("GET", &format!("/api/timeline/review/{r}"), None, ADMIN);
        assert_eq!(status, 404, "{doc}");
        server.ok("GET", &format!("/api/review/{r}"), None, CERT);
    }
    // the timeline keeps the arrival and loses the rest
    for s in &sealed {
        let t = server.ok("GET", &format!("/api/timeline/stack/{s}"), None, ADMIN);
        assert_eq!(t["sealed"], true, "{t}");
        for e in t["events"].as_array().unwrap() {
            assert!(
                matches!(e["kind"].as_str(), Some("landed" | "classified")),
                "{e}"
            );
        }
    }
    // the certificate's grant reads the decision on the sealed stack
    let t = server.ok(
        "GET",
        &format!("/api/timeline/stack/{}", ids[1]),
        None,
        CERT,
    );
    assert!(t.get("sealed").is_none(), "{t}");
    assert!(
        t["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"] == "decision"),
        "{t}"
    );
    // System 1's question on a sealed stack is never raised
    let raised = raise(&home, stack, out.path());
    assert!(
        !raised.status.success(),
        "a sealed stack became a review item"
    );
    let stderr = String::from_utf8_lossy(&raised.stderr);
    assert!(stderr.contains("sealed"), "{stderr}");
    // a label set of decisions leaves the sealed stacks' decisions out
    assert_eq!(set["rows"].as_i64(), Some(1), "{set}");
    assert_eq!(set["left_out_sealed"].as_i64(), Some(1), "{set}");
    let shown = server.ok("GET", &format!("/api/label-sets/{decisions}"), None, ADMIN);
    let tsv = shown["files"]["labels.tsv"].as_str().unwrap_or_default();
    for line in tsv.lines().skip(1) {
        let s: i64 = line.split('\t').next().unwrap().parse().unwrap();
        assert!(!sealed.contains(&s), "{tsv}");
    }
    // the set made before the seal reads without the sealed stack's line,
    // and whole to the certificate's grant
    let before = decided["id"].as_i64().unwrap();
    let shown = server.ok("GET", &format!("/api/label-sets/{before}"), None, ADMIN);
    assert_eq!(shown["withheld_sealed"].as_i64(), Some(1), "{shown}");
    let tsv = shown["files"]["labels.tsv"].as_str().unwrap();
    assert!(
        !tsv.lines()
            .skip(1)
            .any(|l| l.starts_with(&format!("{}\t", ids[1]))),
        "{tsv}"
    );
    let whole = server.ok("GET", &format!("/api/label-sets/{before}"), None, CERT);
    let tsv = whole["files"]["labels.tsv"].as_str().unwrap();
    assert!(
        tsv.lines()
            .skip(1)
            .any(|l| l.starts_with(&format!("{}\t", ids[1]))),
        "{tsv}"
    );
    // the classifier's signals and a rehearsal say nothing of them
    let signals = server.ok("GET", "/api/classify/signals?scope=batch:1", None, ADMIN);
    for c in &canaries {
        assert!(
            !signals.to_string().contains(&format!("\"{c}\"")),
            "{c}: {signals}"
        );
    }
    // a release never carries a sealed stack
    let selected = server.ok(
        "POST",
        "/api/select",
        Some(json!({"subjects": ["P1", "P2"]})),
        ADMIN,
    );
    let reached = selected["reaches"]["stacks"].as_i64().unwrap();
    assert!((1..=2).contains(&reached), "{selected}");
    // the pictures are the file: a sealed stack's are not refused for the
    // seal (none were built here, so the door says so)
    let (status, doc) = server.call(
        "GET",
        &format!("/api/instances/{stack}/manifest"),
        None,
        ADMIN,
    );
    assert_ne!(status, 403, "{doc}");
    // the rater reads the file and answers, blind as ever
    let why = server.ok(
        "GET",
        &format!("/api/campaigns/read/items/{item}/why"),
        None,
        RATER,
    );
    assert_eq!(why["blind"], true, "{why}");
    assert!(why["suggested"].is_null(), "{why}");
    assert!(why["header"].is_object(), "{why}");

    // ------------------------------------------------ no decision is written
    // an explicit sealed stack or decision is refused, even to the
    // certificate's grant, which reads and never writes
    for token in [ADMIN, CERT] {
        let (status, doc) = server.call(
            "POST",
            "/api/decisions/commit",
            Some(json!({"stacks": [ids[3]]})),
            token,
        );
        assert_eq!(status, 409, "{doc}");
        let (status, doc) = server.call(
            "POST",
            &format!("/api/decisions/{}/commit", staged[&ids[3]]),
            Some(json!({})),
            token,
        );
        assert_eq!(status, 409, "{doc}");
        let (status, doc) = server.call(
            "POST",
            "/api/picks",
            Some(json!({"role": "t1w", "stacks": [ids[3]], "why": "the sharpest"})),
            token,
        );
        assert_eq!(status, 409, "{doc}");
        assert!(doc.to_string().contains("sealed"), "{doc}");
    }
    let (status, doc) = server.call(
        "POST",
        &format!("/api/review/{}/apply", asked[&ids[3]]),
        Some(answer.clone()),
        CERT,
    );
    assert_eq!(status, 409, "{doc}");
    // a filter that reaches a sealed stack leaves it out and says so
    let part = server.ok(
        "POST",
        "/api/decisions/commit",
        Some(json!({"axis": "base", "anyway": true})),
        ADMIN,
    );
    assert_eq!(part["left_out_sealed"].as_i64(), Some(1), "{part}");
    assert_eq!(
        part["committed"].as_array().unwrap(),
        &vec![json!(staged[&ids[2]])],
        "{part}"
    );
    // a campaign closing into decisions writes none on a sealed stack
    server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "gold",
            "question": {"kind": "axis", "axis": "base"},
            "source": {"selection": "sealed@1"},
            "raters_per_item": 1,
            "raters": ["rita@lab"],
            "adjudication": {"when": "never"},
            "closes_into": "decision",
        })),
        ADMIN,
    );
    let claimed = server.ok("POST", "/api/campaigns/gold/claim", Some(json!({})), RATER);
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    server.ok(
        "POST",
        &format!("/api/campaigns/gold/assignments/{assignment}/answer"),
        Some(json!({"value": "T1w"})),
        RATER,
    );
    let closed = server.ok("POST", "/api/campaigns/gold/close", None, ADMIN);
    assert_eq!(closed["left_out_sealed"].as_i64(), Some(1), "{closed}");
    assert!(
        closed["decisions"].as_array().unwrap().is_empty(),
        "{closed}"
    );

    // ------------------------------------------------ the keyboard
    // the commit by filter refuses a sealed stack, unless --unsealed-access
    // says why, which the audit keeps
    let stack3 = ids[3].to_string();
    let (done, _, stderr) = cli(
        &home,
        &["review", "commit", "--stacks", &stack3, "--anyway"],
    );
    assert!(!done, "the keyboard committed on a sealed stack");
    assert!(stderr.contains("sealed"), "{stderr}");
    let (done, stdout, stderr) = cli(
        &home,
        &[
            "review",
            "commit",
            "--stacks",
            &stack3,
            "--anyway",
            "--unsealed-access",
            "committing the reference after the certificate's reading",
        ],
    );
    assert!(done, "{stderr}");
    assert!(stdout.contains("committed 3 decision"), "{stdout}");
    let (done, _, stderr) = cli(&home, &["explain", &stack.to_string()]);
    assert!(!done, "the keyboard explained a sealed stack");
    assert!(stderr.contains("--unsealed-access"), "{stderr}");
    let (done, stdout, stderr) = cli(
        &home,
        &[
            "explain",
            &stack.to_string(),
            "--json",
            "--unsealed-access",
            "computing the certificate of the test sample",
        ],
    );
    assert!(done, "{stderr}");
    assert!(
        !serde_json::from_str::<Value>(&stdout).unwrap()["axes"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{stdout}"
    );
    let (done, _, stderr) = cli(
        &home,
        &["explain", &stack.to_string(), "--unsealed-access", " "],
    );
    assert!(!done, "an empty reason is no reason");
    assert!(stderr.contains("reason"), "{stderr}");
    let (done, stdout, stderr) = cli(&home, &["review", "list", "--json"]);
    assert!(done, "{stderr}");
    let mut named = BTreeSet::new();
    for i in serde_json::from_str::<Value>(&stdout).unwrap()["items"]
        .as_array()
        .unwrap()
    {
        if let Some(s) = i["ref"]["stack_id"].as_i64() {
            named.insert(s);
        }
    }
    assert!(named.is_disjoint(&sealed), "{stdout}");
    let doc = every_stack()["document"].clone();
    let file = out.path().join("every.json");
    std::fs::write(&file, doc.to_string()).unwrap();
    let (done, stdout, stderr) = cli(
        &home,
        &[
            "ask",
            "run",
            "--file",
            file.to_str().unwrap(),
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
    );
    assert!(done, "{stderr}");
    let answer: Value = serde_json::from_str(&stdout).unwrap();
    let mut named = BTreeSet::new();
    for r in answer["rows"].as_array().into_iter().flatten() {
        if let Some(k) = r[0].as_i64() {
            named.insert(k);
        }
    }
    assert!(named.is_disjoint(&sealed), "{stdout}");
    let (done, stdout, _) = cli(&home, &["classify", "votes"]);
    assert!(done);
    for line in stdout.lines().skip(1) {
        let s: i64 = line.split('\t').next().unwrap().parse().unwrap();
        assert!(!sealed.contains(&s), "{line}");
    }
    // every use of --unsealed-access is audited with its reason
    let audit = server.ok("GET", "/api/audit?action=sealed.read", None, ADMIN);
    let text = audit.to_string();
    assert!(
        text.contains("computing the certificate of the test sample"),
        "{text}"
    );
    assert!(
        text.contains("committing the reference after the certificate's reading"),
        "{text}"
    );
    // a server takes no --unsealed-access
    let (done, _, stderr) = cli(&home, &["serve", "--unsealed-access", "a server"]);
    assert!(!done && stderr.contains("keyboard"), "{stderr}");
}
