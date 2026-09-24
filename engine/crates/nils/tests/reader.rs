// SPDX-License-Identifier: AGPL-3.0-only

//! Record 48 through the door alone: the reader's evidence line, a batch of
//! like stacks accepted in one move with a share held back, claims in order
//! of value, the time and the suggestion kept on each answer and a
//! campaign's speed, a sealed sample that no batch takes, and the
//! certificate that unseals it so its labels join the development labels.

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

/// A registry of four stacks of two people, classified, made by the
/// command line.
fn registry() -> TempDir {
    let home = TempDir::new("campaign-home");
    let dir = TempDir::new("campaign-src");
    for (patient, study, sop, description) in [
        ("P1", "1.2.3.A", "1.2.3.A.1.1", "t1 mprage"),
        ("P1", "1.2.3.B", "1.2.3.B.1.1", "flair axial"),
        ("P2", "1.2.3.C", "1.2.3.C.1.1", "t1 mprage"),
        ("P2", "1.2.3.D", "1.2.3.D.1.1", "t2 spine"),
    ] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
        // the FLAIR's physics, which its deciding clause reads
        if description == "flair axial" {
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
        let mut cmd = nils();
        cmd.arg("--registry")
            .arg(home.path())
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        if let Some(text) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["key", "add", "k"], Some("a campaign test key\n"));
    run(&["init", "--key", "k"], None);
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

const CURATOR: &str = "curator-token-of-length";
const ANNA: &str = "anna-rater-token-of-length";
const BO: &str = "bo-rater-token-of-length";
const RITA: &str = "rita-reader-token-of-length";

impl Server {
    fn start(home: &TempDir) -> Server {
        let tokens = [
            // a reviewer (detail quasi) who runs campaigns, declares places
            // and certifies models
            format!("{CURATOR}=cleo@lab:reviewer,campaigns:work,places:work,models:work,audit:see"),
            // raters hold the campaign's grant alone, at detail plain
            format!("{ANNA}=anna@lab:campaigns:work"),
            format!("{BO}=bo@lab:campaigns:work"),
            // a reader of the queue at detail plain
            format!("{RITA}=rita@lab:review:see"),
        ]
        .join(",");
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
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
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

/// Run the command line in the registry as a principal: its status, stdout
/// and stderr.
fn cli(home: &TempDir, who: &str, args: &[&str]) -> (bool, String, String) {
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "cleo")
        .env("HOSTNAME", "lab")
        .env("NILS_PRINCIPAL", who)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Every key named `matched` or `why`, anywhere in a document.
fn words_in(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "matched" || k == "why" {
                    out.push(k.clone());
                }
                out.extend(words_in(x));
            }
        }
        Value::Array(a) => a.iter().for_each(|x| out.extend(words_in(x))),
        _ => {}
    }
    out
}

#[test]
fn the_reader_reads_batches_orders_and_times_and_a_certificate_unseals() {
    let home = registry();
    let out = TempDir::new("reader-export");
    let server = Server::start(&home);
    server.ok(
        "POST",
        "/api/places",
        Some(json!({"name": "labels-out", "role": "export", "path": out.path().to_str().unwrap()})),
        CURATOR,
    );
    server.ok(
        "PUT",
        "/api/ask/selections/every-stack",
        Some(json!({"document": {
            "ast_version": 1,
            "sets": {"every": {"grain": "stack"}},
            "out": {"set": "every", "level": "record"},
        }})),
        CURATOR,
    );
    let made = server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "bases",
            "question": {"kind": "axis", "axis": "base"},
            "source": {"selection": "every-stack@1"},
            "raters_per_item": 1,
            "adjudication": {"when": "never"},
            "closes_into": "stage",
        })),
        CURATOR,
    );
    let handle = made["handle_id"].as_i64().unwrap();
    let items: Vec<Value> = made["items"].as_array().unwrap().clone();
    assert!(items.len() >= 4, "{made}");
    let stack = items[0]["stack_id"].as_i64().unwrap();

    // ------------------------------------------------ the evidence line
    let quasi = server.ok("GET", &format!("/api/stacks/{stack}/why"), None, CURATOR);
    assert_eq!(quasi["detail"], "quasi", "{quasi}");
    let base = quasi["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == "base")
        .unwrap_or_else(|| panic!("no base axis: {quasi}"))
        .clone();
    assert!(
        base["line"].as_str().unwrap().starts_with("base: "),
        "{base}"
    );
    assert!(base["set_by"]["kind"] == "rule", "{base}");
    assert!(base["decided"]["rule_set"].is_string(), "{base}");
    assert!(base["voted"].is_array(), "{base}");
    assert!(quasi["header"].is_object(), "{quasi}");
    // at detail plain, nothing a stack carried as words
    let plain = server.ok("GET", &format!("/api/stacks/{stack}/why"), None, RITA);
    assert_eq!(plain["detail"], "plain", "{plain}");
    assert!(words_in(&plain).is_empty(), "{plain}");
    // a rater reads it through the campaign's item, never the whole archive
    let (status, _) = server.call("GET", &format!("/api/stacks/{stack}/why"), None, ANNA);
    assert_eq!(status, 403);
    let item0 = items[0]["id"].as_i64().unwrap();
    let why = server.ok(
        "GET",
        &format!("/api/campaigns/bases/items/{item0}/why"),
        None,
        ANNA,
    );
    assert!(words_in(&why).is_empty(), "{why}");
    assert_eq!(why["item"], item0, "{why}");
    assert!(why["worth"]["confidence"].is_number(), "{why}");
    assert_eq!(why["suggested"], base["value"], "{why}");
    // the FLAIR's base was decided by its echo time, which the line shows
    // at every detail, since it is a number of the header
    let flair = items
        .iter()
        .filter_map(|i| i["stack_id"].as_i64())
        .map(|s| server.ok("GET", &format!("/api/stacks/{s}/why"), None, RITA))
        .find(|d| d["header"]["echo_time"] == 100.0)
        .unwrap_or_else(|| panic!("no stack with its echo time: {items:?}"));
    assert_eq!(flair["header"]["repetition_time"], 9000.0, "{flair}");
    let base = flair["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == "base")
        .unwrap()
        .clone();
    assert_eq!(base["decided"]["rule"], "flair:long_te", "{base}");
    assert_eq!(base["decided"]["header"]["echo_time"], 100.0, "{base}");
    assert!(
        base["decided"]["reads"]["fields"]
            .as_array()
            .unwrap()
            .contains(&json!("echo_time")),
        "{base}"
    );
    assert!(base["line"].as_str().unwrap().contains("TE 100"), "{base}");
    let (status, _) = server.call("GET", "/api/stacks/999999/why", None, CURATOR);
    assert_eq!(status, 404);

    // ------------------------------------------------ batches
    let listed = server.ok("GET", "/api/campaigns/bases/batches", None, ANNA);
    let groups = listed["groups"].as_array().unwrap().clone();
    let grouped: i64 = groups.iter().map(|g| g["count"].as_i64().unwrap()).sum();
    assert_eq!(
        grouped + listed["unsuggested"].as_i64().unwrap() + listed["sealed"].as_i64().unwrap(),
        listed["open"].as_i64().unwrap(),
        "{listed}"
    );
    // the two t1 mprage stacks look the same and are suggested the same
    let pair = groups
        .iter()
        .find(|g| g["count"] == 2)
        .unwrap_or_else(|| panic!("no batch of two: {listed}"))
        .clone();
    assert!(pair["signature"]["rules"].is_object(), "{pair}");
    assert!(pair["signature"]["header"].is_object(), "{pair}");
    let key = pair["key"].as_str().unwrap().to_string();
    // a share outside 0 to 1 is refused
    let (status, _) = server.call(
        "POST",
        &format!("/api/campaigns/bases/batches/{key}/accept"),
        Some(json!({"hold_back": 2})),
        ANNA,
    );
    assert_eq!(status, 400);
    let accepted = server.ok(
        "POST",
        &format!("/api/campaigns/bases/batches/{key}/accept"),
        Some(json!({"hold_back": 0.5, "seed": "fixed"})),
        ANNA,
    );
    assert_eq!(
        accepted["accepted"].as_array().unwrap().len(),
        1,
        "{accepted}"
    );
    assert_eq!(
        accepted["held_back"].as_array().unwrap().len(),
        1,
        "{accepted}"
    );
    let held = accepted["held_back"][0].as_i64().unwrap();
    // the held item is read alone: no batch takes it now
    let again = server.ok("GET", "/api/campaigns/bases/batches", None, BO);
    assert!(
        again["groups"]
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["key"] != key.as_str()),
        "{again}"
    );
    let (status, _) = server.call(
        "POST",
        &format!("/api/campaigns/bases/batches/{key}/accept"),
        Some(json!({})),
        BO,
    );
    assert_eq!(status, 409);

    // ------------------------------------------------ order and time
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/bases/claim",
        Some(json!({"order": "sideways"})),
        BO,
    );
    assert_eq!(status, 400);
    let claimed = server.ok(
        "POST",
        "/api/campaigns/bases/claim?order=value",
        Some(json!({})),
        BO,
    );
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    let it = claimed["item"]["id"].as_i64().unwrap();
    let why = server.ok(
        "GET",
        &format!("/api/campaigns/bases/items/{it}/why"),
        None,
        BO,
    );
    let value = why["suggested"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| "T1w".to_string());
    server.ok(
        "POST",
        &format!("/api/campaigns/bases/assignments/{assignment}/answer"),
        Some(json!({"value": value})),
        BO,
    );
    let answers = server.ok("GET", "/api/campaigns/bases/answers", None, CURATOR);
    let list = answers["answers"].as_array().unwrap();
    let anna = list.iter().find(|a| a["principal"] == "anna@lab").unwrap();
    assert_eq!(anna["via"], "batch", "{anna}");
    assert!(anna["seconds"].is_null(), "{anna}");
    let bo = list.iter().find(|a| a["principal"] == "bo@lab").unwrap();
    assert_eq!(bo["via"], "claim", "{bo}");
    assert!(bo["seconds"].as_f64().is_some(), "{bo}");
    if why["suggested"].is_string() {
        assert_eq!(bo["changed"], false, "{bo}");
    }
    // the speed, per rater; a rater reads their own row
    let stats = server.ok("GET", "/api/campaigns/bases/stats", None, CURATOR);
    assert_eq!(stats["raters"].as_array().unwrap().len(), 2, "{stats}");
    assert_eq!(stats["all"]["batched"], 1, "{stats}");
    let mine = server.ok("GET", "/api/campaigns/bases/stats", None, ANNA);
    assert_eq!(mine["blind"], true, "{mine}");
    assert_eq!(mine["raters"].as_array().unwrap().len(), 1, "{mine}");
    assert_eq!(mine["raters"][0]["principal"], "anna@lab", "{mine}");

    // ------------------------------------------------ sealed, certified
    let (ok, _, err) = cli(
        &home,
        "op@lab",
        &["labels", "seal", "--handle", &handle.to_string()],
    );
    assert!(ok, "{err}");
    let sealed = server.ok("GET", "/api/campaigns/bases/batches", None, BO);
    assert!(sealed["groups"].as_array().unwrap().is_empty(), "{sealed}");
    assert_eq!(sealed["sealed"], sealed["open"], "{sealed}");
    // an item of a sealed sample is never accepted in one move
    let (status, doc) = server.call(
        "POST",
        &format!("/api/campaigns/bases/batches/{key}/accept"),
        Some(json!({"items": [held]})),
        BO,
    );
    assert_eq!(status, 409, "{doc}");
    // its labels are not development labels until a certificate unseals it
    let training = server.ok(
        "POST",
        "/api/label-sets",
        Some(json!({"for_training": true, "authors": ["person", "agent", "model"]})),
        CURATOR,
    );
    assert_eq!(training["sealed"], false, "{training}");
    let sample = format!("handle:{handle}");
    let (ok, _, _) = cli(
        &home,
        "op@lab",
        &["labels", "unseal", &sample, "--certificate", "999"],
    );
    assert!(!ok, "no certificate, no unseal");
    let digest = format!("sha256:{}", "e".repeat(64));
    let model = server.ok(
        "POST",
        "/api/models",
        Some(json!({"name": "an-encoder", "version": "1", "kind": "encoder", "digest": digest, "task": "encoder"})),
        CURATOR,
    );
    let (status, _) = server.call(
        "POST",
        "/api/certificates",
        Some(json!({"sample": "handle:424242", "models": [model["id"]], "result": {"coverage": 0.9}})),
        CURATOR,
    );
    assert_eq!(status, 404);
    let (status, _) = server.call(
        "POST",
        "/api/certificates",
        Some(json!({"sample": sample, "models": [model["id"]], "result": {"coverage": 0.9}})),
        ANNA,
    );
    assert_eq!(status, 403);
    let cert = server.ok(
        "POST",
        "/api/certificates",
        Some(json!({"sample": sample, "models": [digest], "result": {"coverage": 0.9}})),
        CURATOR,
    );
    let listed = server.ok("GET", "/api/certificates", None, CURATOR);
    assert_eq!(listed["count"], 1, "{listed}");
    let done = server.ok(
        "POST",
        &format!("/api/certificates/{}/unseal", cert["id"]),
        None,
        CURATOR,
    );
    assert!(done["stacks"].as_i64().unwrap() >= 4, "{done}");
    let open = server.ok("GET", "/api/campaigns/bases/batches", None, BO);
    assert_eq!(open["sealed"], 0, "{open}");
    // the development labels at the keyboard: every person's decision in
    // force, nothing sealed now
    let (ok, stdout, err) = cli(
        &home,
        "op@lab",
        &[
            "labels",
            "export",
            "--for-training",
            "--to",
            out.path().join("training").to_str().unwrap(),
            "--json",
        ],
    );
    assert!(ok, "{err}");
    assert!(stdout.contains("\"sealed\": false"), "{stdout}");
    let (ok, stdout, err) = cli(&home, "op@lab", &["labels", "certificates"]);
    assert!(ok, "{err}");
    assert!(stdout.contains(&sample), "{stdout}");
    let audit = server.ok("GET", "/api/audit?action=labels.unseal", None, CURATOR);
    assert!(audit.to_string().contains("labels.unseal"), "{audit}");
}
