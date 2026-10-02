// SPDX-License-Identifier: AGPL-3.0-only

//! The post-contrast study's anchored reading: a candidate stack is read
//! beside a known-pre and a known-post anchor of the same subject, in
//! panels whose order the seed draws, and nothing else of the three. Two
//! people: the first has a pre, a post and a same-header rerun after the
//! contrast in one session and a stack of another session; the second a
//! pre, a post and a candidate. Three items; a rater reads each from its
//! pictures alone, every door that would show a time, a series name, a
//! header or the rules' value refuses the campaign, an anchor of another
//! session is flagged, and each answer resolves into the candidate's
//! post-contrast value alone, kept on the item's outcome at the close.

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

/// The series' names and times, which no door of an anchored campaign tells.
const NAMES: [&str; 4] = [
    "t1 se tra pre contrast",
    "t1 se tra post contrast",
    "t1 se tra",
    "t2 tse tra",
];
const TIMES: [&str; 8] = [
    "091500", "093000", "094500", "101500", "103000", "104500", "111500", "113000",
];

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

/// Each stack's depth, which tells the test (and nothing the reader sees)
/// which stack is which.
const DEPTHS: [(&str, u32); 8] = [
    ("p1_pre", NZ),
    ("p1_post", NZ + 1),
    ("p1_rerun", NZ + 2),
    ("p1_other", NZ + 3),
    ("p2_pre", NZ + 4),
    ("p2_post", NZ + 5),
    ("p2_cand", NZ + 6),
    ("alone", NZ + 7),
];

/// The Postgres schema the second run takes.
const SCHEMA: &str = "nils_anchored_doors";

fn drop_schema(dsn: &str) {
    let mut store = nils_registry::Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop");
}

#[test]
fn a_candidate_is_read_beside_two_anchors_and_resolves_alone() {
    read(None);
}

#[test]
fn a_candidate_is_read_beside_two_anchors_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    drop_schema(&dsn);
    read(Some(&dsn));
    drop_schema(&dsn);
}

fn read(dsn: Option<&str>) {
    let home = TempDir::new("anchored-home");
    let src = TempDir::new("anchored-src");
    // P1, one session: a pre, a post, and a rerun with the pre's very
    // header after the contrast; another session: one stack
    series(&src, "P1", "1.2.3.A", 1, NAMES[0], TIMES[0], DEPTHS[0].1);
    series(&src, "P1", "1.2.3.A", 2, NAMES[1], TIMES[1], DEPTHS[1].1);
    series(&src, "P1", "1.2.3.A", 3, NAMES[0], TIMES[2], DEPTHS[2].1);
    series(&src, "P1", "1.2.3.C", 1, NAMES[2], TIMES[3], DEPTHS[3].1);
    // P2: a pre, a post, a candidate, and a stack no item shows
    series(&src, "P2", "1.2.3.B", 1, NAMES[0], TIMES[4], DEPTHS[4].1);
    series(&src, "P2", "1.2.3.B", 2, NAMES[1], TIMES[5], DEPTHS[5].1);
    series(&src, "P2", "1.2.3.B", 3, NAMES[2], TIMES[6], DEPTHS[6].1);
    series(&src, "P2", "1.2.3.B", 4, NAMES[3], TIMES[7], DEPTHS[7].1);
    ok(&home, &["key", "add", "k"], Some("an anchored test key\n"));
    match dsn {
        Some(dsn) => {
            ok(
                &home,
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
        None => {
            ok(&home, &["init", "--key", "k"], None);
        }
    }
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
    let work = TempDir::new("anchored-work");
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
    for stack in 1..=8 {
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

    // which stack is which, by its depth, and what the rules read
    let mut of: BTreeMap<&str, i64> = BTreeMap::new();
    let mut rules: BTreeMap<&str, Value> = BTreeMap::new();
    for stack in 1..=8i64 {
        let why = server.ok("GET", &format!("/api/stacks/{stack}/why"), None, &cleo);
        let n = why["physics"]["n_instances"].as_i64().unwrap();
        let which = DEPTHS
            .iter()
            .find(|(_, d)| i64::from(*d) == n)
            .unwrap_or_else(|| panic!("depth {n}"))
            .0;
        let pc = why["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axis"] == "post_contrast")
            .map(|a| a["value"].clone())
            .unwrap_or(Value::Null);
        of.insert(which, stack);
        rules.insert(which, pc);
    }
    assert_eq!(of.len(), 8, "{of:?}");
    assert_eq!(rules["p1_pre"], json!("not_given"), "{rules:?}");
    assert_eq!(rules["p1_post"], json!("given"), "{rules:?}");
    assert_eq!(
        rules["p1_rerun"], rules["p1_pre"],
        "a rerun with the pre's header: {rules:?}"
    );

    // ------------------------------------------------ making one
    let list = TempDir::new("anchored-list");
    // the round's own columns, in another order, with a note
    list.file(
        "items.tsv",
        format!(
            "subject\tpost_anchor\tcandidate\tpre_anchor\tnote\n\
             # the rerun\n\
             P1\t{}\t{}\t{}\tsame header as the pre\n\
             P1\t{}\t{}\t{}\tanother session\n",
            of["p1_post"],
            of["p1_rerun"],
            of["p1_pre"],
            of["p1_post"],
            of["p1_other"],
            of["p1_pre"],
        )
        .as_bytes(),
    );
    let file = list.path().join("items.tsv");
    let p2_item = format!("{},{},{}", of["p2_cand"], of["p2_pre"], of["p2_post"]);
    let args = |name: &str, extra: &[String]| -> Vec<String> {
        let mut v: Vec<String> = [
            "campaign",
            "anchored",
            name,
            "--items",
            file.to_str().unwrap(),
            "--item",
            &p2_item,
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
    let argv = args("gold-anchored", &["--dry-run".to_string()]);
    let d: Value = serde_json::from_str(&ok(
        &home,
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
    ))
    .unwrap();
    assert_eq!(d["counts"]["items"], 3, "{d}");
    assert_eq!(d["counts"]["stacks"], 7, "{d}");
    assert_eq!(d["counts"]["anchors"], 4, "{d}");
    assert_eq!(
        d["counts"]["items_with_an_anchor_of_another_session"], 1,
        "{d}"
    );
    assert_eq!(d["counts"]["pre_anchors_of_another_session"], 1, "{d}");
    assert_eq!(d["counts"]["post_anchors_of_another_session"], 1, "{d}");
    assert_eq!(d["counts"]["candidates_that_are_also_anchors"], 0, "{d}");
    // two people's stacks, one stack twice, a candidate twice, a stack not
    // there, a value the axis does not have: refused
    let item = |c: &str, p: &str, q: &str| vec!["--item".to_string(), format!("{c},{p},{q}")];
    let (pre, post, alone) = (
        of["p1_pre"].to_string(),
        of["p1_post"].to_string(),
        of["alone"].to_string(),
    );
    for bad in [
        item(&of["p2_pre"].to_string(), &pre, &post),
        item(&alone, &of["p2_pre"].to_string(), &of["p2_pre"].to_string()),
        item(&of["p1_rerun"].to_string(), &pre, &post),
        item(&alone, &of["p2_pre"].to_string(), "99"),
        vec!["--post".to_string(), "maybe".to_string()],
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
    let argv = args("gold-anchored", &[]);
    let made: Value = serde_json::from_str(&ok(
        &home,
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
    ))
    .unwrap();
    let id = made["id"].as_i64().unwrap_or_else(|| panic!("{made}"));
    assert_eq!(made["grain"], "anchored", "{made}");
    assert_eq!(made["suggest"], "none", "{made}");
    assert_eq!(made["question"]["kind"], "anchored", "{made}");
    assert_eq!(
        made["question"]["answers"],
        json!(["like_pre", "like_post", "cant_tell"]),
        "{made}"
    );
    assert!(
        made["source"]["anchored"]["seed_sha256"].is_string(),
        "{made}"
    );
    assert!(made["source"]["anchored"].get("seed").is_none(), "{made}");
    let items = made["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{made}");
    for it in items {
        assert!(
            it["stack_id"].is_null() && it["subject_id"].is_null() && it["session_day"].is_null(),
            "{it}"
        );
        assert!(it["key"].as_str().unwrap().starts_with("anchored:"), "{it}");
    }
    // an anchored question is made at the keyboard, never at the door
    server.ok(
        "PUT",
        "/api/ask/selections/all",
        Some(selection_of(&[of["p1_pre"], of["p1_post"]])),
        &cleo,
    );
    let (st, doc) = server.call(
        "POST",
        "/api/campaigns",
        Some(
            json!({"name": "door-anchored", "question": {"kind": "anchored"},
            "source": {"selection": "all@1"}, "closes_into": "none"}),
        ),
        &cleo,
    );
    assert_eq!(st, 400, "{doc}");

    // ------------------------------------------------ blind while open
    let seen = server.ok("GET", &format!("/api/campaigns/{id}"), None, &anna);
    blind("the campaign", &seen["items"], true);
    blind("the campaign", &seen, false);
    let (st, _) = server.call(
        "GET",
        &format!("/api/instances/{}/manifest", of["alone"]),
        None,
        &anna,
    );
    assert_eq!(st, 403);

    let mut read: Vec<(i64, &str)> = Vec::new();
    let mut candidate_places = Vec::new();
    // what the claim before said comes next: the item and its panels' stacks
    let mut promised: Option<(i64, Vec<i64>)> = None;
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
        let (st, _) = server.call(
            "GET",
            &format!("/api/campaigns/{id}/items/{item}/anchored"),
            None,
            &bo,
        );
        assert!(st == 403 || st == 404, "{st}");
        let sheet = server.ok(
            "GET",
            &format!("/api/campaigns/{id}/items/{item}/anchored"),
            None,
            &anna,
        );
        blind("the sheet", &sheet, true);
        assert_eq!(
            sheet["answers"],
            json!(["like_pre", "like_post", "cant_tell"])
        );
        assert_eq!(sheet["keys"]["1"], "like_pre");
        assert_eq!(sheet["keys"]["3"], "cant_tell");
        let panels = sheet["panels"].as_array().unwrap();
        assert_eq!(panels.len(), 3, "{sheet}");
        let mut labels = Vec::new();
        let mut by_role: BTreeMap<String, (i64, bool)> = BTreeMap::new();
        for (i, p) in panels.iter().enumerate() {
            assert_eq!(p["panel"], i as i64, "{sheet}");
            // each panel says what it is and nothing more
            let mut keys: Vec<&String> = p.as_object().unwrap().keys().collect();
            keys.sort();
            assert_eq!(
                keys,
                ["label", "other_session", "panel", "role", "stack"],
                "{p}"
            );
            labels.push(p["label"].as_str().unwrap().to_string());
            by_role.insert(
                p["role"].as_str().unwrap().to_string(),
                (
                    p["stack"].as_i64().unwrap(),
                    p["other_session"].as_bool().unwrap(),
                ),
            );
            if p["role"] == "candidate" {
                candidate_places.push(i);
            }
        }
        labels.sort();
        assert_eq!(
            labels,
            ["candidate", "reference post", "reference pre"],
            "{sheet}"
        );
        // the claim names what comes next, the stacks in the panels' order
        // and nothing else of them; the claim before named this one
        let shown: Vec<i64> = panels
            .iter()
            .map(|p| p["stack"].as_i64().unwrap())
            .collect();
        if let Some((was, stacks)) = promised.take() {
            assert_eq!(was, item, "{cl}");
            assert_eq!(stacks, shown, "{cl}");
        }
        let ahead = cl["ahead"].as_array().unwrap();
        assert_eq!(ahead.len(), 2 - read.len(), "{cl}");
        for a in ahead {
            let mut keys: Vec<&String> = a.as_object().unwrap().keys().collect();
            keys.sort();
            assert_eq!(keys, ["item", "position", "stacks"], "{a}");
            assert_eq!(a["stacks"].as_array().unwrap().len(), 3, "{a}");
        }
        assert_eq!(cl["next"]["item"], cl["ahead"][0]["item"], "{cl}");
        if let Some(a) = ahead.first() {
            promised = Some((
                a["item"].as_i64().unwrap(),
                a["stacks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_i64().unwrap())
                    .collect(),
            ));
        }
        let cand = by_role["candidate"].0;
        let (pre_anchor, pre_other) = by_role["reference_pre"];
        let (post_anchor, post_other) = by_role["reference_post"];
        assert!(!by_role["candidate"].1, "{sheet}");
        // an anchor of another session is flagged, the others are not
        let other_session = cand == of["p1_other"];
        assert_eq!(pre_other, other_session, "{sheet}");
        assert_eq!(post_other, other_session, "{sheet}");
        // the three pictures open, and nothing of their files
        for s in [cand, pre_anchor, post_anchor] {
            let m = server.ok("GET", &format!("/api/instances/{s}/manifest"), None, &anna);
            blind("a manifest", &m, true);
        }
        // every door that would show more refuses
        for door in ["why", "header", "candidates", "ab", "pair"] {
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
            "pair",
            "pair/values",
        ] {
            let (st, doc) = server.call("GET", &format!("/api/campaigns/{id}/{door}"), None, &anna);
            assert_eq!(st, 409, "{door}: {doc}");
            blind(door, &doc, true);
        }
        // the answer is one of three words
        for bad in [
            json!("given"),
            json!("left_post"),
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
        let said = if cand == of["p1_rerun"] {
            "like_post"
        } else if cand == of["p2_cand"] {
            "like_pre"
        } else {
            "cant_tell"
        };
        let done = server.ok(
            "POST",
            &format!("/api/campaigns/{id}/assignments/{a}/answer"),
            Some(json!({"value": said})),
            &anna,
        );
        assert!(done["answer"].is_i64(), "{done}");
        read.push((cand, said));
    }
    assert_eq!(read.len(), 3, "{read:?}");

    // ------------------------------------------------ resolved per candidate
    let summary = server.ok("GET", &format!("/api/campaigns/{id}/anchored"), None, &anna);
    assert_eq!(summary["items"], 3, "{summary}");
    assert_eq!(summary["answered"], 3, "{summary}");
    assert_eq!(summary["blind"], true, "{summary}");
    assert_eq!(summary["answers"]["like_post"], 1, "{summary}");
    assert_eq!(summary["answers"]["like_pre"], 1, "{summary}");
    assert_eq!(summary["answers"]["cant_tell"], 1, "{summary}");
    assert_eq!(
        summary["items_with_an_anchor_of_another_session"], 1,
        "{summary}"
    );
    let values = server.ok(
        "GET",
        &format!("/api/campaigns/{id}/anchored/values"),
        None,
        &anna,
    );
    assert_eq!(values["count"], 3, "{values}");
    let mut by_stack: BTreeMap<i64, String> = BTreeMap::new();
    for r in values["values"].as_array().unwrap() {
        assert_eq!(r["axis"], "post_contrast", "{r}");
        by_stack.insert(
            r["stack"].as_i64().unwrap(),
            r["value"].as_str().unwrap().to_string(),
        );
        if r["stack"] == of["p1_other"] {
            assert_eq!(r["pre_other_session"], true, "{r}");
            assert_eq!(r["post_other_session"], true, "{r}");
        }
    }
    // the rerun, which the rules read as not given by its header, is given
    assert_eq!(by_stack[&of["p1_rerun"]], "given", "{by_stack:?}");
    assert_eq!(by_stack[&of["p2_cand"]], "not_given", "{by_stack:?}");
    assert_eq!(by_stack[&of["p1_other"]], "cant_tell", "{by_stack:?}");
    // the anchors are never labelled by an answer
    for anchor in ["p1_pre", "p1_post", "p2_pre", "p2_post", "alone"] {
        assert!(
            !by_stack.contains_key(&of[anchor]),
            "{anchor}: {by_stack:?}"
        );
    }
    let (st, other) = server.call(
        "GET",
        &format!("/api/campaigns/{id}/anchored/values"),
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
        assert_eq!(stacks.len(), 1, "{it}");
        assert_eq!(stacks[0]["role"], "candidate", "{it}");
    }
    let exported: Value = serde_json::from_str(&ok(
        &home,
        &["campaign", "anchored-export", &id.to_string()],
        None,
    ))
    .unwrap();
    assert_eq!(
        exported["values"].as_array().unwrap().len(),
        3,
        "{exported}"
    );
    assert_eq!(exported["summary"]["answered"], 3, "{exported}");
    assert!(exported["seed_sha256"].is_string(), "{exported}");
    // once closed, the pictures are no longer read through it
    let (st, _) = server.call(
        "GET",
        &format!("/api/instances/{}/manifest", of["p2_cand"]),
        None,
        &anna,
    );
    assert_eq!(st, 403);
    // a campaign that asks of no anchored item has no anchored doors
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
    for door in ["anchored", "anchored/values"] {
        let (st, _) = server.call("GET", &format!("/api/campaigns/{pid}/{door}"), None, &cleo);
        assert_eq!(st, 409, "{door}");
    }
    // the seed moved the candidate about the panels
    assert_eq!(candidate_places.len(), 3);
}
