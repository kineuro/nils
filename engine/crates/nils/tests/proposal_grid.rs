// SPDX-License-Identifier: AGPL-3.0-only

//! The batch accept of a model's proposals (UX round 5, 2026-11-08), driven
//! through the door as the desk drives it, on SQLite and on Postgres: a
//! page of one model's proposals on one axis narrowed by the value and the
//! band, with the counts the filter row and the histogram draw; an accept
//! that writes one decision a scan in the person's name with the model's
//! probability beside it, closes the item whose scans are all decided,
//! leaves the scans set apart as proposals, never overwrites a person's
//! decision, and queues the re-sort as a job the engine's worker runs; a
//! sealed sample's scans withheld; and the keyboard saying the same. UX
//! round 6: the page says the values the axis takes, and an accept corrects
//! the few scans the person gives another value to, in the same move.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use serde_json::{Value, json};

const SCHEMA: &str = "nils_proposal_grid";

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
    let mut cmd = nils();
    cmd.arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .env_remove("NILS_JOB_ID")
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

/// A registry of six stacks of three people, classified.
fn registry(dsn: Option<&str>, schema: &str) -> TempDir {
    let home = TempDir::new("pgrid-home");
    let dir = TempDir::new("pgrid-src");
    for (patient, study, sop, description) in [
        ("P1", "1.2.3.A", "1.2.3.A.1.1", "t1 mprage"),
        ("P1", "1.2.3.B", "1.2.3.B.1.1", "t2 flair"),
        ("P2", "1.2.3.C", "1.2.3.C.1.1", "t1 mprage"),
        ("P2", "1.2.3.D", "1.2.3.D.1.1", "t2 spine"),
        ("P3", "1.2.3.E", "1.2.3.E.1.1", "t2 tse"),
        ("P3", "1.2.3.F", "1.2.3.F.1.1", "t1 mprage"),
    ] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    ok(
        &home,
        &["key", "add", "k"],
        Some("a proposal grid test key\n"),
    );
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
                    schema,
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
            dir.path().to_str().unwrap(),
        ],
        None,
    );
    ok(&home, &["fingerprint"], None);
    ok(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
        None,
    );
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
const READER: &str = "reader-token-of-length";

impl Server {
    fn start(home: &TempDir) -> Server {
        let tokens = [
            format!("{CURATOR}=cleo@lab:reviewer,campaigns:work,places:work"),
            format!("{READER}=rea@lab:review:see"),
        ]
        .join(",");
        let child = nils()
            .arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--worker",
                "--auth",
                "token",
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .env("NILS_TOKENS", tokens)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_JOB_ID")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
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

fn drop_schema(dsn: &str, schema: &str) {
    let mut store = nils_registry::Store::connect_postgres(dsn, schema).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
        ))
        .expect("drop");
}

/// The stack ids in order of the files' series descriptions, so the test
/// names each by what it is.
fn stacks_by_description(home: &TempDir) -> Vec<i64> {
    let mut reg = nils_registry::home::Home::new(home.path()).open().unwrap();
    let store = reg.store();
    let sql = format!(
        "SELECT st.id, f.text_series_description FROM {} st LEFT JOIN {} f ON f.stack_id = st.id ORDER BY st.id",
        store.qualified("stack"),
        store.qualified("stack_fingerprint")
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect()
}

/// A registered, admitted body-part model `bp-head@1` with threshold 0.9,
/// and one run (41) that proposed the given value and probability on each
/// stack. Answers the model's id.
fn model_proposing(home: &TempDir, given: &[(i64, &str, f64)]) -> i64 {
    let mut reg = nils_registry::home::Home::new(home.path()).open().unwrap();
    let enc = nils_registry::embedding::register_encoder(
        &mut reg,
        &nils_registry::embedding::Encoder {
            name: "biomedclip",
            version: "9f2c1e",
            weights_digest: &format!("sha256:{}", "b".repeat(64)),
            image_digest: None,
        },
        "runner@lab",
    )
    .unwrap();
    let card = json!({"name": "bp-head", "version": "1", "kind": "head",
        "digest": format!("sha256:{}", "1".repeat(64)), "task": "axis:body_part",
        "encoder": {"digest": enc.digest}, "threshold": 0.9});
    let m = nils_registry::model::register(&mut reg, &card, "anna@lab").unwrap();
    let passed =
        json!({"suite": "heldout", "passed": true, "checks": [{"name": "ece", "passed": true}]});
    let m = nils_registry::model::admit(&mut reg, m.id, &passed, "anna@lab").unwrap();
    let proposals: Vec<nils_registry::proposals::Proposal> = given
        .iter()
        .map(|(stack, value, p)| nils_registry::proposals::Proposal {
            stack_id: *stack,
            axis: "body_part".into(),
            value: (*value).into(),
            probabilities: [(value.to_string(), *p), ("other".to_string(), 1.0 - p)]
                .into_iter()
                .collect(),
            model_id: Some(m.id),
            model_digest: None,
            note: None,
        })
        .collect();
    nils_registry::proposals::ingest(
        &mut reg,
        &nils_registry::proposals::Run {
            id: 41,
            job_id: None,
            principal: "runner@lab",
            stacks: None,
            models: None,
        },
        &proposals,
        None,
    )
    .unwrap();
    m.id
}

#[test]
fn a_model_s_proposals_are_read_as_a_grid_and_accepted_in_one_move() {
    grid(None);
}

#[test]
fn a_model_s_proposals_are_read_as_a_grid_and_accepted_in_one_move_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    drop_schema(&dsn, SCHEMA);
    grid(Some(&dsn));
    drop_schema(&dsn, SCHEMA);
}

fn grid(dsn: Option<&str>) {
    let home = registry(dsn, SCHEMA);
    let ids = stacks_by_description(&home);
    assert_eq!(ids.len(), 6, "{ids:?}");
    let [s1, s2, s3, s4, s5, s6] = ids[..] else {
        unreachable!()
    };
    // a body-part model: two sure of brain, three unsure (two neck, one
    // spine), one sealed before any of it is read
    let model_id = model_proposing(
        &home,
        &[
            (s1, "brain", 0.97),
            (s2, "brain", 0.95),
            (s3, "spine", 0.6),
            (s4, "neck", 0.5),
            (s5, "neck", 0.7),
            (s6, "neck", 0.65),
        ],
    );
    // record 48: the sixth is of a sample sealed after the run proposed on it
    {
        let mut reg = nils_registry::home::Home::new(home.path()).open().unwrap();
        nils_registry::labels::seal(&mut reg, "cert-2", None, &[s6], "anna@lab").unwrap();
    }
    let server = Server::start(&home);
    let at = |q: &str| format!("/api/review/proposals/scans?model={model_id}&axis=body_part{q}");

    // ------------------------------------------------ the page, whole
    // the two brain scans at or above the threshold are the model's staged
    // decision, the commit's: the grid holds the three below it
    let page = server.ok("GET", &at(""), None, CURATOR);
    assert_eq!(page["label"], "bp-head@1", "{page}");
    assert_eq!(page["threshold"], 0.9, "{page}");
    assert_eq!(page["total"], 3, "{page}");
    assert_eq!(page["withheld_sealed"], 1, "{page}");
    assert_eq!(page["asked"], 0, "{page}");
    assert_eq!(page["pages"], 1, "{page}");
    assert_eq!(page["size"], 500, "{page}");
    assert_eq!(
        page["values"],
        json!([{"value": "neck", "count": 2}, {"value": "spine", "count": 1}]),
        "{page}"
    );
    assert_eq!(page["range"], json!({"low": 0.5, "high": 0.7}), "{page}");
    let bands = page["bands"].as_array().unwrap();
    assert_eq!(bands.len(), 10, "{page}");
    assert_eq!(
        bands
            .iter()
            .map(|b| b["count"].as_u64().unwrap())
            .sum::<u64>(),
        3,
        "{page}"
    );
    assert_eq!(bands[0]["from"], 0.5, "{page}");
    assert_eq!(bands[9]["to"], 0.7, "{page}");
    // the most sure first, each with what the grid draws
    let shown: Vec<i64> = page["scans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["stack"].as_i64().unwrap())
        .collect();
    assert_eq!(shown, vec![s5, s3, s4], "{page}");
    let first = &page["scans"][0];
    assert_eq!(first["proposed"], "neck", "{first}");
    assert_eq!(first["probability"], 0.7, "{first}");
    assert_eq!(first["run"], 41, "{first}");
    assert_eq!(first["decided"], false, "{first}");
    assert!(
        first.get("now").is_some(),
        "the value in force now: {first}"
    );
    assert!(first["series_description"].is_string(), "{first}");
    assert!(first["subject"]["code"].is_string(), "{first}");
    assert!(first["name"].is_string(), "{first}");
    assert!(first.get("picture").is_some(), "{first}");
    assert!(page["pictures"].is_object(), "{page}");
    // the sealed stack and the staged ones are nowhere in it
    assert!(
        !shown.contains(&s6) && !shown.contains(&s1) && !shown.contains(&s2),
        "{page}"
    );

    // ------------------------------------------------ narrowed
    let below = server.ok("GET", &at("&max=0.65"), None, CURATOR);
    assert_eq!(below["total"], 2, "{below}");
    assert_eq!(
        below["values"],
        json!([{"value": "neck", "count": 1}, {"value": "spine", "count": 1}]),
        "{below}"
    );
    // the bands are over the whole range still, for the value asked
    let neck = server.ok("GET", &at("&value=neck&min=0.5&max=0.9"), None, CURATOR);
    assert_eq!(neck["total"], 2, "{neck}");
    assert_eq!(neck["range"], json!({"low": 0.5, "high": 0.7}), "{neck}");
    assert_eq!(
        neck["bands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["count"].as_u64().unwrap())
            .sum::<u64>(),
        2,
        "{neck}"
    );
    let shown: Vec<i64> = neck["scans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["stack"].as_i64().unwrap())
        .collect();
    assert_eq!(shown, vec![s5, s4], "{neck}");
    // paging
    let paged = server.ok("GET", &at("&value=neck&size=1&page=2"), None, CURATOR);
    assert_eq!(paged["pages"], 2, "{paged}");
    assert_eq!(paged["scans"][0]["stack"], s4, "{paged}");
    assert_eq!(paged["scans"].as_array().unwrap().len(), 1, "{paged}");
    // refusals
    for (q, says) in [
        ("&min=0.9&max=0.5", "min is below max"),
        ("&min=2", "probability"),
        ("&page=0", "page is a number"),
    ] {
        let (status, doc) = server.call("GET", &at(q), None, CURATOR);
        assert_eq!(status, 400, "{q}: {doc}");
        assert!(doc.to_string().contains(says), "{q}: {doc}");
    }
    let (status, _) = server.call(
        "GET",
        "/api/review/proposals/scans?axis=body_part",
        None,
        CURATOR,
    );
    assert_eq!(status, 400);
    let (status, _) = server.call(
        "GET",
        "/api/review/proposals/scans?model=no-such@1&axis=body_part",
        None,
        CURATOR,
    );
    assert_eq!(status, 404);
    // a reader reads the grid and may not accept
    let (status, _) = server.call("GET", &at(""), None, READER);
    assert_eq!(status, 200);
    let (status, _) = server.call(
        "POST",
        "/api/review/proposals/accept",
        Some(json!({"model": model_id.to_string(), "axis": "body_part", "value": "neck", "seen": [s4]})),
        READER,
    );
    assert_eq!(status, 403);

    // ------------------------------------------------ the accept
    // a page that moved on is refused: a scan seen proposes another value
    let (status, doc) = server.call(
        "POST",
        "/api/review/proposals/accept",
        Some(json!({"model": model_id.to_string(), "axis": "body_part", "value": "neck", "min": 0.5, "max": 0.9, "seen": [s4, s3]})),
        CURATOR,
    );
    assert_eq!(status, 409, "{doc}");
    assert!(doc.to_string().contains("read the page again"), "{doc}");
    // a scan set apart the page did not show
    let (status, doc) = server.call(
        "POST",
        "/api/review/proposals/accept",
        Some(json!({"model": model_id.to_string(), "axis": "body_part", "value": "neck", "seen": [s4], "except": [s3]})),
        CURATOR,
    );
    assert_eq!(status, 400, "{doc}");
    // a person decided one of the neck scans meanwhile, through another
    // question: the accept keeps that decision
    {
        let mut reg = nils_registry::home::Home::new(home.path()).open().unwrap();
        let store = reg.store();
        let d = store.dialect();
        let sql = format!(
            "INSERT INTO {} (scope, ref, axis, value, actor, author_kind, decided_at, committed_at, committed_by) VALUES ('stack', {}, 'body_part', 'brain', 'bo@lab', 'person', {}, {}, 'bo@lab')",
            store.qualified("decision"),
            d.param(1, nils_registry::schema::Type::Text),
            d.param(2, nils_registry::schema::Type::Timestamp),
            d.param(3, nils_registry::schema::Type::Timestamp),
        );
        store
            .execute(
                &sql,
                &[
                    nils_registry::store::Param::from(s4.to_string()),
                    nils_registry::store::Param::from("2026-11-08T20:00:00Z"),
                    nils_registry::store::Param::from("2026-11-08T20:00:00Z"),
                ],
            )
            .unwrap();
    }
    let neck = server.ok("GET", &at("&value=neck&max=0.9"), None, CURATOR);
    assert_eq!(neck["scans"][1]["stack"], s4, "{neck}");
    assert_eq!(neck["scans"][1]["decided"], true, "{neck}");
    assert_eq!(neck["scans"][0]["decided"], false, "{neck}");
    // the page accepted with one set apart: one decision, the other stays a
    // proposal, the decided one keeps the person's decision
    let done = server.ok(
        "POST",
        "/api/review/proposals/accept",
        Some(json!({"model": model_id.to_string(), "axis": "body_part", "value": "neck", "min": 0.5, "max": 0.9, "seen": [s5, s4], "except": [], "why": "looked right"})),
        CURATOR,
    );
    assert_eq!(done["decided"], 1, "{done}");
    assert_eq!(done["decisions"][0]["stack"], s5, "{done}");
    assert_eq!(done["kept_decided"], 1, "{done}");
    assert_eq!(done["left_out_sealed"], 0, "{done}");
    assert_eq!(done["set_apart"], 0, "{done}");
    // the neck item held s4, s5 and the sealed s6: s4 and s6 are left in it
    assert_eq!(done["left_in_items"], 2, "{done}");
    assert_eq!(done["closed"].as_array().unwrap().len(), 0, "{done}");
    assert_eq!(done["resort"]["state"], "queued", "{done}");
    assert_eq!(done["resort"]["stacks"], 1, "{done}");
    let job = done["resort"]["job"].as_i64().expect("a job");
    // the decision is the person's, in force, with the model's probability
    // in its words
    let decision = done["decisions"][0]["decision"].as_i64().unwrap();
    {
        let mut reg = nils_registry::home::Home::new(home.path()).open().unwrap();
        let store = reg.store();
        let d = store.dialect();
        let sql = format!(
            "SELECT actor, author_kind, why, value, CASE WHEN committed_at IS NULL THEN 0 ELSE 1 END FROM {} WHERE id = {}",
            store.qualified("decision"),
            d.param(1, nils_registry::schema::Type::Int)
        );
        let row = store
            .query_opt(&sql, &[nils_registry::store::Param::Int(decision)])
            .unwrap()
            .expect("the decision");
        assert_eq!(row.text(0).unwrap(), "cleo@lab");
        assert_eq!(row.text(1).unwrap(), "person");
        let why = row.opt_text(2).unwrap().unwrap_or("").to_string();
        assert!(
            why.contains("bp-head@1's neck at 0.700 (run 41): looked right"),
            "{why}"
        );
        assert_eq!(row.opt_text(3).unwrap(), Some("neck"));
        assert_eq!(row.int(4).unwrap(), 1, "in force");
    }
    // the re-sort runs as a job the worker takes, and the scan then reads neck
    let mut state = String::new();
    for _ in 0..120 {
        let j = server.ok("GET", &format!("/api/jobs/{job}"), None, CURATOR);
        state = j["state"].as_str().unwrap_or("").to_string();
        if ["done", "failed", "cancelled"].contains(&state.as_str()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert_eq!(state, "done", "the re-sort job {job}");
    let sorted = server.ok("GET", &format!("/api/origins?stack={s5}"), None, CURATOR);
    let bp = sorted["stacks"][0]["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == "body_part")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(bp["values"], json!(["neck"]), "{sorted}");
    assert_eq!(bp["state"], "current", "{sorted}");
    // the page no longer offers the accepted scan; the set-apart and decided ones wait
    let neck = server.ok("GET", &at("&value=neck&max=0.9"), None, CURATOR);
    assert_eq!(neck["total"], 1, "{neck}");
    assert_eq!(neck["scans"][0]["stack"], s4, "{neck}");
    // every scan set apart: nothing written, no job
    let none = server.ok(
        "POST",
        "/api/review/proposals/accept",
        Some(json!({"model": model_id.to_string(), "axis": "body_part", "value": "spine", "seen": [s3], "except": [s3]})),
        CURATOR,
    );
    assert_eq!(none["decided"], 0, "{none}");
    assert_eq!(none["set_apart"], 1, "{none}");
    assert_eq!(none["resort"]["state"], "done", "{none}");
    assert!(none["resort"]["job"].is_null(), "{none}");
    // what the proposals door counts agrees: the spine scan, the neck a
    // person decided and the sealed one (which the counts do not withhold)
    // still wait below the threshold
    let standing = &server.ok("GET", "/api/review/proposals", None, CURATOR)["proposals"][0];
    assert_eq!(standing["below"]["stacks"], 3, "{standing}");
    drop(server);

    // ------------------------------------------------ the keyboard
    let out = ok(
        &home,
        &[
            "review",
            "proposals-scans",
            "--model",
            &model_id.to_string(),
            "--axis",
            "body_part",
            "--json",
        ],
        None,
    );
    let doc: Value = serde_json::from_str(out.trim()).unwrap();
    // the spine scan and the neck a person decided
    assert_eq!(doc["total"], 2, "{doc}");
    assert_eq!(doc["withheld_sealed"], 1, "{doc}");
    assert_eq!(doc["scans"][1]["decided"], true, "{doc}");
    assert!(
        doc["scans"][0].get("picture").is_none(),
        "no pictures at the keyboard: {doc}"
    );
    // the spine item has one scan: accepting it closes the item, and the
    // keyboard re-sorts at once
    let out = ok(
        &home,
        &[
            "review",
            "proposals-accept",
            "--model",
            "bp-head@1",
            "--axis",
            "body_part",
            "--value",
            "spine",
            "--seen",
            &s3.to_string(),
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
        None,
    );
    let doc: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(doc["decided"], 1, "{doc}");
    assert_eq!(doc["set_apart"], 0, "{doc}");
    assert_eq!(doc["closed"].as_array().unwrap().len(), 1, "{doc}");
    assert_eq!(doc["left_in_items"], 0, "{doc}");
    assert_eq!(doc["resort"]["state"], "done", "{doc}");
    assert_eq!(doc["resort"]["sorted"], 1, "{doc}");
    {
        let mut reg = nils_registry::home::Home::new(home.path()).open().unwrap();
        let item = doc["closed"][0].as_i64().unwrap();
        let store = reg.store();
        let d = store.dialect();
        let sql = format!(
            "SELECT status FROM {} WHERE id = {}",
            store.qualified("review_item"),
            d.param(1, nils_registry::schema::Type::Int)
        );
        let row = store
            .query_opt(&sql, &[nils_registry::store::Param::Int(item)])
            .unwrap()
            .expect("the item");
        assert_eq!(row.text(0).unwrap(), "accepted");
    }
    // the job's own command, by hand: the scans the decisions reach are
    // sorted again, and a decision none reaches is counted
    let accepted = doc["decisions"][0]["decision"].as_i64().unwrap();
    let out = ok(
        &home,
        &[
            "review",
            "resort",
            "--decisions",
            &format!("{decision},{accepted}"),
            "--who",
            "cleo@lab",
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
        None,
    );
    assert!(out.contains("re-sort: 2 of 2 scan(s) re-sorted"), "{out}");
    let (good, _, err) = run(&home, &["review", "resort"], None);
    assert!(!good && err.contains("--decisions"), "{err}");
}

const SCHEMA_CORRECT: &str = "nils_proposal_grid_correct";

#[test]
fn the_few_scans_set_apart_take_the_person_s_value_in_the_same_accept() {
    corrected(None);
}

#[test]
fn the_few_scans_set_apart_take_the_person_s_value_in_the_same_accept_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    drop_schema(&dsn, SCHEMA_CORRECT);
    corrected(Some(&dsn));
    drop_schema(&dsn, SCHEMA_CORRECT);
}

/// The decision's actor, author kind, words, value and whether in force.
fn decision_row(home: &TempDir, decision: i64) -> (String, String, String, Option<String>, i64) {
    let mut reg = nils_registry::home::Home::new(home.path()).open().unwrap();
    let store = reg.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT actor, author_kind, why, value, CASE WHEN committed_at IS NULL THEN 0 ELSE 1 END FROM {} WHERE id = {}",
        store.qualified("decision"),
        d.param(1, nils_registry::schema::Type::Int)
    );
    let row = store
        .query_opt(&sql, &[nils_registry::store::Param::Int(decision)])
        .unwrap()
        .expect("the decision");
    (
        row.text(0).unwrap().to_string(),
        row.text(1).unwrap().to_string(),
        row.opt_text(2).unwrap().unwrap_or("").to_string(),
        row.opt_text(3).unwrap().map(str::to_string),
        row.int(4).unwrap(),
    )
}

/// UX round 6: a page of five spine proposals; the person corrects two to
/// brain and neck and accepts the rest in one move.
fn corrected(dsn: Option<&str>) {
    let home = registry(dsn, SCHEMA_CORRECT);
    let ids = stacks_by_description(&home);
    let [s1, s2, s3, s4, s5, s6] = ids[..] else {
        unreachable!()
    };
    // five unsure spine proposals and one sure brain (the commit's)
    let model_id = model_proposing(
        &home,
        &[
            (s1, "spine", 0.7),
            (s2, "spine", 0.65),
            (s3, "spine", 0.6),
            (s4, "spine", 0.55),
            (s5, "spine", 0.5),
            (s6, "brain", 0.95),
        ],
    );
    let server = Server::start(&home);
    let at = |q: &str| format!("/api/review/proposals/scans?model={model_id}&axis=body_part{q}");

    // ------------------------------------------------ the page says the axis's values
    let page = server.ok("GET", &at("&value=spine"), None, CURATOR);
    assert_eq!(page["total"], 5, "{page}");
    let legal: Vec<&str> = page["legal"]
        .as_array()
        .expect("legal: the axis's values")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for v in ["brain", "brain-neck", "chest", "neck", "spine", "other"] {
        assert!(legal.contains(&v), "{v} among {legal:?}");
    }
    // the value chips with their counts are still `values`
    assert_eq!(
        page["values"],
        json!([{"value": "spine", "count": 5}]),
        "{page}"
    );
    let body = |corrections: Value, except: Value| {
        json!({"model": model_id.to_string(), "axis": "body_part", "value": "spine",
               "seen": [s1, s2, s3, s4], "except": except, "corrections": corrections, "why": "seen"})
    };

    // ------------------------------------------------ refusals
    // a value the axis does not take, named
    let (status, doc) = server.call(
        "POST",
        "/api/review/proposals/accept",
        Some(body(json!([{"stack": s1, "value": "torso"}]), json!([]))),
        CURATOR,
    );
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap_or("")
            .contains("torso is not a value of body_part"),
        "{doc}"
    );
    // a correction of a scan the page did not show
    let (status, doc) = server.call(
        "POST",
        "/api/review/proposals/accept",
        Some(body(json!([{"stack": s5, "value": "brain"}]), json!([]))),
        CURATOR,
    );
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap_or("")
            .contains("corrected, but the page did not show it"),
        "{doc}"
    );
    // a scan both set apart and corrected
    let (status, doc) = server.call(
        "POST",
        "/api/review/proposals/accept",
        Some(body(json!([{"stack": s1, "value": "brain"}]), json!([s1]))),
        CURATOR,
    );
    assert_eq!(status, 400, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap_or("")
            .contains("both set apart and corrected"),
        "{doc}"
    );
    // a malformed correction
    let (status, doc) = server.call(
        "POST",
        "/api/review/proposals/accept",
        Some(body(json!([{"stack": s1}]), json!([]))),
        CURATOR,
    );
    assert_eq!(status, 400, "{doc}");
    // nothing was written by any of it
    let page = server.ok("GET", &at("&value=spine"), None, CURATOR);
    assert_eq!(page["total"], 5, "{page}");

    // ------------------------------------------------ the accept
    // a person decided s4 meanwhile, through another question
    {
        let mut reg = nils_registry::home::Home::new(home.path()).open().unwrap();
        let store = reg.store();
        let d = store.dialect();
        let sql = format!(
            "INSERT INTO {} (scope, ref, axis, value, actor, author_kind, decided_at, committed_at, committed_by) VALUES ('stack', {}, 'body_part', 'neck', 'bo@lab', 'person', {}, {}, 'bo@lab')",
            store.qualified("decision"),
            d.param(1, nils_registry::schema::Type::Text),
            d.param(2, nils_registry::schema::Type::Timestamp),
            d.param(3, nils_registry::schema::Type::Timestamp),
        );
        store
            .execute(
                &sql,
                &[
                    nils_registry::store::Param::from(s4.to_string()),
                    nils_registry::store::Param::from("2026-11-09T12:00:00Z"),
                    nils_registry::store::Param::from("2026-11-09T12:00:00Z"),
                ],
            )
            .unwrap();
    }
    // s1 to brain, s2 to neck, s4 corrected too but kept as the person
    // decided it; s3 accepted as spine
    let done = server.ok(
        "POST",
        "/api/review/proposals/accept",
        Some(body(
            json!([{"stack": s1, "value": "brain"}, {"stack": s2, "value": "neck"}, {"stack": s4, "value": "chest"}]),
            json!([]),
        )),
        CURATOR,
    );
    assert_eq!(done["decided"], 1, "{done}");
    assert_eq!(done["corrected"], 2, "{done}");
    assert_eq!(done["kept_decided"], 1, "{done}");
    assert_eq!(done["set_apart"], 0, "{done}");
    assert_eq!(done["decisions"].as_array().unwrap().len(), 3, "{done}");
    assert_eq!(done["corrections"].as_array().unwrap().len(), 2, "{done}");
    assert_eq!(done["resort"]["state"], "queued", "{done}");
    assert_eq!(done["resort"]["stacks"], 3, "{done}");
    let job = done["resort"]["job"].as_i64().expect("a job");
    let by_stack = |stack: i64| -> Value {
        done["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["stack"] == stack)
            .cloned()
            .unwrap_or(Value::Null)
    };
    // the corrected ones carry the person's value, in the person's name, in
    // force, with the model's value, probability and run kept in the words
    // and the correction marked
    let one = by_stack(s1);
    assert_eq!(one["value"], "brain", "{one}");
    assert_eq!(one["corrected"], true, "{one}");
    let (actor, kind, why, value, in_force) =
        decision_row(&home, one["decision"].as_i64().unwrap());
    assert_eq!(actor, "cleo@lab");
    assert_eq!(kind, "person");
    assert_eq!(value.as_deref(), Some("brain"));
    assert_eq!(in_force, 1, "in force");
    assert!(
        why.contains("corrected bp-head@1's spine at 0.700 (run 41) to brain: seen"),
        "{why}"
    );
    let two = by_stack(s2);
    let (_, _, why, value, _) = decision_row(&home, two["decision"].as_i64().unwrap());
    assert_eq!(value.as_deref(), Some("neck"));
    assert!(
        why.contains("corrected bp-head@1's spine at 0.650 (run 41) to neck"),
        "{why}"
    );
    // the rest take the model's value, as before
    let three = by_stack(s3);
    assert_eq!(three["value"], "spine", "{three}");
    assert_eq!(three["corrected"], false, "{three}");
    let (_, _, why, value, _) = decision_row(&home, three["decision"].as_i64().unwrap());
    assert_eq!(value.as_deref(), Some("spine"));
    assert!(
        why.contains("accepted bp-head@1's spine at 0.600 (run 41): seen"),
        "{why}"
    );
    // the one decided meanwhile keeps the person's decision
    assert!(by_stack(s4).is_null(), "{done}");
    // the re-sort runs, and the corrected scan then reads brain
    let mut state = String::new();
    for _ in 0..120 {
        let j = server.ok("GET", &format!("/api/jobs/{job}"), None, CURATOR);
        state = j["state"].as_str().unwrap_or("").to_string();
        if ["done", "failed", "cancelled"].contains(&state.as_str()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert_eq!(state, "done", "the re-sort job {job}");
    let sorted = server.ok("GET", &format!("/api/origins?stack={s1}"), None, CURATOR);
    let bp = sorted["stacks"][0]["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == "body_part")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(bp["values"], json!(["brain"]), "{sorted}");
    assert_eq!(bp["decided"]["kind"], "person", "{sorted}");
    // the page offers the one scan left (and the decided one still waits)
    let page = server.ok("GET", &at("&value=spine"), None, CURATOR);
    assert_eq!(page["total"], 2, "{page}");
    drop(server);

    // ------------------------------------------------ the keyboard
    let out = ok(
        &home,
        &[
            "review",
            "proposals-accept",
            "--model",
            "bp-head@1",
            "--axis",
            "body_part",
            "--value",
            "spine",
            "--seen",
            &s5.to_string(),
            "--correct",
            &format!("{s5}=brain-neck"),
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
        None,
    );
    assert!(
        out.contains(
            "accepted 0 scan(s) as spine of bp-head@1 on body_part, corrected 1 (1 brain-neck)"
        ),
        "{out}"
    );
    // its refusals: a malformed pair, and a value the axis does not take
    let (good, _, err) = run(
        &home,
        &[
            "review",
            "proposals-accept",
            "--model",
            "bp-head@1",
            "--axis",
            "body_part",
            "--value",
            "spine",
            "--seen",
            &s4.to_string(),
            "--correct",
            "brain",
        ],
        None,
    );
    assert!(!good && err.contains("STACK=VALUE"), "{err}");
    let (good, _, err) = run(
        &home,
        &[
            "review",
            "proposals-accept",
            "--model",
            "bp-head@1",
            "--axis",
            "body_part",
            "--value",
            "spine",
            "--seen",
            &s4.to_string(),
            "--correct",
            &format!("{s4}=torso"),
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
        None,
    );
    assert!(
        !good && err.contains("torso is not a value of body_part"),
        "{err}"
    );
}
