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
    let server = Server::start(&home, 20, &[]);
    // the capabilities carry the ask block and the contract version
    let (status, caps) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["contracts"]["openapi"], "2");
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
    let contract = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/openapi/v2/openapi.yaml"),
    )
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
