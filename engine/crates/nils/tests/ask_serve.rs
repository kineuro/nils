// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4b §12.2, slice 9: the ask doors of `nils serve` over a synthetic
//! registry, driven as a second process would drive them: the
//! capabilities carry the caps, the schema digest and the epoch; a run
//! leaves a handle and its first page; a capped run is flagged truncated
//! and has no hash; the affordances answer; a document is addressed by
//! handle; a token with no role gets 403; the job path queues `ask run`
//! and a worker runs it under the queued principal.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../nils-ask/fixtures")
}

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) -> String {
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A synthetic registry with its sessions built: the yardstick's world.
fn synthetic() -> TempDir {
    let home = TempDir::new("ask-serve-home");
    run(&home, &["key", "add", "k"], Some("an ask serve test key\n"));
    run(&home, &["init", "--key", "k"], None);
    run(&home, &["synth", "--seed", "11", "--subjects", "48"], None);
    run(&home, &["session", "rebuild"], None);
    home
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
            // Never the test's own stderr: a server that outlives a
            // panic would hold the pipe open and hang the whole run.
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
        self.request_with(method, path, body, token, &[])
    }

    fn request_with(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        token: Option<&str>,
        headers: &[(&str, &str)],
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
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
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

    /// The server exits on its own once it has served the count the test
    /// started it with; a count that is wrong must fail fast, never hang.
    fn finish(mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match self.child.try_wait().unwrap() {
                Some(status) => {
                    assert!(status.success(), "nils serve exited {status}");
                    return;
                }
                None if std::time::Instant::now() > deadline => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!(
                        "nils serve still waiting for requests after 20 s: the test's request count is wrong"
                    );
                }
                None => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
    }
}

fn yardstick() -> serde_json::Value {
    let text = std::fs::read_to_string(fixtures().join("yardstick.ask.yml")).unwrap();
    let ask = nils_ask::parse(&text).unwrap();
    serde_json::to_value(ask).unwrap()
}

fn body(v: serde_json::Value) -> String {
    v.to_string()
}

#[test]
fn the_ask_doors_run_a_document_to_a_handle_and_its_affordances_answer() {
    let home = synthetic();
    let server = Server::start(&home, 22, &[]);
    // the capabilities carry the ask block and the contract version
    let (status, caps) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["contracts"]["openapi"], "3");
    // the synthetic marker: `nils synth` set it, a desk shows a banner on it
    assert_eq!(caps["registry"]["synthetic"], "nils-synth", "{caps}");
    let ask = &caps["ask"];
    assert_eq!(ask["caps"]["sync_max_rows"], 5000, "{ask}");
    assert_eq!(ask["move_kinds_cap"], 30);
    assert!(ask["schema_digest"].is_string(), "{ask}");
    assert!(ask["ask_schema_digest"].is_string());
    let epoch = ask["epoch"].as_i64().unwrap();
    let doors: Vec<&str> = caps["doors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect();
    assert!(
        doors.contains(&"POST /api/ask/run") && doors.contains(&"POST /api/sessions/rebuild"),
        "{doors:?}"
    );
    // every door in the contract
    let version = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/openapi/VERSION"),
    )
    .unwrap();
    let contract = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../../contracts/openapi/v{}/openapi.yaml",
        version.trim()
    )))
    .unwrap();
    for door in &doors {
        let path = door.split_whitespace().nth(1).unwrap();
        assert!(
            contract.contains(&format!("  {path}:")),
            "{door} is not in the contract"
        );
    }
    // the schema and the catalog
    let (status, schema) = server.request("GET", "/api/ask/schema", None, None);
    assert_eq!(status, 200);
    assert_eq!(
        schema["digest"],
        ask["ask_schema_digest"],
        "the schema door answered {} bytes: {}",
        schema.to_string().len(),
        schema.to_string().chars().take(300).collect::<String>()
    );
    let (status, catalog) = server.request("GET", "/api/ask/catalog", None, None);
    assert_eq!(status, 200, "{catalog}");
    assert_eq!(catalog["move_kinds_cap"], 30);
    let (status, page) = server.request("GET", "/api/ask/catalog/stack", None, None);
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["level"], "stack");
    let (status, _) = server.request("GET", "/api/ask/catalog/nowhere", None, None);
    assert_eq!(status, 404);
    // validate: a good document and a bad one
    let doc = yardstick();
    let (status, v) = server.request(
        "POST",
        "/api/ask/validate",
        Some(&body(serde_json::json!({"document": doc}))),
        None,
    );
    assert_eq!(status, 200, "{v}");
    assert!(v["hash"].is_string());
    let (status, bad) = server.request(
        "POST",
        "/api/ask/validate",
        Some(r#"{"document": {"ast_version": 1, "sets": {"a": {"grain": "subject", "from": "nowhere"}}, "out": {"set": "a", "level": "count"}}}"#),
        None,
    );
    assert_eq!(status, 400, "{bad}");
    assert_eq!(bad["issues"][0]["code"], "unknown_set", "{bad}");
    // run: a handle and its first page
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(&body(
            serde_json::json!({"document": doc, "name": "converters", "keep": true}),
        )),
        None,
    );
    assert_eq!(status, 200, "{ran}");
    assert_eq!(ran["row_count"], 14, "{ran}");
    assert_eq!(ran["truncated"], false);
    assert!(ran["content_hash"].is_string());
    assert_eq!(ran["rows"].as_array().unwrap().len(), 14);
    assert_eq!(ran["kept"].as_array().unwrap().len(), 3, "{ran}");
    let handle = ran["handle"].as_i64().unwrap();
    let (status, h) = server.request("GET", &format!("/api/ask/handles/{handle}"), None, None);
    assert_eq!(status, 200, "{h}");
    assert_eq!(h["name"], "converters");
    assert_eq!(h["pages"], 1);
    // Wave 4c §7.4: the list door names the handle newest first with what
    // the result surface reads, and the policy table carries the door.
    let (status, list) = server.request("GET", "/api/ask/handles?limit=5", None, None);
    assert_eq!(status, 200, "{list}");
    // the run kept three child sets after the answer: newest first
    assert_eq!(list["count"], 4, "{list}");
    let ids: Vec<i64> = list["handles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_i64().unwrap())
        .collect();
    assert!(ids.windows(2).all(|w| w[0] > w[1]), "{ids:?}");
    let first = list["handles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["id"] == handle)
        .unwrap();
    assert_eq!(first["name"], "converters");
    assert_eq!(first["row_count"], 14);
    assert_eq!(first["kept"], true);
    assert_eq!(first["truncated"], false);
    assert!(first["ask_hash"].is_string(), "{first}");
    assert_eq!(
        first["columns"].as_array().unwrap().len(),
        h["columns"].as_array().unwrap().len()
    );
    let (status, caps) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 200);
    assert!(
        caps["ask"]["doors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == "GET /api/ask/handles"),
        "{}",
        caps["ask"]["doors"]
    );
    let (status, rows) = server.request(
        "GET",
        &format!("/api/ask/handles/{handle}/rows?page=0"),
        None,
        None,
    );
    assert_eq!(status, 200, "{rows}");
    assert_eq!(rows["rows"].as_array().unwrap().len(), 14);
    let (status, _) = server.request(
        "GET",
        &format!("/api/ask/handles/{handle}/rows?page=7"),
        None,
        None,
    );
    assert_eq!(status, 404);
    // explain, describe, preview, diagnose
    let (status, ex) = server.request(
        "POST",
        "/api/ask/explain",
        Some(&body(serde_json::json!({"document": doc}))),
        None,
    );
    assert_eq!(status, 200, "{ex}");
    assert!(ex["sqlite"].as_str().unwrap().contains("WITH s_"));
    assert!(ex["postgres"].as_str().unwrap().contains("$1"));
    let (status, d) = server.request(
        "POST",
        "/api/ask/describe",
        Some(&body(serde_json::json!({"document": doc}))),
        None,
    );
    assert_eq!(status, 200, "{d}");
    assert!(d["sets"].as_array().unwrap().len() >= 8);
    let (status, p) = server.request(
        "POST",
        "/api/ask/preview",
        Some(&body(serde_json::json!({"document": doc, "rows": 3}))),
        None,
    );
    assert_eq!(status, 200, "{p}");
    assert_eq!(p["rows"].as_array().unwrap().len(), 3);
    let (status, dg) = server.request(
        "POST",
        "/api/ask/diagnose",
        Some(&body(serde_json::json!({"document": doc}))),
        None,
    );
    assert_eq!(status, 200, "{dg}");
    assert_eq!(dg["valid"], true);
    assert!(dg["funnel"].as_array().unwrap().len() > 10, "{dg}");
    // a document by handle, options and apply
    let (status, posted) = server.request(
        "POST",
        "/api/ask/documents",
        Some(&body(serde_json::json!({"document": doc}))),
        None,
    );
    assert_eq!(status, 200, "{posted}");
    let document = posted["document"].as_i64().unwrap();
    let (status, fetched) =
        server.request("GET", &format!("/api/ask/documents/{document}"), None, None);
    assert_eq!(status, 200);
    assert_eq!(fetched["ask"]["out"]["set"], "answer");
    let (status, opts) = server.request(
        "POST",
        "/api/ask/options",
        Some(&body(
            serde_json::json!({"document_id": document, "set": "good"}),
        )),
        None,
    );
    assert_eq!(status, 200, "{opts}");
    let window = opts["moves"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "set_window")
        .unwrap();
    let (status, applied) = server.request(
        "POST",
        "/api/ask/apply",
        Some(&body(serde_json::json!({
            "document_id": document, "epoch": epoch, "token": opts["token"], "set": "good",
            "moves": [{"move_id": window["id"], "args": {"relation": "near:edss", "preset": "3 months"}}]
        }))),
        None,
    );
    assert_eq!(status, 200, "{applied}");
    assert_ne!(applied["document"], posted["document"]);
    assert_eq!(applied["changed"][0][0], "good");
    let (status, stale) = server.request(
        "POST",
        "/api/ask/apply",
        Some(&body(serde_json::json!({
            "document_id": applied["document"], "epoch": epoch, "token": opts["token"], "set": "good",
            "moves": [{"move_id": window["id"], "args": {"relation": "near:edss", "preset": "3 months"}}]
        }))),
        None,
    );
    assert_eq!(status, 409, "{stale}");
    assert_eq!(stale["error"], "stale_options");
    server.finish();
}

#[test]
fn a_capped_run_is_truncated_and_a_token_with_no_role_is_refused() {
    let home = synthetic();
    let server = Server::start(
        &home,
        4,
        &[
            "--ask-caps",
            r#"{"sync_max_rows": 3}"#,
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=lou@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--token",
            "a-roleless-token-of-len=nobody@lab:",
        ],
    );
    let reader = Some("a-reader-token-of-length");
    let (status, caps) = server.request("GET", "/api/capabilities", None, reader);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["roles"], serde_json::json!(["reader"]));
    assert_eq!(caps["ask"]["caps"]["sync_max_rows"], 3);
    // the cap: rows cut, flagged, no hash; a capped handle may be paged and read
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(&body(serde_json::json!({"document": yardstick()}))),
        reader,
    );
    assert_eq!(status, 200, "{ran}");
    assert_eq!(ran["truncated"], true, "{ran}");
    assert!(ran["content_hash"].is_null(), "{ran}");
    assert_eq!(ran["row_count"], 3);
    // the promotion of a truncated handle is refused before it is queued? No: the
    // job refuses it; the door asks for the operator
    let handle = ran["handle"].as_i64().unwrap();
    let (status, doc) = server.request(
        "POST",
        &format!("/api/ask/handles/{handle}/promote"),
        Some(r#"{"cohort": "converters", "create": true}"#),
        reader,
    );
    assert_eq!(status, 403, "{doc}");
    assert!(doc["error"].as_str().unwrap().contains("operator"));
    // a token with no role is refused at every door, and says so
    let (status, doc) = server.request(
        "GET",
        "/api/capabilities",
        None,
        Some("a-roleless-token-of-len"),
    );
    assert_eq!(status, 403, "{doc}");
    assert!(doc["error"].as_str().unwrap().contains("no role"), "{doc}");
    server.finish();
}

#[test]
fn the_job_path_queues_an_ask_run_and_a_worker_runs_it() {
    let home = synthetic();
    let server = Server::start(
        &home,
        3,
        &[
            "--auth",
            "token",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
    );
    let ops = Some("an-operator-token-of-len");
    let (status, queued) = server.request(
        "POST",
        "/api/ask/jobs",
        Some(&body(
            serde_json::json!({"document": yardstick(), "name": "queued-converters"}),
        )),
        ops,
    );
    assert_eq!(status, 202, "{queued}");
    let job = queued["job"].as_i64().unwrap();
    let document = queued["document"].as_i64().unwrap();
    let (status, shown) = server.request("GET", &format!("/api/jobs/{job}"), None, ops);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["state"], "queued");
    let (status, fetched) =
        server.request("GET", &format!("/api/ask/documents/{document}"), None, ops);
    assert_eq!(status, 200, "{fetched}");
    server.finish();
    // a worker runs it under the queued principal
    let out = run(&home, &["jobs", "work", "--once"], None);
    assert!(out.contains("ask run"), "{out}");
    let listed = run(&home, &["jobs", "list", "--all", "--json"], None);
    assert!(listed.contains("done"), "{listed}");
    // the handle it left is named and complete
    let shown = run(
        &home,
        &[
            "ask",
            "run",
            "--file",
            fixtures().join("gold-c.ask.yml").to_str().unwrap(),
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(v["truncated"], false, "{v}");
    assert!(v["handle"].as_i64().unwrap() > 0);
}

/// A subject grain document that projects an identifier: what only an
/// operator may run, on either path.
fn identified(name: &str) -> serde_json::Value {
    serde_json::json!({
        "ast_version": 1,
        "name": name,
        "scheme": "default",
        "params": {"cohorts": {"type": "list", "value": ["ms-cohort-a", "ms-cohort-b"]}},
        "sets": {
            "scope": {"grain": "cohort", "where": [["in", {}, ["field", {}, "name"], ["param", {}, "cohorts"]]]},
            "people": {"grain": "subject", "of": "scope"}
        },
        "keep": ["people"],
        "out": {"set": "people", "level": "record", "identifiers": ["patient-id"]}
    })
}

/// The same population at record level with no identifier: a reader may
/// run it, and gets the columns the reader's scope reaches.
fn plain(name: &str) -> serde_json::Value {
    let mut d = identified(name);
    d["out"] = serde_json::json!({"set": "people", "level": "record"});
    d
}

/// Wave 4c §6.1, gate fixture 2: the job path refuses what the synchronous
/// door refuses, the job carries the caller's roles, and the worker runs it
/// under them and records what it produced on the row.
#[test]
fn a_queued_job_runs_under_the_roles_the_door_recorded() {
    let home = synthetic();
    let server = Server::start(
        &home,
        4,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
    );
    let reader = Some("a-reader-token-of-length");
    let ops = Some("an-operator-token-of-len");
    // a reader queuing identifiers is refused before anything is queued
    let (status, refused) = server.request(
        "POST",
        "/api/ask/jobs",
        Some(&body(
            serde_json::json!({"document": identified("reader-idents")}),
        )),
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    // a reader queuing a plain document is accepted, and the row records the roles
    let (status, queued) = server.request(
        "POST",
        "/api/ask/jobs",
        Some(&body(
            serde_json::json!({"document": plain("reader-plain"), "name": "reader-plain"}),
        )),
        reader,
    );
    assert_eq!(status, 202, "{queued}");
    let reader_job = queued["job"].as_i64().unwrap();
    let (status, shown) = server.request("GET", &format!("/api/jobs/{reader_job}"), None, ops);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(
        shown["args"]["roles"],
        serde_json::json!(["reader"]),
        "{shown}"
    );
    assert_eq!(shown["args"]["may_project_raw"], false);
    // an operator queuing identifiers is accepted
    let (status, queued) = server.request(
        "POST",
        "/api/ask/jobs",
        Some(&body(
            serde_json::json!({"document": identified("ops-idents"), "name": "ops-idents"}),
        )),
        ops,
    );
    assert_eq!(status, 202, "{queued}");
    let ops_job = queued["job"].as_i64().unwrap();
    server.finish();
    // the worker runs both, each under its own roles
    run(&home, &["jobs", "work", "--once"], None);
    run(&home, &["jobs", "work", "--once"], None);
    let listed = run(&home, &["jobs", "list", "--all", "--json"], None);
    let jobs: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let job = |id: i64| -> serde_json::Value {
        jobs.as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no job {id} in {listed}"))
    };
    let r = job(reader_job);
    assert_eq!(r["state"], "done", "{r}");
    assert!(r["result"]["handle"].as_i64().unwrap() > 0, "{r}");
    assert_eq!(
        r["args"]["roles"],
        serde_json::json!(["reader"]),
        "the roles survive the claim: {r}"
    );
    let o = job(ops_job);
    assert_eq!(o["state"], "done", "{o}");
    assert!(o["result"]["handle"].as_i64().unwrap() > 0, "{o}");
    // the reader's handle carries no class; the operator's carries both
    let reader_handle = r["result"]["handle"].as_i64().unwrap();
    let ops_handle = o["result"]["handle"].as_i64().unwrap();
    let shown = run(
        &home,
        &[
            "ask",
            "handles",
            "show",
            "--handle",
            &reader_handle.to_string(),
            "--json",
        ],
        None,
    );
    let h: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(h["suppression"]["classes"], serde_json::json!([]), "{h}");
    let shown = run(
        &home,
        &[
            "ask",
            "handles",
            "show",
            "--handle",
            &ops_handle.to_string(),
            "--json",
        ],
        None,
    );
    let h: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(
        h["suppression"]["classes"],
        serde_json::json!(["quasi_identifying", "sensitive"]),
        "{h}"
    );
}

/// Wave 4c §6.1, gate fixture 3: a handle is read within the caller's own
/// scope, whoever produced it, and every page read is audited.
#[test]
fn a_handle_is_read_within_the_callers_scope_and_every_page_is_audited() {
    let home = synthetic();
    let server = Server::start(
        &home,
        8,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
    );
    let reader = Some("a-reader-token-of-length");
    let ops = Some("an-operator-token-of-len");
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(&body(
            serde_json::json!({"document": plain("ops-rows"), "name": "ops-rows"}),
        )),
        ops,
    );
    assert_eq!(status, 200, "{ran}");
    let ops_handle = ran["handle"].as_i64().unwrap();
    // a reader is refused the operator's handle and its rows
    let (status, refused) = server.request(
        "GET",
        &format!("/api/ask/handles/{ops_handle}"),
        None,
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    let (status, refused) = server.request(
        "GET",
        &format!("/api/ask/handles/{ops_handle}/rows?page=0"),
        None,
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    // the operator pages it, and the page read is audited with its purpose
    let (status, page) = server.request(
        "GET",
        &format!("/api/ask/handles/{ops_handle}/rows?page=0&purpose=a%20look"),
        None,
        ops,
    );
    assert_eq!(status, 200, "{page}");
    let (status, shown) =
        server.request("GET", &format!("/api/ask/handles/{ops_handle}"), None, ops);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["reads"], 1, "{shown}");
    // a reader's own run leaves a handle a reader may read
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(&body(
            serde_json::json!({"document": plain("reader-rows"), "name": "reader-rows"}),
        )),
        reader,
    );
    assert_eq!(status, 200, "{ran}");
    let reader_handle = ran["handle"].as_i64().unwrap();
    let (status, page) = server.request(
        "GET",
        &format!("/api/ask/handles/{reader_handle}/rows?page=0"),
        None,
        reader,
    );
    assert_eq!(status, 200, "{page}");
    let (status, shown) = server.request(
        "GET",
        &format!("/api/ask/handles/{reader_handle}"),
        None,
        reader,
    );
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["reads"], 1, "{shown}");
    server.finish();
    // an export from the command line is a read too
    run(
        &home,
        &[
            "ask",
            "handles",
            "export",
            "--handle",
            &reader_handle.to_string(),
        ],
        None,
    );
    let shown = run(
        &home,
        &[
            "ask",
            "handles",
            "show",
            "--handle",
            &reader_handle.to_string(),
            "--json",
        ],
        None,
    );
    let h: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(h["reads"], 2, "{h}");
}

/// Wave 4c §6.1, gate fixture 6: the event stream asks for the reader role
/// and is capped, so open streams cannot wedge the other doors.
#[test]
fn event_streams_ask_for_the_reader_role_and_are_capped() {
    let home = synthetic();
    let server = Server::start(
        &home,
        4,
        &[
            "--event-streams",
            "1",
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "a-roleless-token-of-len=guest@lab:",
        ],
    );
    let reader = Some("a-reader-token-of-length");
    let (status, refused) =
        server.request("GET", "/api/events", None, Some("a-roleless-token-of-len"));
    assert_eq!(status, 403, "{refused}");
    // one stream held open
    let mut held = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    held.write_all(
        b"GET /api/events HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer a-reader-token-of-length\r\n\r\n",
    )
    .unwrap();
    let mut first = [0u8; 64];
    let n = held.read(&mut first).unwrap();
    assert!(n > 0);
    // the second is refused with a typed reason, and the other doors answer
    let (status, refused) = server.request("GET", "/api/events", None, reader);
    assert_eq!(status, 503, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .starts_with("event_streams_full"),
        "{refused}"
    );
    let (status, caps) = server.request("GET", "/api/capabilities", None, reader);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["event_streams"], 1, "{caps}");
    drop(held);
    server.finish();
}

/// Wave 4c §5.5, gate fixture 7: a ceiling only removes roles, and the
/// actor is recorded on what the call touched: the handle, the queued job,
/// the audit row, the handle a worker later writes.
#[test]
fn a_ceiling_only_removes_roles_and_the_actor_is_recorded_on_what_it_touches() {
    let home = synthetic();
    let server = Server::start(
        &home,
        11,
        &[
            "--auth",
            "token",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--token",
            "an-admin-token-of-length=chief@lab:admin",
        ],
    );
    let ops = Some("an-operator-token-of-len");
    let admin = Some("an-admin-token-of-length");
    let actor = r#"{"kind":"agent","name":"ask-help","model":"qwen-27b","version":"1","conversation":"c1"}"#;
    let acting: &[(&str, &str)] = &[("X-Nils-Ceiling", "reviewer"), ("X-Nils-Actor", actor)];
    // the ceiling narrows the operator and is written into the actor
    let (status, caps) = server.request_with("GET", "/api/capabilities", None, ops, acting);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(
        caps["roles"],
        serde_json::json!(["reader", "reviewer"]),
        "{caps}"
    );
    assert_eq!(caps["actor"]["name"], "ask-help", "{caps}");
    assert_eq!(caps["actor"]["ceiling"], "reviewer", "{caps}");
    // a ceiling that is not a role is refused, not ignored
    let (status, doc) = server.request_with(
        "GET",
        "/api/capabilities",
        None,
        ops,
        &[("X-Nils-Ceiling", "chief")],
    );
    assert_eq!(status, 400, "{doc}");
    // a run under the actor leaves a handle that names it
    let (status, ran) = server.request_with(
        "POST",
        "/api/ask/run",
        Some(&body(
            serde_json::json!({"document": plain("acted"), "name": "acted"}),
        )),
        ops,
        acting,
    );
    assert_eq!(status, 200, "{ran}");
    let acted_handle = ran["handle"].as_i64().unwrap();
    let (status, shown) = server.request(
        "GET",
        &format!("/api/ask/handles/{acted_handle}"),
        None,
        ops,
    );
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["actor"]["name"], "ask-help", "{shown}");
    assert_eq!(shown["actor"]["ceiling"], "reviewer", "{shown}");
    // the ceiling holds at an operator door
    let (status, refused) = server.request_with(
        "POST",
        &format!("/api/ask/handles/{acted_handle}/promote"),
        Some(r#"{"cohort": "ms-cohort-a"}"#),
        ops,
        acting,
    );
    assert_eq!(status, 403, "{refused}");
    // a person alone is recorded as absent, never as nothing
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(&body(
            serde_json::json!({"document": plain("alone"), "name": "alone"}),
        )),
        ops,
    );
    assert_eq!(status, 200, "{ran}");
    let alone = ran["handle"].as_i64().unwrap();
    let (status, shown) = server.request("GET", &format!("/api/ask/handles/{alone}"), None, ops);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(
        shown["actor"],
        serde_json::json!({"kind": "absent"}),
        "{shown}"
    );
    // a queued job carries the actor to the worker
    let (status, queued) = server.request_with(
        "POST",
        "/api/ask/jobs",
        Some(&body(
            serde_json::json!({"document": plain("acted-job"), "name": "acted-job"}),
        )),
        ops,
        acting,
    );
    assert_eq!(status, 202, "{queued}");
    let job = queued["job"].as_i64().unwrap();
    let (status, shown) = server.request("GET", &format!("/api/jobs/{job}"), None, ops);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["args"]["actor"]["name"], "ask-help", "{shown}");
    assert_eq!(
        shown["args"]["roles"],
        serde_json::json!(["reader", "reviewer"]),
        "{shown}"
    );
    // a saved selection writes an audit row that names the actor
    let (status, saved) = server.request_with(
        "PUT",
        "/api/ask/selections/acted-selection",
        Some(&body(
            serde_json::json!({"document": plain("acted-selection"), "note": "by the helper"}),
        )),
        ops,
        acting,
    );
    assert!(status == 200 || status == 201, "{status} {saved}");
    let (status, audit) = server.request("GET", "/api/audit?limit=5", None, admin);
    assert_eq!(status, 200, "{audit}");
    let rows = audit
        .as_array()
        .cloned()
        .unwrap_or_else(|| audit["rows"].as_array().cloned().unwrap_or_default());
    assert!(
        rows.iter()
            .any(|r| r["actor"]["name"] == "ask-help" && r["actor"]["ceiling"] == "reviewer"),
        "{audit}"
    );
    server.finish();
    // the worker runs the queued job under the actor it carried
    run(&home, &["jobs", "work", "--once"], None);
    let listed = run(&home, &["jobs", "list", "--all", "--json"], None);
    let jobs: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let done = jobs
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == job)
        .cloned()
        .unwrap();
    assert_eq!(done["state"], "done", "{done}");
    let handle = done["result"]["handle"].as_i64().unwrap();
    let shown = run(
        &home,
        &[
            "ask",
            "handles",
            "show",
            "--handle",
            &handle.to_string(),
            "--json",
        ],
        None,
    );
    let h: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(h["actor"]["name"], "ask-help", "{h}");
}

/// Wave 4c §6.3, gate fixture 4: two identical calls under one key produce
/// one handle and one answer, the second saying so; the same key with a
/// different body is refused; a job is queued once.
#[test]
fn an_idempotency_key_makes_a_repeat_one_handle_and_one_answer() {
    let home = synthetic();
    let server = Server::start(
        &home,
        6,
        &[
            "--auth",
            "token",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
    );
    let ops = Some("an-operator-token-of-len");
    let keyed: &[(&str, &str)] = &[("Idempotency-Key", "call-0001")];
    let doc = body(serde_json::json!({"document": plain("once"), "name": "once"}));
    let (status, first) = server.request_with("POST", "/api/ask/run", Some(&doc), ops, keyed);
    assert_eq!(status, 200, "{first}");
    assert!(first["deduplicated"].is_null(), "{first}");
    let (status, again) = server.request_with("POST", "/api/ask/run", Some(&doc), ops, keyed);
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["deduplicated"], true, "{again}");
    assert_eq!(again["handle"], first["handle"], "{again}");
    // the same key with another body is a refusal, not a second run
    let other = body(serde_json::json!({"document": plain("twice"), "name": "twice"}));
    let (status, refused) = server.request_with("POST", "/api/ask/run", Some(&other), ops, keyed);
    assert_eq!(status, 409, "{refused}");
    // a job is queued once under its key
    let job_key: &[(&str, &str)] = &[("Idempotency-Key", "call-0002")];
    let (status, queued) = server.request_with("POST", "/api/ask/jobs", Some(&doc), ops, job_key);
    assert_eq!(status, 202, "{queued}");
    let (status, again) = server.request_with("POST", "/api/ask/jobs", Some(&doc), ops, job_key);
    assert_eq!(status, 202, "{again}");
    assert_eq!(again["job"], queued["job"], "{again}");
    assert_eq!(again["deduplicated"], true, "{again}");
    // the capabilities say which doors take the key
    let (status, caps) = server.request("GET", "/api/capabilities", None, ops);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["idempotency"]["hours"], 24, "{caps}");
    server.finish();
    // one handle named once, one job queued
    let listed = run(&home, &["ask", "handles", "list", "--json"], None);
    let handles: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let named: Vec<&serde_json::Value> = handles["handles"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["name"] == "once")
        .collect();
    assert_eq!(named.len(), 1, "{listed}");
    let jobs = run(&home, &["jobs", "list", "--all", "--json"], None);
    let jobs: serde_json::Value = serde_json::from_str(&jobs).unwrap();
    assert_eq!(
        jobs.as_array()
            .unwrap()
            .iter()
            .filter(|j| j["state"] == "queued")
            .count(),
        1,
        "{jobs}"
    );
}

/// Wave 4c §6.4: the guide, the draft, the declaration on every answer,
/// the node describe, the diff and the value sampler.
#[test]
fn the_ask_additions_answer_the_guide_the_draft_the_declaration_the_diff_and_the_sampler() {
    let home = synthetic();
    let server = Server::start(
        &home,
        9,
        &[
            "--auth",
            "token",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
    );
    let ops = Some("an-operator-token-of-len");
    // the guide
    let (status, guide) = server.request("GET", "/api/ask/guide", None, ops);
    assert_eq!(status, 200, "{guide}");
    assert!(
        guide["grounding"].as_array().is_some_and(|g| !g.is_empty()),
        "{guide}"
    );
    assert!(
        guide["examples"].as_array().is_some_and(|e| !e.is_empty()),
        "{guide}"
    );
    assert!(guide["schema_digest"].is_string(), "{guide}");
    // the draft
    let text = serde_json::to_string(&plain("drafted")).unwrap();
    let (status, drafted) = server.request(
        "POST",
        "/api/ask/draft",
        Some(&body(serde_json::json!({"text": text}))),
        ops,
    );
    assert_eq!(status, 200, "{drafted}");
    assert!(drafted["document"].as_i64().is_some(), "{drafted}");
    assert!(drafted["hash"].is_string(), "{drafted}");
    // the declaration on a run
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(&body(
            serde_json::json!({"document": plain("declared"), "name": "declared"}),
        )),
        ops,
    );
    assert_eq!(status, 200, "{ran}");
    let d = &ran["declaration"];
    assert_eq!(d["grain"], "subject", "{ran}");
    assert_eq!(d["session_scheme"]["name"], "default", "{ran}");
    assert!(d["session_scheme"]["digest"].is_string(), "{ran}");
    assert!(
        d["key_namespace"]
            .as_str()
            .unwrap()
            .contains("pseudonymous"),
        "{ran}"
    );
    assert_eq!(d["truncated"], false, "{ran}");
    let handle = ran["handle"].as_i64().unwrap();
    // and on a preview
    let (status, previewed) = server.request(
        "POST",
        "/api/ask/preview",
        Some(&body(serde_json::json!({"document": plain("previewed")}))),
        ops,
    );
    assert_eq!(status, 200, "{previewed}");
    assert_eq!(previewed["declaration"]["grain"], "subject", "{previewed}");
    // one node described
    let (status, node) = server.request(
        "POST",
        "/api/ask/describe",
        Some(&body(serde_json::json!({"document": plain("node"), "node": {"set": "people", "part": "set"}}))),
        ops,
    );
    assert_eq!(status, 200, "{node}");
    assert_eq!(node["display_name"], "people", "{node}");
    assert!(
        node["long_display_name"].as_str().unwrap().len() > 6,
        "{node}"
    );
    // the whole description carries the declaration too
    let (status, described) = server.request(
        "POST",
        "/api/ask/describe",
        Some(&body(serde_json::json!({"document": plain("described")}))),
        ops,
    );
    assert_eq!(status, 200, "{described}");
    assert_eq!(described["declaration"]["grain"], "subject", "{described}");
    // two documents differ set by set
    let mut changed = plain("changed");
    changed["sets"]["scope"]["where"] = serde_json::json!([
        ["in", {}, ["field", {}, "name"], ["param", {}, "cohorts"]],
        ["contains", {}, ["field", {}, "name"], "a"]
    ]);
    let (status, diff) = server.request(
        "POST",
        "/api/ask/diff",
        Some(&body(
            serde_json::json!({"a": {"document": plain("changed")}, "b": {"document": changed}}),
        )),
        ops,
    );
    assert_eq!(status, 200, "{diff}");
    assert_eq!(diff["same"], false, "{diff}");
    let change = &diff["changes"][0];
    assert_eq!(change["set"], "scope", "{diff}");
    assert_eq!(change["part"], "where", "{diff}");
    assert_eq!(change["kind"], "changed", "{diff}");
    assert!(
        diff["canonical_a"].is_string() && diff["canonical_b"].is_string(),
        "{diff}"
    );
    // two handles compare by hash
    let (status, same) = server.request(
        "POST",
        "/api/ask/diff",
        Some(&body(
            serde_json::json!({"a": {"handle": handle}, "b": {"handle": handle}}),
        )),
        ops,
    );
    assert_eq!(status, 200, "{same}");
    assert_eq!(same["same"], true, "{same}");
    // the value sampler lists a declared field's values with counts
    let (status, sample) = server.request("GET", "/api/ask/catalog/cohort/name/values", None, ops);
    assert_eq!(status, 200, "{sample}");
    assert_eq!(sample["kind"], "values", "{sample}");
    assert!(sample["distinct"].as_i64().unwrap() >= 2, "{sample}");
    assert!(
        sample["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i[0] == "ms-cohort-a" && i[1].as_i64().unwrap() > 0),
        "{sample}"
    );
    server.finish();
}

/// Wave 4c (kineuro/nils#93): strict validate, the documents door and the
/// draft door compile what run would, so a document they accept runs.
/// A cohort record that counts a subject set in its columns passes the
/// validator and not the compiler; every door refuses it by name.
#[test]
fn validate_the_documents_door_and_the_draft_door_refuse_what_run_would() {
    let home = synthetic();
    let server = Server::start(&home, 5, &[]);
    let doc = serde_json::json!({
        "ast_version": 1,
        "sets": {"co": {"grain": "cohort"}, "people": {"grain": "subject", "of": "co"}},
        "out": {"set": "co", "level": "record",
                "columns": [["field", {}, "name"], ["field", {}, "owner"], ["count", {"set": "people"}]]}
    });
    let (status, v) = server.request(
        "POST",
        "/api/ask/validate",
        Some(&body(
            serde_json::json!({"document": doc, "mode": "strict"}),
        )),
        None,
    );
    assert_eq!(status, 400, "{v}");
    assert_eq!(v["issues"][0]["code"], "not_compilable", "{v}");
    assert!(
        v["issues"][0]["path"]
            .as_str()
            .unwrap()
            .starts_with("out.columns"),
        "{v}"
    );
    let (status, d) = server.request(
        "POST",
        "/api/ask/documents",
        Some(&body(serde_json::json!({"document": doc}))),
        None,
    );
    assert_eq!(status, 400, "{d}");
    assert_eq!(d["issues"][0]["code"], "not_compilable", "{d}");
    let (status, drafted) = server.request(
        "POST",
        "/api/ask/draft",
        Some(&body(serde_json::json!({"text": doc.to_string()}))),
        None,
    );
    assert_eq!(status, 200, "{drafted}");
    assert_eq!(drafted["diagnosis"]["valid"], false, "{drafted}");
    assert!(drafted["document"].is_null(), "{drafted}");
    assert!(
        drafted["diagnosis"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "not_compilable"),
        "{drafted}"
    );
    // a document that compiles still validates, stores and drafts
    let good = yardstick();
    let (status, v) = server.request(
        "POST",
        "/api/ask/validate",
        Some(&body(
            serde_json::json!({"document": good, "mode": "strict"}),
        )),
        None,
    );
    assert_eq!(status, 200, "{v}");
    let text = std::fs::read_to_string(fixtures().join("yardstick.ask.yml")).unwrap();
    let (status, drafted) = server.request(
        "POST",
        "/api/ask/draft",
        Some(&body(serde_json::json!({"text": text}))),
        None,
    );
    assert_eq!(status, 200, "{drafted}");
    assert!(drafted["document"].is_number(), "{drafted}");
    server.finish();
}

#[test]
fn the_summary_the_start_from_resolver_and_the_document_list_answer_on_the_synthetic_home() {
    // Wave 5 §12.1 (slice A1)
    let home = synthetic();
    let server = Server::start(
        &home,
        30,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=lou@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
    );
    let reader = Some("a-reader-token-of-length");
    let operator = Some("an-operator-token-of-len");
    // the summary: counts, by cohort, never a row
    let (status, summary) = server.request("GET", "/api/summary", None, reader);
    assert_eq!(status, 200, "{summary}");
    let subjects = summary["subjects"]["total"].as_i64().unwrap();
    let sessions = summary["sessions"]["total"].as_i64().unwrap();
    let stacks = summary["stacks"]["total"].as_i64().unwrap();
    assert_eq!(subjects, 48, "{summary}");
    assert!(sessions > 0 && stacks > 0, "{summary}");
    assert!(summary["epoch"].as_i64().unwrap() >= 1);
    assert!(summary["cohorts"].as_i64().unwrap() >= 1, "{summary}");
    let by_cohort = summary["subjects"]["by_cohort"].as_object().unwrap();
    assert!(!by_cohort.is_empty(), "{summary}");
    let membership: i64 = by_cohort.values().map(|v| v.as_i64().unwrap()).sum();
    assert!(membership >= 1 && by_cohort.values().all(|v| v.as_i64().unwrap() <= subjects));
    assert!(
        summary["stacks"]["by_pack_version"].is_object(),
        "{summary}"
    );
    assert!(
        summary["synthetic"].is_null() || summary["synthetic"] == "nils-synth",
        "{summary}"
    );
    // what arrived since a date: everything, then nothing, then a refusal
    let (status, s) = server.request("GET", "/api/summary?since=2000-01-01", None, reader);
    assert_eq!(status, 200, "{s}");
    assert_eq!(s["since"]["subjects"], subjects, "{s}");
    assert_eq!(s["since"]["stacks"], stacks, "{s}");
    let (status, s) = server.request(
        "GET",
        "/api/summary?since=2999-01-01T00:00:00Z",
        None,
        reader,
    );
    assert_eq!(status, 200, "{s}");
    assert_eq!(s["since"]["subjects"], 0, "{s}");
    assert_eq!(s["since"]["handles"], 0, "{s}");
    let (status, _) = server.request("GET", "/api/summary?since=yesterday", None, reader);
    assert_eq!(status, 400);
    // start from nothing: everyone, with the sessions under them
    let (status, all) = server.request(
        "POST",
        "/api/ask/start",
        Some(&body(serde_json::json!({"from": {}}))),
        reader,
    );
    assert_eq!(status, 200, "{all}");
    assert_eq!(all["grain"], "subject");
    assert_eq!(all["count"], subjects, "{all}");
    assert_eq!(all["sessions"], sessions, "{all}");
    assert_eq!(all["document"]["out"]["set"], "everyone");
    assert!(all["document"]["sets"]["sessions_under"].is_null(), "{all}");
    // start from a cohort: fewer people, the summary's own number
    let (cohort, n) = by_cohort.iter().next().unwrap();
    let (status, one) = server.request(
        "POST",
        "/api/ask/start",
        Some(&body(serde_json::json!({"from": {"cohorts": [cohort]}}))),
        reader,
    );
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["set"], "people");
    assert_eq!(one["count"], n.as_i64().unwrap(), "{one}");
    assert!(one["count"].as_i64().unwrap() < subjects, "{one}");
    assert!(one["sessions"].as_i64().unwrap() <= sessions, "{one}");
    assert_eq!(one["document"]["params"]["cohorts"]["value"][0], *cohort);
    // an uploaded list under a reader is refused before anything resolves
    let (status, refused) = server.request(
        "POST",
        "/api/ask/start",
        Some(&body(serde_json::json!({"from": {"values": "nowhere"}}))),
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    let (status, refused) = server.request(
        "POST",
        "/api/ask/values",
        Some(&body(
            serde_json::json!({"namespace": "x", "values": ["1"]}),
        )),
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    // under an operator the same start reaches the resolver, which has no such upload
    let (status, gone) = server.request(
        "POST",
        "/api/ask/start",
        Some(&body(serde_json::json!({"from": {"values": "nowhere"}}))),
        operator,
    );
    assert_eq!(status, 400, "{gone}");
    // a stored document, run once, is listed with its run
    let (status, stored) = server.request(
        "POST",
        "/api/ask/documents",
        Some(&body(serde_json::json!({"document": yardstick()}))),
        reader,
    );
    assert_eq!(status, 200, "{stored}");
    let id = stored["document"].as_i64().unwrap();
    let (status, from_doc) = server.request(
        "POST",
        "/api/ask/start",
        Some(&body(serde_json::json!({"from": {"document": id}}))),
        reader,
    );
    assert_eq!(status, 200, "{from_doc}");
    assert_eq!(from_doc["set"], yardstick()["out"]["set"]);
    assert!(from_doc["count"].as_i64().unwrap() > 0, "{from_doc}");
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(&body(
            serde_json::json!({"document_id": id, "name": "listed"}),
        )),
        reader,
    );
    assert_eq!(status, 200, "{ran}");
    let handle = ran["handle"].as_i64().unwrap();
    let (status, from_handle) = server.request(
        "POST",
        "/api/ask/start",
        Some(&body(serde_json::json!({"from": {"handle": handle}}))),
        reader,
    );
    assert_eq!(status, 200, "{from_handle}");
    assert_eq!(from_handle["grain"], ran["grain"]);
    assert_eq!(
        from_handle["document"]["sets"]["start"]["from"],
        format!("handle:{handle}")
    );
    let (status, listed) = server.request("GET", "/api/ask/documents", None, reader);
    assert_eq!(status, 200, "{listed}");
    let row = listed["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["document"] == id)
        .unwrap_or_else(|| panic!("{listed}"));
    assert_eq!(row["versions"], 1);
    assert_eq!(row["author"], "lou@lab");
    assert_eq!(row["grain"], ran["grain"]);
    assert_eq!(row["last_run"]["handle"], handle, "{row}");
    // paging by the latest document id
    let (status, page) = server.request("GET", "/api/ask/documents?limit=1", None, reader);
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["count"], 1);
    if listed["count"].as_i64().unwrap() > 1 {
        assert!(page["next"].is_number(), "{page}");
    }
    // the three doors are in the capabilities and the policy
    let (_, caps) = server.request("GET", "/api/capabilities", None, reader);
    for door in [
        "GET /api/summary",
        "POST /api/ask/start",
        "GET /api/ask/documents",
    ] {
        assert!(
            caps["doors"].as_array().unwrap().iter().any(|d| d == door),
            "{door}"
        );
        assert!(
            caps["policy"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["door"] == door),
            "{door}"
        );
    }
}

/// Wave 5 section 12.2, slice A2: the timeline door orders a document's
/// versions, runs and promotion; a handle's timeline carries its source
/// document and its promotion; a job's carries what it did; the door
/// refuses a kind it does not serve and an id no row has.
#[test]
fn the_timeline_door_orders_a_documents_versions_runs_and_promotion() {
    let home = synthetic();
    let server = Server::start(
        &home,
        15,
        &[
            "--auth",
            "token",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
    );
    let ops = Some("an-operator-token-of-len");
    // a document, then a second version by a move
    let (status, posted) = server.request(
        "POST",
        "/api/ask/documents",
        Some(&body(serde_json::json!({"document": yardstick()}))),
        ops,
    );
    assert_eq!(status, 200, "{posted}");
    let first = posted["document"].as_i64().unwrap();
    let (status, caps) = server.request("GET", "/api/capabilities", None, ops);
    assert_eq!(status, 200, "{caps}");
    let epoch = caps["ask"]["epoch"].as_i64().unwrap();
    let (status, opts) = server.request(
        "POST",
        "/api/ask/options",
        Some(&body(
            serde_json::json!({"document_id": first, "set": "good"}),
        )),
        ops,
    );
    assert_eq!(status, 200, "{opts}");
    let window = opts["moves"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "set_window")
        .unwrap();
    let (status, applied) = server.request(
        "POST",
        "/api/ask/apply",
        Some(&body(serde_json::json!({
            "document_id": first, "epoch": epoch, "token": opts["token"], "set": "good",
            "moves": [{"move_id": window["id"], "args": {"relation": "near:edss", "preset": "3 months"}}]
        }))),
        ops,
    );
    assert_eq!(status, 200, "{applied}");
    let second = applied["document"].as_i64().unwrap();
    assert_ne!(first, second);
    // a run of the first version, then its promotion through a worker
    let (status, ran) = server.request(
        "POST",
        "/api/ask/run",
        Some(&body(
            serde_json::json!({"document": yardstick(), "name": "timed"}),
        )),
        ops,
    );
    assert_eq!(status, 200, "{ran}");
    let handle = ran["handle"].as_i64().unwrap();
    let (status, queued) = server.request(
        "POST",
        &format!("/api/ask/handles/{handle}/promote"),
        Some(r#"{"cohort": "timed", "create": true}"#),
        ops,
    );
    assert_eq!(status, 202, "{queued}");
    let job = queued["job"].as_i64().unwrap();
    let out = run(&home, &["jobs", "work", "--once"], None);
    assert!(out.contains("promote") || out.contains("job"), "{out}");
    // the document's timeline: two versions, the run, the promotion, in order
    let (status, timeline) = server.request(
        "GET",
        &format!("/api/timeline/document/{second}"),
        None,
        ops,
    );
    assert_eq!(status, 200, "{timeline}");
    let kinds: Vec<&str> = timeline["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        vec!["version", "version", "run", "promotion"],
        "{timeline}"
    );
    let stamps: Vec<&str> = timeline["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["at"].as_str())
        .collect();
    assert!(stamps.windows(2).all(|w| w[0] <= w[1]), "{stamps:?}");
    let events = timeline["events"].as_array().unwrap();
    assert_eq!(events[0]["produced"]["id"], first, "{timeline}");
    assert_eq!(events[1]["produced"]["id"], second, "{timeline}");
    assert_eq!(
        events[2]["produced"],
        serde_json::json!({"kind": "handle", "id": handle})
    );
    assert_eq!(events[3]["produced"]["kind"], "cohort", "{timeline}");
    assert_eq!(events[3]["actor"], "ops@lab", "{timeline}");
    assert_eq!(events[3]["source"], "audit");
    // the first version sees the same chain
    let (status, from_first) =
        server.request("GET", &format!("/api/timeline/document/{first}"), None, ops);
    assert_eq!(status, 200, "{from_first}");
    assert_eq!(from_first["count"], timeline["count"]);
    // the handle's timeline: its source document, its run, its promotion
    let (status, of_handle) =
        server.request("GET", &format!("/api/timeline/handle/{handle}"), None, ops);
    assert_eq!(status, 200, "{of_handle}");
    let kinds: Vec<&str> = of_handle["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert_eq!(kinds, vec!["document", "run", "promotion"], "{of_handle}");
    // the job's: started, what it recorded, finished
    let (status, of_job) = server.request("GET", &format!("/api/timeline/job/{job}"), None, ops);
    assert_eq!(status, 200, "{of_job}");
    let kinds: Vec<&str> = of_job["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert!(
        kinds.contains(&"started") && kinds.contains(&"finished"),
        "{of_job}"
    );
    assert!(kinds.contains(&"promotion"), "{of_job}");
    let finished = of_job["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "finished")
        .unwrap();
    assert_eq!(finished["produced"]["kind"], "cohort", "{of_job}");
    // a subject's, a stack's and a session's: dated events on the synthetic rows
    let (status, of_subject) = server.request("GET", "/api/timeline/subject/1", None, ops);
    assert_eq!(status, 200, "{of_subject}");
    let kinds: Vec<&str> = of_subject["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    for kind in ["arrived", "study", "joined", "session"] {
        assert!(kinds.contains(&kind), "{kind} missing: {of_subject}");
    }
    // a kind the door does not serve, and an id no row has
    let (status, refused) = server.request("GET", "/api/timeline/nothing/1", None, ops);
    assert_eq!(status, 404, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("document, handle"),
        "{refused}"
    );
    let (status, missing) = server.request("GET", "/api/timeline/document/999999", None, ops);
    assert_eq!(status, 404, "{missing}");
    let (status, of_stack) = server.request("GET", "/api/timeline/stack/1", None, ops);
    assert_eq!(status, 200, "{of_stack}");
    assert_eq!(of_stack["events"][0]["kind"], "landed", "{of_stack}");
    let (status, of_session) = server.request("GET", "/api/timeline/session/1", None, ops);
    assert_eq!(status, 200, "{of_session}");
    assert_eq!(of_session["events"][0]["kind"], "built", "{of_session}");
    server.finish();
}

/// Wave 5 slice A3: the funnel keyed by clause group sums to the funnel keyed
/// by set; every error carries its disclosure; the declaration carries the
/// registry's timezone and week start, and they are part of the hash.
#[test]
fn the_clause_funnel_sums_and_errors_disclose_and_the_timezone_is_the_registrys() {
    let home = synthetic();
    let server = Server::start(&home, 7, &[]);
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gate/fixtures/gold-a.ask.yml"),
    )
    .unwrap();
    let gold = serde_json::to_value(nils_ask::parse(&text).unwrap()).unwrap();
    let (status, by_set) = server.request(
        "POST",
        "/api/ask/diagnose",
        Some(&body(serde_json::json!({"document": gold.clone()}))),
        None,
    );
    assert_eq!(status, 200, "{by_set}");
    assert_eq!(by_set["by"], "set");
    assert!(by_set["groups"].is_null(), "{by_set}");
    let (status, by_clause) = server.request(
        "POST",
        "/api/ask/diagnose",
        Some(&body(serde_json::json!({"document": gold, "by": "clause"}))),
        None,
    );
    assert_eq!(status, 200, "{by_clause}");
    assert_eq!(by_clause["by"], "clause");
    let groups = by_clause["groups"].as_array().unwrap();
    let funnel = by_set["funnel"].as_array().unwrap();
    assert!(!groups.is_empty());
    let order = ["source", "near", "attach", "has", "where", "pick", "out"];
    let mut sets: Vec<String> = groups
        .iter()
        .map(|g| g["set"].as_str().unwrap().to_string())
        .collect();
    sets.dedup();
    for set in &sets {
        let mine: Vec<&serde_json::Value> = groups.iter().filter(|g| g["set"] == *set).collect();
        let source = mine.iter().find(|g| g["group"] == "source").unwrap()["kept"]
            .as_i64()
            .unwrap();
        let lost: i64 = mine.iter().map(|g| g["lost"].as_i64().unwrap()).sum();
        let last = funnel.iter().rfind(|s| s["set"] == *set).unwrap()["rows"]
            .as_i64()
            .unwrap();
        // the set's source less what its groups took is its answer
        assert_eq!(source - lost, last, "{set}: {mine:?}");
        let positions: Vec<usize> = mine
            .iter()
            .map(|g| {
                order
                    .iter()
                    .position(|o| *o == g["group"].as_str().unwrap())
                    .unwrap()
            })
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] < w[1]),
            "{set}: {positions:?}"
        );
    }
    assert!(groups.iter().any(|g| g["group"] == "has"), "{by_clause}");
    assert!(groups.iter().any(|g| g["group"] == "where"), "{by_clause}");
    assert!(
        groups
            .iter()
            .any(|g| g["group"] == "out" && g["set"] == "both"),
        "{by_clause}"
    );
    // the disclosure: a taxonomy error and a missing id are safe, a bad keying too
    let (status, e) = server.request(
        "POST",
        "/api/ask/validate",
        Some(&body(serde_json::json!({"document": {"ast_version": 1, "sets": {"x": {"grain": "subject", "of": "nowhere"}}, "out": {"set": "x", "level": "count"}}}))),
        None,
    );
    assert_eq!(status, 400, "{e}");
    assert_eq!(e["disclosure"], "safe", "{e}");
    let (status, e) = server.request("GET", "/api/review/999999", None, None);
    assert_eq!(status, 404, "{e}");
    assert_eq!(e["disclosure"], "safe");
    let (status, e) = server.request(
        "POST",
        "/api/ask/diagnose",
        Some(&body(
            serde_json::json!({"document": yardstick(), "by": "rows"}),
        )),
        None,
    );
    assert_eq!(status, 400, "{e}");
    assert_eq!(e["disclosure"], "safe");
    // the declaration carries the defaults, and the hash is the one it always was
    let (status, d) = server.request(
        "POST",
        "/api/ask/describe",
        Some(&body(serde_json::json!({"document": yardstick()}))),
        None,
    );
    assert_eq!(status, 200, "{d}");
    assert_eq!(d["declaration"]["timezone"], "UTC", "{d}");
    assert_eq!(d["declaration"]["week_start"], "monday");
    let (status, stored) = server.request(
        "POST",
        "/api/ask/validate",
        Some(&body(serde_json::json!({"document": yardstick()}))),
        None,
    );
    assert_eq!(status, 200, "{stored}");
    // the gate's canonicals hold this hash unchanged: the default locale is
    // left out of the core
    let utc = stored["hash"].as_str().unwrap().to_string();
    server.finish();
    // the registry moves to Stockholm: the epoch moves, the declaration says
    // so, and the same document is another question
    let out = run(
        &home,
        &["settings", "set", "timezone", "Europe/Stockholm"],
        None,
    );
    assert!(out.contains("Europe/Stockholm"), "{out}");
    let shown = run(&home, &["settings", "show", "--json"], None);
    assert!(
        shown.contains("\"timezone\":\"Europe/Stockholm\""),
        "{shown}"
    );
    let server = Server::start(&home, 2, &[]);
    let (status, d) = server.request(
        "POST",
        "/api/ask/describe",
        Some(&body(serde_json::json!({"document": yardstick()}))),
        None,
    );
    assert_eq!(status, 200, "{d}");
    assert_eq!(d["declaration"]["timezone"], "Europe/Stockholm", "{d}");
    // a stored document keeps the hash it was put with; a fresh preparation
    // reads the registry's locale and is another question
    let (status, stored) = server.request(
        "POST",
        "/api/ask/validate",
        Some(&body(serde_json::json!({"document": yardstick()}))),
        None,
    );
    assert_eq!(status, 200, "{stored}");
    assert_ne!(stored["hash"].as_str().unwrap(), utc);
    server.finish();
}
