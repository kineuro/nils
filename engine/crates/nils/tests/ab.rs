// SPDX-License-Identifier: AGPL-3.0-only

//! Record 48, the reference read by judges: an A/B campaign. Two raters'
//! readings of eight stacks, one file per stack, make a campaign of the
//! stacks they split on and a seeded audit of those they agree on. The
//! person who settles an item reads its candidates as letters with a
//! reason, never which rater said what, nor whether the item is an audit;
//! a localizer asks its provenance and body part and nothing else; what
//! the answer chose and the cause given are kept; the voters are told once
//! the campaign is closed. The rules vote on a sealed stack only with
//! `--unsealed-access`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use nils_dicom::synth::{self, MetaFields, TempDir};
use serde_json::{Value, json};

const ISSUER: &str = "https://desk.example/";

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc")
}

fn run(home: &TempDir, args: &[&str]) -> (bool, String, String) {
    let mut cmd = nils();
    cmd.arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "cleo")
        .env("HOSTNAME", "lab")
        .env("NILS_PRINCIPAL", "cleo@desk.example")
        .env_remove("NILS_JOB_ID")
        .env_remove("NILS_JOB_DETAIL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if args.first() == Some(&"key") {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"an ab test key\n")
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(home: &TempDir, args: &[&str]) -> String {
    let (good, out, err) = run(home, args);
    assert!(good, "nils {args:?} failed: {err}");
    out
}

/// One FLAIR-looking file per study.
fn study(dir: &TempDir, n: usize) {
    let study = format!("1.2.3.{n}");
    let series = format!("{study}.1");
    let sop = format!("{series}.1");
    let mut e = synth::minimal_mr(&study, &series, &sop);
    for (tag, vr, v) in [
        (tags::PATIENT_ID, VR::LO, "P1"),
        (tags::SERIES_DESCRIPTION, VR::LO, "t2 flair tra"),
        (tags::SEQUENCE_NAME, VR::SH, "*tir2d1rr99"),
        (tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
        (tags::SCANNING_SEQUENCE, VR::CS, "SE\\IR"),
        (tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
        (tags::ECHO_TIME, VR::DS, "100"),
        (tags::REPETITION_TIME, VR::DS, "9000"),
        (tags::INVERSION_TIME, VR::DS, "2500"),
        (tags::MANUFACTURER, VR::LO, "SIEMENS"),
    ] {
        e.push(synth::text(tag, vr, v));
    }
    dir.file(
        &format!("{study}/{sop}"),
        &synth::part10(&MetaFields::mr(&sop), &e, true),
    );
}

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
        let trust = format!(
            "issuer={ISSUER},audience=nils,jwks={}",
            fixtures().join("jwks.json").display()
        );
        let mut child = nils()
            .arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--auth",
                "oidc",
                "--oidc-trust",
                &trust,
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .env("USER", "cleo")
            .env("HOSTNAME", "lab")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            let _ = child.kill();
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        Server { child, port }
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
        let split = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a header block");
        let head = String::from_utf8_lossy(&response[..split]).to_string();
        let status: u16 = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let doc = serde_json::from_slice(&response[split + 4..]).unwrap_or(Value::Null);
        (status, doc)
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

fn token(sub: &str, grants: &[&str], detail: &str) -> String {
    let key =
        EncodingKey::from_rsa_pem(&std::fs::read(fixtures().join("signing-key.pem")).unwrap())
            .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-2026".to_string());
    encode(
        &header,
        &json!({"iss": ISSUER, "aud": "nils", "sub": sub, "grants": grants, "detail": detail,
                "exp": now + 3600, "iat": now}),
        &key,
    )
    .unwrap()
}

fn selection_of(ids: &[i64]) -> Value {
    json!({"document": {
        "ast_version": 1,
        "params": {"ids": {"type": "list", "value": ids}},
        "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
        "out": {"set": "s", "level": "record"},
    }})
}

/// A rater's reading of a FLAIR, each axis with its reason.
fn reading(over: &[(&str, Value)]) -> Value {
    let mut a = json!({
        "provenance": "RawRecon", "technique": "TSE", "modifier": ["FLAIR"],
        "construct": [], "base": "T2w", "body_part": "brain",
        "post_contrast": "not_given",
    });
    for (k, v) in over {
        a[*k] = v.clone();
    }
    let mut out = json!({"unsure": false});
    for (k, v) in a.as_object().unwrap() {
        out[k] = json!({"reason": format!("SequenceName *tir2d1rr99 says so for {k}"), "value": v, "confidence": "high"});
    }
    out
}

/// Every key and every text anywhere in a document.
fn words_in(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                out.push(k.clone());
                words_in(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| words_in(x, out)),
        Value::String(s) => out.push(s.clone()),
        _ => {}
    }
}

fn blind(doc: &Value) {
    let mut w = Vec::new();
    words_in(doc, &mut w);
    for leak in [
        "judge-a",
        "judge-b",
        "rules",
        "voter",
        "voters",
        "sources",
        "kind",
        "audit",
        "split_by_axis_voters",
        "chosen_voters",
        "seed",
    ] {
        assert!(
            !w.iter().any(|x| x == leak),
            "{leak} is told while the campaign is open: {doc}"
        );
    }
}

#[test]
fn a_person_settles_what_the_voters_split_on_blind() {
    let home = TempDir::new("ab-home");
    let src = TempDir::new("ab-src");
    for n in 1..=8 {
        study(&src, n);
    }
    ok(&home, &["key", "add", "k"]);
    ok(&home, &["init", "--key", "k"]);
    ok(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            src.path().to_str().unwrap(),
        ],
    );
    ok(&home, &["fingerprint"]);
    ok(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
    );

    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        &["query:work", "data:see", "review:work", "campaigns:work"],
        "quasi",
    );
    let anna = token("anna", &["campaigns:see", "campaigns:work"], "quasi");
    let bo = token("bo", &["campaigns:see", "campaigns:work"], "quasi");
    let anna_p = "anna@desk.example";
    server.ok(
        "PUT",
        "/api/ask/selections/set",
        Some(selection_of(&[1, 2, 3, 4, 5, 6, 7, 8])),
        &cleo,
    );

    // ------------------------------------------------ the voters
    // judge-a writes the header judge's record; judge-b the answer alone.
    // They split on stack 2's technique, stack 3's base and stack 4's
    // modifier; stack 5 is a localizer both call so, split on body part;
    // stack 6 splits only where judge-b cannot tell (no split); stacks 1,
    // 6, 7 and 8 agree; judge-b never read stack 8.
    let a_dir = TempDir::new("ab-judge-a");
    let b_dir = TempDir::new("ab-judge-b");
    let loc = |bp: &str| {
        reading(&[
            ("provenance", json!("Localizer")),
            ("technique", json!("GRE")),
            ("modifier", json!([])),
            ("base", json!("none")),
            ("body_part", json!(bp)),
        ])
    };
    for s in 1..=8i64 {
        let (a, b) = match s {
            2 => (reading(&[]), reading(&[("technique", json!("IR-TSE"))])),
            3 => (reading(&[]), reading(&[("base", json!("PDw"))])),
            4 => (
                reading(&[]),
                reading(&[("modifier", json!(["FatSat", "FLAIR"]))]),
            ),
            5 => (loc("brain"), loc("brain-neck")),
            6 => (reading(&[]), reading(&[("base", json!("cant_tell"))])),
            _ => (reading(&[]), reading(&[])),
        };
        a_dir.file(
            &format!("{s}.json"),
            json!({"stack": s, "model": "judge-a", "answer": a})
                .to_string()
                .as_bytes(),
        );
        if s != 8 {
            b_dir.file(&format!("{s}.json"), b.to_string().as_bytes());
        }
    }
    let voters = [
        "--voter",
        &format!("judge-a={}", a_dir.path().display()),
        "--voter",
        &format!("judge-b={}", b_dir.path().display()),
    ]
    .map(|s| s.to_string());
    let base_args = |name: &str| -> Vec<String> {
        let mut v: Vec<String> = [
            "campaign",
            "ab",
            name,
            "--select",
            "set@1",
            "--seed",
            "written down on 2026-09-28",
            "--audit-share",
            "0.5",
            "--audit-cells",
            "0",
            "--rater",
            anna_p,
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        v.extend(voters.iter().cloned());
        v
    };
    let args = base_args("ab-1");
    let dry: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .chain(["--dry-run"])
        .collect();
    let d: Value = serde_json::from_str(&ok(&home, &dry)).unwrap();
    assert_eq!(d["counts"]["split"], 4, "{d}");
    assert_eq!(d["counts"]["agree"], 4, "{d}");
    assert_eq!(d["counts"]["audit"], 2, "half of four: {d}");
    assert_eq!(d["counts"]["localizers"], 1, "{d}");
    // the judge's none is an axes answer's null: every value read
    for v in ["judge-a", "judge-b"] {
        assert_eq!(d["voters"][v]["refused"], json!({}), "{d}");
    }
    // the same seed, the same draw
    let d2: Value = serde_json::from_str(&ok(&home, &dry)).unwrap();
    assert_eq!(d["counts"], d2["counts"]);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let made: Value = serde_json::from_str(&ok(&home, &argv)).unwrap();
    let id = made["id"].as_i64().unwrap_or_else(|| panic!("{made}"));
    assert_eq!(made["items"].as_array().unwrap().len(), 6, "{made}");
    assert!(made["source"]["ab"]["seed_sha256"].is_string(), "{made}");
    assert!(made["source"]["ab"].get("seed").is_none(), "{made}");
    // the candidates are the only answers shown: nothing is suggested
    assert_eq!(made["suggest"], "none", "{made}");

    // ------------------------------------------------ blind while open
    let seen = server.ok("GET", &format!("/api/campaigns/{id}"), None, &anna);
    let items_json = seen["items"].to_string();
    assert!(
        !items_json.contains("audit") && !items_json.contains("split"),
        "{items_json}"
    );
    let (st, _) = server.call("GET", &format!("/api/campaigns/{id}/batches"), None, &anna);
    assert_eq!(st, 409);
    let (st, _) = server.call(
        "POST",
        &format!("/api/campaigns/{id}/suggestions"),
        Some(json!({"tsv": "stack\tvalue\n1\tT2w\n", "author": "x"})),
        &cleo,
    );
    assert_eq!(st, 409);

    let mut answered = 0;
    let mut split_answer = None;
    let mut localizer_seen = false;
    loop {
        let cl = server.ok(
            "POST",
            &format!("/api/campaigns/{id}/claim"),
            Some(json!({})),
            &anna,
        );
        if cl["item"].is_null() {
            break;
        }
        let item = cl["item"]["id"].as_i64().unwrap();
        let a = cl["assignment"]["id"].as_i64().unwrap();
        // bo holds nothing here
        let (st, _) = server.call(
            "GET",
            &format!("/api/campaigns/{id}/items/{item}/ab"),
            None,
            &bo,
        );
        assert!(st == 403 || st == 404, "{st}");
        let sheet = server.ok(
            "GET",
            &format!("/api/campaigns/{id}/items/{item}/ab"),
            None,
            &anna,
        );
        blind(&sheet);
        let why = server.ok(
            "GET",
            &format!("/api/campaigns/{id}/items/{item}/why"),
            None,
            &anna,
        );
        assert_eq!(why["blind"], true, "{why}");
        assert!(why["suggested"].is_null(), "{why}");
        let rows = sheet["axes"].as_array().unwrap();
        assert_eq!(rows.len(), 7, "{sheet}");
        let mut value = serde_json::Map::new();
        let mut split_axes = Vec::new();
        for r in rows {
            let axis = r["axis"].as_str().unwrap();
            if r["asked"] == false {
                value.insert(axis.into(), json!("not_asked"));
                continue;
            }
            let cands = r["candidates"].as_array().unwrap();
            if r["split"] == true {
                split_axes.push(axis.to_string());
                assert!(cands.len() >= 2, "{r}");
                let mut labels: Vec<&str> =
                    cands.iter().map(|c| c["label"].as_str().unwrap()).collect();
                labels.sort();
                assert_eq!(labels[..2], ["A", "B"]);
                // every candidate carries one reason, never who gave it
                assert!(cands.iter().all(|c| c["reason"].is_string()), "{r}");
            } else {
                assert_eq!(cands.len(), 1, "{r}");
            }
            // settle on the FLAIR reading where it is a candidate
            let want = match axis {
                "technique" => json!("TSE"),
                "base" => json!("T2w"),
                "modifier" => json!(["FLAIR"]),
                "body_part" => json!("brain"),
                _ => cands[0]["value"].clone(),
            };
            value.insert(axis.into(), want);
        }
        if sheet["localizer"] == true {
            localizer_seen = true;
            assert_eq!(split_axes, vec!["body_part".to_string()], "{sheet}");
            // not asked only on what a localizer is not asked
            let mut bad = value.clone();
            bad.insert("body_part".into(), json!("not_asked"));
            let (st, doc) = server.call(
                "POST",
                &format!("/api/campaigns/{id}/assignments/{a}/answer"),
                Some(json!({"value": Value::Object(bad)})),
                &anna,
            );
            assert_eq!(st, 400, "{doc}");
        } else {
            // not asked on a stack that is no localizer is refused
            let mut bad = value.clone();
            bad.insert("construct".into(), json!("not_asked"));
            let (st, doc) = server.call(
                "POST",
                &format!("/api/campaigns/{id}/assignments/{a}/answer"),
                Some(json!({"value": Value::Object(bad)})),
                &anna,
            );
            assert_eq!(st, 400, "{doc}");
        }
        let done = server.ok(
            "POST",
            &format!("/api/campaigns/{id}/assignments/{a}/answer"),
            Some(json!({"value": Value::Object(value)})),
            &anna,
        );
        answered += 1;
        let answer = done["answer"].as_i64().unwrap();
        if let Some(axis) = split_axes.first()
            && split_answer.is_none()
        {
            split_answer = Some((answer, axis.clone()));
        }
    }
    assert_eq!(answered, 6);
    assert!(localizer_seen);

    // what the answers chose, as the engine read it
    let all = server.ok("GET", &format!("/api/campaigns/{id}/answers"), None, &anna);
    let mut chose: BTreeMap<String, usize> = BTreeMap::new();
    for a in all["answers"].as_array().unwrap() {
        for (_, c) in a["choices"].as_object().unwrap_or_else(|| panic!("{a}")) {
            *chose.entry(c.as_str().unwrap().to_string()).or_default() += 1;
        }
    }
    assert!(chose["confirm"] > 0, "{chose:?}");
    assert!(
        chose.get("not_asked").copied().unwrap_or(0) == 5,
        "{chose:?}"
    );
    assert!(chose.keys().any(|k| k == "A" || k == "B"), "{chose:?}");

    // ------------------------------------------------ causes
    let (answer, axis) = split_answer.unwrap();
    server.ok(
        "POST",
        &format!("/api/campaigns/{id}/answers/{answer}/cause"),
        Some(json!({"axis": axis, "cause": "convention_gap"})),
        &anna,
    );
    let (st, _) = server.call(
        "POST",
        &format!("/api/campaigns/{id}/answers/{answer}/cause"),
        Some(json!({"axis": axis, "cause": "bad luck"})),
        &anna,
    );
    assert_eq!(st, 400);
    let (st, _) = server.call(
        "POST",
        &format!("/api/campaigns/{id}/answers/{answer}/cause"),
        Some(json!({"axis": axis, "cause": "rule_bug"})),
        &cleo,
    );
    assert_eq!(st, 403, "a cause is given by who answered");

    let summary = server.ok("GET", &format!("/api/campaigns/{id}/ab"), None, &anna);
    assert_eq!(summary["answered"], 6, "{summary}");
    assert_eq!(summary["sources"], false);
    assert_eq!(summary["causes"][&axis]["convention_gap"], 1, "{summary}");
    assert!(summary["seconds"]["median"].is_number(), "{summary}");
    blind(&json!({"x": summary["split_choices"], "y": summary["choices"]}));
    let rows = server.ok(
        "GET",
        &format!("/api/campaigns/{id}/ab/decisions"),
        None,
        &anna,
    );
    assert_eq!(rows["sources"], false);
    assert_eq!(rows["count"], 6 * 7);
    blind(&rows["decisions"]);
    // another reader of the campaign reads no one's decisions but their
    // own while it is open
    let (st, other) = server.call(
        "GET",
        &format!("/api/campaigns/{id}/ab/decisions"),
        None,
        &bo,
    );
    assert!(
        st == 404 || (st == 200 && other["count"] == 0 && other["blind"] == true),
        "{st} {other}"
    );

    // ------------------------------------------------ told once closed
    server.ok(
        "POST",
        &format!("/api/campaigns/{id}/close"),
        Some(json!({})),
        &cleo,
    );
    let rows = server.ok(
        "GET",
        &format!("/api/campaigns/{id}/ab/decisions"),
        None,
        &cleo,
    );
    assert_eq!(rows["sources"], true, "{rows}");
    let kinds: Vec<&str> = rows["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["kind"].as_str())
        .collect();
    assert!(
        kinds.contains(&"audit") && kinds.contains(&"split"),
        "{kinds:?}"
    );
    let summary = server.ok("GET", &format!("/api/campaigns/{id}/ab"), None, &cleo);
    assert_eq!(summary["audit"]["items"], 2, "{summary}");
    assert_eq!(summary["audit"]["changed"], 0, "{summary}");
    let exported: Value =
        serde_json::from_str(&ok(&home, &["campaign", "ab-export", &id.to_string()])).unwrap();
    assert_eq!(exported["sources"], true, "{exported}");
    assert_eq!(exported["decisions"].as_array().unwrap().len(), 42);

    // ------------------------------------------------ the rules, sealed
    let handle = made["handle_id"].as_i64().unwrap();
    ok(
        &home,
        &["labels", "seal", "--handle", &handle.to_string(), "--json"],
    );
    let mut with_rules: Vec<String> = base_args("ab-2");
    with_rules.extend(["--rules".to_string(), "--dry-run".to_string()]);
    let argv: Vec<&str> = with_rules.iter().map(String::as_str).collect();
    let (good, _, err) = run(&home, &argv);
    assert!(
        !good,
        "the rules of a sealed stack are read only with --unsealed-access"
    );
    assert!(err.contains("--unsealed-access"), "{err}");
    let mut argv = argv.clone();
    argv.extend(["--unsealed-access", "the A/B test's rules voter"]);
    let d: Value = serde_json::from_str(&ok(&home, &argv)).unwrap();
    assert_eq!(d["voters"]["rules"]["files"], 8, "{d}");
    // a closed campaign's voters on a sealed stack: only sealed:see
    let rows = server.ok(
        "GET",
        &format!("/api/campaigns/{id}/ab/decisions"),
        None,
        &cleo,
    );
    assert_eq!(rows["sources"], false, "{rows}");
}
