// SPDX-License-Identifier: AGPL-3.0-only

//! Record 58 P6 (Wave 7b spec §16, gate E6) on a lab registry: `nils audit
//! draw --seed --count --packets` draws the same stacks from the same seed
//! twice, makes the verify campaign over them with the accepted answer as
//! each proposal, and writes one header packet per stack named by its place
//! in the draw, naming no subject code and no path; two people read it
//! through the doors; `nils audit export` writes the counts, the exact
//! bound and the readers' disagreements by axis, vendor and year, with a
//! cell under 5 held back with its complement, and nothing that names a
//! stack. SQLite, and Postgres where `NILS_TEST_POSTGRES_DSN` is set.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use serde_json::{Value, json};

const ISSUER: &str = "https://desk.example/";

/// The lab registry's stacks: 14 of one maker, 3 of a second, 23 of a
/// third, half of them in each of two years.
const STACKS: usize = 40;
const SMALL_MAKER: [usize; 3] = [1, 5, 7];

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc")
}

fn cli(home: &TempDir, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = nils();
    cmd.arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "cleo")
        .env("HOSTNAME", "lab")
        .env_remove("NILS_DSN")
        .env_remove("NILS_PACK_DIR")
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
    child.wait_with_output().unwrap()
}

fn run(home: &TempDir, args: &[&str]) -> String {
    let out = cli(home, args, None);
    assert!(
        out.status.success(),
        "{}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The lab registry, classified, made by the command line, on SQLite or on
/// a Postgres schema.
fn registry(pg: Option<&(String, String)>) -> (TempDir, TempDir) {
    let home = TempDir::new("sealed-home");
    let dir = TempDir::new("sealed-src");
    for i in 0..STACKS {
        let study = format!("1.2.826.0.1.3680043.8.498.{}", 2000 + i);
        let series = format!("{study}.1");
        let sop = format!("{series}.1");
        let mut e = synth::minimal_mr(&study, &series, &sop);
        e.push(synth::text(
            tags::PATIENT_ID,
            VR::LO,
            &format!("SUBJ{:03}", i / 2),
        ));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        let maker = if SMALL_MAKER.contains(&i) {
            "Philips Medical Systems"
        } else if i % 3 == 0 {
            "GE MEDICAL SYSTEMS"
        } else {
            "SIEMENS"
        };
        e.push(synth::text(tags::MANUFACTURER, VR::LO, maker));
        let day = if i % 2 == 0 { "20190304" } else { "20240611" };
        e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
        e.push(synth::text(tags::ACQUISITION_DATE, VR::DA, day));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    let out = cli(
        &home,
        &["key", "add", "k"],
        Some("a sealed protocol test key\n"),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    match pg {
        Some((dsn, schema)) => {
            run(
                &home,
                &[
                    "init",
                    "--backend",
                    "postgres",
                    "--dsn",
                    dsn,
                    "--schema",
                    schema,
                    "--key",
                    "k",
                ],
            );
        }
        None => {
            run(&home, &["init", "--key", "k"]);
        }
    }
    run(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            dir.path().to_str().unwrap(),
        ],
    );
    run(&home, &["fingerprint"]);
    run(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
    );
    (home, dir)
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

/// A token the desk's issuer could have minted: the subject, the account's
/// name, its grants and detail.
fn token(sub: &str, name: &str, grants: &[&str], detail: &str) -> String {
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
        &json!({"iss": ISSUER, "aud": "nils", "sub": sub, "name": name, "preferred_username": sub,
                "grants": grants, "detail": detail, "exp": now + 3600, "iat": now}),
        &key,
    )
    .unwrap()
}

/// Claim and answer every item a token is given in a campaign, the answer
/// made from the item's stack. Answers how many it answered.
fn answer_all(
    server: &Server,
    campaign: i64,
    token: &str,
    mut answer: impl FnMut(i64, &Value) -> Value,
) -> usize {
    let mut n = 0;
    loop {
        let claimed = server.ok(
            "POST",
            &format!("/api/campaigns/{campaign}/claim"),
            Some(json!({"role": "rater"})),
            token,
        );
        if claimed["assignment"].is_null() {
            return n;
        }
        let item = claimed["item"]["id"].as_i64().unwrap();
        let stack = claimed["item"]["stack_id"].as_i64().unwrap();
        let why = server.ok(
            "GET",
            &format!("/api/campaigns/{campaign}/items/{item}/why"),
            None,
            token,
        );
        let value = answer(stack, &why);
        let a = claimed["assignment"]["id"].as_i64().unwrap();
        server.ok(
            "POST",
            &format!("/api/campaigns/{campaign}/assignments/{a}/answer"),
            Some(json!({"value": value.to_string()})),
            token,
        );
        n += 1;
    }
}

/// A draw's stacks by id: their DICOM identity and the base accepted.
fn drawn(server: &Server, draw: i64, token: &str) -> Vec<(i64, String, String)> {
    server.ok("GET", &format!("/api/audit/draws/{draw}"), None, token)["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["stack_id"].as_i64().unwrap(),
                format!("{}|{}|{}", s["study_uid"], s["series_uid"], s["stack_key"]),
                s["accepted"]["base"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// `nils audit draw` into a directory, as JSON.
fn draw(home: &TempDir, seed: &str, packets: &Path, more: &[&str]) -> Value {
    let packs = packs();
    let mut args = vec![
        "audit",
        "draw",
        "--seed",
        seed,
        "--count",
        "100",
        "--since",
        "2000-01-01T00:00:00Z",
        "--axes",
        "base",
        "--packets",
        packets.to_str().unwrap(),
        "--pack-dir",
        packs.to_str().unwrap(),
        "--json",
    ];
    args.extend_from_slice(more);
    serde_json::from_str(&run(home, &args)).unwrap()
}

/// A packet's text past its head (the draw, the campaign and the item).
fn body_of(text: &str) -> String {
    text.lines().skip(3).collect::<Vec<_>>().join("\n")
}

fn protocol(pg: Option<(String, String)>) {
    let (home, src) = registry(pg.as_ref());
    let mut store = match &pg {
        Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
        None => Store::open_sqlite(&home.path().join("registry.db")).unwrap(),
    };
    let codes: Vec<String> = store
        .query(
            &format!("SELECT code FROM {}", store.qualified("subject")),
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
    assert_eq!(codes.len(), STACKS / 2);
    let draws = |store: &mut Store| {
        store
            .query(
                &format!("SELECT id FROM {}", store.qualified("audit_draw")),
                &[],
            )
            .unwrap()
            .len()
    };

    let server = Server::start(&home);
    let cleo = token(
        "cleo",
        "Cleo Curator",
        &[
            "review:see",
            "review:work",
            "campaigns:see",
            "campaigns:work",
        ],
        "quasi",
    );
    let reader = |sub: &str| token(sub, sub, &["campaigns:see", "campaigns:work"], "quasi");
    let (nora, otto) = (reader("nora"), reader("otto"));
    let principal = |t: &str| {
        server.ok("GET", "/api/capabilities", None, t)["principal"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let readers = format!("{},{}", principal(&nora), principal(&otto));

    // ------------------------------------------- the draw and its packets
    let p1 = TempDir::new("sealed-p1");
    let first = draw(
        &home,
        "site-1",
        &p1.path().join("packets"),
        &["--readers-per-item", "2", "--readers", &readers],
    );
    let d1 = first["draw"]["id"].as_i64().unwrap();
    assert_eq!(first["draw"]["frame"]["stacks"], STACKS, "{first}");
    assert_eq!(first["draw"]["drawn"], STACKS, "the whole frame: {first}");
    assert_eq!(first["campaign"]["purpose"], "verify", "{first}");
    let verify = first["campaign"]["id"].as_i64().unwrap();
    let dir1 = p1.path().join("packets");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(dir1.join("MANIFEST.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["seed"], "site-1", "{manifest}");
    assert_eq!(
        manifest["frame"]["digest"], first["draw"]["frame"]["digest"],
        "{manifest}"
    );
    let listed = manifest["packets"].as_array().unwrap();
    assert_eq!(listed.len(), STACKS, "{manifest}");
    assert_eq!(listed[0]["file"], "0001.txt", "{manifest}");
    let files: BTreeSet<String> = std::fs::read_dir(&dir1)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(files.len(), STACKS + 1, "{files:?}");
    // no packet names a subject code, a patient's id or a path
    let source = src.path().display().to_string();
    let home_dir = home.path().display().to_string();
    let mut texts: Vec<String> = Vec::new();
    for p in listed {
        let text = std::fs::read_to_string(dir1.join(p["file"].as_str().unwrap())).unwrap();
        for code in &codes {
            assert!(
                !text.contains(code.as_str()),
                "a subject code in a packet: {text}"
            );
        }
        assert!(
            !text.contains("SUBJ0"),
            "a patient's id in a packet: {text}"
        );
        assert!(
            !text.contains(&source) && !text.contains(&home_dir),
            "{text}"
        );
        assert!(!text.contains("PatientID"), "{text}");
        assert!(
            text.contains("SeriesDescription"),
            "the header is there: {text}"
        );
        assert!(text.contains("t1 mprage"), "{text}");
        texts.push(text);
    }
    assert!(!manifest.to_string().contains(&source), "{manifest}");
    for code in &codes {
        assert!(!manifest.to_string().contains(code.as_str()), "{manifest}");
    }

    // a directory that holds packets already is refused before anything is drawn
    let before = draws(&mut store);
    let out = cli(
        &home,
        &[
            "audit",
            "draw",
            "--seed",
            "site-1",
            "--count",
            "5",
            "--since",
            "2000-01-01",
            "--packets",
            dir1.to_str().unwrap(),
            "--pack-dir",
            packs().to_str().unwrap(),
            "--readers",
            &readers,
        ],
        None,
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not empty"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(draws(&mut store), before, "nothing was drawn");

    // the same seed draws the same stacks in the same order again
    let p2 = TempDir::new("sealed-p2");
    let again = draw(
        &home,
        "site-1",
        &p2.path().join("packets"),
        &["--readers", &readers],
    );
    assert_eq!(
        again["draw"]["frame"]["digest"], first["draw"]["frame"]["digest"],
        "{again}"
    );
    let d2 = again["draw"]["id"].as_i64().unwrap();
    for (i, text) in texts.iter().enumerate() {
        let other =
            std::fs::read_to_string(p2.path().join("packets").join(format!("{:04}.txt", i + 1)))
                .unwrap();
        assert_eq!(body_of(text), body_of(&other), "packet {}", i + 1);
    }
    // a different seed draws them in another order
    let p3 = TempDir::new("sealed-p3");
    let third = draw(
        &home,
        "site-2",
        &p3.path().join("packets"),
        &["--readers", &readers],
    );
    let d3 = third["draw"]["id"].as_i64().unwrap();

    let one = drawn(&server, d1, &cleo);
    let two = drawn(&server, d2, &cleo);
    let three = drawn(&server, d3, &cleo);
    let idents = |v: &[(i64, String, String)]| v.iter().map(|x| x.1.clone()).collect::<Vec<_>>();
    assert_eq!(idents(&one), idents(&two), "the same seed, the same order");
    assert_ne!(idents(&one), idents(&three), "another seed, another order");

    // ------------------------------ two people read it through the doors
    let accepted: BTreeMap<i64, String> = one.iter().map(|x| (x.0, x.2.clone())).collect();
    let ids: Vec<i64> = one.iter().map(|x| x.0).collect();
    let other = |b: &str| if b == "PDw" { "T2w" } else { "PDw" }.to_string();
    // both find the rules wrong on the first two stacks; they disagree on the third
    let wrong: BTreeSet<i64> = [ids[0], ids[1]].into();
    let c = server.ok("GET", &format!("/api/campaigns/{verify}"), None, &cleo);
    assert_eq!(c["purpose"], "verify", "{c}");
    let read = |stack: i64, why: &Value, odd: bool| {
        assert_eq!(why["purpose"], "verify", "{why}");
        let b = accepted[&stack].clone();
        json!({"base": if wrong.contains(&stack) || (odd && stack == ids[2]) { other(&b) } else { b }})
    };
    assert_eq!(
        answer_all(&server, verify, &nora, |s, w| read(s, w, false)),
        STACKS
    );
    assert_eq!(
        answer_all(&server, verify, &otto, |s, w| read(s, w, true)),
        STACKS
    );
    server.ok(
        "POST",
        &format!("/api/campaigns/{verify}/close"),
        Some(json!({})),
        &cleo,
    );

    // ------------------------------------------------------- the export
    let out = TempDir::new("sealed-out");
    let file = out.path().join("export.json");
    run(
        &home,
        &[
            "audit",
            "export",
            "--draw",
            &d1.to_string(),
            "--out",
            file.to_str().unwrap(),
        ],
    );
    let text = std::fs::read_to_string(&file).unwrap();
    let x: Value = serde_json::from_str(&text).unwrap();
    let c = &x["counts"];
    assert_eq!(c["frame"], STACKS, "{x}");
    assert_eq!(c["drawn"], STACKS, "{x}");
    assert_eq!(c["wrong"], 2, "{x}");
    assert_eq!(
        c["read"].as_u64().unwrap() + c["not_settled"].as_u64().unwrap(),
        STACKS as u64,
        "{x}"
    );
    let bound = c["bound"].as_f64().unwrap();
    assert!(bound > 2.0 / STACKS as f64 && bound < 0.3, "{x}");
    assert_eq!(x["disagreement"]["read_by_two_or_more"], STACKS, "{x}");
    assert_eq!(x["disagreement"]["disagreed"], 1, "{x}");
    assert_eq!(x["by_axis"]["base"]["disagreed"], 1, "{x}");
    assert_eq!(x["by_axis"]["base"]["wrong"], 2, "{x}");
    // the cell of 3 is held back, and the next smallest with it
    assert_eq!(x["min_cell"]["k"], 5, "{x}");
    let v = &x["by_vendor"];
    assert_eq!(v["philips"]["held_back"], true, "{x}");
    assert!(v["philips"]["drawn"].is_null(), "{x}");
    assert_eq!(v["ge"]["held_back"], true, "the complement: {x}");
    assert_eq!(v["siemens"]["drawn"], 23, "{x}");
    assert!(v["siemens"]["held_back"].is_null(), "{x}");
    assert_eq!(x["by_year"]["2019"]["drawn"], 20, "{x}");
    assert_eq!(x["by_year"]["2024"]["drawn"], 20, "{x}");
    let held: BTreeSet<&str> = x["min_cell"]["held_back"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap())
        .collect();
    assert_eq!(held, ["vendor:ge", "vendor:philips"].into(), "{x}");
    // nothing that names a stack, a subject or a place
    for code in &codes {
        assert!(!text.contains(code.as_str()), "{text}");
    }
    for bad in [
        "stack_id",
        "study_uid",
        "series_uid",
        "SUBJ0",
        "t1 mprage",
        source.as_str(),
    ] {
        assert!(!text.contains(bad), "{bad} in the export: {text}");
    }
    assert!(x["wrong_stacks"].is_null(), "{x}");

    // the grade door reads the same draw the same way
    let graded = server.ok(
        "POST",
        &format!("/api/audit/draws/{d1}/grade"),
        Some(json!({})),
        &cleo,
    );
    assert_eq!(graded["grade"]["wrong"], c["wrong"], "{graded}");
    assert_eq!(graded["grade"]["read"], c["read"], "{graded}");

    // the newest draw by count is exported where none is named: nobody read it
    let file3 = out.path().join("newest.json");
    run(
        &home,
        &["audit", "export", "--out", file3.to_str().unwrap()],
    );
    let x3: Value = serde_json::from_str(&std::fs::read_to_string(&file3).unwrap()).unwrap();
    assert_eq!(x3["draw"]["id"], d3, "{x3}");
    assert_eq!(x3["counts"]["read"], 0, "{x3}");
    // and `nils audit list` is as it was
    run(&home, &["audit", "list"]);
}

#[test]
fn the_sealed_protocol_draws_packets_and_exports_counts() {
    protocol(None);
}

#[test]
fn the_sealed_protocol_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_sealed_protocol";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    protocol(Some((dsn.clone(), schema.to_string())));
    drop();
}
