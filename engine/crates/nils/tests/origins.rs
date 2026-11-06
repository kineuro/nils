// SPDX-License-Identifier: AGPL-3.0-only
//! Where each axis label of a scan came from (UX round 2), end to end: one
//! visit of three scans brought in and sorted at the keyboard. `nils
//! origins` and `GET /api/origins` say each axis's value came from the
//! rules, with the pack and the rule; a campaign's close decides the spine
//! scan's body part, and the label then says it came from that decision and
//! links the campaign's item. With the axis put back to the rules' value,
//! as an engine whose close did not re-sort left it, the label says it was
//! decided neck and sorted before the decision; `POST /api/origins/resort`
//! sorts that scan again and the label reads neck, the other scans
//! untouched. A decision withdrawn says so, and `nils origins --resort`
//! brings the rules' value back. The grants: a reader reads the origins at
//! detail plain without the words a rule matched, a reviewer with them, and
//! only who may run a sort sorts again. On SQLite always, and on Postgres
//! where a test DSN is set.

use std::io::{BufRead, Read as _, Write as _};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use nils_registry::store::Cell;
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packs")
        .to_str()
        .unwrap()
        .to_string()
}

/// Reading data at detail plain, and nothing else.
const READS: &str = "a-reader-token-of-its-length";
/// Review at detail quasi, and no pipelines work.
const REVIEWS: &str = "a-reviewer-token-of-length";
/// Pipelines work: who may run a sort.
const OPS: &str = "an-operator-token-of-length";
/// The model registry alone: neither data nor review.
const MODELS: &str = "a-models-only-token-length";

struct Home {
    dir: TempDir,
    /// The Postgres DSN and schema, when the registry lives there.
    pg: Option<(String, String)>,
}

impl Home {
    fn run(&self, who: &str, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        let mut child = nils()
            .arg("--registry")
            .arg(self.dir.path())
            .args(args)
            .env("USER", "cleo")
            .env("HOSTNAME", "lab")
            .env("NILS_PRINCIPAL", who)
            .env("NILS_PACK_DIR", packs())
            .env_remove("NILS_DSN")
            .env_remove("NILS_ACTOR")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
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

    fn ok(&self, who: &str, args: &[&str]) -> String {
        let (good, out, err) = self.run(who, args, None);
        assert!(good, "nils {args:?} failed: {err}\n{out}");
        out
    }

    fn json(&self, who: &str, args: &[&str]) -> Value {
        let out = self.ok(who, args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("nils {args:?}: {e}\n{out}"))
    }

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap(),
        }
    }

    fn exec(&self, sql: &str) {
        let mut store = self.store();
        let text = qualified(&store, sql);
        store.execute(&text, &[]).unwrap();
    }

    fn rows(&self, sql: &str) -> Vec<Vec<Value>> {
        let mut store = self.store();
        let text = qualified(&store, sql);
        store
            .query(&text, &[])
            .unwrap()
            .iter()
            .map(|r| {
                r.0.iter()
                    .map(|c| match c {
                        Cell::Int(n) => json!(n),
                        Cell::Bool(b) => json!(i64::from(*b)),
                        Cell::Double(d) => json!(d),
                        Cell::Text(t) => json!(t),
                        Cell::Null | Cell::Bytes(_) => Value::Null,
                    })
                    .collect()
            })
            .collect()
    }
}

/// A statement's `{table}` names, as the store qualifies them.
fn qualified(store: &Store, sql: &str) -> String {
    let mut text = sql.to_string();
    for t in [
        "classification_axis",
        "classification_evidence",
        "classification",
        "stack_fingerprint",
        "session_cache_study",
        "decision",
    ] {
        text = text.replace(&format!("{{{t}}}"), &store.qualified(t));
    }
    text
}

/// A `nils serve` with the four tokens, killed when dropped.
struct Served {
    child: Child,
    port: u16,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Served {
    fn start(home: &Home) -> Served {
        let tokens = [
            format!("{READS}=lou@lab:reader"),
            format!("{REVIEWS}=rae@lab:reviewer"),
            format!("{OPS}=ops@lab:operator"),
            format!("{MODELS}=mo@lab:models:see"),
        ]
        .join(",");
        let child = nils()
            .arg("--registry")
            .arg(home.dir.path())
            .args(["serve", "--bind", "127.0.0.1:0", "--workers", "2"])
            .args(["--auth", "token", "--pack-dir", &packs()])
            .env("NILS_TOKENS", tokens)
            .env("USER", "cleo")
            .env("HOSTNAME", "lab")
            .env_remove("NILS_DSN")
            .env_remove("NILS_PACK_DIR")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut held = Served { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        std::thread::spawn(move || for _ in lines {});
        held
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let response = String::from_utf8_lossy(&bytes).to_string();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            serde_json::from_str(text).unwrap_or(Value::String(text.to_string())),
        )
    }

    fn get(&self, path: &str, token: &str) -> Value {
        let (status, doc) = self.call("GET", path, None, token);
        assert_eq!(status, 200, "{path}: {doc}");
        doc
    }
}

/// One single-image series of a study.
fn series(dir: &TempDir, study: &str, n: usize, description: &str) {
    let series = format!("{study}.{n}");
    let sop = format!("{series}.1");
    let mut e = synth::minimal_mr(study, &series, &sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, "P-ONE"));
    e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
    e.push(synth::text(tags::SERIES_NUMBER, VR::IS, &n.to_string()));
    e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
    dir.file(
        &format!("s{n}/IM_0001"),
        &synth::part10(&MetaFields::mr(&sop), &e, true),
    );
}

/// The stack a series description names.
fn stack_named(home: &Home, description: &str) -> i64 {
    home.rows(&format!(
        "SELECT stack_id FROM {{stack_fingerprint}} WHERE text_series_description = '{description}'"
    ))[0][0]
        .as_i64()
        .unwrap()
}

/// One stack's answer, and one axis of it.
fn stack_of(doc: &Value, stack: i64) -> &Value {
    doc["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["stack"] == stack)
        .unwrap_or_else(|| panic!("no stack {stack} in {doc}"))
}

fn axis_of<'a>(doc: &'a Value, stack: i64, axis: &str) -> &'a Value {
    stack_of(doc, stack)["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == axis)
        .unwrap_or_else(|| panic!("no {axis} on stack {stack} in {doc}"))
}

fn round(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("origins-home"),
        pg,
    };
    let (good, _, err) = home.run(
        "cleo@lab",
        &["key", "add", "k"],
        Some("an origins test key\n"),
    );
    assert!(good, "{err}");
    match &home.pg {
        Some((dsn, schema)) => {
            home.ok(
                "cleo@lab",
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
            home.ok("cleo@lab", &["init", "--key", "k"]);
        }
    }
    let src = TempDir::new("origins-src");
    let study = "1.2.826.0.1.3680043.8.499.7.1";
    series(&src, study, 1, "t1 mprage");
    series(&src, study, 2, "t2 spine");
    series(&src, study, 3, "t2 flair");
    home.ok(
        "cleo@lab",
        &[
            "digest",
            "--name",
            "visit",
            "--no-private",
            src.path().to_str().unwrap(),
        ],
    );
    home.ok("cleo@lab", &["fingerprint"]);
    home.ok("cleo@lab", &["classify", "--pack-dir", &packs()]);
    home.ok("cleo@lab", &["session", "rebuild"]);
    let spine = stack_named(&home, "t2 spine");
    let brain = stack_named(&home, "t1 mprage");
    let study_id = home.rows(&format!(
        "SELECT study_id FROM {{stack_fingerprint}} WHERE stack_id = {spine}"
    ))[0][0]
        .as_i64()
        .unwrap();
    let session = home.rows(&format!(
        "SELECT session_id FROM {{session_cache_study}} WHERE study_id = {study_id} ORDER BY session_id"
    ))[0][0]
        .as_i64()
        .unwrap();

    // the rules said it: the pack, its version and the rule
    let doc = home.json(
        "cleo@lab",
        &["origins", "--session", &session.to_string(), "--json"],
    );
    assert_eq!(doc["count"], 3, "{doc}");
    assert_eq!(doc["unsorted"], 0, "{doc}");
    let bp = axis_of(&doc, spine, "body_part");
    assert_eq!(bp["values"], json!(["spine"]), "{bp}");
    assert_eq!(bp["origin"], "rules", "{bp}");
    assert_eq!(bp["state"], "current", "{bp}");
    assert!(bp["decided"].is_null(), "{bp}");
    assert!(
        !bp["rules"][0]["rule"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "{bp}"
    );
    assert_eq!(stack_of(&doc, spine)["sorted"]["pack"], "mri", "{doc}");
    assert!(
        bp["line"].as_str().unwrap().starts_with("rules mri@"),
        "{bp}"
    );
    // the same visit named by its studies, and one scan
    let by_studies = home.json(
        "cleo@lab",
        &["origins", "--studies", &study_id.to_string(), "--json"],
    );
    assert_eq!(by_studies["count"], 3, "{by_studies}");
    let one = home.json(
        "cleo@lab",
        &["origins", "--stack", &spine.to_string(), "--json"],
    );
    assert_eq!(one["count"], 1, "{one}");
    let text = home.ok("cleo@lab", &["origins", "--stack", &spine.to_string()]);
    assert!(text.contains("body_part: spine (rules mri@"), "{text}");
    // the rules' rows on the spine scan, kept to put back below
    let rules_rows = home.rows(&format!(
        "SELECT value, confidence, tier FROM {{classification_axis}} WHERE stack_id = {spine} AND axis = 'body_part'"
    ));

    // a campaign of the body part, one rater; its close decides and re-sorts
    let work = TempDir::new("origins-work");
    let every = work.path().join("every.json");
    std::fs::write(
        &every,
        json!({"ast_version": 1, "sets": {"every": {"grain": "stack"}}, "out": {"set": "every", "level": "record"}}).to_string(),
    )
    .unwrap();
    home.ok(
        "cleo@lab",
        &[
            "ask",
            "selections",
            "save",
            "--name",
            "every",
            "--file",
            every.to_str().unwrap(),
            "--pack-dir",
            &packs(),
        ],
    );
    let made = home.json(
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
            "1",
            "--closes-into",
            "decision",
            "--pack-dir",
            &packs(),
            "--json",
        ],
    );
    let n = made["items"].as_array().unwrap().len();
    assert_eq!(n, 3, "{made}");
    for _ in 0..n {
        let claimed = home.json("anna@lab", &["campaign", "claim", "curate", "--json"]);
        let a = claimed["assignment"]["id"].as_i64().unwrap().to_string();
        let value = if claimed["item"]["stack_id"] == spine {
            "neck"
        } else {
            "brain"
        };
        home.ok("anna@lab", &["campaign", "answer", &a, "--value", value]);
    }
    home.ok("cleo@lab", &["campaign", "close", "curate", "--json"]);
    let doc = home.json(
        "cleo@lab",
        &["origins", "--stack", &spine.to_string(), "--json"],
    );
    let bp = axis_of(&doc, spine, "body_part");
    assert_eq!(bp["values"], json!(["neck"]), "{bp}");
    assert_eq!(bp["origin"], "decision", "{bp}");
    assert_eq!(bp["state"], "current", "{bp}");
    assert_eq!(bp["decided"]["campaign"]["name"], "curate", "{bp}");
    assert!(bp["decided"]["campaign"]["item"].is_i64(), "{bp}");
    assert_eq!(bp["decided"]["kind"], "person", "{bp}");
    let decision = bp["decided"]["id"].as_i64().unwrap();

    // the axis put back to the rules' value, as an engine whose close did
    // not re-sort left it (2026-11-05 on production)
    home.exec(&format!(
        "DELETE FROM {{classification_axis}} WHERE stack_id = {spine} AND axis = 'body_part'"
    ));
    home.exec(&format!(
        "DELETE FROM {{classification_evidence}} WHERE stack_id = {spine} AND axis = 'body_part' AND tier = 'decision'"
    ));
    for r in &rules_rows {
        let value = r[0]
            .as_str()
            .map_or("NULL".to_string(), |v| format!("'{v}'"));
        home.exec(&format!(
            "INSERT INTO {{classification_axis}} (stack_id, axis, value, confidence, tier) VALUES ({spine}, 'body_part', {value}, {}, '{}')",
            r[1],
            r[2].as_str().unwrap()
        ));
    }

    // the sort that wrote the brain scan last (the close's re-sort), which
    // the spine scan's re-sort below leaves as it is
    let brain_job = home.rows(&format!(
        "SELECT job_id FROM {{classification}} WHERE stack_id = {brain}"
    ))[0][0]
        .clone();
    let served = Served::start(&home);
    let path = format!("/api/origins?session={session}");
    let doc = served.get(&path, READS);
    assert_eq!(doc["unsorted"], 1, "{doc}");
    let bp = axis_of(&doc, spine, "body_part");
    assert_eq!(bp["values"], json!(["spine"]), "{bp}");
    assert_eq!(bp["origin"], "rules", "{bp}");
    assert_eq!(bp["state"], "unsorted", "{bp}");
    assert_eq!(
        bp["words"], "decided neck; sorted before the decision",
        "{bp}"
    );
    assert_eq!(bp["decided"]["id"], decision, "{bp}");
    assert!(bp["decided"]["campaign"]["item"].is_i64(), "{bp}");
    // a reader at detail plain: no words a rule matched
    assert!(
        bp["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("matched").is_none()),
        "{bp}"
    );
    // a reviewer at detail quasi reads them
    let doc = served.get(&path, REVIEWS);
    let bp = axis_of(&doc, spine, "body_part");
    assert!(
        bp["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("matched").is_some()),
        "{bp}"
    );
    // neither data nor review: refused; a target named wrong: refused
    assert_eq!(served.call("GET", &path, None, MODELS).0, 403);
    assert_eq!(served.call("GET", "/api/origins", None, READS).0, 400);
    assert_eq!(
        served
            .call(
                "GET",
                &format!("/api/origins?stack={spine}&session={session}"),
                None,
                READS
            )
            .0,
        400
    );
    assert_eq!(
        served
            .call("GET", "/api/origins?stack=999999", None, READS)
            .0,
        404
    );

    // the re-sort: only who may run a sort; the scan reads neck, the
    // other scans untouched
    let body = json!({"stack": spine});
    let (status, _) = served.call("POST", "/api/origins/resort", Some(body.clone()), READS);
    assert_eq!(status, 403);
    let (status, _) = served.call("POST", "/api/origins/resort", Some(body.clone()), REVIEWS);
    assert_eq!(status, 403);
    let (status, done) = served.call("POST", "/api/origins/resort", Some(body), OPS);
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["resort"]["state"], "done", "{done}");
    assert_eq!(done["resort"]["sorted"], 1, "{done}");
    let bp = axis_of(&done["origins"], spine, "body_part");
    assert_eq!(bp["values"], json!(["neck"]), "{bp}");
    assert_eq!(bp["state"], "current", "{bp}");
    assert_eq!(bp["origin"], "decision", "{bp}");
    assert_eq!(
        home.rows(&format!(
            "SELECT job_id FROM {{classification}} WHERE stack_id = {brain}"
        ))[0][0],
        brain_job
    );
    drop(served);

    // a decision withdrawn: the held value says it was decided, and is no
    // longer; a re-sort at the keyboard brings back the rules' value
    home.ok("cleo@lab", &["review", "withdraw", &decision.to_string()]);
    let doc = home.json(
        "cleo@lab",
        &["origins", "--stack", &spine.to_string(), "--json"],
    );
    let bp = axis_of(&doc, spine, "body_part");
    assert_eq!(bp["state"], "withdrawn", "{bp}");
    assert_eq!(bp["origin"], "decision", "{bp}");
    let doc = home.json(
        "cleo@lab",
        &[
            "origins",
            "--stack",
            &spine.to_string(),
            "--resort",
            "--json",
        ],
    );
    assert_eq!(doc["resort"]["state"], "done", "{doc}");
    let bp = axis_of(&doc["origins"], spine, "body_part");
    assert_eq!(bp["values"], json!(["spine"]), "{bp}");
    assert_eq!(bp["origin"], "rules", "{bp}");
    assert_eq!(bp["state"], "current", "{bp}");
}

#[test]
fn a_label_says_where_it_came_from_and_a_decision_not_sorted_yet() {
    round(None);
}

#[test]
fn a_label_says_where_it_came_from_on_postgres_too() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_origins";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    round(Some((dsn.clone(), schema.to_string())));
    drop();
}
