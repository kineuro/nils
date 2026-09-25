// SPDX-License-Identifier: AGPL-3.0-only

//! Record 48, after the first real read: blind hides the systems' answers,
//! never the file. A sealed item shows the header's text and physics in
//! full, and the whole stored header one door away, less what names a
//! person; a rater answers only the axes that need a person, and the pack
//! derives the rest from that answer, shown live and kept marked derived;
//! and an open campaign asked under the old form is moved to the new one
//! without losing its items or its answer.

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

/// What names the person in the files, none of which a reader may see.
const PATIENT_NAME: &str = "Doe^Jane";
const PATIENT_ID: &str = "PID-7788-X";
const BIRTH: &str = "19500317";
const INSTITUTION: &str = "Nowhere Test Hospital";
const STATION: &str = "STATION-QX9";
const ACCESSION: &str = "ACC-55123";

/// The axes Phase 0 asks, and the five the pack derives beside them.
const ASKED: &[&str] = &[
    "provenance",
    "technique",
    "modifier",
    "construct",
    "base",
    "body_part",
    "post_contrast",
];
const DERIVED: &[&str] = &[
    "quality",
    "directory_type",
    "disposition",
    "convertible",
    "role",
];

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

/// One FLAIR-looking file per study, with everything that names a person.
fn study(dir: &TempDir, n: usize) {
    let study = format!("1.2.3.{n}");
    let series = format!("{study}.1");
    let sop = format!("{series}.1");
    let mut e = synth::minimal_mr(&study, &series, &sop);
    for (tag, vr, v) in [
        (tags::PATIENT_NAME, VR::PN, PATIENT_NAME),
        (tags::PATIENT_ID, VR::LO, PATIENT_ID),
        (tags::PATIENT_BIRTH_DATE, VR::DA, BIRTH),
        (tags::INSTITUTION_NAME, VR::LO, INSTITUTION),
        (tags::STATION_NAME, VR::SH, STATION),
        (tags::ACCESSION_NUMBER, VR::SH, ACCESSION),
        (tags::SERIES_DESCRIPTION, VR::LO, "t2 flair tra"),
        (tags::PROTOCOL_NAME, VR::LO, "ax_dark_fluid_5mm"),
        (tags::SEQUENCE_NAME, VR::SH, "*tir2d1rr99"),
        (tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
        (tags::SCANNING_SEQUENCE, VR::CS, "SE\\IR"),
        (tags::SEQUENCE_VARIANT, VR::CS, "SK\\SP\\MP"),
        (tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
        (tags::BODY_PART_EXAMINED, VR::CS, "HEAD"),
        (tags::ECHO_TIME, VR::DS, "100"),
        (tags::REPETITION_TIME, VR::DS, "9000"),
        (tags::INVERSION_TIME, VR::DS, "2500"),
        (tags::FLIP_ANGLE, VR::DS, "150"),
        (tags::MAGNETIC_FIELD_STRENGTH, VR::DS, "3"),
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

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Every key anywhere in a document.
fn keys_in(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                out.push(k.clone());
                keys_in(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| keys_in(x, out)),
        _ => {}
    }
}

/// A Phase 0 answer: a T2w FLAIR of the brain, no contrast.
fn flair() -> Value {
    json!({
        "provenance": "RawRecon", "technique": "TSE", "modifier": ["FLAIR"],
        "construct": [], "base": "T2w", "body_part": "brain",
        "post_contrast": "not_given",
    })
}

#[test]
fn a_blind_reader_sees_the_file_and_answers_what_needs_a_person() {
    let home = TempDir::new("blind-home");
    let src = TempDir::new("blind-src");
    for n in 1..=4 {
        study(&src, n);
    }
    ok(
        &home,
        &["key", "add", "k"],
        Some("a blind reader test key\n"),
    );
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
    let out = TempDir::new("blind-out");

    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        &[
            "query:work",
            "data:see",
            "review:work",
            "campaigns:work",
            "places:work",
        ],
        "quasi",
    );
    let anna = token("anna", &["campaigns:see", "campaigns:work"], "quasi");
    let pia = token("pia", &["campaigns:see", "campaigns:work"], "plain");
    let bo = token("bo", &["campaigns:see", "campaigns:work"], "quasi");
    let (anna_p, pia_p, bo_p) = ("anna@desk.example", "pia@desk.example", "bo@desk.example");
    server.ok(
        "POST",
        "/api/places",
        Some(json!({"name": "labels-out", "role": "export", "path": out.path().to_str().unwrap()})),
        &cleo,
    );
    server.ok(
        "PUT",
        "/api/ask/selections/two",
        Some(selection_of(&[1, 2])),
        &cleo,
    );
    server.ok(
        "PUT",
        "/api/ask/selections/other",
        Some(selection_of(&[3, 4])),
        &cleo,
    );

    // ------------------------------------------------ asked and derived
    // the seven asked axes leave five to the pack, found in it when the
    // question names none
    let made = server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "read-anna",
            "question": {"kind": "axes", "axes": ASKED},
            "source": {"selection": "two@1"},
            "raters": [anna_p, pia_p], "raters_per_item": 1,
            "adjudication": {"when": "never"}, "closes_into": "none",
        })),
        &cleo,
    );
    let mut derive: Vec<String> = made["question"]["derive"]
        .as_array()
        .unwrap_or_else(|| panic!("{made}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    derive.sort();
    let mut want = words(DERIVED);
    want.sort();
    assert_eq!(derive, want, "{}", made["question"]);
    // an axis that reads the file's numbers or words is never derived
    let (status, doc) = server.call(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "bad", "question": {"kind": "axes", "axes": ["technique"], "derive": ["base"]},
            "source": {"selection": "two@1"}, "raters": [anna_p],
        })),
        &cleo,
    );
    assert_eq!(status, 400, "{doc}");
    // bo reads a campaign of his own, over other stacks
    server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "read-bo",
            "question": {"kind": "axes", "axes": ASKED},
            "source": {"selection": "other@1"},
            "raters": [bo_p], "raters_per_item": 1,
            "adjudication": {"when": "never"}, "closes_into": "none",
        })),
        &cleo,
    );
    let handle = made["handle_id"].as_i64().unwrap();
    ok(
        &home,
        &["labels", "seal", "--handle", &handle.to_string(), "--json"],
        None,
    );
    let item = made["items"][0]["id"].as_i64().unwrap();
    let stack = made["items"][0]["stack_id"].as_i64().unwrap();

    // ------------------------------------------------ blind shows the file
    let why = server.ok(
        "GET",
        &format!("/api/campaigns/read-anna/items/{item}/why"),
        None,
        &anna,
    );
    assert_eq!(why["blind"], true, "{why}");
    assert_eq!(why["texts"]["series_description"], "t2 flair tra", "{why}");
    assert_eq!(why["texts"]["protocol_name"], "ax_dark_fluid_5mm", "{why}");
    assert_eq!(why["texts"]["sequence_name"], "*tir2d1rr99", "{why}");
    assert!(
        why["texts"]["image_type"]
            .as_str()
            .unwrap()
            .contains("ORIGINAL"),
        "{why}"
    );
    assert_eq!(why["texts"]["body_part_examined"], "HEAD", "{why}");
    assert_eq!(why["physics"]["repetition_time"], 9000.0, "{why}");
    assert_eq!(why["physics"]["inversion_time"], 2500.0, "{why}");
    assert_eq!(why["physics"]["manufacturer"], "SIEMENS", "{why}");
    assert_eq!(
        why["header_door"],
        format!("/api/campaigns/{}/items/{item}/header", made["id"]),
        "{why}"
    );
    // and nothing any system said of the stack
    assert!(
        why["suggested"].is_null() && why["worth"].is_null(),
        "{why}"
    );
    let mut keys = Vec::new();
    keys_in(&why, &mut keys);
    for k in [
        "axes",
        "line",
        "voted",
        "decided",
        "set_by",
        "matched",
        "candidates",
        "asked",
        "votes",
    ] {
        assert!(!keys.iter().any(|x| x == k), "{k}: {why}");
    }
    let text = why.to_string();
    for word in ["T2w", PATIENT_NAME, PATIENT_ID, BIRTH, INSTITUTION, STATION] {
        assert!(!text.contains(word), "{word}: {why}");
    }
    // at detail plain the typed text is left out, the scanner's tokens stay
    let plain = server.ok(
        "GET",
        &format!("/api/campaigns/read-anna/items/{item}/why"),
        None,
        &pia,
    );
    assert!(
        plain["texts"].get("series_description").is_none(),
        "{plain}"
    );
    assert!(plain["texts"].get("protocol_name").is_none(), "{plain}");
    assert!(plain["texts"]["image_type"].is_string(), "{plain}");

    // ------------------------------------------------ the whole header
    let head = server.ok(
        "GET",
        &format!("/api/campaigns/read-anna/items/{item}/header"),
        None,
        &anna,
    );
    assert_eq!(head["stack"], stack, "{head}");
    let sd = head["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["column"] == "series_description")
        .unwrap()
        .clone();
    assert_eq!(sd["keyword"], "SeriesDescription", "{sd}");
    assert_eq!(sd["tag"], "(0008,103E)", "{sd}");
    assert_eq!(head["blind"], true, "{head}");
    let fields = head["fields"].as_array().unwrap();
    let column = |c: &str| {
        fields
            .iter()
            .find(|f| f["column"] == c)
            .map(|f| f["value"].clone())
    };
    assert_eq!(
        column("series_description"),
        Some(json!("t2 flair tra")),
        "{head}"
    );
    assert_eq!(
        column("protocol_name"),
        Some(json!("ax_dark_fluid_5mm")),
        "{head}"
    );
    assert!(column("manufacturer").is_some(), "{head}");
    for c in ["birth_date", "institution_name", "station_name", "sex"] {
        assert!(column(c).is_none(), "{c}: {head}");
    }
    let text = head.to_string();
    for word in [
        PATIENT_NAME,
        PATIENT_ID,
        BIRTH,
        INSTITUTION,
        STATION,
        ACCESSION,
        "1.2.3.1",
    ] {
        assert!(!text.contains(word), "{word}: {head}");
    }
    assert!(
        head["left_out"]["identifying"].as_u64().unwrap() > 0,
        "{head}"
    );
    let plain = server.ok(
        "GET",
        &format!("/api/campaigns/read-anna/items/{item}/header"),
        None,
        &pia,
    );
    assert!(
        !plain["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["column"] == "series_description"),
        "{plain}"
    );
    // nothing opens for a stack outside the rater's campaigns
    for door in ["header", "why"] {
        let (status, doc) = server.call(
            "GET",
            &format!("/api/campaigns/read-anna/items/{item}/{door}"),
            None,
            &bo,
        );
        assert_eq!(status, 404, "{door}: {doc}");
    }
    let (status, doc) = server.call(
        "POST",
        &format!("/api/campaigns/read-anna/items/{item}/derive"),
        Some(json!({"value": flair()})),
        &bo,
    );
    assert_eq!(status, 404, "{doc}");

    // ------------------------------------------------ derived, live
    let live = server.ok(
        "POST",
        &format!("/api/campaigns/read-anna/items/{item}/derive"),
        Some(json!({"value": {"base": "T2w", "modifier": ["FLAIR"]}})),
        &anna,
    );
    // provenance and construct are not answered yet: what reads them waits
    assert_eq!(live["derived"]["directory_type"], "cant_tell", "{live}");
    assert_eq!(live["derived"]["role"], "cant_tell", "{live}");
    // quality is read from ImageType alone
    assert_eq!(live["derived"]["quality"], json!([]), "{live}");
    let whole = server.ok(
        "POST",
        &format!("/api/campaigns/read-anna/items/{item}/derive"),
        Some(json!({"value": flair()})),
        &anna,
    );
    let d = &whole["derived"];
    assert_eq!(d["directory_type"], "anat", "{whole}");
    assert_eq!(d["disposition"], "acquisition", "{whole}");
    assert_eq!(d["convertible"], "yes", "{whole}");
    assert_eq!(d["role"], json!(["flair"]), "{whole}");
    // the derived follow the rater, not the file's words: the same FLAIR
    // answered as a T1w derives the T1w role
    let mut t1 = flair();
    t1["base"] = json!("T1w");
    t1["modifier"] = json!([]);
    t1["technique"] = json!("MPRAGE");
    let other = server.ok(
        "POST",
        &format!("/api/campaigns/read-anna/items/{item}/derive"),
        Some(json!({"value": t1})),
        &anna,
    );
    assert_eq!(other["derived"]["role"], json!(["t1w"]), "{other}");
    // can't tell on base leaves what reads base can't tell
    let mut unsure = flair();
    unsure["base"] = json!("cant_tell");
    let unknown = server.ok(
        "POST",
        &format!("/api/campaigns/read-anna/items/{item}/derive"),
        Some(json!({"value": unsure})),
        &anna,
    );
    assert_eq!(
        unknown["derived"]["directory_type"], "cant_tell",
        "{unknown}"
    );
    assert_eq!(unknown["derived"]["quality"], json!([]), "{unknown}");

    // ------------------------------------------------ the answer keeps them
    let claimed = server.ok(
        "POST",
        "/api/campaigns/read-anna/claim",
        Some(json!({})),
        &anna,
    );
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    // a derived axis is never the rater's to answer
    let mut named = flair();
    named["role"] = json!(["flair"]);
    let (status, doc) = server.call(
        "POST",
        &format!("/api/campaigns/read-anna/assignments/{assignment}/answer"),
        Some(json!({"value": named.to_string()})),
        &anna,
    );
    assert_eq!(status, 400, "{doc}");
    let answered = server.ok(
        "POST",
        &format!("/api/campaigns/read-anna/assignments/{assignment}/answer"),
        Some(json!({"value": flair().to_string()})),
        &anna,
    );
    assert_eq!(answered["derived"]["directory_type"], "anat", "{answered}");
    let answers = server.ok("GET", "/api/campaigns/read-anna/answers", None, &anna);
    let mine = &answers["answers"][0];
    assert_eq!(mine["derived"]["role"], json!(["flair"]), "{answers}");
    // an export says which labels were derived
    let set = server.ok(
        "POST",
        "/api/campaigns/read-anna/export",
        Some(json!({"of": "answers", "place": "labels-out"})),
        &cleo,
    );
    let dir = set["path"].as_str().unwrap_or_else(|| panic!("{set}"));
    let tsv = std::fs::read_to_string(Path::new(dir).join("labels.tsv")).unwrap();
    let mut lines = tsv.lines();
    let header: Vec<&str> = lines.next().unwrap().split('\t').collect();
    let at = |c: &str| header.iter().position(|h| *h == c).unwrap();
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split('\t').collect()).collect();
    let row = |what: &str| rows.iter().find(|r| r[at("what")] == what).unwrap().clone();
    assert_eq!(row("role")[at("derived")], "true", "{tsv}");
    assert_eq!(row("role")[at("value")], "flair", "{tsv}");
    assert_eq!(row("base")[at("derived")], "false", "{tsv}");
    assert_eq!(row("base")[at("value")], "T2w", "{tsv}");
    assert_eq!(rows.len(), ASKED.len() + DERIVED.len(), "{tsv}");
}

#[test]
fn an_open_campaign_moves_to_the_seven_asked_axes_keeping_its_answer() {
    let home = TempDir::new("requestion-home");
    let src = TempDir::new("requestion-src");
    for n in 1..=3 {
        study(&src, n);
    }
    ok(&home, &["key", "add", "k"], Some("a requestion test key\n"));
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
    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        &["query:work", "data:see", "review:work", "campaigns:work"],
        "quasi",
    );
    let anna = token("anna", &["campaigns:see", "campaigns:work"], "quasi");
    let anna_p = "anna@desk.example";
    server.ok(
        "PUT",
        "/api/ask/selections/three",
        Some(selection_of(&[1, 2, 3])),
        &cleo,
    );
    // made the way Phase 0's first campaigns were: all twelve axes asked
    let twelve: Vec<&str> = ASKED.iter().chain(DERIVED).copied().collect();
    let made = server.ok(
        "POST",
        "/api/campaigns",
        Some(json!({
            "name": "read-r1",
            "question": {"kind": "axes", "axes": twelve, "derive": []},
            "source": {"selection": "three@1"},
            "raters": [anna_p], "raters_per_item": 1,
            "adjudication": {"when": "never"}, "closes_into": "none",
        })),
        &cleo,
    );
    assert!(
        made["question"].get("derive").is_none(),
        "{}",
        made["question"]
    );
    let items = made["items"].as_array().unwrap().len();
    // one answer, given under the old form, with a quality of the rater's
    let claimed = server.ok(
        "POST",
        "/api/campaigns/read-r1/claim",
        Some(json!({})),
        &anna,
    );
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    let mut old = flair();
    for (k, v) in [
        ("quality", json!([])),
        ("directory_type", json!("anat")),
        ("disposition", json!("acquisition")),
        ("convertible", json!("yes")),
        ("role", json!(["flair"])),
    ] {
        old[k] = v;
    }
    server.ok(
        "POST",
        &format!("/api/campaigns/read-r1/assignments/{assignment}/answer"),
        Some(json!({"value": old.to_string()})),
        &anna,
    );
    let before = server.ok("GET", "/api/campaigns/read-r1/answers", None, &cleo);
    let kept_value = before["answers"][0]["value"].clone();

    // refused while an item is leased under the old form
    let held = server.ok(
        "POST",
        "/api/campaigns/read-r1/claim",
        Some(json!({})),
        &anna,
    );
    let asked = ASKED.join(",");
    let pack_dir = packs();
    let args = [
        "campaign",
        "requestion",
        "read-r1",
        "--axes",
        &asked,
        "--pack-dir",
        pack_dir.to_str().unwrap(),
        "--json",
    ];
    let (good, _, err) = run(&home, &args, None);
    assert!(!good, "requestion ran while an item was leased");
    assert!(err.contains("leased"), "{err}");
    server.ok(
        "POST",
        &format!(
            "/api/campaigns/read-r1/assignments/{}/release",
            held["assignment"]["id"]
        ),
        Some(json!({})),
        &anna,
    );
    // an axis never asked cannot be asked now
    let (good, _, err) = run(
        &home,
        &[
            "campaign",
            "requestion",
            "read-r1",
            "--axes",
            "base,acceleration",
            "--pack-dir",
            pack_dir.to_str().unwrap(),
        ],
        None,
    );
    assert!(!good, "{err}");
    let done: Value = serde_json::from_str(&ok(&home, &args, None)).unwrap();
    assert_eq!(done["answers_kept"], 1, "{done}");
    assert_eq!(done["answers_derived"], 1, "{done}");
    assert_eq!(done["asked"], json!(ASKED), "{done}");

    // the items and the answer stay; the answer is never rewritten, and
    // now carries what it derives
    let now = server.ok("GET", "/api/campaigns/read-r1", None, &cleo);
    assert_eq!(now["items"].as_array().unwrap().len(), items, "{now}");
    assert_eq!(now["question"]["axes"], json!(ASKED), "{now}");
    let after = server.ok("GET", "/api/campaigns/read-r1/answers", None, &cleo);
    assert_eq!(after["answers"][0]["value"], kept_value, "{after}");
    assert_eq!(
        after["answers"][0]["derived"]["role"],
        json!(["flair"]),
        "{after}"
    );
    assert_eq!(
        after["answers"][0]["derived"]["directory_type"], "anat",
        "{after}"
    );
    // the next item is answered on the seven, and derives the five
    let claimed = server.ok(
        "POST",
        "/api/campaigns/read-r1/claim",
        Some(json!({})),
        &anna,
    );
    let assignment = claimed["assignment"]["id"].as_i64().unwrap();
    let answered = server.ok(
        "POST",
        &format!("/api/campaigns/read-r1/assignments/{assignment}/answer"),
        Some(json!({"value": flair().to_string()})),
        &anna,
    );
    assert_eq!(
        answered["derived"]["disposition"], "acquisition",
        "{answered}"
    );
    // the campaign's audit names the move
    let (good, out, err) = run(&home, &["campaign", "show", "read-r1", "--json"], None);
    assert!(good, "{err}");
    assert!(out.contains("\"derive\""), "{out}");
}
