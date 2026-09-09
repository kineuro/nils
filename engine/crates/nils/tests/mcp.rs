// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4b §12.3, slice 11: the MCP door, driven over streamable HTTP the
//! way a client drives it. The tool list is the pack's opt-in and not the
//! endpoint list; a page is bounded; a domain refusal is text with
//! `isError`; the metadata of RFC 9728 is public and a refusal names it;
//! and a grounding rule edited or an example swapped ships with the pack,
//! against the same binary.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../nils-ask/fixtures/{name}.ask.yml")),
    )
    .unwrap();
    serde_json::to_value(nils_ask::parse(&text).unwrap()).unwrap()
}

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) {
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
}

fn synthetic() -> TempDir {
    let home = TempDir::new("mcp-home");
    run(&home, &["key", "add", "k"], Some("an mcp test key\n"));
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
    fn start(home: &TempDir, pack_dir: &Path, extra: &[&str]) -> Server {
        let mut child = nils()
            .arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--requests",
                "512",
            ])
            .args(["--pack-dir", pack_dir.to_str().unwrap()])
            .args(extra)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdout(Stdio::piped())
            // Never the test's own stderr: a server that outlives a
            // panic would hold the pipe open and hang the whole run.
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let first = BufReader::new(stdout).lines().next().unwrap().unwrap();
        let addr = first.split_whitespace().nth(2).unwrap();
        Server {
            child,
            port: addr.rsplit(':').next().unwrap().parse().unwrap(),
        }
    }

    /// One request, with its status, its headers and its body.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        token: Option<&str>,
    ) -> (u16, String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.unwrap_or("");
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
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
        (status, headers.to_string(), body.to_string())
    }

    /// One JSON-RPC call over the door.
    fn rpc(&self, message: Value, token: Option<&str>) -> Value {
        let (status, _, body) = self.request("POST", "/mcp", Some(&message.to_string()), token);
        assert_eq!(status, 200, "{message}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("{body}: {e}"))
    }

    fn call(&self, tool: &str, arguments: Value, token: Option<&str>) -> Value {
        let answer = self.rpc(
            json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                   "params": {"name": tool, "arguments": arguments}}),
            token,
        );
        assert!(
            answer.get("result").is_some(),
            "{tool} answered an error: {answer}"
        );
        answer["result"].clone()
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_client_lists_the_opted_in_tools_pages_a_result_and_reads_a_refusal_as_text() {
    let home = synthetic();
    let server = Server::start(&home, &packs(), &["--ask-caps", r#"{"page_rows_mcp": 5}"#]);

    // the handshake: the protocol, the tools capability, the pack's rules
    let hello = server.rpc(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "a test", "version": "0"}}}),
        None,
    );
    let r = &hello["result"];
    assert_eq!(r["protocolVersion"], "2025-06-18");
    assert_eq!(r["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(r["serverInfo"]["name"], "nils");
    let instructions = r["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("A question is a document, not SQL"),
        "{instructions}"
    );

    // a notification is answered with nothing at all
    let (status, _, body) = server.request(
        "POST",
        "/mcp",
        Some(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string()),
        None,
    );
    assert_eq!(status, 202, "{body}");
    assert!(body.is_empty(), "{body}");

    // the tool list is the pack's opt-in, not the endpoint list
    let listed = server.rpc(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        None,
    );
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "nils_guide",
            "nils_catalog",
            "nils_validate",
            "nils_describe",
            "nils_options",
            "nils_apply",
            "nils_diagnose",
            "nils_preview",
            "nils_draft",
            "nils_run",
            "nils_job",
            "nils_job_status",
            "nils_rows"
        ]
    );
    // Wave 4c §6.4: a tool only client obtains the worked examples
    let guide = server.call("nils_guide", json!({}), None);
    let text = guide["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("examples"), "{guide}");
    assert!(
        guide["structuredContent"]["examples"]
            .as_array()
            .is_some_and(|e| !e.is_empty()),
        "{guide}"
    );
    // the engine serves these doors and the pack opted neither in
    assert!(!names.contains(&"nils_handle") && !names.contains(&"nils_selections"));
    let catalog = tools.iter().find(|t| t["name"] == "nils_catalog").unwrap();
    assert!(
        catalog["description"]
            .as_str()
            .unwrap()
            .contains("page a level with after")
    );
    assert_eq!(
        catalog["inputSchema"]["properties"]["after"]["type"],
        "string"
    );
    let validate = tools.iter().find(|t| t["name"] == "nils_validate").unwrap();
    assert!(
        validate["description"]
            .as_str()
            .unwrap()
            .contains("- Validate every document before running it"),
        "the tool's own rules ride on its description: {}",
        validate["description"]
    );

    // a document that runs: the handle, and the rows bounded to the door's page
    let ran = server.call(
        "nils_run",
        json!({"document": fixture("yardstick"), "name": "through-mcp"}),
        None,
    );
    assert_eq!(ran["isError"], false, "{ran}");
    let doc = &ran["structuredContent"];
    assert_eq!(doc["row_count"], 14);
    assert_eq!(doc["rows"].as_array().unwrap().len(), 5, "bounded: {doc}");
    assert_eq!(doc["rows_shown"], 5);
    assert!(
        doc["more"].as_str().unwrap().contains("from the handle"),
        "a run sends the model to the handle for the rest: {doc}"
    );
    assert_eq!(ran["_meta"]["bounded"], true);
    let handle = doc["handle"].as_i64().unwrap();

    // and its rows, a bounded slice at a time, through the door
    let rows = server.call("nils_rows", json!({"handle": handle, "page": 0}), None);
    assert_eq!(rows["isError"], false, "{rows}");
    let page = &rows["structuredContent"];
    assert_eq!(page["rows_in_page"], 14, "{page}");
    assert_eq!(page["rows"].as_array().unwrap().len(), 5);
    assert_eq!(page["next_offset"], 5);
    let more = server.call(
        "nils_rows",
        json!({"handle": handle, "page": 0, "offset": 10}),
        None,
    );
    let page = &more["structuredContent"];
    assert_eq!(page["rows"].as_array().unwrap().len(), 4, "{page}");
    assert_eq!(page["offset"], 10);
    assert!(page["next_offset"].is_null(), "the last slice: {page}");
    assert!(
        page["more"].as_str().unwrap().contains("ends here"),
        "{page}"
    );

    // a domain refusal is text a model can act on, never a transport error
    let refused = server.call(
        "nils_validate",
        json!({"document": {"ast_version": 1, "sets": {"a": {"grain": "subject", "from": "nowhere"}}, "out": {"set": "a", "level": "count"}}}),
        None,
    );
    assert_eq!(refused["isError"], true, "{refused}");
    let text = refused["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("unknown_set at sets.a.from"), "{text}");
    assert!(text.contains("no set named nowhere"), "{text}");

    // a tool nobody opted in, and a method this door has not
    let missing = server.rpc(
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
               "params": {"name": "nils_promote", "arguments": {}}}),
        None,
    );
    assert_eq!(missing["error"]["code"], -32602, "{missing}");
    let unknown = server.rpc(
        json!({"jsonrpc": "2.0", "id": 4, "method": "resources/read", "params": {}}),
        None,
    );
    assert_eq!(unknown["error"]["code"], -32601, "{unknown}");

    // the pack's examples reach a client as prompts
    let prompts = server.rpc(
        json!({"jsonrpc": "2.0", "id": 5, "method": "prompts/list"}),
        None,
    );
    let listed = prompts["result"]["prompts"].as_array().unwrap();
    assert_eq!(listed.len(), 2, "{prompts}");
    assert!(
        listed[0]["title"]
            .as_str()
            .unwrap()
            .contains("How many subjects")
    );
    let got = server.rpc(
        json!({"jsonrpc": "2.0", "id": 6, "method": "prompts/get", "params": {"name": "example_2"}}),
        None,
    );
    let text = got["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("ast_version"), "{text}");

    // the capabilities name the door, its protocol and its tools
    let (status, _, body) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 200);
    let caps: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(caps["mcp"]["path"], "/mcp");
    assert_eq!(caps["mcp"]["tools"].as_array().unwrap().len(), 13);
    assert_eq!(caps["mcp"]["content_version"], "1");
    server.stop();
}

#[test]
fn the_metadata_is_public_and_a_refusal_names_it() {
    let home = synthetic();
    let server = Server::start(
        &home,
        &packs(),
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=lou@lab:reader",
            "--token",
            "a-roleless-token-of-len=nobody@lab:",
            "--mcp-authorization-server",
            "https://id.example.org/application/o/nils/",
        ],
    );

    // RFC 9728: the metadata needs no token, and names the authorization
    // server and the resource
    let (status, _, body) =
        server.request("GET", "/.well-known/oauth-protected-resource", None, None);
    assert_eq!(status, 200, "{body}");
    let doc: Value = serde_json::from_str(&body).unwrap();
    assert!(doc["resource"].as_str().unwrap().ends_with("/mcp"), "{doc}");
    assert_eq!(
        doc["authorization_servers"][0],
        "https://id.example.org/application/o/nils/"
    );
    assert!(
        doc["deviation"]
            .as_str()
            .unwrap()
            .contains("audience binding"),
        "{doc}"
    );

    // no token: 401 that names the metadata
    let (status, headers, body) = server.request(
        "POST",
        "/mcp",
        Some(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string()),
        None,
    );
    assert_eq!(status, 401, "{body}");
    let auth = headers
        .lines()
        .find(|l| l.to_lowercase().starts_with("www-authenticate"))
        .unwrap_or_default();
    assert!(auth.contains("resource_metadata="), "{auth}");
    assert!(
        auth.contains("/.well-known/oauth-protected-resource"),
        "{auth}"
    );

    // a token with no role: 403 with insufficient_scope
    let (status, headers, body) = server.request(
        "POST",
        "/mcp",
        Some(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string()),
        Some("a-roleless-token-of-len"),
    );
    assert_eq!(status, 403, "{body}");
    let auth = headers
        .lines()
        .find(|l| l.to_lowercase().starts_with("www-authenticate"))
        .unwrap_or_default();
    assert!(auth.contains("insufficient_scope"), "{auth}");

    // a reader reads
    let listed = server.rpc(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        Some("a-reader-token-of-length"),
    );
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 13);

    // no stream of its own, and a session ends politely
    let (status, _, _) = server.request("GET", "/mcp", None, Some("a-reader-token-of-length"));
    assert_eq!(status, 405);
    let (status, _, _) = server.request("DELETE", "/mcp", None, Some("a-reader-token-of-length"));
    assert_eq!(status, 204);
    server.stop();
}

/// The bar's second half: a grounding rule edited and an example swapped
/// ship with the pack, against the same binary.
#[test]
fn model_content_ships_with_the_pack() {
    let home = synthetic();
    let dir = TempDir::new("mcp-pack");
    let copy = dir.path().join("packs");
    copy_tree(&packs(), &copy);
    let mcp = copy.join("mri/mcp.yml");
    let text = std::fs::read_to_string(&mcp).unwrap();
    let edited = text
        .replace("version: \"1\"", "version: \"2\"")
        .replace(
            "A question is a document, not SQL.",
            "A question is a document the engine compiles.",
        )
        .replace(
            "question: How many subjects are in the cohort ms-cohort-a?",
            "question: How many subjects has this registry?",
        );
    assert_ne!(edited, text, "the edit lands");
    std::fs::write(&mcp, edited).unwrap();

    let server = Server::start(&home, &copy, &[]);
    let hello = server.rpc(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        None,
    );
    let instructions = hello["result"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("A question is a document the engine compiles"),
        "{instructions}"
    );
    let prompts = server.rpc(
        json!({"jsonrpc": "2.0", "id": 2, "method": "prompts/list"}),
        None,
    );
    assert!(
        prompts["result"]["prompts"][0]["title"]
            .as_str()
            .unwrap()
            .contains("has this registry"),
        "{prompts}"
    );
    let (_, _, body) = server.request("GET", "/api/capabilities", None, None);
    let caps: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(caps["mcp"]["content_version"], "2");
    assert_eq!(
        caps["contracts"]["pack"], "4",
        "the pack contract carries the model content"
    );
    server.stop();
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Wave 4c §6.7: every tool a live server lists carries its operation's
/// input schema exactly as `contracts/mcp/v1` fixes it; the operation of a
/// tool is what the pack opted it in as.
#[test]
fn every_listed_tool_s_input_schema_is_the_contract_s() {
    let contract: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../contracts/mcp/v1/mcp.schema.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let pack = nils_pack::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri"),
        None,
    )
    .unwrap();
    let model = pack.mcp.as_ref().expect("the MRI pack opts tools in");
    let home = synthetic();
    let server = Server::start(&home, &packs(), &[]);
    let hello = server.rpc(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        None,
    );
    assert_eq!(hello["result"]["serverInfo"]["name"], "nils", "{hello}");
    let listed = server.rpc(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        None,
    );
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        let op = &model
            .tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} is a tool the pack opted in"))
            .operation;
        assert_eq!(
            tool["inputSchema"], contract["$defs"]["input"][op],
            "{name} ({op}): the input schema is the contract's, verbatim"
        );
        assert!(name.starts_with("nils_"), "{name}");
    }
}
