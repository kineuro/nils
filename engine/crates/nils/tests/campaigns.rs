// SPDX-License-Identifier: AGPL-3.0-only

//! The proof of record 42: the one campaign mechanism, driven only through
//! the door by clients that each hold a token and never open the database.
//! The registry is made by the command line, as every serve test makes
//! its own; from then on every act is an HTTP call.
//!
//! - **(a) annotation:** a form and derivative campaign on a synthetic
//!   selection; two raters answer with masks, an external metric sends the
//!   item to an adjudicator, and the closed campaign exports.
//! - **(b) curation:** an axis `body_part` campaign over the same stacks,
//!   whose answers become person decisions and a label set.
//!
//! The masks are uploaded through the derivative door (record 42 S4) into
//! a working place the curator declares, and an answer names the id the
//! upload answered; the campaign checks the row, never the file.

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
        ("P1", "1.2.3.B", "1.2.3.B.1.1", "t2 flair"),
        ("P2", "1.2.3.C", "1.2.3.C.1.1", "t1 mprage"),
        ("P2", "1.2.3.D", "1.2.3.D.1.1", "t2 spine"),
    ] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
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
const JUDGE: &str = "judge-token-of-length";

impl Server {
    fn start(home: &TempDir) -> Server {
        let tokens = [
            // a reviewer who also runs campaigns and may declare a place
            format!("{CURATOR}=cleo@lab:reviewer,campaigns:work,places:work,audit:see"),
            // raters hold the campaign's grant and nothing of the queue, and
            // the Pipelines page's work to upload their masks
            format!("{ANNA}=anna@lab:campaigns:work,pipelines:work"),
            format!("{BO}=bo@lab:campaigns:work,pipelines:work"),
            format!("{JUDGE}=judge@lab:campaigns:work,pipelines:work"),
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

    /// Upload a mask made from one stack through the derivative door, and
    /// answer the derivative's id.
    fn mask(&self, stack: i64, content: &str, token: &str) -> i64 {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let path = format!(
            "/api/derivatives?kind=mask&stack={stack}&sha256={}&name=mask.nii.gz",
            sha256(content)
        );
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nAuthorization: Bearer {token}\r\n\r\n",
            content.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(content.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        assert!(headers.starts_with("HTTP/1.1 201"), "{headers}: {text}");
        let doc: Value = serde_json::from_str(text).unwrap();
        assert_eq!(doc["stack_id"], stack, "{doc}");
        doc["id"].as_i64().unwrap()
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

fn sha256(text: &str) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, text.as_bytes()).as_ref())
}

/// A rater claims the next item, uploads a mask of its stack and answers
/// with it; answers the claim, what the answer did and the mask's id.
fn rate_mask(
    server: &Server,
    campaign: &str,
    token: &str,
    content: &str,
    form: Value,
) -> (Value, Value, i64) {
    let claimed = server.ok(
        "POST",
        &format!("/api/campaigns/{campaign}/claim"),
        Some(json!({})),
        token,
    );
    let assignment = claimed["assignment"]["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("nothing claimed: {claimed}"));
    let stack = claimed["item"]["stack_id"].as_i64().unwrap();
    let mask = server.mask(stack, content, token);
    let done = server.ok(
        "POST",
        &format!("/api/campaigns/{campaign}/assignments/{assignment}/answer"),
        Some(json!({"derivative_id": mask, "form": form})),
        token,
    );
    (claimed, done, mask)
}

/// A rater claims the next item and answers it; answers the claim and what
/// the answer did.
fn rate(server: &Server, campaign: &str, token: &str, answer: Value) -> (Value, Value) {
    let claimed = server.ok(
        "POST",
        &format!("/api/campaigns/{campaign}/claim"),
        Some(json!({})),
        token,
    );
    let assignment = claimed["assignment"]["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("nothing claimed: {claimed}"));
    let done = server.ok(
        "POST",
        &format!("/api/campaigns/{campaign}/assignments/{assignment}/answer"),
        Some(answer),
        token,
    );
    (claimed, done)
}

#[test]
fn one_campaign_mechanism_annotates_and_curates_through_the_door_alone() {
    let home = registry();
    let out = TempDir::new("campaign-export");
    let server = Server::start(&home);

    // the curator declares where labels go and where derivatives live, and
    // saves a selection of every stack
    server.ok(
        "POST",
        "/api/places",
        Some(json!({"name": "labels-out", "role": "export", "path": out.path().to_str().unwrap()})),
        CURATOR,
    );
    let work = TempDir::new("campaign-work");
    server.ok(
        "POST",
        "/api/places",
        Some(json!({"name": "scratch", "role": "working", "path": work.path().to_str().unwrap()})),
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

    // R7: a rater holds no reviewer's grant, so a rater can neither freeze a
    // selection (a question is Query work) nor read the queue
    let (status, refused) = server.call(
        "POST",
        "/api/campaigns",
        Some(json!({"name": "nope", "question": {"kind": "free"}, "source": {"selection": "every-stack@1"}})),
        ANNA,
    );
    assert_eq!(status, 403, "{refused}");
    let (status, _) = server.call("GET", "/api/review", None, ANNA);
    assert_eq!(status, 403);

    // ------------------------------------------------ (a) annotation
    let made = server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "lesions",
            "question": {"kind": "derivative", "derivative_kind": "mask", "form": {
                "properties": {"lesions": {"type": "integer"}, "note": {"type": "string"}},
                "required": ["lesions"],
            }},
            "source": {"selection": "every-stack@1"},
            "raters_per_item": 2,
            "adjudicators": ["judge@lab"],
            "adjudication": {"when": "disagree", "metric": "external", "threshold": 0.8},
            "closes_into": "none",
            "lease_seconds": 600,
        })),
        CURATOR,
    );
    let stacks = made["items"].as_array().unwrap().len();
    assert!(stacks >= 4, "{made}");
    assert!(
        made["handle_id"].is_i64(),
        "the frozen list is a handle: {made}"
    );
    // the two raters take the same first item, and each uploads a mask
    let (a, _, anna_mask) = rate_mask(
        &server,
        "lesions",
        ANNA,
        "anna's mask",
        json!({"lesions": 2}),
    );
    let (b, rated, bo_mask) = rate_mask(&server, "lesions", BO, "bo's mask", json!({"lesions": 3}));
    let item = a["item"]["id"].as_i64().unwrap();
    let item_stack = a["item"]["stack_id"].as_i64().unwrap();
    assert_eq!(b["item"]["id"].as_i64(), Some(item));
    assert_eq!(rated["state"], "awaiting_metric", "{rated}");
    // a mask answer without its file is refused, and so is one naming a
    // file the derivative door never registered, or a mask of another stack
    let (claimed, _) = (
        server.ok(
            "POST",
            "/api/campaigns/lesions/claim",
            Some(json!({})),
            ANNA,
        ),
        (),
    );
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    let (status, _) = server.call(
        "POST",
        &format!("/api/campaigns/lesions/assignments/{assignment}/answer"),
        Some(json!({"form": {"lesions": 1}})),
        ANNA,
    );
    assert_eq!(status, 400);
    let (status, doc) = server.call(
        "POST",
        &format!("/api/campaigns/lesions/assignments/{assignment}/answer"),
        Some(json!({"derivative_id": 9001, "form": {"lesions": 1}})),
        ANNA,
    );
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap()
            .contains("no derivative 9001"),
        "{doc}"
    );
    let (status, doc) = server.call(
        "POST",
        &format!("/api/campaigns/lesions/assignments/{assignment}/answer"),
        Some(json!({"derivative_id": anna_mask, "form": {"lesions": 1}})),
        ANNA,
    );
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap()
            .contains(&format!("stack {item_stack}")),
        "{doc}"
    );
    server.ok(
        "POST",
        &format!("/api/campaigns/lesions/assignments/{assignment}/release"),
        None,
        ANNA,
    );
    // what compared the masks posts its Dice; below the threshold the item
    // goes to the adjudicator
    let measured = server.ok(
        "POST",
        &format!("/api/campaigns/lesions/items/{item}/metric"),
        Some(json!({"name": "dice", "value": 0.62})),
        CURATOR,
    );
    assert_eq!(measured["state"], "needs_adjudication", "{measured}");
    // a rater of the item is not its adjudicator
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/lesions/claim",
        Some(json!({"role": "adjudicator"})),
        ANNA,
    );
    assert_eq!(status, 403);
    let claimed = server.ok(
        "POST",
        "/api/campaigns/lesions/claim",
        Some(json!({"role": "adjudicator"})),
        JUDGE,
    );
    assert_eq!(claimed["item"]["id"].as_i64(), Some(item), "{claimed}");
    assert_eq!(claimed["assignment"]["round"], 2, "{claimed}");
    let union = server.mask(item_stack, "the union, trimmed", JUDGE);
    assert!(union != anna_mask && union != bo_mask);
    let adjudicated = server.ok(
        "POST",
        &format!(
            "/api/campaigns/lesions/assignments/{}/answer",
            claimed["assignment"]["id"]
        ),
        Some(json!({"derivative_id": union, "form": {"lesions": 3}, "why": "the union, trimmed"})),
        JUDGE,
    );
    assert_eq!(adjudicated["state"], "adjudicated", "{adjudicated}");
    // closing writes into the registry, which a rater may not do
    let (status, _) = server.call("POST", "/api/campaigns/lesions/close", None, ANNA);
    assert_eq!(status, 403);
    let closed = server.ok("POST", "/api/campaigns/lesions/close", None, CURATOR);
    assert_eq!(closed["resolved"], 1, "{closed}");
    assert_eq!(
        closed["unresolved"].as_i64(),
        Some(stacks as i64 - 1),
        "{closed}"
    );
    assert_eq!(
        closed["decisions"],
        json!([]),
        "a mask closes into no decision"
    );
    // at plain detail a rater reads the answers without their free text,
    // their forms or who acted; a reviewer at quasi reads them whole
    let (status, plain) = server.call("GET", "/api/campaigns/lesions/answers", None, ANNA);
    assert_eq!(status, 200, "{plain}");
    for a in plain["answers"].as_array().unwrap() {
        for field in ["why", "form", "actor_detail"] {
            assert!(a.get(field).is_none(), "{field} at plain: {a}");
        }
    }
    assert!(!plain.to_string().contains("the union, trimmed"), "{plain}");
    let full = server.ok("GET", "/api/campaigns/lesions/answers", None, CURATOR);
    assert!(full.to_string().contains("the union, trimmed"), "{full}");
    let (status, shown) = server.call("GET", "/api/campaigns/lesions", None, ANNA);
    assert_eq!(status, 200, "{shown}");
    for it in shown["items"].as_array().unwrap() {
        assert!(it["outcome"].get("form").is_none(), "{it}");
    }
    // the closed campaign exports: its outcome, the adjudicator's mask, and
    // every answer
    let outcome = server.ok(
        "POST",
        "/api/campaigns/lesions/export",
        Some(json!({"place": "labels-out"})),
        CURATOR,
    );
    assert_eq!(outcome["rows"], 1, "{outcome}");
    let answers = server.ok(
        "POST",
        "/api/campaigns/lesions/export",
        Some(json!({"place": "labels-out", "of": "answers", "name": "lesions-answers"})),
        CURATOR,
    );
    assert_eq!(answers["rows"], 3, "{answers}");
    let set = server.ok(
        "GET",
        &format!("/api/label-sets/{}", outcome["id"]),
        None,
        ANNA,
    );
    let tsv = set["files"]["labels.tsv"].as_str().unwrap();
    assert_eq!(sha256(tsv), set["digest"].as_str().unwrap(), "{set}");
    assert_eq!(
        set["files"]["provenance.json"]["digest"]["sha256"], set["digest"],
        "{set}"
    );
    let row: Vec<&str> = tsv.lines().nth(1).unwrap().split('\t').collect();
    assert_eq!(row[5], union.to_string(), "the adjudicator's mask: {tsv}");
    assert_eq!(row[7], "judge@lab", "{tsv}");

    // ------------------------------------------------ (b) curation
    let made = server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "body-part",
            "question": {"kind": "axis", "axis": "body_part"},
            "source": {"selection": "every-stack@1"},
            "raters_per_item": 2,
            "adjudicators": ["judge@lab"],
            "adjudication": {"when": "disagree", "metric": "kappa"},
            "closes_into": "decision",
        })),
        CURATOR,
    );
    // the pack's vocabulary is the answer's
    assert!(
        made["question"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "brain"),
        "{made}"
    );
    let n = made["items"].as_array().unwrap().len();
    let (status, _) = server.call(
        "POST",
        "/api/campaigns/body-part/claim",
        Some(json!({})),
        ANNA,
    );
    assert_eq!(status, 200);
    // anna and bo rate every item; they disagree on the last
    let mut adjudications = 0;
    for i in 0..n {
        let first = if i == 0 {
            // anna's claim above is handed back, not a second
            let held = server.ok(
                "POST",
                "/api/campaigns/body-part/claim",
                Some(json!({})),
                ANNA,
            );
            assert_eq!(held["held"], true, "{held}");
            server.ok(
                "POST",
                &format!(
                    "/api/campaigns/body-part/assignments/{}/answer",
                    held["assignment"]["id"]
                ),
                Some(json!({"value": "brain"})),
                ANNA,
            )
        } else {
            rate(&server, "body-part", ANNA, json!({"value": "brain"})).1
        };
        assert_eq!(first["state"], "open", "{first}");
        let value = if i + 1 == n { "spine" } else { "brain" };
        let (_, done) = rate(&server, "body-part", BO, json!({"value": value}));
        if done["adjudication"].is_i64() {
            adjudications += 1;
        }
    }
    assert_eq!(adjudications, 1);
    // an answer outside the pack's vocabulary never got in; no third rater
    // is wanted
    let (status, nothing) = server.call(
        "POST",
        "/api/campaigns/body-part/claim",
        Some(json!({})),
        JUDGE,
    );
    assert_eq!(status, 200);
    assert!(nothing["assignment"].is_null(), "{nothing}");
    let (_, settled) = {
        let claimed = server.ok(
            "POST",
            "/api/campaigns/body-part/claim",
            Some(json!({"role": "adjudicator"})),
            JUDGE,
        );
        let done = server.ok(
            "POST",
            &format!(
                "/api/campaigns/body-part/assignments/{}/answer",
                claimed["assignment"]["id"]
            ),
            Some(json!({"value": "brain-neck"})),
            JUDGE,
        );
        (claimed, done)
    };
    assert_eq!(settled["state"], "adjudicated", "{settled}");
    let shown = server.ok("GET", "/api/campaigns/body-part", None, ANNA);
    assert_eq!(shown["counts"]["answers"].as_i64(), Some(2 * n as i64 + 1));
    let closed = server.ok("POST", "/api/campaigns/body-part/close", None, CURATOR);
    assert_eq!(closed["decisions"].as_array().unwrap().len(), n, "{closed}");
    assert_eq!(closed["agreement"]["items"].as_i64(), Some(n as i64));
    let exact = closed["agreement"]["exact"].as_f64().unwrap();
    assert!(
        (exact - (n as f64 - 1.0) / n as f64).abs() < 1e-9,
        "{closed}"
    );
    // the decisions are a label set
    let labels = server.ok(
        "POST",
        "/api/label-sets",
        Some(json!({"axis": "body_part", "campaign": "body-part", "place": "labels-out", "name": "body-part-curated"})),
        CURATOR,
    );
    assert_eq!(labels["rows"].as_i64(), Some(n as i64), "{labels}");
    assert_eq!(labels["sealed"], false);
    // whether a set is sealed is the registry's to say (record 40 R3)
    let (status, refused) = server.call(
        "POST",
        "/api/label-sets",
        Some(json!({"axis": "body_part", "place": "labels-out", "sealed": false})),
        CURATOR,
    );
    assert_eq!(status, 400, "{refused}");
    let again = server.ok(
        "POST",
        "/api/label-sets",
        Some(json!({"axis": "body_part", "campaign": "body-part", "place": "labels-out", "name": "body-part-curated"})),
        CURATOR,
    );
    assert_eq!(
        again["digest"], labels["digest"],
        "the same state, the same digest"
    );
    // a set under a name taken is the name's next version, in a directory
    // of its own, and never writes over an earlier version's files
    assert_eq!(labels["version"], 1, "{labels}");
    assert_eq!(again["version"], 2, "{again}");
    assert_ne!(again["path"], labels["path"], "{again}");
    let other = server.ok(
        "POST",
        "/api/label-sets",
        Some(json!({"axis": "body_part", "authors": ["model"], "place": "labels-out", "name": "body-part-curated"})),
        CURATOR,
    );
    assert_eq!(other["version"], 3, "{other}");
    assert_ne!(other["digest"], labels["digest"], "{other}");
    for set in [&labels, &again, &other] {
        let read = server.ok(
            "GET",
            &format!("/api/label-sets/{}", set["id"]),
            None,
            CURATOR,
        );
        let tsv = read["files"]["labels.tsv"].as_str().unwrap();
        assert_eq!(sha256(tsv), read["digest"].as_str().unwrap(), "{read}");
    }
    let set = server.ok(
        "GET",
        &format!("/api/label-sets/{}", labels["id"]),
        None,
        CURATOR,
    );
    let tsv = set["files"]["labels.tsv"].as_str().unwrap();
    let mut by_author = std::collections::BTreeMap::new();
    for line in tsv.lines().skip(1) {
        let cells: Vec<&str> = line.split('\t').collect();
        assert!(!cells[8].is_empty(), "every row names its decision: {line}");
        assert_eq!(cells[6], "person", "{line}");
        *by_author.entry(cells[7].to_string()).or_insert(0) += 1;
        assert_eq!(cells[9], closed_campaign_id(&shown), "{line}");
    }
    assert_eq!(by_author.get("judge@lab"), Some(&1), "{by_author:?}");
    assert_eq!(by_author.get("cleo@lab"), Some(&(n - 1)), "{by_author:?}");
    // counts only
    eprintln!(
        "record 42 proof: annotation 1 item adjudicated of {stacks}, 3 answers exported; curation {n} decisions, {} by the adjudicator, exact agreement {exact:.2}",
        by_author.get("judge@lab").copied().unwrap_or(0)
    );
    // the audit shows the campaign's acts
    let audit = server.ok("GET", "/api/audit?action=campaign.close", None, CURATOR);
    assert!(audit.to_string().contains("campaign.close"), "{audit}");
}

fn closed_campaign_id(shown: &Value) -> String {
    shown["id"].as_i64().unwrap().to_string()
}

/// Run the command line in the registry as a principal, and answer its
/// stdout; a failure says why.
fn cli(home: &TempDir, who: &str, args: &[&str]) -> String {
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
    assert!(
        out.status.success(),
        "{}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The same mechanism at the keyboard: a campaign frozen from a saved
/// selection, two raters and an adjudicator, the close staged, the commit
/// by a minimum confidence that takes only its part, the labels exported
/// twice to the same digest, and v0's labels imported beside them.
#[test]
fn the_keyboard_runs_a_campaign_and_commits_only_the_confident_part() {
    let home = registry();
    let work = TempDir::new("campaign-cli");
    let pack_dir = packs();
    let pack_dir = pack_dir.to_str().unwrap();
    let doc = work.path().join("every.json");
    std::fs::write(
        &doc,
        json!({"ast_version": 1, "sets": {"every": {"grain": "stack"}}, "out": {"set": "every", "level": "record"}}).to_string(),
    )
    .unwrap();
    cli(
        &home,
        "cleo@lab",
        &[
            "ask",
            "selections",
            "save",
            "--name",
            "every",
            "--file",
            doc.to_str().unwrap(),
            "--pack-dir",
            pack_dir,
        ],
    );
    let made: Value = serde_json::from_str(&cli(
        &home,
        "cleo@lab",
        &[
            "campaign",
            "create",
            "curate",
            "--axis",
            "body_part",
            "--select",
            "selection:every@1",
            "--raters-per-item",
            "2",
            "--adjudicator",
            "judge@lab",
            "--closes-into",
            "stage",
            "--pack-dir",
            pack_dir,
            "--json",
        ],
    ))
    .unwrap();
    let n = made["items"].as_array().unwrap().len();
    assert!(n >= 4, "{made}");
    assert!(
        made["question"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "spine")
    );
    for i in 0..n {
        for (who, value) in [
            ("anna@lab", "brain"),
            ("bo@lab", if i == 0 { "spine" } else { "brain" }),
        ] {
            let claimed: Value =
                serde_json::from_str(&cli(&home, who, &["campaign", "claim", "curate", "--json"]))
                    .unwrap();
            let a = claimed["assignment"]["id"].as_i64().unwrap().to_string();
            cli(&home, who, &["campaign", "answer", &a, "--value", value]);
        }
    }
    let claimed: Value = serde_json::from_str(&cli(
        &home,
        "judge@lab",
        &["campaign", "claim", "curate", "--adjudicator", "--json"],
    ))
    .unwrap();
    let a = claimed["assignment"]["id"].as_i64().unwrap().to_string();
    cli(
        &home,
        "judge@lab",
        &["campaign", "answer", &a, "--value", "brain"],
    );
    let closed: Value = serde_json::from_str(&cli(
        &home,
        "cleo@lab",
        &["campaign", "close", "curate", "--json"],
    ))
    .unwrap();
    assert_eq!(closed["decisions"].as_array().unwrap().len(), n, "{closed}");
    assert_eq!(closed["staged"], true);
    // the adjudicated item's answer had one rater of two behind it: 0.5
    let text = cli(
        &home,
        "cleo@lab",
        &["review", "commit", "--min-confidence", "0.9"],
    );
    assert!(
        text.contains(&format!("committed {} decision(s)", n - 1))
            && text.contains("1 left staged"),
        "{text}"
    );
    // the part committed moved the epoch, so the rest reads as drifted
    let text = cli(
        &home,
        "cleo@lab",
        &["review", "commit", "--campaign", "curate", "--anyway"],
    );
    assert!(text.contains("committed 1 decision(s)"), "{text}");
    // the labels, twice, to one digest
    let first = work.path().join("one");
    let second = work.path().join("two");
    let set: Value = serde_json::from_str(&cli(
        &home,
        "cleo@lab",
        &[
            "labels",
            "export",
            "--axis",
            "body_part",
            "--campaign",
            "curate",
            "--to",
            first.to_str().unwrap(),
            "--json",
        ],
    ))
    .unwrap();
    // an operator seals the selection as a certification sample: a set of
    // its stacks is sealed now, whatever anyone asks, and trains nothing
    let status = nils()
        .arg("--registry")
        .arg(home.path())
        .args([
            "labels",
            "export",
            "--axis",
            "body_part",
            "--to",
            second.to_str().unwrap(),
            "--sealed",
        ])
        .env("NILS_PRINCIPAL", "cleo@lab")
        .output()
        .unwrap()
        .status;
    assert!(!status.success(), "--sealed is not a caller's to say");
    let sealed: Value = serde_json::from_str(&cli(
        &home,
        "cleo@lab",
        &[
            "labels",
            "seal",
            "--select",
            "selection:every@1",
            "--pack-dir",
            pack_dir,
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(sealed["stacks"].as_i64(), Some(n as i64), "{sealed}");
    let again: Value = serde_json::from_str(&cli(
        &home,
        "cleo@lab",
        &[
            "labels",
            "export",
            "--axis",
            "body_part",
            "--to",
            second.to_str().unwrap(),
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(set["rows"].as_i64(), Some(n as i64), "{set}");
    assert_eq!(
        set["digest"], again["digest"],
        "the same state, the same digest"
    );
    assert!(
        again["training"].as_str().unwrap().starts_with("refused"),
        "{again}"
    );
    let tsv = std::fs::read_to_string(first.join("labels.tsv")).unwrap();
    assert_eq!(sha256(&tsv), set["digest"].as_str().unwrap());
    // a directory that holds a set already is never written over
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args([
            "labels",
            "export",
            "--axis",
            "body_part",
            "--author",
            "model",
            "--to",
            first.to_str().unwrap(),
        ])
        .env("NILS_PRINCIPAL", "cleo@lab")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        sha256(&std::fs::read_to_string(first.join("labels.tsv")).unwrap()),
        set["digest"].as_str().unwrap()
    );
    let provenance: Value =
        serde_json::from_str(&std::fs::read_to_string(second.join("provenance.json")).unwrap())
            .unwrap();
    assert_eq!(provenance["sealed"], true, "{provenance}");
    // a model trained on those labels is refused, by either set's digest
    let card = work.path().join("card.json");
    std::fs::write(
        &card,
        json!({"name": "bp", "version": "1", "kind": "pass",
               "digest": format!("sha256:{}", "7".repeat(64)), "task": "axis:body_part",
               "trained_on": {"label_set": format!("sha256:{}", set["digest"].as_str().unwrap())}})
        .to_string(),
    )
    .unwrap();
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(["model", "register", "--card", card.to_str().unwrap()])
        .env("NILS_PRINCIPAL", "cleo@lab")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("R3"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // the campaign's own answers, one row each
    let answers: Value = serde_json::from_str(&cli(
        &home,
        "cleo@lab",
        &[
            "campaign",
            "export",
            "curate",
            "--answers",
            "--to",
            work.path().join("answers").to_str().unwrap(),
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(
        answers["rows"].as_i64(),
        Some(2 * n as i64 + 1),
        "{answers}"
    );
    // v0's labels by SeriesInstanceUID: a person's decision here keeps its place
    let v0 = work.path().join("v0.tsv");
    std::fs::write(
        &v0,
        "SeriesInstanceUID\tbody_part\tdate\n1.2.3.A.1\tBrain\t2024-05-06\n9.9.9\tSpine\t2024-05-07\n",
    )
    .unwrap();
    let imported: Value = serde_json::from_str(&cli(
        &home,
        "cleo@lab",
        &[
            "labels",
            "import-v0",
            "--tsv",
            v0.to_str().unwrap(),
            "--pack-dir",
            pack_dir,
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(imported["series_matched"], 1, "{imported}");
    assert_eq!(imported["series_unmatched"], 1, "{imported}");
    assert_eq!(imported["held"], 1, "{imported}");
    assert_eq!(imported["decisions"], 0, "{imported}");
    let listed = cli(&home, "cleo@lab", &["labels", "list"]);
    assert!(listed.contains("sealed"), "{listed}");
    let shown = cli(&home, "cleo@lab", &["campaign", "show", "curate"]);
    assert!(shown.contains("closed"), "{shown}");
}
