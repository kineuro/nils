// SPDX-License-Identifier: AGPL-3.0-only

//! Record 26, sections 8 to 11: a dataset feeds its cohort, the cohort doors,
//! review by cohort, promotion at any grain and the explain door, driven as
//! a second process would drive them.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

/// Run the command line in the registry and hand back its stdout; a
/// failure hands back its stderr with `ok` false.
struct Ran {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run(home: &TempDir, args: &[&str]) -> Ran {
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    Ran {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

fn ok(home: &TempDir, args: &[&str]) -> String {
    let r = run(home, args);
    assert!(r.ok, "{}: {}", args.join(" "), r.stderr);
    r.stdout
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

/// A registry whose one source place is a dataset feeding the cohort
/// `fed`: two studies of one person, classified with their questions, and
/// one file the digest refuses.
fn registry() -> (TempDir, TempDir) {
    let home = TempDir::new("cohorts-home");
    let dir = TempDir::new("cohorts-src");
    for (study, sop, day) in [
        ("1.2.3.A", "1.2.3.A.1.1", "20220115"),
        ("1.2.3.B", "1.2.3.B.1.1", "20220715"),
    ] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::STUDY_DATE, VR::DA, day));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    dir.file("notes.txt", b"not a dicom file");
    let key = nils()
        .arg("--registry")
        .arg(home.path())
        .args(["key", "add", "k"])
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    key.stdin
        .as_ref()
        .unwrap()
        .write_all(b"a cohorts test key\n")
        .unwrap();
    assert!(key.wait_with_output().unwrap().status.success());
    ok(&home, &["init", "--key", "k"]);
    ok(
        &home,
        &[
            "place",
            "add",
            "ds",
            dir.path().to_str().unwrap(),
            "--role",
            "source",
            "--cohort",
            "fed",
        ],
    );
    ok(
        &home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            dir.path().to_str().unwrap(),
        ],
    );
    ok(&home, &["fingerprint"]);
    ok(
        &home,
        &[
            "classify",
            "--review-below",
            "1.0",
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
    );
    ok(&home, &["session", "rebuild"]);
    (home, dir)
}

/// The one subject's code, read from the registry itself.
fn code(home: &TempDir) -> String {
    let mut store = nils_registry::Store::open_sqlite(&home.path().join("registry.db")).unwrap();
    store
        .query("SELECT code FROM subject ORDER BY id", &[])
        .unwrap()[0]
        .text(0)
        .unwrap()
        .to_string()
}

struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(home: &TempDir, requests: usize, extra: &[&str]) -> Server {
        let mut cmd = nils();
        cmd.arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--requests",
            ])
            .arg(requests.to_string())
            .args(["--pack-dir", packs().to_str().unwrap()])
            .args(extra)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let first = lines.next().unwrap().unwrap();
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        Server { child, port }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        token: Option<&str>,
    ) -> (u16, serde_json::Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.unwrap_or("");
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        if !body.is_empty() {
            head.push_str("Content-Type: application/json\r\n");
        }
        if let Some(t) = token {
            head.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let json =
            serde_json::from_str(body).unwrap_or(serde_json::Value::String(body.to_string()));
        (status, json)
    }

    fn finish(mut self) {
        let status = self.child.wait().unwrap();
        assert!(status.success(), "nils serve exited {status}");
    }
}

#[test]
fn a_digest_of_a_dataset_feeds_its_cohort_and_review_reads_by_cohort() {
    let (home, _dir) = registry();
    // the batch's record carries what joined
    let batch = ok(&home, &["status", "--batch", "1", "--json"]);
    assert!(batch.contains("\"joined\""), "{batch}");
    let listed: serde_json::Value =
        serde_json::from_str(&ok(&home, &["clinical", "cohort", "list", "--json"])).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    let fed = &listed[0];
    assert_eq!(fed["name"], "fed", "{fed}");
    assert_eq!(fed["subjects"], 1, "{fed}");
    assert_eq!(fed["stacks"], 2, "{fed}");
    assert_eq!(fed["feeds"], serde_json::json!(["ds"]), "{fed}");
    assert_eq!(fed["from"]["kind"], "source", "{fed}");
    assert_eq!(fed["from"]["detail"]["dataset"], "ds", "{fed}");
    assert_eq!(fed["from"]["detail"]["batch"], 1, "{fed}");
    assert_eq!(fed["owner"], "anna@ward-3", "{fed}");
    assert!(fed["waiting"].as_i64().unwrap() > 0, "{fed}");
    assert_eq!(fed["releases"], 0, "{fed}");
    assert!(fed["retired_at"].is_null(), "{fed}");

    let server = Server::start(&home, 8, &[]);
    let (status, doc) = server.request("GET", "/api/cohorts/fed", None, None);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["description"], "fed by the dataset ds", "{doc}");
    let joins = doc["joins"].as_array().unwrap();
    assert_eq!(joins.len(), 1, "{doc}");
    assert_eq!(joins[0]["what"], "digest", "{doc}");
    assert_eq!(joins[0]["batch"], 1, "{doc}");
    assert_eq!(joins[0]["subjects"], 1, "{doc}");
    assert_eq!(joins[0]["left"], false, "{doc}");
    assert_eq!(
        doc["sources_holding"],
        serde_json::json!([{"place": "ds", "subjects": 1}]),
        "{doc}"
    );
    assert_eq!(doc["releases"], serde_json::json!([]), "{doc}");

    // the queue by cohort: every open question is about the one member
    let (status, all) = server.request("GET", "/api/review?status=open", None, None);
    assert_eq!(status, 200, "{all}");
    let (status, mine) = server.request("GET", "/api/review?status=open&cohort=fed", None, None);
    assert_eq!(status, 200, "{mine}");
    let about_subjects = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|it| it["kind"] != "ingest.quarantine")
        .count();
    assert!(about_subjects > 0, "{all}");
    assert_eq!(
        mine["count"].as_u64().unwrap() as usize,
        about_subjects,
        "{mine}"
    );
    let (status, none) = server.request("GET", "/api/review?cohort=nobody", None, None);
    assert_eq!(status, 404, "{none}");
    // the summary: by kind, by cohort, and the items about no member
    let (status, summary) = server.request("GET", "/api/review/summary", None, None);
    assert_eq!(status, 200, "{summary}");
    assert_eq!(
        summary["cohorts"],
        serde_json::json!([{"name": "fed", "open": about_subjects}]),
        "{summary}"
    );
    assert_eq!(
        summary["none"], 1,
        "the quarantine item is about no subject: {summary}"
    );
    assert_eq!(summary["by_kind"]["ingest.quarantine"], 1, "{summary}");
    let (status, narrowed) = server.request("GET", "/api/review/summary?cohort=fed", None, None);
    assert_eq!(status, 200, "{narrowed}");
    assert!(
        narrowed["by_kind"]["ingest.quarantine"].is_null(),
        "{narrowed}"
    );
    // the quarantine by cohort: the batches that fed it
    let (status, q) = server.request("GET", "/api/quarantine?cohort=fed", None, None);
    assert_eq!(status, 200, "{q}");
    assert_eq!(q["count"], 1, "{q}");
    let (status, q) = server.request("GET", "/api/quarantine?cohort=nobody", None, None);
    assert_eq!(status, 404, "{q}");
    // a second digest of the same tree joins nobody new and says so
    server.finish();
    let again = ok(
        &home,
        &[
            "digest",
            "--name",
            "b",
            "--no-private",
            "--json",
            _dir.path().to_str().unwrap(),
        ],
    );
    let report: serde_json::Value = serde_json::from_str(&again).unwrap();
    assert_eq!(report["joined"]["cohort"], "fed", "{report}");
    assert_eq!(report["joined"]["subjects"], 0, "{report}");
    assert_eq!(report["joined"]["met"], 1, "{report}");
    assert_eq!(report["joined"]["created"], false, "{report}");
}

#[test]
fn the_cohort_doors_make_rename_retire_and_change_members_by_hand() {
    let (home, _dir) = registry();
    let member = code(&home);
    let server = Server::start(&home, 16, &[]);
    // a name in use is refused
    let (status, doc) = server.request("POST", "/api/cohorts", Some(r#"{"name": "fed"}"#), None);
    assert_eq!(status, 409, "{doc}");
    let (status, doc) = server.request("POST", "/api/cohorts", Some(r#"{"name": "a b"}"#), None);
    assert_eq!(status, 400, "{doc}");
    // made, owned by the caller unless said
    let (status, made) = server.request(
        "POST",
        "/api/cohorts",
        Some(r#"{"name": "hand", "description": "picked by hand"}"#),
        None,
    );
    assert_eq!(status, 201, "{made}");
    assert_eq!(made["owner"], "anna@ward-3", "{made}");
    assert_eq!(made["from"]["kind"], "manual", "{made}");
    assert_eq!(made["subjects"], 0, "{made}");
    // renamed: the id stays
    let id = made["id"].as_i64().unwrap();
    let (status, renamed) = server.request(
        "PUT",
        "/api/cohorts/hand",
        Some(r#"{"name": "hands"}"#),
        None,
    );
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["name"], "hands", "{renamed}");
    assert_eq!(renamed["id"], id, "{renamed}");
    let (status, gone) = server.request("GET", "/api/cohorts/hand", None, None);
    assert_eq!(status, 404, "{gone}");
    // an unknown code is refused before anything is written
    let body = format!(r#"{{"add": ["{member}", "S-nobody"], "why": "a test"}}"#);
    let (status, refused) = server.request("POST", "/api/cohorts/hands/members", Some(&body), None);
    assert_eq!(status, 400, "{refused}");
    assert_eq!(
        refused["unknown"],
        serde_json::json!(["S-nobody"]),
        "{refused}"
    );
    let (_, shown) = server.request("GET", "/api/cohorts/hands", None, None);
    assert_eq!(shown["subjects"], 0, "nothing was written: {shown}");
    // added, then already a member, then removed with the reason kept
    let body = format!(r#"{{"add": ["{member}"], "why": "asked for"}}"#);
    let (status, added) = server.request("POST", "/api/cohorts/hands/members", Some(&body), None);
    assert_eq!(status, 200, "{added}");
    assert_eq!(added["added"], 1, "{added}");
    let (status, again) = server.request("POST", "/api/cohorts/hands/members", Some(&body), None);
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["added"], 0, "{again}");
    assert_eq!(again["already"], 1, "{again}");
    assert!(
        again["epoch"].as_i64().unwrap() > added["epoch"].as_i64().unwrap(),
        "{again}"
    );
    let body = format!(r#"{{"remove": ["{member}"], "why": "moved away"}}"#);
    let (status, removed) = server.request("POST", "/api/cohorts/hands/members", Some(&body), None);
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["removed"], 1, "{removed}");
    let (_, shown) = server.request("GET", "/api/cohorts/hands", None, None);
    assert_eq!(shown["subjects"], 0, "{shown}");
    let joins = shown["joins"].as_array().unwrap();
    assert_eq!(joins.len(), 2, "{shown}");
    assert_eq!(joins[0]["what"], "remove", "{shown}");
    assert_eq!(joins[0]["reason"], "moved away", "{shown}");
    assert_eq!(joins[1]["what"], "manual", "{shown}");
    assert_eq!(joins[1]["left"], true, "{shown}");
    // retired: gone from the summary, kept with its history; and back
    let (status, retired) = server.request(
        "PUT",
        "/api/cohorts/hands",
        Some(r#"{"retired": true}"#),
        None,
    );
    assert_eq!(status, 200, "{retired}");
    assert!(retired["retired_at"].is_string(), "{retired}");
    let (_, summary) = server.request("GET", "/api/summary", None, None);
    assert_eq!(summary["cohorts"], 1, "{summary}");
    let (_, listed) = server.request("GET", "/api/cohorts", None, None);
    assert_eq!(listed.as_array().unwrap().len(), 2, "{listed}");
    let (status, back) = server.request(
        "PUT",
        "/api/cohorts/hands",
        Some(r#"{"retired": false}"#),
        None,
    );
    assert_eq!(status, 200, "{back}");
    assert!(back["retired_at"].is_null(), "{back}");
    let (_, summary) = server.request("GET", "/api/summary", None, None);
    assert_eq!(summary["cohorts"], 2, "{summary}");
    server.finish();
    // every act is on the audit log
    let audit = ok(&home, &["audit", "list", "--action", "cohort.", "--json"]);
    for action in [
        "cohort.create",
        "cohort.join",
        "cohort.rename",
        "cohort.member.add",
        "cohort.member.remove",
        "cohort.retire",
        "cohort.restore",
    ] {
        assert!(
            audit.contains(&format!("\"{action}\"")),
            "{action}: {audit}"
        );
    }
    // and the command line does the same
    let shown = ok(&home, &["clinical", "cohort", "show", "hands"]);
    assert!(shown.contains("cohort hands"), "{shown}");
    ok(
        &home,
        &[
            "clinical",
            "cohort",
            "make",
            "third",
            "--description",
            "by hand",
        ],
    );
    ok(
        &home,
        &[
            "clinical", "cohort", "add", "third", &member, "--why", "asked",
        ],
    );
    ok(
        &home,
        &["clinical", "cohort", "rename", "third", "--to", "fourth"],
    );
    let r = run(&home, &["clinical", "cohort", "add", "fourth", "S-nobody"]);
    assert!(!r.ok && r.stderr.contains("S-nobody"), "{}", r.stderr);
    ok(&home, &["clinical", "cohort", "retire", "fourth"]);
    let listed = ok(&home, &["clinical", "cohort", "list"]);
    assert!(
        listed.contains("fourth") && listed.contains("(retired)"),
        "{listed}"
    );
}

#[test]
fn a_session_handle_promotes_the_explain_door_matches_the_command_line_and_the_grants_hold() {
    let (home, _dir) = registry();
    let server = Server::start(
        &home,
        8,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=lou@lab:reader",
            "--token",
            "a-reviewer-token-of-leng=rev@lab:reviewer",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
    );
    let reader = Some("a-reader-token-of-length");
    let reviewer = Some("a-reviewer-token-of-leng");
    let operator = Some("an-operator-token-of-len");
    // a session grain answer, kept as a handle
    let document = serde_json::json!({
        "ast_version": 1,
        "name": "visits",
        "scheme": "default",
        "sets": {
            "people": {"grain": "subject"},
            "visits": {"grain": "session", "of": "people"}
        },
        "keep": ["visits"],
        "out": {"set": "visits", "level": "record"}
    });
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(
            &serde_json::json!({"document": document, "name": "visits", "keep": true}).to_string(),
        ),
        operator,
    );
    assert_eq!(status, 200, "{ran}");
    let handle = ran["handle"].as_i64().unwrap();
    assert!(ran["row_count"].as_i64().unwrap() >= 1, "{ran}");
    // the grants: reading a cohort is data:see, writing one data:work,
    // promoting data:work too, and why a stack was judged so review:see
    let (status, listed) = server.request("GET", "/api/cohorts", None, reader);
    assert_eq!(status, 200, "{listed}");
    let (status, refused) =
        server.request("POST", "/api/cohorts", Some(r#"{"name": "no"}"#), reader);
    assert_eq!(status, 403, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("data:work"),
        "{refused}"
    );
    let (status, refused) = server.request(
        "POST",
        &format!("/api/ask/handles/{handle}/promote"),
        Some(r#"{"cohort": "visits", "create": true}"#),
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("data:work"),
        "{refused}"
    );
    let (status, refused) = server.request("GET", "/api/explain/1", None, reader);
    assert_eq!(status, 403, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("review:see"),
        "{refused}"
    );
    let (status, door) = server.request("GET", "/api/explain/1", None, reviewer);
    assert_eq!(status, 200, "{door}");
    let (status, missing) = server.request("GET", "/api/explain/999", None, reviewer);
    assert_eq!(status, 404, "{missing}");
    // the policy rows name the new doors with their grants
    let (_, caps) = server.request("GET", "/api/capabilities", None, operator);
    let policy = caps["policy"].as_array().unwrap();
    let grant = |door: &str| {
        policy
            .iter()
            .find(|r| r["door"] == door)
            .unwrap_or_else(|| panic!("no policy row for {door}: {caps}"))["grant"]
            .clone()
    };
    assert_eq!(grant("GET /api/cohorts"), "data:see");
    assert_eq!(grant("POST /api/cohorts/{name}/members"), "data:work");
    assert_eq!(grant("POST /api/ask/handles/{id}/promote"), "data:work");
    assert_eq!(grant("GET /api/explain/{stack}"), "review:see");
    assert_eq!(grant("GET /api/review/summary"), "review:see");
    server.finish();

    // the door answers what the command line reads
    let cli: serde_json::Value = serde_json::from_str(&ok(
        &home,
        &[
            "explain",
            "1",
            "--json",
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
    ))
    .unwrap();
    assert_eq!(cli, door, "the door and the command line disagree");
    assert_eq!(door["pack"], "mri", "{door}");
    let axes = door["axes"].as_array().unwrap();
    assert!(!axes.is_empty(), "{door}");
    assert!(
        axes.iter()
            .all(|a| a.get("evidence").is_some() && a.get("decision").is_some()),
        "{door}"
    );

    // the session grain handle promotes: the distinct subjects of its rows
    let promoted: serde_json::Value = serde_json::from_str(&ok(
        &home,
        &[
            "ask",
            "promote",
            "--handle",
            &handle.to_string(),
            "--cohort",
            "visits",
            "--create",
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(promoted["grain"], "session", "{promoted}");
    assert_eq!(promoted["added"], 1, "{promoted}");
    assert_eq!(promoted["created"], true, "{promoted}");
    assert_eq!(promoted["rows"], ran["row_count"], "{promoted}");
    let shown: serde_json::Value = serde_json::from_str(&ok(
        &home,
        &["clinical", "cohort", "show", "visits", "--json"],
    ))
    .unwrap();
    assert_eq!(shown["from"]["kind"], "promotion", "{shown}");
    assert_eq!(shown["from"]["detail"]["handle"], handle, "{shown}");
    assert_eq!(shown["subjects"], 1, "{shown}");
    assert_eq!(shown["joins"][0]["what"], "promotion", "{shown}");
    assert_eq!(shown["joins"][0]["handle"], handle, "{shown}");
    // the interval says what kind of answer opened it
    let mut store = nils_registry::Store::open_sqlite(&home.path().join("registry.db")).unwrap();
    let params = store
        .query(
            "SELECT params FROM cohort_member WHERE source = 'promotion'",
            &[],
        )
        .unwrap()[0]
        .text(0)
        .unwrap()
        .to_string();
    let params: serde_json::Value = serde_json::from_str(&params).unwrap();
    assert_eq!(params["grain"], "session", "{params}");
    assert_eq!(params["rows"], ran["row_count"], "{params}");
}
