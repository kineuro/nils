// SPDX-License-Identifier: AGPL-3.0-only

//! The post-contrast study's pair mode (P6): a pair campaign shows two
//! stacks of one session side by side, left and right drawn from its seed,
//! and nothing else of them. Five stacks of two people, a pre, a post and
//! a same-header rerun of one, and a pair of the other, make three pairs;
//! a rater reads each from its pictures alone, every door that would show a
//! time, a series name, a header or the rules' value refuses the campaign,
//! and each answer resolves into the post-contrast value of each stack,
//! kept on the item's outcome at the close.

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

const NZ: u32 = 4;
const NY: u32 = 24;
const NX: u32 = 20;
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

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
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
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(home: &TempDir, args: &[&str], stdin: Option<&str>) -> String {
    let (good, out, err) = run(home, args, stdin);
    assert!(good, "nils {args:?} failed: {err}");
    out
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

    /// A call: the status, and the body as JSON, or its length for bytes.
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
        let rest = &response[split + 4..];
        let status: u16 = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let doc = serde_json::from_slice(rest).unwrap_or_else(|_| json!({"bytes": rest.len()}));
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

/// A token the desk would sign: the subject, its grants and its detail.
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

/// The stack ids a selection over these ids freezes to.
fn selection_of(ids: &[i64]) -> Value {
    json!({"document": {
        "ast_version": 1,
        "params": {"ids": {"type": "list", "value": ids}},
        "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
        "out": {"set": "s", "level": "record"},
    }})
}

/// A series of NZ small planes, named and timed as a scanner would.
/// `nz` planes, so a rerun with its pre's very header is still told apart
/// here, by its depth.
fn series(dir: &TempDir, patient: &str, study: &str, n: u32, name: &str, time: &str, nz: u32) {
    let series = format!("{study}.{n}");
    for z in 0..nz {
        let sop = format!("{series}.{}", z + 1);
        let mut e = synth::minimal_mr(study, &series, &sop);
        for (tag, vr, v) in [
            (tags::PATIENT_ID, VR::LO, patient.to_string()),
            (tags::SERIES_DESCRIPTION, VR::LO, name.to_string()),
            (tags::PROTOCOL_NAME, VR::LO, name.to_string()),
            (tags::SERIES_NUMBER, VR::IS, n.to_string()),
            (tags::SERIES_TIME, VR::TM, time.to_string()),
            (tags::ACQUISITION_TIME, VR::TM, time.to_string()),
            (tags::INSTANCE_NUMBER, VR::IS, (z + 1).to_string()),
            (
                tags::IMAGE_POSITION_PATIENT,
                VR::DS,
                format!("0\\0\\{}", z as f64 * 2.0),
            ),
            (
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                "1\\0\\0\\0\\1\\0".to_string(),
            ),
            (tags::PIXEL_SPACING, VR::DS, "1\\1".to_string()),
            (
                tags::PHOTOMETRIC_INTERPRETATION,
                VR::CS,
                "MONOCHROME2".to_string(),
            ),
            (tags::SEQUENCE_NAME, VR::SH, "*se2d1".to_string()),
            (tags::SCANNING_SEQUENCE, VR::CS, "SE".to_string()),
            (tags::MR_ACQUISITION_TYPE, VR::CS, "2D".to_string()),
            (tags::ECHO_TIME, VR::DS, "10".to_string()),
            (tags::REPETITION_TIME, VR::DS, "500".to_string()),
            (
                tags::IMAGE_TYPE,
                VR::CS,
                "ORIGINAL\\PRIMARY\\M\\ND".to_string(),
            ),
        ] {
            e.push(synth::text(tag, vr, &v));
        }
        e.push(synth::us(tags::SAMPLES_PER_PIXEL, 1));
        e.push(synth::us(tags::ROWS, NY as u16));
        e.push(synth::us(tags::COLUMNS, NX as u16));
        e.push(synth::us(tags::BITS_ALLOCATED, 16));
        e.push(synth::us(tags::BITS_STORED, 16));
        e.push(synth::us(tags::HIGH_BIT, 15));
        e.push(synth::us(tags::PIXEL_REPRESENTATION, 0));
        let mut px = Vec::with_capacity((NY * NX * 2) as usize);
        for y in 0..NY {
            for x in 0..NX {
                px.extend_from_slice(
                    &(((x * 5 + y * 3 + z * 50 + n * 100) % 1000) as u16).to_le_bytes(),
                );
            }
        }
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, px));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
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

/// The series' names and times, which no door of a pair campaign tells.
const NAMES: [&str; 4] = [
    "t1 se tra pre contrast",
    "t1 se tra post contrast",
    "t1 se tra",
    "t2 tse tra",
];
const TIMES: [&str; 5] = ["091500", "093000", "094500", "101500", "103000"];

fn blind(what: &str, doc: &Value, values_too: bool) {
    let mut w = Vec::new();
    words_in(doc, &mut w);
    let text = doc.to_string();
    for leak in NAMES.iter().chain(TIMES.iter()) {
        assert!(!text.contains(leak), "{what} tells {leak:?}: {doc}");
    }
    for leak in [
        "series_description",
        "SeriesDescription",
        "protocol_name",
        "series_time",
        "acquisition_time",
        "header",
        "suggested",
    ] {
        assert!(!w.iter().any(|x| x == leak), "{what} tells {leak}: {doc}");
    }
    if values_too {
        // the rules' post_contrast of either stack
        for leak in ["given", "not_given"] {
            assert!(!w.iter().any(|x| x == leak), "{what} tells {leak}: {doc}");
        }
    }
}

#[test]
fn a_pair_is_read_from_its_two_pictures_alone_and_resolves_per_stack() {
    let home = TempDir::new("pair-home");
    let src = TempDir::new("pair-src");
    // P1: a pre, a post, and a rerun with the pre's very header that came
    // after the contrast; P2: a pair, and a stack no pair shows
    series(&src, "P1", "1.2.3.A", 1, NAMES[0], TIMES[0], NZ);
    series(&src, "P1", "1.2.3.A", 2, NAMES[1], TIMES[1], NZ);
    series(&src, "P1", "1.2.3.A", 3, NAMES[0], TIMES[2], NZ + 1);
    series(&src, "P2", "1.2.3.B", 1, NAMES[2], TIMES[3], NZ);
    series(&src, "P2", "1.2.3.B", 2, NAMES[2], TIMES[4], NZ);
    series(&src, "P2", "1.2.3.B", 3, NAMES[3], "110000", NZ);
    ok(&home, &["key", "add", "k"], Some("a pair test key\n"));
    ok(&home, &["init", "--key", "k"], None);
    ok(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            src.path().to_str().unwrap(),
        ],
        None,
    );
    ok(&home, &["fingerprint"], None);
    ok(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
        None,
    );
    let work = TempDir::new("pair-work");
    ok(
        &home,
        &[
            "place",
            "add",
            "scratch",
            work.path().to_str().unwrap(),
            "--role",
            "working",
        ],
        None,
    );
    for stack in 1..=6 {
        ok(
            &home,
            &["pyramid", "build", "--stack", &stack.to_string()],
            None,
        );
    }

    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        &["query:work", "data:see", "review:work", "campaigns:work"],
        "quasi",
    );
    let anna = token("anna", &["campaigns:see", "campaigns:work"], "quasi");
    let bo = token("bo", &["campaigns:see", "campaigns:work"], "quasi");
    let anna_p = "anna@desk.example";

    // which stack is which, and what the rules read in their words, none
    // of which the reader will ever see
    let mut rules: BTreeMap<&str, Value> = BTreeMap::new();
    let mut of: BTreeMap<&str, i64> = BTreeMap::new();
    let mut p2 = Vec::new();
    for stack in 1..=6i64 {
        let why = server.ok("GET", &format!("/api/stacks/{stack}/why"), None, &cleo);
        let pc = why["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axis"] == "post_contrast")
            .map(|a| a["value"].clone())
            .unwrap_or(Value::Null);
        let name = why["texts"]["series_description"]
            .as_str()
            .unwrap()
            .to_string();
        let deep = why["physics"]["n_instances"].as_i64() == Some(i64::from(NZ + 1));
        let which = match (name.as_str(), deep) {
            (n, false) if n == NAMES[0] => "pre",
            (n, true) if n == NAMES[0] => "rerun",
            (n, _) if n == NAMES[1] => "post",
            (n, _) if n == NAMES[3] => "alone",
            _ => {
                p2.push(stack);
                continue;
            }
        };
        of.insert(which, stack);
        rules.insert(which, pc);
    }
    assert_eq!(p2.len(), 2, "{of:?}");
    assert_eq!(rules["post"], json!("given"), "{rules:?}");
    assert_eq!(rules["pre"], json!("not_given"), "{rules:?}");
    assert_eq!(
        rules["rerun"], rules["pre"],
        "a rerun with the pre's header: {rules:?}"
    );
    let (pre, post, rerun, alone) = (of["pre"], of["post"], of["rerun"], of["alone"]);

    // ------------------------------------------------ making one
    let pairs = TempDir::new("pair-list");
    pairs.file(
        "pairs.tsv",
        format!("left\tright\n{pre}\t{post}\n# the rerun\n{rerun},{post}\n").as_bytes(),
    );
    let file = pairs.path().join("pairs.tsv");
    let p2_pair = format!("{},{}", p2[1], p2[0]);
    let args = |name: &str, extra: &[String]| -> Vec<String> {
        let mut v: Vec<String> = [
            "campaign",
            "pair",
            name,
            "--pairs",
            file.to_str().unwrap(),
            "--pair",
            &p2_pair,
            "--seed",
            "written down on 2026-09-29",
            "--rater",
            anna_p,
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        v.extend(extra.iter().cloned());
        v
    };
    let argv = args("gold-pairs", &["--dry-run".to_string()]);
    let d: Value = serde_json::from_str(&ok(
        &home,
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
    ))
    .unwrap();
    assert_eq!(d["counts"]["pairs"], 3, "{d}");
    assert_eq!(d["counts"]["stacks"], 5, "{d}");
    assert_eq!(d["counts"]["stacks_in_two_or_more_pairs"], 1, "{d}");
    // two people's stacks, one stack twice, a stack not there, a value the
    // axis does not have: refused
    for bad in [
        vec!["--pair".to_string(), format!("{pre},{}", p2[0])],
        vec!["--pair".to_string(), format!("{alone},{alone}")],
        vec!["--pair".to_string(), format!("{pre},99")],
        vec!["--post".to_string(), "maybe".to_string()],
        vec!["--pair".to_string(), format!("{post},{pre}")],
    ] {
        let argv = args("bad", &bad);
        let (good, _, err) = run(
            &home,
            &argv.iter().map(String::as_str).collect::<Vec<_>>(),
            None,
        );
        assert!(!good, "{bad:?} was taken");
        assert!(!err.is_empty(), "{bad:?}");
    }
    let argv = args("gold-pairs", &[]);
    let made: Value = serde_json::from_str(&ok(
        &home,
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
    ))
    .unwrap();
    let id = made["id"].as_i64().unwrap_or_else(|| panic!("{made}"));
    assert_eq!(made["grain"], "pair", "{made}");
    assert_eq!(made["suggest"], "none", "{made}");
    assert_eq!(made["question"]["kind"], "pair", "{made}");
    assert!(made["source"]["pair"]["seed_sha256"].is_string(), "{made}");
    assert!(made["source"]["pair"].get("seed").is_none(), "{made}");
    let items = made["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{made}");
    for it in items {
        assert!(
            it["stack_id"].is_null() && it["subject_id"].is_null() && it["session_day"].is_null(),
            "{it}"
        );
        assert!(it["key"].as_str().unwrap().starts_with("pair:"), "{it}");
    }
    // a pair question is made at the keyboard, from pairs, never at the door
    server.ok(
        "PUT",
        "/api/ask/selections/all",
        Some(selection_of(&[pre, post])),
        &cleo,
    );
    let (st, doc) = server.call(
        "POST",
        "/api/campaigns",
        Some(json!({"name": "door-pair", "question": {"kind": "pair"}, "source": {"selection": "all@1"},
            "closes_into": "none"})),
        &cleo,
    );
    assert_eq!(st, 400, "{doc}");

    // ------------------------------------------------ blind while open
    let seen = server.ok("GET", &format!("/api/campaigns/{id}"), None, &anna);
    blind("the campaign", &seen["items"], true);
    blind("the campaign", &seen, false);
    // a stack no pair shows opens no picture to the rater
    let (st, _) = server.call(
        "GET",
        &format!("/api/instances/{alone}/manifest"),
        None,
        &anna,
    );
    assert_eq!(st, 403);

    let truth = |x: i64| -> bool { x == post || x == rerun };
    let mut read: Vec<(i64, i64, &str)> = Vec::new();
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
        blind("the claim", &cl, true);
        let item = cl["item"]["id"].as_i64().unwrap();
        let a = cl["assignment"]["id"].as_i64().unwrap();
        // bo holds nothing here and reads nothing of the campaign
        let (st, _) = server.call(
            "GET",
            &format!("/api/campaigns/{id}/items/{item}/pair"),
            None,
            &bo,
        );
        assert!(st == 403 || st == 404, "{st}");
        let sheet = server.ok(
            "GET",
            &format!("/api/campaigns/{id}/items/{item}/pair"),
            None,
            &anna,
        );
        blind("the sheet", &sheet, true);
        assert_eq!(
            sheet["answers"],
            json!([
                "left_post",
                "right_post",
                "both_pre",
                "both_post",
                "cant_tell"
            ])
        );
        assert_eq!(sheet["keys"]["1"], "left_post");
        assert_eq!(sheet["keys"]["5"], "cant_tell");
        let (left, right) = (
            sheet["left"]["stack"].as_i64().unwrap(),
            sheet["right"]["stack"].as_i64().unwrap(),
        );
        // the pictures of both, and nothing of their files
        for s in [left, right] {
            let m = server.ok("GET", &format!("/api/instances/{s}/manifest"), None, &anna);
            blind("a manifest", &m, true);
        }
        // every door that would show more refuses
        for door in ["why", "header", "candidates", "ab"] {
            let (st, doc) = server.call(
                "GET",
                &format!("/api/campaigns/{id}/items/{item}/{door}"),
                None,
                &anna,
            );
            assert_eq!(st, 409, "{door}: {doc}");
            blind(door, &doc, true);
        }
        let (st, _) = server.call(
            "POST",
            &format!("/api/campaigns/{id}/items/{item}/derive"),
            Some(json!({"value": {"post_contrast": "given"}})),
            &anna,
        );
        assert_eq!(st, 409);
        for door in [
            "batches",
            "gallery",
            "suggestions",
            "combinations",
            "ab",
            "ab/decisions",
        ] {
            let (st, doc) = server.call("GET", &format!("/api/campaigns/{id}/{door}"), None, &anna);
            assert_eq!(st, 409, "{door}: {doc}");
        }
        // the answer is one of five words
        for bad in [
            json!("given"),
            json!("left"),
            json!({"post_contrast": "given"}),
        ] {
            let (st, doc) = server.call(
                "POST",
                &format!("/api/campaigns/{id}/assignments/{a}/answer"),
                Some(json!({"value": bad})),
                &anna,
            );
            assert_eq!(st, 400, "{bad}: {doc}");
        }
        let said = match p2.contains(&left) {
            true => "cant_tell",
            false => match (truth(left), truth(right)) {
                (true, false) => "left_post",
                (false, true) => "right_post",
                (true, true) => "both_post",
                (false, false) => "both_pre",
            },
        };
        let done = server.ok(
            "POST",
            &format!("/api/campaigns/{id}/assignments/{a}/answer"),
            Some(json!({"value": said})),
            &anna,
        );
        assert!(done["answer"].is_i64(), "{done}");
        read.push((left, right, said));
    }
    assert_eq!(read.len(), 3, "{read:?}");
    // the rerun pair came to both post, whatever the header says
    assert!(read.iter().any(|r| r.2 == "both_post"), "{read:?}");

    // ------------------------------------------------ resolved per stack
    let summary = server.ok("GET", &format!("/api/campaigns/{id}/pair"), None, &anna);
    assert_eq!(summary["items"], 3, "{summary}");
    assert_eq!(summary["answered"], 3, "{summary}");
    assert_eq!(summary["blind"], true, "{summary}");
    assert_eq!(summary["answers"]["both_post"], 1, "{summary}");
    assert_eq!(summary["answers"]["cant_tell"], 1, "{summary}");
    assert!(summary["seconds"]["median"].is_number(), "{summary}");
    let values = server.ok(
        "GET",
        &format!("/api/campaigns/{id}/pair/values"),
        None,
        &anna,
    );
    assert_eq!(values["count"], 6, "{values}");
    let mut by_stack: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for r in values["values"].as_array().unwrap() {
        assert_eq!(r["axis"], "post_contrast", "{r}");
        by_stack
            .entry(r["stack"].as_i64().unwrap())
            .or_default()
            .push(r["value"].as_str().unwrap().to_string());
    }
    assert_eq!(by_stack[&pre], vec!["not_given"], "{by_stack:?}");
    assert_eq!(by_stack[&post], vec!["given", "given"], "{by_stack:?}");
    assert_eq!(by_stack[&rerun], vec!["given"], "{by_stack:?}");
    assert_eq!(by_stack[&p2[0]], vec!["cant_tell"], "{by_stack:?}");
    assert_eq!(by_stack[&p2[1]], vec!["cant_tell"], "{by_stack:?}");
    assert!(!by_stack.contains_key(&alone), "{by_stack:?}");
    // another reader of the campaign reads no one's values but their own
    let (st, other) = server.call(
        "GET",
        &format!("/api/campaigns/{id}/pair/values"),
        None,
        &bo,
    );
    assert!(
        st == 404 || (st == 200 && other["count"] == 0),
        "{st} {other}"
    );

    // ------------------------------------------------ the close keeps them
    let closed = server.ok(
        "POST",
        &format!("/api/campaigns/{id}/close"),
        Some(json!({})),
        &cleo,
    );
    assert_eq!(closed["resolved"], 3, "{closed}");
    let after = server.ok("GET", &format!("/api/campaigns/{id}"), None, &cleo);
    for it in after["items"].as_array().unwrap() {
        let stacks = it["outcome"]["stacks"]
            .as_array()
            .unwrap_or_else(|| panic!("{it}"));
        assert_eq!(stacks.len(), 2, "{it}");
        assert_eq!(stacks[0]["side"], "left");
        assert_eq!(stacks[1]["side"], "right");
    }
    let exported: Value = serde_json::from_str(&ok(
        &home,
        &["campaign", "pair-export", &id.to_string()],
        None,
    ))
    .unwrap();
    assert_eq!(
        exported["values"].as_array().unwrap().len(),
        6,
        "{exported}"
    );
    assert_eq!(exported["summary"]["answered"], 3, "{exported}");
    // a campaign that asks of no pair has no pair doors
    let plain = server.ok(
        "POST",
        "/api/campaigns",
        Some(
            json!({"name": "plain", "question": {"kind": "axis", "axis": "base"},
            "source": {"selection": "all@1"}, "closes_into": "none"}),
        ),
        &cleo,
    );
    let pid = plain["id"].as_i64().unwrap();
    for door in ["pair", "pair/values"] {
        let (st, _) = server.call("GET", &format!("/api/campaigns/{pid}/{door}"), None, &cleo);
        assert_eq!(st, 409, "{door}");
    }
}
