// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 H3, nothing typed by hand: the create door draws a pair, an
//! anchored and an A/B campaign's items from saved selections, and its
//! preview answers the counts and makes nothing. Two people: P1 holds one
//! session with a pre, a post and a candidate, and a second session months
//! later with one stack; P2 holds a session of two stacks and a later one
//! of one. Pairs are drawn within a session, an anchor is its session's
//! nearest or else the subject's nearest session's, and the A/B voters ride
//! in the body beside the rules, which vote on a sealed stack only for a
//! holder of sealed:see. Each made campaign is read through its reader's
//! own door.

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
        let child = nils()
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
#[allow(clippy::too_many_arguments)]
fn series(
    dir: &TempDir,
    patient: &str,
    study: &str,
    date: &str,
    n: u32,
    name: &str,
    time: &str,
    nz: u32,
) {
    let series = format!("{study}.{n}");
    for z in 0..nz {
        let sop = format!("{series}.{}", z + 1);
        let mut e = synth::minimal_mr(study, &series, &sop);
        for (tag, vr, v) in [
            (tags::PATIENT_ID, VR::LO, patient.to_string()),
            (tags::STUDY_DATE, VR::DA, date.to_string()),
            (tags::SERIES_DATE, VR::DA, date.to_string()),
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

/// The Postgres schema the second run takes.
const SCHEMA: &str = "nils_drawn_doors";

fn drop_schema(dsn: &str) {
    let mut store = nils_registry::Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop");
}

/// Each stack's depth names it.
const DEPTHS: [(&str, u32); 7] = [
    ("pre", 4),
    ("post", 5),
    ("candidate", 6),
    ("later", 7),
    ("b1", 8),
    ("b2", 9),
    ("alone", 10),
];

#[test]
fn the_create_door_draws_pairs_anchors_and_ab_items() {
    draw(None);
}

#[test]
fn the_create_door_draws_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    drop_schema(&dsn);
    draw(Some(&dsn));
    drop_schema(&dsn);
}

fn draw(dsn: Option<&str>) {
    let home = TempDir::new("drawn-home");
    let src = TempDir::new("drawn-src");
    let depth = |w: &str| DEPTHS.iter().find(|(n, _)| *n == w).unwrap().1;
    series(
        &src,
        "P1",
        "1.2.3.A",
        "20200101",
        1,
        "t1 pre",
        "090000",
        depth("pre"),
    );
    series(
        &src,
        "P1",
        "1.2.3.A",
        "20200101",
        2,
        "t1 post",
        "091000",
        depth("post"),
    );
    series(
        &src,
        "P1",
        "1.2.3.A",
        "20200101",
        3,
        "t1 late",
        "092000",
        depth("candidate"),
    );
    series(
        &src,
        "P1",
        "1.2.3.C",
        "20200801",
        1,
        "t1 post",
        "090000",
        depth("later"),
    );
    series(
        &src,
        "P2",
        "1.2.3.B",
        "20210101",
        1,
        "t2 a",
        "090000",
        depth("b1"),
    );
    series(
        &src,
        "P2",
        "1.2.3.B",
        "20210101",
        2,
        "t2 b",
        "091000",
        depth("b2"),
    );
    series(
        &src,
        "P2",
        "1.2.3.D",
        "20220101",
        1,
        "t2 c",
        "090000",
        depth("alone"),
    );
    ok(&home, &["key", "add", "k"], Some("a drawn test key\n"));
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
    ok(&home, &["session", "rebuild"], None);

    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        &["query:work", "data:see", "review:work", "campaigns:work"],
        "quasi",
    );
    let sealer = token(
        "sam",
        &[
            "query:work",
            "data:see",
            "review:work",
            "campaigns:work",
            "sealed:see",
        ],
        "quasi",
    );
    let caps = server.ok("GET", "/api/capabilities", None, &cleo);
    assert_eq!(
        caps["campaigns"]["draws"],
        json!(["pair", "anchored", "ab"]),
        "{}",
        caps["campaigns"]
    );

    // which stack is which, by its depth, and what the rules said of it
    let mut of: BTreeMap<&str, i64> = BTreeMap::new();
    let mut rules: BTreeMap<i64, Value> = BTreeMap::new();
    for stack in 1..=7i64 {
        let why = server.ok("GET", &format!("/api/stacks/{stack}/why"), None, &cleo);
        let n = why["physics"]["n_instances"].as_i64().unwrap();
        let which = DEPTHS
            .iter()
            .find(|(_, d)| i64::from(*d) == n)
            .unwrap_or_else(|| panic!("depth {n}"))
            .0;
        of.insert(which, stack);
        rules.insert(stack, why["axes"].clone());
    }
    let ids = |names: &[&str]| -> Vec<i64> { names.iter().map(|n| of[n]).collect() };
    let save = |name: &str, names: &[&str]| {
        server.ok(
            "PUT",
            &format!("/api/ask/selections/{name}"),
            Some(selection_of(&ids(names))),
            &cleo,
        );
    };
    save(
        "all",
        &["pre", "post", "candidate", "later", "b1", "b2", "alone"],
    );
    save("pre", &["pre"]);
    save("others", &["post", "candidate", "b1"]);
    save("apart", &["later", "alone"]);
    save("candidates", &["candidate", "b2"]);
    save("pres", &["candidate", "pre", "b1"]);
    save("posts", &["later"]);
    let campaigns = |server: &Server| {
        server.ok("GET", "/api/campaigns", None, &cleo)["count"]
            .as_i64()
            .unwrap()
    };

    // ------------------------------------------------ pairs
    let body = |name: &str, source: Value| json!({"name": name, "question": {"kind": "pair"}, "source": {"pair": source}});
    let preview = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(body("", json!({"selection": "all", "seed": "s1"}))),
        &cleo,
    );
    assert_eq!(preview["kind"], "pair", "{preview}");
    assert_eq!(preview["counts"]["pairs"], 4, "{preview}");
    assert_eq!(preview["counts"]["left_out"], 2, "{preview}");
    assert_eq!(preview["dry_run"], true, "{preview}");
    assert_eq!(campaigns(&server), 0, "a preview makes nothing");
    let (st, made) = server.call(
        "POST",
        "/api/campaigns",
        Some(body("pairs-1", json!({"selection": "all", "seed": "s1"}))),
        &cleo,
    );
    assert_eq!(st, 201, "{made}");
    assert_eq!(made["pair"]["counts"]["pairs"], 4, "{made}");
    assert_eq!(made["question"]["kind"], "pair", "{made}");
    let id = made["id"].as_i64().unwrap();
    let shown = server.ok("GET", &format!("/api/campaigns/{id}"), None, &cleo);
    let item = shown["items"][0]["id"].as_i64().unwrap();
    let sheet = server.ok(
        "GET",
        &format!("/api/campaigns/{id}/items/{item}/pair"),
        None,
        &cleo,
    );
    assert!(sheet.to_string().contains("stack"), "{sheet}");
    // with a second selection: the pre beside each of the others in its session
    let p = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(body(
            "",
            json!({"selection": "pre", "with": "others", "seed": "s1"}),
        )),
        &cleo,
    );
    assert_eq!(p["counts"]["pairs"], 2, "{p}");
    assert_eq!(p["counts"]["left_out"], 1, "{p}");
    // no two of one session: refused at the door, said by the preview
    let p = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(body("", json!({"selection": "apart", "seed": "s1"}))),
        &cleo,
    );
    assert_eq!(p["counts"]["pairs"], 0, "{p}");
    assert!(p["refusal"].is_string(), "{p}");
    let (st, r) = server.call(
        "POST",
        "/api/campaigns",
        Some(body("pairs-2", json!({"selection": "apart", "seed": "s1"}))),
        &cleo,
    );
    assert_eq!(st, 400, "{r}");
    assert_eq!(r["counts"]["left_out"], 2, "{r}");
    let (st, r) = server.call(
        "POST",
        "/api/campaigns",
        Some(body(
            "pairs-3",
            json!({"selection": "nothing-saved", "seed": "s1"}),
        )),
        &cleo,
    );
    assert_eq!(st, 404, "{r}");
    let (st, r) = server.call(
        "POST",
        "/api/campaigns",
        Some(json!({"name": "pairs-4", "question": {"kind": "axis", "axis": "base"}, "source": {"pair": {"selection": "all", "seed": "s"}}})),
        &cleo,
    );
    assert_eq!(st, 400, "a pair source asks a pair question: {r}");

    // ------------------------------------------------ anchored
    let anchored = |name: &str| {
        json!({"name": name, "question": {"kind": "anchored"},
            "source": {"anchored": {"selection": "candidates", "pre": "pres", "post": "posts", "seed": "s2"}}})
    };
    let p = server.ok("POST", "/api/campaigns/preview", Some(anchored("")), &cleo);
    assert_eq!(p["counts"]["items"], 1, "{p}");
    assert_eq!(p["counts"]["left_out"], 1, "{p}");
    assert_eq!(p["counts"]["post_anchors_of_another_session"], 1, "{p}");
    assert_eq!(p["counts"]["pre_anchors_of_another_session"], 0, "{p}");
    let (st, made) = server.call(
        "POST",
        "/api/campaigns",
        Some(anchored("anchored-1")),
        &cleo,
    );
    assert_eq!(st, 201, "{made}");
    let id = made["id"].as_i64().unwrap();
    let shown = server.ok("GET", &format!("/api/campaigns/{id}"), None, &cleo);
    let item = shown["items"][0]["id"].as_i64().unwrap();
    let sheet = server.ok(
        "GET",
        &format!("/api/campaigns/{id}/items/{item}/anchored"),
        None,
        &cleo,
    );
    let text = sheet.to_string();
    // the candidate's own session's pre, never the candidate as its own anchor
    assert!(text.contains(&of["pre"].to_string()), "{sheet}");
    assert!(text.contains(&of["later"].to_string()), "{sheet}");

    // ------------------------------------------------ A/B
    let value_of = |stack: i64, axis: &str| -> Value {
        rules[&stack]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axis"] == axis)
            .map(|a| a["value"].clone())
            .unwrap_or(Value::Null)
    };
    // a judge who agrees with the rules but on the candidate's base
    let mut readings = serde_json::Map::new();
    for stack in 1..=7i64 {
        let mut base = value_of(stack, "base");
        if stack == of["candidate"] {
            base = json!(if base == json!("T2w") { "T1w" } else { "T2w" });
        }
        readings.insert(
            stack.to_string(),
            json!({"provenance": {"value": value_of(stack, "provenance"), "reason": "the header"},
                   "base": {"value": base, "reason": "the picture"}}),
        );
    }
    let ab = |name: &str, rules: bool, voters: Value| {
        json!({"name": name, "question": {"kind": "axes", "axes": ["provenance", "base"]},
            "source": {"ab": {"selection": "all", "rules": rules, "voters": voters, "seed": "s3",
                "localizer_asks": ["provenance"]}}})
    };
    let (st, r) = server.call(
        "POST",
        "/api/campaigns",
        Some(ab("ab-0", false, json!({}))),
        &cleo,
    );
    assert_eq!(st, 400, "an A/B campaign needs a voter: {r}");
    let p = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(ab("", true, json!({"judge": readings}))),
        &cleo,
    );
    assert_eq!(p["kind"], "ab", "{p}");
    assert_eq!(p["voters"]["judge"]["files"], 7, "{p}");
    assert_eq!(p["voters"]["rules"]["files"], 7, "{p}");
    assert!(p["counts"]["items"].as_i64().unwrap() >= 1, "{p}");
    let (st, made) = server.call(
        "POST",
        "/api/campaigns",
        Some(ab("ab-1", true, json!({"judge": readings}))),
        &cleo,
    );
    assert_eq!(st, 201, "{made}");
    let id = made["id"].as_i64().unwrap();
    let shown = server.ok("GET", &format!("/api/campaigns/{id}"), None, &cleo);
    let item = shown["items"][0]["id"].as_i64().unwrap();
    server.ok(
        "GET",
        &format!("/api/campaigns/{id}/items/{item}/ab"),
        None,
        &cleo,
    );
    // the rules on a sealed stack vote only for a holder of sealed:see
    let handle = shown["handle_id"].as_i64().unwrap();
    ok(
        &home,
        &["labels", "seal", "--handle", &handle.to_string(), "--json"],
        None,
    );
    let (st, r) = server.call(
        "POST",
        "/api/campaigns/preview",
        Some(ab("", true, json!({"judge": readings}))),
        &cleo,
    );
    assert_eq!(st, 403, "{r}");
    assert_eq!(r["sealed"], true, "{r}");
    let p = server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(ab("", true, json!({"judge": readings}))),
        &sealer,
    );
    assert_eq!(p["voters"]["rules"]["files"], 7, "{p}");
    // the readings alone need no grant
    server.ok(
        "POST",
        "/api/campaigns/preview",
        Some(ab("", false, json!({"judge": readings}))),
        &cleo,
    );
    let audit = ok(
        &home,
        &["audit", "list", "--action", "sealed.read", "--json"],
        None,
    );
    assert!(
        audit.contains("POST /api/campaigns"),
        "the sealed read is audited: {audit}"
    );
}
