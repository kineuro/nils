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
const OTTO: &str = "otto-certifier-token-of-length";
/// Record 48, D1 of the move: the token a certificate's computation runs
/// under, which alone reads what a system said of a sealed stack.
const SEALED: &str = "sealed-reading-token-of-length";

impl Server {
    fn start(home: &TempDir) -> Server {
        let tokens = [
            // a reviewer (detail quasi) who runs campaigns, declares places
            // and certifies models
            format!("{CURATOR}=cleo@lab:reviewer,campaigns:work,places:work,models:work,audit:see"),
            // raters hold the campaign's grant alone, at detail plain
            format!("{ANNA}=anna@lab:campaigns:work"),
            // bo rates and reads the queue too
            format!("{BO}=bo@lab:campaigns:work,review:see"),
            // a reader of the queue at detail plain
            format!("{RITA}=rita@lab:review:see"),
            // a second person who works the model registry
            format!("{OTTO}=otto@lab:models:work"),
            // the certificate's computation: a reviewer with the one grant
            // that reads sealed stacks
            format!("{SEALED}=certify@lab:reviewer,sealed:see"),
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
        self.call_with(method, path, body, token, "")
    }

    /// A call with headers of its own, each line ending in CRLF.
    fn call_with(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        token: &str,
        extra: &str,
    ) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n{extra}",
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
            "raters": ["anna@lab", "bo@lab"],
            "adjudication": {"when": "never"},
            "closes_into": "stage",
            "hold_back": 0.5,
            "suggest": "rules",
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
    // header or name, from the tier the verdict recorded
    let basis = nils_pack::basis_of(base["tier"].as_str().unwrap());
    assert_eq!(base["basis"], basis, "{base}");
    assert_eq!(
        base["decided"]["basis"],
        nils_pack::basis_of(base["decided"]["tier"].as_str().unwrap()),
        "{base}"
    );
    assert!(
        base["line"]
            .as_str()
            .unwrap()
            .contains(&format!("({})", base["decided"]["basis"].as_str().unwrap())),
        "{base}"
    );
    assert!(base["voted"].is_array(), "{base}");
    assert!(quasi["header"].is_object(), "{quasi}");
    // at detail plain, nothing a stack carried as words
    let plain = server.ok("GET", &format!("/api/stacks/{stack}/why"), None, RITA);
    assert_eq!(plain["detail"], "plain", "{plain}");
    assert!(words_in(&plain).is_empty(), "{plain}");
    // and the explanation keeps no matched words below quasi either
    let explained = server.ok("GET", &format!("/api/explain/{stack}"), None, RITA);
    assert!(
        !explained.to_string().contains("\"matched\""),
        "{explained}"
    );
    assert_eq!(explained["blind"], false, "{explained}");
    // the sequence name is quasi-identifying text: never below quasi, never
    // in a batch's signature
    let seq = "*tir2d1rr99";
    for s in items.iter().filter_map(|i| i["stack_id"].as_i64()) {
        let p = server.ok("GET", &format!("/api/stacks/{s}/why"), None, RITA);
        assert!(!p.to_string().contains(seq), "{p}");
        assert!(!p.to_string().contains("\"text_sequence_name\":"), "{p}");
        let q = server.ok("GET", &format!("/api/stacks/{s}/why"), None, CURATOR);
        if q["header"]["echo_time"] == 100.0 {
            assert_eq!(q["header"]["text_sequence_name"], seq, "{q}");
        }
    }
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
    let every = server.ok("GET", "/api/campaigns/bases/batches", None, CURATOR);
    assert!(!every.to_string().contains("*tir2d1rr99"), "{every}");
    // what is held back is the campaign's: a caller's share or seed is
    // refused, and a maker's share below a tenth too
    for body in [json!({"hold_back": 0}), json!({"seed": "fixed"})] {
        let (status, _) = server.call(
            "POST",
            &format!("/api/campaigns/bases/batches/{key}/accept"),
            Some(body),
            ANNA,
        );
        assert_eq!(status, 400);
    }
    let (status, _) = server.call(
        "POST",
        "/api/campaigns",
        Some(
            json!({"name": "loose", "question": {"kind": "axis", "axis": "base"},
            "source": {"selection": "every-stack@1"}, "hold_back": 0.01}),
        ),
        CURATOR,
    );
    assert_eq!(status, 400);
    // naming one item of the batch still holds back the share of the whole
    let one = pair["sample"][0].as_i64().unwrap();
    let accepted = server.ok(
        "POST",
        &format!("/api/campaigns/bases/batches/{key}/accept"),
        Some(json!({"items": [one]})),
        ANNA,
    );
    assert_eq!(
        accepted["held_back"].as_array().unwrap().len(),
        1,
        "{accepted}"
    );
    let named_held = accepted["held_back"][0] == one;
    assert_eq!(
        accepted["accepted"].as_array().unwrap().len(),
        usize::from(!named_held),
        "{accepted}"
    );
    let held = accepted["held_back"][0].as_i64().unwrap();
    // with the named item held back, the rest of the batch is accepted
    // whole; a batch of one holds none back
    if named_held {
        let rest = server.ok(
            "POST",
            &format!("/api/campaigns/bases/batches/{key}/accept"),
            Some(json!({})),
            ANNA,
        );
        assert_eq!(rest["accepted"].as_array().unwrap().len(), 1, "{rest}");
        assert!(rest["held_back"].as_array().unwrap().is_empty(), "{rest}");
    }
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
    assert!(mine.get("all").is_none(), "{mine}");
    assert_eq!(mine["raters"].as_array().unwrap().len(), 1, "{mine}");
    assert_eq!(mine["raters"][0]["principal"], "anna@lab", "{mine}");

    // ------------------------------------------------ sealed, certified
    let (ok, sealed_out, err) = cli(
        &home,
        "op@lab",
        &["labels", "seal", "--handle", &handle.to_string(), "--json"],
    );
    assert!(ok, "{err}");
    let sample_digest = serde_json::from_str::<Value>(&sealed_out).unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_string();
    // a sealed item is read blind: no suggestion, nothing of System 1's
    let sealed_why = server.ok(
        "GET",
        &format!("/api/campaigns/bases/items/{item0}/why"),
        None,
        BO,
    );
    assert_eq!(sealed_why["blind"], true, "{sealed_why}");
    assert!(sealed_why["suggested"].is_null(), "{sealed_why}");
    assert!(sealed_why["asked"].is_null(), "{sealed_why}");
    // truly blind while the campaign is open: the pictures and the raw
    // header, and nothing any system said of the stack
    for key in ["axes", "line", "set_by", "voted", "decided"] {
        assert!(sealed_why.get(key).is_none(), "{key}: {sealed_why}");
    }
    assert!(
        sealed_why["pictures"]["instances"].is_string(),
        "{sealed_why}"
    );
    assert!(sealed_why["header"].is_object(), "{sealed_why}");
    let seen = |token: &str| {
        let why = server.ok("GET", &format!("/api/stacks/{stack}/why"), None, token);
        let explained = server.ok("GET", &format!("/api/explain/{stack}"), None, token);
        let review = items[0]["review_item_id"].as_i64().unwrap();
        let item = server.call("GET", &format!("/api/review/{review}"), None, token);
        let list = server.ok("GET", "/api/review?status=open", None, token);
        let listed = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == review)
            .cloned()
            .unwrap_or(Value::Null);
        (why, explained, item, listed)
    };
    // record 48, D1 of the move: sealed means sealed on every door, for a
    // rater of the campaign, a reader, and a holder of review:work who rates
    // in no campaign alike; the review item is not there
    for token in [BO, RITA, CURATOR] {
        let (why, explained, (status, item), listed) = seen(token);
        assert_eq!(why["blind"], true, "{why}");
        assert!(why.get("axes").is_none(), "{why}");
        assert_eq!(explained["blind"], true, "{explained}");
        assert!(
            explained["axes"].as_array().unwrap().is_empty(),
            "{explained}"
        );
        assert_eq!(status, 404, "{item}");
        assert!(listed.is_null(), "{listed}");
    }
    // the certificate's grant alone sees it
    let (why, explained, (status, item), _) = seen(SEALED);
    assert_eq!(why["blind"], false, "{why}");
    assert!(!why["axes"].as_array().unwrap().is_empty(), "{why}");
    assert_eq!(explained["blind"], false, "{explained}");
    assert_eq!(status, 200, "{item}");
    assert!(item.get("blind").is_none(), "{item}");
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
    // a set written while its sample is sealed trains nothing, and says so
    // at the list door and the set's own door alike
    let drawn = server.ok(
        "POST",
        "/api/campaigns/bases/export",
        Some(json!({"of": "answers", "name": "drawn"})),
        CURATOR,
    );
    assert_eq!(drawn["sealed"], true, "{drawn}");
    let training_of = |server: &Server| {
        let one = server.ok(
            "GET",
            &format!("/api/label-sets/{}", drawn["id"]),
            None,
            CURATOR,
        );
        let list = server.ok("GET", "/api/label-sets", None, CURATOR);
        let listed = list["label_sets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == drawn["id"])
            .unwrap()
            .clone();
        assert_eq!(one["training"], listed["training"], "{one} {listed}");
        listed["training"].as_str().unwrap().to_string()
    };
    assert!(training_of(&server).starts_with("refused"));
    let sample = format!("handle:{handle}");
    // the keyboard neither records a certificate nor unseals: both are a
    // person's acts at the door, by verified identities
    let (ok, _, err) = cli(
        &home,
        "op@lab",
        &["labels", "unseal", &sample, "--certificate", "1"],
    );
    assert!(!ok && err.contains("/api/certificates"), "{err}");
    let (ok, _, err) = cli(
        &home,
        "op@lab",
        &[
            "labels",
            "certificate",
            "--sample",
            &sample,
            "--model",
            "1",
            "--result",
            "r.json",
        ],
    );
    assert!(!ok && err.contains("/api/certificates"), "{err}");
    let digest = format!("sha256:{}", "e".repeat(64));
    let model = server.ok(
        "POST",
        "/api/models",
        Some(json!({"name": "an-encoder", "version": "1", "kind": "encoder", "digest": digest, "task": "encoder"})),
        CURATOR,
    );
    let n = items.len();
    let result = json!({"sample": sample, "sample_digest": sample_digest, "risk": 0.05, "n": n, "errors": 1});
    let (status, _) = server.call(
        "POST",
        "/api/certificates",
        Some(json!({"sample": "handle:424242", "models": [model["id"]], "result": result})),
        CURATOR,
    );
    assert_eq!(status, 404);
    let (status, _) = server.call(
        "POST",
        "/api/certificates",
        Some(json!({"sample": sample, "models": [model["id"]], "result": result})),
        ANNA,
    );
    assert_eq!(status, 403);
    // an empty result is no certificate
    let (status, _) = server.call(
        "POST",
        "/api/certificates",
        Some(json!({"sample": sample, "models": [model["id"]], "result": {}})),
        CURATOR,
    );
    assert_eq!(status, 400);
    // an agent's token is refused, whatever it holds
    let (status, doc) = server.call_with(
        "POST",
        "/api/certificates",
        Some(json!({"sample": sample, "models": [digest], "result": result})),
        CURATOR,
        "X-Nils-Actor: {\"kind\": \"agent\", \"name\": \"certifier\"}\r\n",
    );
    assert_eq!(status, 403, "{doc}");
    let cert = server.ok(
        "POST",
        "/api/certificates",
        Some(json!({"sample": sample, "models": [digest], "result": result})),
        CURATOR,
    );
    // who recorded it does not unseal it
    let (status, _) = server.call(
        "POST",
        &format!("/api/certificates/{}/unseal", cert["id"]),
        None,
        CURATOR,
    );
    assert_eq!(status, 409);
    let listed = server.ok("GET", "/api/certificates", None, CURATOR);
    assert_eq!(listed["count"], 1, "{listed}");
    let done = server.ok(
        "POST",
        &format!("/api/certificates/{}/unseal", cert["id"]),
        None,
        OTTO,
    );
    assert!(done["stacks"].as_i64().unwrap() >= 4, "{done}");
    let open = server.ok("GET", "/api/campaigns/bases/batches", None, BO);
    assert_eq!(open["sealed"], 0, "{open}");
    // unsealed, it is read as usual again
    let (why, ..) = seen(RITA);
    assert_eq!(why["blind"], false, "{why}");
    assert!(training_of(&server).starts_with("allowed"));
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

/// Record 50 through the door alone: suggestions from outside brought into
/// a body-part campaign by its maker, each with its author and its
/// confidences, never on a sealed stack; a gallery of the items with the
/// least certain first; a page accepted in one move, each item its own
/// answer with the suggestion and who made it beside it; and every refusal
/// that keeps a sealed or held-back item out of a batch.
#[test]
fn a_gallery_takes_suggestions_from_outside_and_accepts_a_page() {
    let home = registry();
    let server = Server::start(&home);
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
            "name": "parts",
            "question": {"kind": "axis", "axis": "body_part"},
            "source": {"selection": "every-stack@1"},
            "raters_per_item": 1,
            "raters": ["anna@lab", "bo@lab"],
            "adjudication": {"when": "never"},
            "closes_into": "stage",
            "suggest": "imported",
        })),
        CURATOR,
    );
    let items: Vec<Value> = made["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 4, "{made}");
    let stacks: Vec<i64> = items
        .iter()
        .map(|i| i["stack_id"].as_i64().unwrap())
        .collect();
    let item_of = |stack: i64| {
        items.iter().find(|i| i["stack_id"] == stack).unwrap()["id"]
            .as_i64()
            .unwrap()
    };
    // the first stack is of a sample sealed for a certificate
    server.ok(
        "PUT",
        "/api/ask/selections/first",
        Some(json!({"document": {
            "ast_version": 1,
            "params": {"ids": {"type": "list", "value": [stacks[0]]}},
            "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
            "out": {"set": "s", "level": "record"},
        }})),
        CURATOR,
    );
    let (ok, _, err) = cli(
        &home,
        "cleo@lab",
        &[
            "labels",
            "seal",
            "--select",
            "selection:first@1",
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
    );
    assert!(ok, "{err}");

    // ------------------------------------------------ suggestions
    let mut tsv = String::from("stack_id\tvalue\tp:brain\tp:Brain-Neck\tp:spine\n");
    for (i, s) in stacks.iter().enumerate() {
        let p = [0.9, 0.55, 0.7, 0.8][i];
        tsv.push_str(&format!("{s}\tBrain\t{p}\t{:.2}\t0\n", 1.0 - p));
    }
    tsv.push_str(&format!("{}\thand\t\t\t\n", stacks[1]));
    // a rater does not bring suggestions into the campaign they rate
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/parts/suggestions",
        Some(json!({"tsv": tsv, "author": "v0-model"})),
        ANNA,
    );
    assert_eq!(status, 403);
    let brought = server.ok(
        "POST",
        "/api/campaigns/parts/suggestions",
        Some(json!({"tsv": tsv, "author": "v0-model", "source": "v0 committed"})),
        CURATOR,
    );
    assert_eq!(brought["suggestions"], 3, "{brought}");
    assert_eq!(brought["sealed"], 1, "{brought}");
    assert_eq!(brought["refused_values"], 1, "{brought}");
    let held = server.ok("GET", "/api/campaigns/parts/suggestions", None, CURATOR);
    assert_eq!(held["count"], 3, "{held}");
    let rows = held["suggestions"].as_array().unwrap();
    assert!(rows.iter().all(|s| s["author"] == "v0-model"), "{held}");
    assert!(rows.iter().all(|s| s["stack"] != stacks[0]), "{held}");
    assert_eq!(rows[0]["source"], "v0 committed", "{held}");
    // a rater reads the counts, never the rows
    let counts = server.ok("GET", "/api/campaigns/parts/suggestions", None, ANNA);
    assert!(counts.get("suggestions").is_none(), "{counts}");

    // ------------------------------------------------ the gallery
    let (status, _) = server.call(
        "GET",
        "/api/campaigns/parts/gallery?order=sideways",
        None,
        ANNA,
    );
    assert_eq!(status, 400);
    let page = server.ok("GET", "/api/campaigns/parts/gallery", None, ANNA);
    assert_eq!(page["axis"], "body_part", "{page}");
    assert!(
        page["values"].as_array().unwrap().contains(&json!("other")),
        "{page}"
    );
    assert_eq!(page["open"], 4, "{page}");
    assert_eq!(page["sealed"], 1, "{page}");
    let listed = page["items"].as_array().unwrap().clone();
    assert_eq!(
        listed.len() as i64 + page["held_back"].as_i64().unwrap(),
        3,
        "{page}"
    );
    assert!(listed.iter().all(|i| i["stack"] != stacks[0]), "{page}");
    for i in &listed {
        assert_eq!(i["suggested"], "brain", "{i}");
        assert_eq!(i["by"], "v0-model", "{i}");
        assert!(i["confidences"]["brain-neck"].is_number(), "{i}");
        assert_eq!(
            i["thumb"],
            format!("/api/instances/{}/thumb", i["stack"]),
            "{i}"
        );
    }
    // the least certain first
    let sure: Vec<f64> = listed
        .iter()
        .map(|i| i["confidence"].as_f64().unwrap())
        .collect();
    assert!(sure.windows(2).all(|w| w[0] <= w[1]), "{sure:?}");
    // the item's evidence names the suggestion and its author; the sealed
    // one is read blind, with none
    if let Some(first) = listed.first() {
        let it = first["item"].as_i64().unwrap();
        let why = server.ok(
            "GET",
            &format!("/api/campaigns/parts/items/{it}/why"),
            None,
            ANNA,
        );
        assert_eq!(why["suggested"], "brain", "{why}");
        assert_eq!(why["suggested_by"], "v0-model", "{why}");
        assert_eq!(why["suggestions"][0]["author"], "v0-model", "{why}");
    }
    let sealed_item = item_of(stacks[0]);
    let blind = server.ok(
        "GET",
        &format!("/api/campaigns/parts/items/{sealed_item}/why"),
        None,
        ANNA,
    );
    assert_eq!(blind["blind"], true, "{blind}");
    assert!(blind["suggested"].is_null(), "{blind}");
    assert!(
        blind
            .get("suggestions")
            .is_none_or(|s| s.as_array().is_some_and(Vec::is_empty)),
        "{blind}"
    );

    // ------------------------------------------------ accepting a page
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/parts/gallery/accept",
        Some(json!({"answers": [{"item": sealed_item, "value": "brain"}]})),
        ANNA,
    );
    assert_eq!(status, 409, "a sealed item is never accepted in one move");
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/parts/gallery/accept",
        Some(json!({"answers": [], "seconds": 3})),
        ANNA,
    );
    assert_eq!(status, 400);
    // an item the seed holds back is read alone
    let shown: Vec<i64> = listed.iter().map(|i| i["item"].as_i64().unwrap()).collect();
    if let Some(back) = stacks[1..]
        .iter()
        .map(|s| item_of(*s))
        .find(|i| !shown.contains(i))
    {
        let (status, _) = server.call(
            "POST",
            "/api/campaigns/parts/gallery/accept",
            Some(json!({"answers": [{"item": back, "value": "brain"}]})),
            ANNA,
        );
        assert_eq!(status, 409);
    }
    if !shown.is_empty() {
        // the first corrected, the rest as shown
        let answers: Vec<Value> = shown
            .iter()
            .enumerate()
            .map(|(n, i)| json!({"item": i, "value": if n == 0 { "other" } else { "brain" }}))
            .collect();
        let (status, _) = server.call(
            "POST",
            "/api/campaigns/parts/gallery/accept",
            Some(json!({"answers": [{"item": shown[0], "value": "hand"}]})),
            ANNA,
        );
        assert_eq!(
            status, 400,
            "a value the axis does not take refuses the move"
        );
        let done = server.ok(
            "POST",
            "/api/campaigns/parts/gallery/accept",
            Some(json!({"answers": answers})),
            ANNA,
        );
        let accepted = done["accepted"].as_array().unwrap();
        assert_eq!(accepted.len(), shown.len(), "{done}");
        assert_eq!(accepted[0]["changed"], true, "{done}");
        assert!(
            accepted[1..].iter().all(|a| a["changed"] == false),
            "{done}"
        );
        let all = server.ok("GET", "/api/campaigns/parts/answers", None, CURATOR);
        let list = all["answers"].as_array().unwrap();
        assert_eq!(list.len(), shown.len(), "{all}");
        for a in list {
            assert_eq!(a["principal"], "anna@lab", "{a}");
            assert_eq!(a["via"], "batch", "{a}");
            assert_eq!(a["suggested"], "brain", "{a}");
            assert_eq!(a["suggested_by"], "v0-model", "{a}");
            assert!(a["seconds"].as_f64().is_some(), "{a}");
        }
        let corrected = list.iter().find(|a| a["item_id"] == shown[0]).unwrap();
        assert_eq!(corrected["value"], "other", "{corrected}");
        // what was accepted is no longer shown
        let after = server.ok("GET", "/api/campaigns/parts/gallery", None, ANNA);
        assert!(
            after["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| !shown.contains(&i["item"].as_i64().unwrap())),
            "{after}"
        );
    }

    // ------------------------------------------------ the keyboard
    let file = TempDir::new("suggest-file");
    let path = file.path().join("people.tsv");
    std::fs::write(
        &path,
        "SeriesInstanceUID\tvalue\tdate\nnone.such\tBrain\t2025-01-01\n",
    )
    .unwrap();
    let (ok, out, err) = cli(
        &home,
        "cleo@lab",
        &[
            "campaign",
            "suggest",
            "parts",
            "--file",
            path.to_str().unwrap(),
            "--author",
            "v0-person",
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
    );
    assert!(ok, "{err}");
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["unmatched"], 1, "{doc}");
    assert_eq!(doc["suggestions"], 0, "{doc}");

    // ------------------------------------------------ one axis only
    server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "two-axes",
            "question": {"kind": "axes", "axes": ["base", "body_part"]},
            "source": {"selection": "every-stack@1"},
            "raters": ["anna@lab"],
            "adjudication": {"when": "never"},
            "closes_into": "none",
        })),
        CURATOR,
    );
    let (status, _) = server.call("GET", "/api/campaigns/two-axes/gallery", None, ANNA);
    assert_eq!(status, 400);
}

/// The headers of a GET, as text.
fn headers_of(server: &Server, path: &str, token: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let head = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
        .split_once("\r\n\r\n")
        .map(|(h, _)| h.to_string())
        .unwrap_or(response)
}

/// Record 48, after the first gold campaign, through the door: the gallery
/// says what is read one by one, a claim asks for those alone and names
/// the next, a rater lists and corrects their own answers (the earlier
/// kept and exported beside it, blind on a sealed stack, refused once the
/// campaign closes), and the campaign's maker adds a rater.
#[test]
fn a_rater_corrects_their_answers_and_a_rater_joins_through_the_door() {
    let home = registry();
    let server = Server::start(&home);
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
            "name": "gold",
            "question": {"kind": "axis", "axis": "body_part"},
            "source": {"selection": "every-stack@1"},
            "raters_per_item": 1,
            "raters": ["anna@lab"],
            "adjudication": {"when": "never"},
            "closes_into": "none",
            "hold_back": 0.5,
        })),
        CURATOR,
    );
    let items: Vec<Value> = made["items"].as_array().unwrap().clone();
    let stacks: Vec<i64> = items
        .iter()
        .map(|i| i["stack_id"].as_i64().unwrap())
        .collect();
    // the first stack is of a sealed sample
    server.ok(
        "PUT",
        "/api/ask/selections/first",
        Some(json!({"document": {
            "ast_version": 1,
            "params": {"ids": {"type": "list", "value": [stacks[0]]}},
            "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
            "out": {"set": "s", "level": "record"},
        }})),
        CURATOR,
    );
    let (ok, _, err) = cli(
        &home,
        "cleo@lab",
        &[
            "labels",
            "seal",
            "--select",
            "selection:first@1",
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
    );
    assert!(ok, "{err}");

    // ------------------------------------------------ what is read alone
    let page = server.ok("GET", "/api/campaigns/gold/gallery", None, ANNA);
    let held = page["held_back_open"].as_i64().unwrap();
    assert_eq!(page["sealed"], 1, "{page}");
    assert_eq!(page["alone"], held + 1, "{page}");
    assert_eq!(
        page["items"].as_array().unwrap().len() as i64 + held + 1,
        4,
        "{page}"
    );
    // a claim of those alone, naming the next
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/gold/claim",
        Some(json!({"alone": "yes"})),
        ANNA,
    );
    assert_eq!(status, 400);
    let claimed = server.ok(
        "POST",
        "/api/campaigns/gold/claim",
        Some(json!({"alone": true})),
        ANNA,
    );
    let shown: Vec<i64> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["item"].as_i64().unwrap())
        .collect();
    let item = claimed["item"]["id"].as_i64().unwrap();
    assert!(
        !shown.contains(&item),
        "{claimed}: never one the gallery shows"
    );
    assert!(claimed.get("next").is_some(), "{claimed}");
    if held + 1 > 1 {
        assert!(claimed["next"]["item"].is_i64(), "{claimed}");
        assert!(claimed["next"]["stack"].is_i64(), "{claimed}");
    }
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    let answered = server.ok(
        "POST",
        &format!("/api/campaigns/gold/assignments/{assignment}/answer"),
        Some(json!({"value": "brain"})),
        ANNA,
    );
    let first = answered["answer"].as_i64().unwrap();

    // ------------------------------------------------ my answers
    let mine = server.ok("GET", "/api/campaigns/gold/mine", None, ANNA);
    assert_eq!(mine["count"], 1, "{mine}");
    assert_eq!(mine["open"], true, "{mine}");
    assert_eq!(mine["values"]["brain"], 1, "{mine}");
    let row = &mine["answers"][0];
    assert_eq!(row["answer"], first, "{mine}");
    assert_eq!(row["value"], "brain", "{mine}");
    assert_eq!(row["via"], "claim", "{mine}");
    let stack = row["stack"].as_i64().unwrap();
    assert_eq!(row["sealed"], stack == stacks[0], "{mine}");
    assert!(
        row.get("suggested").is_none(),
        "{mine}: nothing suggested beside it"
    );
    let none = server.ok("GET", "/api/campaigns/gold/mine?value=spine", None, ANNA);
    assert_eq!(none["count"], 0, "{none}");

    // ------------------------------------------------ a correction
    let (status, _) = server.call(
        "POST",
        &format!("/api/campaigns/gold/answers/{first}/amend"),
        Some(json!({"value": "spine", "suggested": "brain"})),
        ANNA,
    );
    assert_eq!(status, 400, "the suggestion is the engine's to keep");
    // another rater does not reach the campaign, nor its answers
    let (status, _) = server.call(
        "POST",
        &format!("/api/campaigns/gold/answers/{first}/amend"),
        Some(json!({"value": "spine"})),
        BO,
    );
    assert_eq!(status, 404);
    let fixed = server.ok(
        "POST",
        &format!("/api/campaigns/gold/answers/{first}/amend"),
        Some(json!({"value": "spine"})),
        ANNA,
    );
    assert_eq!(fixed["supersedes"], first, "{fixed}");
    assert_eq!(fixed["unchanged"], false, "{fixed}");
    let later = fixed["answer"].as_i64().unwrap();
    let (status, _) = server.call(
        "POST",
        &format!("/api/campaigns/gold/answers/{first}/amend"),
        Some(json!({"value": "neck"})),
        ANNA,
    );
    assert_eq!(status, 409, "the earlier is superseded");
    let mine = server.ok("GET", "/api/campaigns/gold/mine", None, ANNA);
    assert_eq!(mine["count"], 1, "{mine}");
    assert_eq!(mine["answers"][0]["value"], "spine", "{mine}");
    assert_eq!(mine["answers"][0]["supersedes"], first, "{mine}");
    // the rater reads their own answers, both of them
    let own = server.ok("GET", "/api/campaigns/gold/answers", None, ANNA);
    assert_eq!(own["count"], 2, "{own}");
    let earlier = own["answers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == first)
        .unwrap();
    assert_eq!(earlier["superseded_by"], later, "{own}");
    let stats = server.ok("GET", "/api/campaigns/gold/stats", None, CURATOR);
    assert_eq!(stats["all"]["amended"], 1, "{stats}");
    // audited
    let audit = server.ok("GET", "/api/audit?action=campaign.amend", None, CURATOR);
    assert_eq!(audit["count"], 1, "{audit}");
    assert_eq!(audit["rows"][0]["details"]["value"], "spine", "{audit}");
    assert_eq!(audit["rows"][0]["details"]["earlier"], "brain", "{audit}");

    // ------------------------------------------------ the header is kept
    let h = headers_of(
        &server,
        &format!("/api/campaigns/gold/items/{item}/header"),
        ANNA,
    );
    assert!(
        h.to_ascii_lowercase()
            .contains("cache-control: private, max-age=300"),
        "{h}"
    );

    // ------------------------------------------------ a rater joins
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/gold/raters",
        Some(json!({"add": ["bo@lab"]})),
        ANNA,
    );
    assert_eq!(status, 403, "a rater does not change the raters");
    let joined = server.ok(
        "POST",
        "/api/campaigns/gold/raters",
        Some(json!({"add": ["bo@lab"]})),
        CURATOR,
    );
    assert_eq!(joined["raters"], json!(["anna@lab", "bo@lab"]), "{joined}");
    assert_eq!(joined["added"], json!(["bo@lab"]), "{joined}");
    assert!(
        server.ok("POST", "/api/campaigns/gold/claim", Some(json!({})), BO)["item"].is_object()
    );
    let (ok, out, err) = cli(
        &home,
        "cleo@lab",
        &["campaign", "raters", "gold", "--add", "rita@lab", "--json"],
    );
    assert!(ok, "{err}");
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["added"], json!(["rita@lab"]), "{doc}");

    // ------------------------------------------------ closed
    server.ok("POST", "/api/campaigns/gold/close", None, CURATOR);
    let (status, _) = server.call(
        "POST",
        &format!("/api/campaigns/gold/answers/{later}/amend"),
        Some(json!({"value": "brain"})),
        ANNA,
    );
    assert_eq!(status, 409);
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/gold/raters",
        Some(json!({"remove": ["bo@lab"]})),
        CURATOR,
    );
    assert_eq!(status, 409);
    let mine = server.ok("GET", "/api/campaigns/gold/mine", None, ANNA);
    assert_eq!(mine["open"], false, "{mine}");
}

/// A campaign says when it is made what its raters are shown beside each
/// item. One made without saying shows nothing on any of its doors: no
/// suggestion and no rules' line at the evidence door, which reads it blind
/// as a sealed stack is read, no batch, a gallery with nothing suggested in
/// the order the items were listed, and every answer kept with nothing
/// suggested beside it. One made to show what is brought in hides the
/// rules' lines the same way and shows only what was brought.
#[test]
fn a_campaign_made_without_suggestions_shows_none_on_any_door() {
    let home = registry();
    let server = Server::start(&home);
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
    let make = |name: &str, suggest: Option<&str>| {
        let mut body = json!({
            "name": name,
            "question": {"kind": "axis", "axis": "base"},
            "source": {"selection": "every-stack@1"},
            "raters_per_item": 1,
            "raters": ["anna@lab", "bo@lab"],
            "adjudication": {"when": "never"},
            "closes_into": "none",
        });
        if let Some(s) = suggest {
            body["suggest"] = json!(s);
        }
        server.ok("POST", "/api/campaigns", Some(body), CURATOR)
    };
    // the same stacks shown the rules, so what the other hides is there
    let led = make("led", Some("rules"));
    assert_eq!(led["suggest"], "rules", "{led}");
    let led_item = led["items"][0]["id"].as_i64().unwrap();
    let why = server.ok(
        "GET",
        &format!("/api/campaigns/led/items/{led_item}/why"),
        None,
        ANNA,
    );
    assert!(why["suggested"].is_string(), "{why}");
    assert!(
        why["axes"].as_array().is_some_and(|a| !a.is_empty()),
        "{why}"
    );

    let (status, doc) = server.call(
        "POST",
        "/api/campaigns",
        Some(
            json!({"name": "odd", "question": {"kind": "axis", "axis": "base"},
            "source": {"selection": "every-stack@1"}, "suggest": "always"}),
        ),
        CURATOR,
    );
    assert_eq!(status, 400, "{doc}");

    let made = make("unled", None);
    assert_eq!(made["suggest"], "none", "{made}");
    let shown = server.ok("GET", "/api/campaigns/unled", None, ANNA);
    assert_eq!(shown["suggest"], "none", "{shown}");
    let items: Vec<i64> = made["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_i64().unwrap())
        .collect();
    assert!(items.len() >= 4, "{made}");
    // every item is read blind: the file, and nothing a system said of it
    for item in &items {
        for who in [ANNA, CURATOR] {
            let why = server.ok(
                "GET",
                &format!("/api/campaigns/unled/items/{item}/why"),
                None,
                who,
            );
            assert_eq!(why["blind"], true, "{why}");
            assert_eq!(why["suggest"], "none", "{why}");
            assert!(why["suggested"].is_null(), "{why}");
            assert!(why["suggested_by"].is_null(), "{why}");
            assert_eq!(why["suggestions"], json!([]), "{why}");
            assert!(why["worth"].is_null(), "{why}");
            for k in ["axes", "asked", "candidates", "voted", "line"] {
                assert!(!why.to_string().contains(&format!("\"{k}\"")), "{k}: {why}");
            }
            assert!(why["header"].is_object(), "{why}");
        }
        let head = server.ok(
            "GET",
            &format!("/api/campaigns/unled/items/{item}/header"),
            None,
            ANNA,
        );
        assert_eq!(head["blind"], true, "{head}");
    }
    // no batch is formed or accepted
    let (status, doc) = server.call("GET", "/api/campaigns/unled/batches", None, ANNA);
    assert_eq!(status, 409, "{doc}");
    assert!(doc.to_string().contains("suggests none"), "{doc}");
    let (status, doc) = server.call(
        "POST",
        "/api/campaigns/unled/batches/any/accept",
        Some(json!({})),
        ANNA,
    );
    assert_eq!(status, 409, "{doc}");
    // the gallery suggests nothing, in the order the items were listed,
    // whatever order is asked
    for order in ["uncertain", "suggested", "position"] {
        let g = server.ok(
            "GET",
            &format!("/api/campaigns/unled/gallery?order={order}"),
            None,
            BO,
        );
        assert_eq!(g["order"], "position", "{g}");
        assert_eq!(g["suggest"], "none", "{g}");
        let shown = g["items"].as_array().unwrap();
        assert!(!shown.is_empty(), "{g}");
        let positions: Vec<i64> = shown
            .iter()
            .map(|i| i["position"].as_i64().unwrap())
            .collect();
        let mut sorted = positions.clone();
        sorted.sort();
        assert_eq!(positions, sorted, "{g}");
        for i in shown {
            for k in ["suggested", "by", "confidence", "confidences"] {
                assert!(i[k].is_null(), "{k}: {i}");
            }
            assert_eq!(i["others"], json!([]), "{i}");
            assert_eq!(i["disagree"], false, "{i}");
        }
    }
    // an answer by claim, in the order of value, keeps nothing suggested
    let claimed = server.ok(
        "POST",
        "/api/campaigns/unled/claim",
        Some(json!({"order": "value"})),
        ANNA,
    );
    let a = claimed["assignment"]["id"].as_i64().unwrap();
    server.ok(
        "POST",
        &format!("/api/campaigns/unled/assignments/{a}/answer"),
        Some(json!({"value": "T1w"})),
        ANNA,
    );
    // and a page of the gallery accepted keeps nothing suggested either
    let g = server.ok("GET", "/api/campaigns/unled/gallery", None, BO);
    let page: Vec<Value> = g["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| json!({"item": i["item"], "value": "T2w"}))
        .collect();
    let accepted = server.ok(
        "POST",
        "/api/campaigns/unled/gallery/accept",
        Some(json!({"answers": page})),
        BO,
    );
    for a in accepted["accepted"].as_array().unwrap() {
        assert!(a["changed"].is_null(), "{a}");
    }
    let answers = server.ok("GET", "/api/campaigns/unled/answers", None, CURATOR);
    let list = answers["answers"].as_array().unwrap();
    assert!(list.len() >= 2, "{answers}");
    for a in list {
        assert!(a["suggested"].is_null(), "{a}");
        assert!(a["suggested_by"].is_null(), "{a}");
    }
    // suggestions are never brought into it later
    let (status, doc) = server.call(
        "POST",
        "/api/campaigns/unled/suggestions",
        Some(json!({"suggestions": [{"item": items[1], "value": "T1w", "author": "v0-model"}]})),
        CURATOR,
    );
    assert_eq!(status, 409, "{doc}");

    // a campaign that shows what is brought in never shows the rules
    let told = make("told", Some("imported"));
    let first = told["items"][0]["id"].as_i64().unwrap();
    let before = server.ok(
        "GET",
        &format!("/api/campaigns/told/items/{first}/why"),
        None,
        ANNA,
    );
    assert_eq!(before["blind"], false, "{before}");
    assert!(before["suggested"].is_null(), "{before}");
    assert!(!before.to_string().contains("\"axes\""), "{before}");
    let (status, doc) = server.call("GET", "/api/campaigns/told/batches", None, ANNA);
    assert_eq!(status, 409, "{doc}");
    server.ok(
        "POST",
        "/api/campaigns/told/suggestions",
        Some(json!({"suggestions": [{"item": first, "value": "T2w", "author": "v0-model"}]})),
        CURATOR,
    );
    let after = server.ok(
        "GET",
        &format!("/api/campaigns/told/items/{first}/why"),
        None,
        ANNA,
    );
    assert_eq!(after["suggested"], "T2w", "{after}");
    assert_eq!(after["suggested_by"], "v0-model", "{after}");
    assert!(!after.to_string().contains("\"axes\""), "{after}");
    let g = server.ok("GET", "/api/campaigns/told/gallery", None, BO);
    for i in g["items"].as_array().unwrap() {
        if i["item"] != first {
            assert!(i["suggested"].is_null(), "{i}");
        }
    }
}
