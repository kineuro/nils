// SPDX-License-Identifier: AGPL-3.0-only

//! Record 48, found when the reader was set up for Phase 0: a rater's
//! account holds the campaign grants and nothing else, and through them
//! reads the pictures of the stacks of their own open campaigns and nothing
//! more; raters are blind to each other's campaigns and answers; and one
//! file without the Part 10 meta group no longer fails its stack's pyramid.

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

/// A series of NZ small planes; with `bare`, its first file is written as a
/// bare data set, without the preamble, the `DICM` marker or the meta group.
fn series(dir: &TempDir, patient: &str, study: &str, series: &str, bare: bool) {
    for z in 0..NZ {
        let sop = format!("{series}.{}", z + 1);
        let mut e = synth::minimal_mr(study, series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        e.push(synth::text(
            tags::INSTANCE_NUMBER,
            VR::IS,
            &(z + 1).to_string(),
        ));
        e.push(synth::text(
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            &format!("0\\0\\{}", z as f64 * 2.0),
        ));
        e.push(synth::text(
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            "1\\0\\0\\0\\1\\0",
        ));
        e.push(synth::text(tags::PIXEL_SPACING, VR::DS, "1\\1"));
        e.push(synth::us(tags::SAMPLES_PER_PIXEL, 1));
        e.push(synth::text(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            "MONOCHROME2",
        ));
        e.push(synth::us(tags::ROWS, NY as u16));
        e.push(synth::us(tags::COLUMNS, NX as u16));
        e.push(synth::us(tags::BITS_ALLOCATED, 16));
        e.push(synth::us(tags::BITS_STORED, 16));
        e.push(synth::us(tags::HIGH_BIT, 15));
        e.push(synth::us(tags::PIXEL_REPRESENTATION, 0));
        let mut px = Vec::with_capacity((NY * NX * 2) as usize);
        for y in 0..NY {
            for x in 0..NX {
                px.extend_from_slice(&(((x * 5 + y * 3 + z * 50) % 1000) as u16).to_le_bytes());
            }
        }
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, px));
        let bytes = if bare && z == 0 {
            synth::bare(&e, true)
        } else {
            synth::part10(&MetaFields::mr(&sop), &e, true)
        };
        dir.file(&format!("{study}/{sop}"), &bytes);
    }
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

fn campaign(name: &str, raters: &[&str]) -> Value {
    json!({
        "name": name,
        "question": {"kind": "axis", "axis": "base"},
        "source": {"selection": "two@1"},
        "raters": raters,
        "raters_per_item": 1,
        "adjudication": {"when": "never"},
        "closes_into": "none",
    })
}

/// Claim one item of a campaign and answer it: the item's id.
fn answer_one(server: &Server, name: &str, token: &str) -> i64 {
    let claimed = server.ok(
        "POST",
        &format!("/api/campaigns/{name}/claim"),
        Some(json!({})),
        token,
    );
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    let item = claimed["item"]["id"].as_i64().unwrap();
    server.ok(
        "POST",
        &format!("/api/campaigns/{name}/assignments/{assignment}/answer"),
        Some(json!({"value": "T1w"})),
        token,
    );
    item
}

#[test]
fn a_rater_reads_the_pictures_of_their_own_campaign_and_nothing_else() {
    let home = TempDir::new("raters-home");
    let src = TempDir::new("raters-src");
    series(&src, "P1", "1.2.3.A", "1.2.3.A.1", false);
    series(&src, "P2", "1.2.3.B", "1.2.3.B.1", true);
    series(&src, "P3", "1.2.3.C", "1.2.3.C.1", false);
    ok(&home, &["key", "add", "k"], Some("a raters test key\n"));
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
    let work = TempDir::new("raters-work");
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
    // every stack builds, the one holding a bare file among them, and every
    // plane of it is there
    for stack in 1..=3 {
        ok(
            &home,
            &["pyramid", "build", "--stack", &stack.to_string()],
            None,
        );
        let m: Value = serde_json::from_str(
            &std::fs::read_to_string(
                work.path()
                    .join("pyramids")
                    .join(stack.to_string())
                    .join("manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(m["shape"], json!([NZ, NY, NX]), "stack {stack}: {m}");
    }

    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        &[
            "query:work",
            "data:see",
            "review:work",
            "campaigns:work",
            "audit:see",
        ],
        "quasi",
    );
    let anna = token("anna", &["campaigns:work"], "quasi");
    let bo = token("bo", &["campaigns:work"], "quasi");
    let pia = token("pia", &["campaigns:work"], "plain");
    let (anna_p, bo_p, pia_p) = ("anna@desk.example", "bo@desk.example", "pia@desk.example");

    // two stacks of three, sealed, read by anna in hers and by bo in his, and
    // by both in a shared one
    server.ok(
        "PUT",
        "/api/ask/selections/two",
        Some(selection_of(&[1, 2])),
        &cleo,
    );
    let hers = server.ok(
        "POST",
        "/api/campaigns",
        Some(campaign("read-anna", &[anna_p, pia_p])),
        &cleo,
    );
    server.ok(
        "POST",
        "/api/campaigns",
        Some(campaign("read-bo", &[bo_p])),
        &cleo,
    );
    server.ok(
        "POST",
        "/api/campaigns",
        Some(campaign("shared", &[anna_p, bo_p])),
        &cleo,
    );
    let handle = hers["handle_id"].as_i64().unwrap();
    ok(
        &home,
        &["labels", "seal", "--handle", &handle.to_string(), "--json"],
        None,
    );

    // ------------------------------------------------ pictures
    for door in ["manifest", "tiles/0/0", "slab/0/0-2", "render/0/1?axis=y"] {
        let (status, doc) = server.call("GET", &format!("/api/instances/1/{door}"), None, &anna);
        assert_eq!(status, 200, "{door}: {doc}");
    }
    // a stack outside her campaigns, or no stack at all, is refused alike,
    // before anything of it is looked up
    for stack in ["3", "999"] {
        let (status, doc) = server.call(
            "GET",
            &format!("/api/instances/{stack}/manifest"),
            None,
            &anna,
        );
        assert_eq!(status, 403, "{stack}: {doc}");
        assert!(
            doc["error"].as_str().unwrap().contains("query:see"),
            "{doc}"
        );
    }
    // a rater at detail plain is held at the pixels' class
    let (status, doc) = server.call("GET", "/api/instances/1/manifest", None, &pia);
    assert_eq!(status, 403, "{doc}");
    assert_eq!(doc["disclosure"], "gated", "{doc}");
    // the audit row names the campaign the pictures were read through
    let audit = server.ok(
        "GET",
        "/api/audit?action=instance.open&limit=50",
        None,
        &cleo,
    );
    let rows: Vec<&Value> = audit["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["principal"] == anna_p)
        .collect();
    assert_eq!(rows.len(), 1, "{audit}");
    assert_eq!(rows[0]["scope"]["stack"], 1, "{audit}");
    assert!(rows[0]["scope"]["campaign"].is_i64(), "{audit}");

    // nothing else of the stack opens: no ask, no explanation, no evidence
    // line outside the campaign, no decisions as a label set, no campaign
    // over a handle or the review queue
    let (status, doc) = server.call(
        "POST",
        "/api/ask/run",
        Some(json!({"document": selection_of(&[1])["document"]})),
        &anna,
    );
    assert_eq!(status, 403, "{doc}");
    assert!(
        doc["error"].as_str().unwrap().contains("query:see"),
        "{doc}"
    );
    for door in ["/api/stacks/1/why", "/api/explain/1", "/api/review"] {
        let (status, doc) = server.call("GET", door, None, &anna);
        assert_eq!(status, 403, "{door}: {doc}");
    }
    let (status, doc) = server.call(
        "POST",
        "/api/label-sets",
        Some(json!({"axis": "base"})),
        &anna,
    );
    assert_eq!(status, 403, "{doc}");
    for source in [
        json!({"selection": "two@1"}),
        json!({"handle": handle}),
        json!({"review": {"kind_prefix": ""}}),
    ] {
        let mut body = campaign("mine", &[anna_p]);
        body["source"] = source;
        let (status, doc) = server.call("POST", "/api/campaigns", Some(body), &anna);
        assert_eq!(status, 403, "{doc}");
    }

    // a sealed item is still read blind
    let claimed = server.ok(
        "POST",
        "/api/campaigns/read-anna/claim",
        Some(json!({})),
        &anna,
    );
    let item = claimed["item"]["id"].as_i64().unwrap();
    let why = server.ok(
        "GET",
        &format!("/api/campaigns/read-anna/items/{item}/why"),
        None,
        &anna,
    );
    assert_eq!(why["blind"], true, "{why}");
    assert!(why["suggested"].is_null(), "{why}");
    for key in ["axes", "line", "set_by", "voted", "decided"] {
        assert!(why.get(key).is_none(), "{key}: {why}");
    }
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    server.ok(
        "POST",
        &format!("/api/campaigns/read-anna/assignments/{assignment}/release"),
        Some(json!({})),
        &anna,
    );

    // ------------------------------------------------ blind to each other
    answer_one(&server, "read-anna", &anna);
    let shared_item = answer_one(&server, "shared", &anna);
    // bo lists his own campaigns, and another's is as if it were not there
    let listed = server.ok("GET", "/api/campaigns", None, &bo);
    let names: Vec<&str> = listed["campaigns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{listed}");
    assert!(
        names.contains(&"read-bo") && names.contains(&"shared"),
        "{names:?}"
    );
    for (method, door) in [
        ("GET", "/api/campaigns/read-anna"),
        ("GET", "/api/campaigns/read-anna/answers"),
        ("GET", "/api/campaigns/read-anna/stats"),
        ("GET", "/api/campaigns/read-anna/batches"),
        ("GET", &format!("/api/campaigns/read-anna/items/{item}/why")),
        ("POST", "/api/campaigns/read-anna/claim"),
        ("POST", "/api/campaigns/read-anna/export"),
    ] {
        let body = (method == "POST").then(|| json!({}));
        let (status, doc) = server.call(method, door, body, &bo);
        assert_eq!(status, 404, "{method} {door}: {doc}");
    }
    // in the campaign they share, bo does not read anna's answer as the
    // item's outcome, nor her assignment, nor the agreement
    let shared = server.ok("GET", "/api/campaigns/shared", None, &bo);
    let it = shared["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == shared_item)
        .unwrap()
        .clone();
    assert!(it["outcome"].is_null(), "{it}");
    assert_eq!(it["blind"], true, "{it}");
    assert!(
        shared["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["principal"] == bo_p),
        "{shared}"
    );
    assert!(shared.get("agreement").is_none(), "{shared}");
    let answers = server.ok("GET", "/api/campaigns/shared/answers", None, &bo);
    assert_eq!(answers["count"], 0, "{answers}");
    for of in ["outcomes", "answers"] {
        let (status, doc) = server.call(
            "POST",
            "/api/campaigns/shared/export",
            Some(json!({"of": of})),
            &bo,
        );
        assert_eq!(status, 403, "{of}: {doc}");
    }
    // and bo reads no picture through a campaign he is not a rater of: the
    // stacks are the same, so his own campaign opens them to him
    let (status, _) = server.call("GET", "/api/instances/3/manifest", None, &bo);
    assert_eq!(status, 403);
    let (status, _) = server.call("GET", "/api/instances/2/manifest", None, &bo);
    assert_eq!(status, 200);

    // the holder of review:work reads every campaign as before, with its
    // outcomes
    let every = server.ok("GET", "/api/campaigns", None, &cleo);
    assert_eq!(every["count"], 3, "{every}");
    let shared = server.ok("GET", "/api/campaigns/shared", None, &cleo);
    let it = shared["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == shared_item)
        .unwrap()
        .clone();
    assert!(!it["outcome"].is_null(), "{it}");
    assert!(it.get("blind").is_none(), "{it}");

    // once a campaign closes, its raters no longer read pictures through it
    ok(&home, &["campaign", "close", "read-anna"], None);
    ok(&home, &["campaign", "close", "shared"], None);
    let (status, _) = server.call("GET", "/api/instances/1/manifest", None, &anna);
    assert_eq!(status, 403);
}
