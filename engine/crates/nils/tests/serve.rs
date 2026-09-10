// SPDX-License-Identifier: AGPL-3.0-only

//! `nils serve` (Wave 4a §11.1): the one door, driven as a second process
//! would drive it, over plain HTTP.

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

/// Run the command line in the registry and hand back its stdout.
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

fn packs() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

/// A registry with two classified stacks and their questions.
fn registry() -> TempDir {
    let home = TempDir::new("serve-home");
    let dir = TempDir::new("serve-src");
    for (study, sop) in [("1.2.3.A", "1.2.3.A.1.1"), ("1.2.3.B", "1.2.3.B.1.1")] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    let run = |args: &[&str], stdin: Option<&str>| {
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
    };
    run(&["key", "add", "k"], Some("a serve test key\n"));
    run(&["init", "--key", "k"], None);
    run(
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            dir.path().to_str().unwrap(),
        ],
        None,
    );
    run(&["fingerprint"], None);
    run(
        &[
            "classify",
            "--review-below",
            "1.0",
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
        None,
    );
    // The tree must outlive the registry's use only for the digest; the
    // registry keeps its own copy of what it read.
    std::mem::forget(dir);
    home
}

struct Server {
    child: Child,
    port: u16,
}

impl Server {
    /// Start `nils serve` on a free port, serving `requests` requests.
    fn start(home: &TempDir, requests: usize, extra: &[&str], env: &[(&str, &str)]) -> Server {
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
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        // A server that dies before it listens says why, never a bare unwrap.
        let Some(Ok(first)) = lines.next() else {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            let _ = child.wait();
            panic!("nils serve did not listen: {err}");
        };
        // "nils serve   127.0.0.1:PORT   auth ..."
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
        let (status, text) = self.raw(method, path, body, token);
        let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
        (status, json)
    }

    fn raw(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        token: Option<&str>,
    ) -> (u16, String) {
        self.raw_with(method, path, body, token, &[])
    }

    fn request_with(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        token: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (u16, serde_json::Value) {
        let (status, text) = self.raw_with(method, path, body, token, headers);
        let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
        (status, json)
    }

    fn raw_with(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        token: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (u16, String) {
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
        (status, body.to_string())
    }

    fn finish(mut self) {
        let status = self.child.wait().unwrap();
        assert!(status.success(), "nils serve exited {status}");
    }
}

#[test]
fn the_door_serves_what_the_command_line_has() {
    let home = registry();
    let server = Server::start(&home, 17, &[], &[]);

    // C26: the capabilities name the contracts, the pack, the epoch.
    let (status, caps) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["contracts"]["openapi"], "3", "{caps}");
    assert_eq!(caps["contracts"]["review_item"], "4", "{caps}");
    // Wave 4c §4.5: the engine's document is the `engine` part of the
    // deployment capabilities document, and carries what the suite requires.
    assert_eq!(caps["contracts"]["suite"], "1", "{caps}");
    assert_eq!(caps["contracts"]["mcp"], "1", "{caps}");
    let suite: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../contracts/suite/v1/capabilities.schema.json"),
        )
        .unwrap(),
    )
    .unwrap();
    for key in suite["properties"]["engine"]["required"]
        .as_array()
        .unwrap()
    {
        let key = key.as_str().unwrap();
        assert!(
            !caps[key].is_null(),
            "the suite requires {key} of the engine: {caps}"
        );
    }
    for row in caps["policy"].as_array().unwrap() {
        for key in suite["$defs"]["policy_row"]["required"].as_array().unwrap() {
            assert!(!row[key.as_str().unwrap()].is_null(), "policy row {row}");
        }
        let costs: Vec<&str> = suite["$defs"]["policy_row"]["properties"]["cost"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c.as_str())
            .collect();
        assert!(
            costs.contains(&row["cost"].as_str().unwrap()),
            "policy row {row}"
        );
    }
    assert!(
        caps["packs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "mri"),
        "{caps}"
    );
    let epoch = caps["registry"]["epoch"].as_i64().unwrap();
    assert!(epoch > 0, "{caps}");
    assert_eq!(caps["auth"], "off", "{caps}");
    assert_eq!(caps["principal"], "anna@ward-3", "{caps}");
    let doors: Vec<&str> = caps["doors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect();
    assert!(doors.contains(&"POST /api/review/{id}/apply"), "{doors:?}");

    // Every door the engine lists is in the contract document.
    let version = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/openapi/VERSION"),
    )
    .unwrap();
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../../contracts/openapi/v{}/openapi.yaml",
        version.trim()
    )))
    .unwrap();
    for door in &doors {
        let path = door.split_whitespace().nth(1).unwrap();
        assert!(
            text.contains(&format!("  {path}:")),
            "{door} is not in the contract"
        );
    }

    let (status, doc) = server.request("GET", "/api/status", None, None);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["registry"]["epoch"], epoch, "{doc}");
    let (status, doc) = server.request("GET", "/api/custody", None, None);
    assert_eq!(status, 200, "{doc}");
    assert!(
        doc["stores"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["store"] == "audit log"),
        "{doc}"
    );

    // The review queue: grouped questions, one applied through the door.
    let (status, listed) = server.request("GET", "/api/review?status=open", None, None);
    assert_eq!(status, 200, "{listed}");
    let base = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "base:low_confidence")
        .unwrap()
        .clone();
    assert_eq!(base["scope"], "group", "{base}");
    let id = base["id"].as_i64().unwrap();
    let (status, shown) = server.request("GET", &format!("/api/review/{id}"), None, None);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(
        shown["member_stacks"].as_array().unwrap().len(),
        2,
        "{shown}"
    );
    let (status, applied) = server.request(
        "POST",
        &format!("/api/review/{id}/apply"),
        Some(r#"{"value": "T2w", "why": "through the door"}"#),
        None,
    );
    assert_eq!(status, 200, "{applied}");
    assert_eq!(applied["scope"], "group", "{applied}");
    assert_eq!(applied["members"], 2, "{applied}");
    let decision = applied["decision"].as_i64().unwrap();
    // Twice is a refusal, 409.
    let (status, again) = server.request(
        "POST",
        &format!("/api/review/{id}/apply"),
        Some(r#"{"value": "T2w"}"#),
        None,
    );
    assert_eq!(status, 409, "{again}");
    // Withdrawn through the door, with the principal on the audit row.
    let (status, withdrawn) = server.request(
        "POST",
        &format!("/api/decisions/{decision}/withdraw"),
        None,
        None,
    );
    assert_eq!(status, 200, "{withdrawn}");
    assert_eq!(withdrawn["reopened"], 1, "{withdrawn}");
    let (status, audit) = server.request("GET", "/api/audit?action=decision&limit=5", None, None);
    assert_eq!(status, 200, "{audit}");
    assert_eq!(audit["rows"][0]["principal"], "anna@ward-3", "{audit}");

    // Anything heavy is a queued job, 202, read back by its id.
    let (status, queued) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["fingerprint", "--name", "through-the-door"], "name": "door"}"#),
        None,
    );
    assert_eq!(status, 202, "{queued}");
    let job = queued["job"].as_i64().unwrap();
    let (status, shown) = server.request("GET", &format!("/api/jobs/{job}"), None, None);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["state"], "queued", "{shown}");
    assert_eq!(shown["args"]["principal"], "anna@ward-3", "{shown}");
    let (status, refused) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["linkage", "purge", "--all"]}"#),
        None,
    );
    assert_eq!(status, 400, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("linkage import"),
        "{refused}"
    );
    // Wave 4c §6.5: linkage import is queued, over a registered location only
    let (status, refused) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["linkage", "import", "/etc/passwd"]}"#),
        None,
    );
    assert_eq!(status, 400, "{refused}");
    let (status, cancelled) =
        server.request("POST", &format!("/api/jobs/{job}/cancel"), None, None);
    assert_eq!(status, 200, "{cancelled}");
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");

    // The bounded synchronous path: a selection, previewed.
    let (status, selected) =
        server.request("POST", "/api/select", Some(r#"{"subjects": ["P1"]}"#), None);
    assert_eq!(status, 200, "{selected}");
    assert_eq!(selected["reaches"]["subjects"], 1, "{selected}");
    assert_eq!(
        selected["items"][0]["how"]["kind"], "identifier",
        "{selected}"
    );

    // Nothing released yet; a door that does not exist says so.
    let (status, releases) = server.request("GET", "/api/releases", None, None);
    assert_eq!(status, 200, "{releases}");
    assert_eq!(releases["count"], 0, "{releases}");
    let (status, _) = server.request("GET", "/api/nothing", None, None);
    assert_eq!(status, 404);
    server.finish();
}

#[test]
fn under_token_auth_the_token_names_the_principal() {
    let home = registry();
    let server = Server::start(
        &home,
        3,
        &[
            "--auth",
            "token",
            "--token",
            "sixteen-characters-long=bo@lab-2",
        ],
        &[],
    );
    let (status, doc) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 401, "{doc}");
    let (status, doc) = server.request(
        "GET",
        "/api/capabilities",
        None,
        Some("wrong-token-of-length"),
    );
    assert_eq!(status, 401, "{doc}");
    let (status, doc) = server.request(
        "GET",
        "/api/capabilities",
        None,
        Some("sixteen-characters-long"),
    );
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["auth"], "token", "{doc}");
    assert_eq!(doc["principal"], "bo@lab-2", "{doc}");
    server.finish();
}

#[test]
fn the_event_stream_is_display_plumbing() {
    let home = registry();
    let server = Server::start(&home, 1, &[], &[("NILS_EVENTS_ONCE", "1")]);
    let (status, text) = server.raw("GET", "/api/events", None, None);
    assert_eq!(status, 200, "{text}");
    assert!(text.starts_with("event: hello"), "{text}");
    assert!(text.contains("event: jobs\ndata: {"), "{text}");
    server.finish();
}

/// Wave 4a §11.2: the `oidc` mode. The engine validates the token against
/// the issuer's keys and audience, maps groups to roles, makes the subject
/// the audit principal, and keeps no user table beyond a cache of claims.
#[test]
fn under_oidc_the_subject_is_the_principal_and_groups_are_roles() {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc");
    let jwks = fixtures.join("jwks.json");
    let pem = std::fs::read(fixtures.join("signing-key.pem")).unwrap();
    let key = EncodingKey::from_rsa_pem(&pem).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let token = |sub: &str, groups: &[&str], aud: &str, exp: u64, kid: Option<&str>| -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = kid.map(String::from);
        let claims = serde_json::json!({
            "iss": "https://id.example.org/application/o/nils/",
            "aud": aud,
            "sub": sub,
            "exp": exp,
            "iat": now,
            "groups": groups,
        });
        encode(&header, &claims, &key).unwrap()
    };
    let home = registry();
    let server = Server::start(
        &home,
        11,
        &[
            "--auth",
            "oidc",
            "--oidc-issuer",
            "https://id.example.org/application/o/nils/",
            "--oidc-audience",
            "nils",
            "--oidc-jwks",
            jwks.to_str().unwrap(),
            "--role",
            "students=reader",
            "--role",
            "neuro-reviewers=reviewer",
            "--role",
            "neuro-ops=operator",
        ],
        &[],
    );
    // No token, a token for another audience, an expired one: 401.
    let (status, doc) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 401, "{doc}");
    let other = token("anna", &[], "someone-else", now + 600, Some("test-2026"));
    let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&other));
    assert_eq!(status, 401, "{doc}");
    let stale = token("anna", &[], "nils", now - 600, Some("test-2026"));
    let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&stale));
    assert_eq!(status, 401, "{doc}");

    // Wave 4b §12.4: a token whose groups map to nothing holds no role and
    // is refused at every door, never defaulted to reader.
    let unmapped = token("kit", &["guests"], "nils", now + 600, Some("test-2026"));
    let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&unmapped));
    assert_eq!(status, 403, "{doc}");
    assert!(doc["error"].as_str().unwrap().contains("no role"), "{doc}");

    // A reader: the subject at the issuer's node, the reader role only, and
    // a door that asks for more says so with 403.
    let reader = token("anna", &["students"], "nils", now + 600, Some("test-2026"));
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&reader));
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["auth"], "oidc", "{caps}");
    assert_eq!(caps["principal"], "anna@id.example.org", "{caps}");
    assert_eq!(caps["roles"], serde_json::json!(["reader"]), "{caps}");
    let (status, doc) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["fingerprint"]}"#),
        Some(&reader),
    );
    assert_eq!(status, 403, "{doc}");
    assert!(doc["error"].as_str().unwrap().contains("operator"), "{doc}");
    let (status, listed) = server.request("GET", "/api/review?status=open", None, Some(&reader));
    assert_eq!(status, 200, "{listed}");

    // A reviewer decides, and the audit row carries the subject.
    let reviewer = token(
        "bo",
        &["neuro-reviewers"],
        "nils",
        now + 600,
        Some("test-2026"),
    );
    let id = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "base:low_confidence")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let (status, applied) = server.request(
        "POST",
        &format!("/api/review/{id}/apply"),
        Some(r#"{"value": "T2w"}"#),
        Some(&reviewer),
    );
    assert_eq!(status, 200, "{applied}");
    // An operator reads the audit? No: that is the admin's; an operator
    // queues work. A role implies the ones below it, so the operator
    // reads and decides too.
    let operator = token(
        "cy",
        &["neuro-ops", "students"],
        "nils",
        now + 600,
        Some("test-2026"),
    );
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&operator));
    assert_eq!(status, 200, "{caps}");
    assert_eq!(
        caps["roles"],
        serde_json::json!(["reader", "reviewer", "operator"]),
        "{caps}"
    );
    let (status, doc) = server.request(
        "GET",
        "/api/audit?action=decision&limit=1",
        None,
        Some(&operator),
    );
    assert_eq!(status, 403, "{doc}");
    let (status, queued) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["fingerprint"]}"#),
        Some(&operator),
    );
    assert_eq!(status, 202, "{queued}");
    server.finish();
    // The audit row of the decision names the subject at the issuer's node.
    let mut store = nils_registry::Store::open_sqlite(&home.path().join("registry.db")).unwrap();
    let who = store
        .query(
            "SELECT principal FROM audit WHERE action = 'decision' ORDER BY id DESC LIMIT 1",
            &[],
        )
        .unwrap()[0]
        .text(0)
        .unwrap()
        .to_string();
    assert_eq!(who, "bo@id.example.org");
    let queued_by = store
        .query(
            "SELECT args FROM job WHERE state = 'queued' ORDER BY id DESC LIMIT 1",
            &[],
        )
        .unwrap()[0]
        .text(0)
        .unwrap()
        .to_string();
    assert!(queued_by.contains("cy@id.example.org"), "{queued_by}");
}

#[test]
fn oidc_refuses_a_misconfiguration_before_it_listens() {
    let home = registry();
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(["serve", "--auth", "oidc", "--bind", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--oidc-issuer"));
}

/// A JWKS document served over HTTP from a thread, replaceable while the
/// engine runs: what an issuer looks like to `--oidc-trust jwks=URL`.
fn serve_jwks(doc: std::sync::Arc<std::sync::Mutex<String>>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    serve_jwks_listener(listener, doc);
    port
}

/// The same, on a port already chosen, for an issuer that starts late.
fn serve_jwks_on(port: u16, doc: std::sync::Arc<std::sync::Mutex<String>>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    serve_jwks_listener(listener, doc);
}

fn serve_jwks_listener(
    listener: std::net::TcpListener,
    doc: std::sync::Arc<std::sync::Mutex<String>>,
) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let body = doc.lock().unwrap().clone();
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
}

/// Wave 4c §5.3 and §5.9: a trust list of two issuers, each with its own
/// audience and keys; keys by URL refetched on a key id the engine does not
/// hold; the `act` claim read as the actor; the display name and mail kept.
#[test]
fn a_trust_list_verifies_two_issuers_and_refetches_a_rotated_key_by_url() {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc");
    let key1 = EncodingKey::from_rsa_pem(&std::fs::read(fixtures.join("signing-key.pem")).unwrap())
        .unwrap();
    let key2 =
        EncodingKey::from_rsa_pem(&std::fs::read(fixtures.join("signing-key-2.pem")).unwrap())
            .unwrap();
    let served = std::sync::Arc::new(std::sync::Mutex::new(
        std::fs::read_to_string(fixtures.join("jwks.json")).unwrap(),
    ));
    let port = serve_jwks(served.clone());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mint = |key: &EncodingKey, kid: &str, mut claims: serde_json::Value| -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        claims["exp"] = serde_json::json!(now + 600);
        claims["iat"] = serde_json::json!(now);
        encode(&header, &claims, key).unwrap()
    };
    let iss1 = "https://id.example.org/application/o/nils/";
    let iss2 = "https://other.example.org/";
    let home = registry();
    let trust1 = format!("issuer={iss1},audience=nils,jwks=http://127.0.0.1:{port}/jwks");
    let trust2 = format!(
        "issuer={iss2},audience=desk,jwks={}",
        fixtures.join("jwks-2.json").display()
    );
    let server = Server::start(
        &home,
        7,
        &[
            "--auth",
            "oidc",
            "--oidc-trust",
            &trust1,
            "--oidc-trust",
            &trust2,
            "--jwks-refetch-secs",
            "0",
            "--role",
            "students=reader",
            "--role",
            "neuro-ops=operator",
        ],
        &[],
    );
    // 1. the first issuer, its key fetched by URL at start
    let t = mint(
        &key1,
        "test-2026",
        serde_json::json!({"iss": iss1, "aud": "nils", "sub": "anna", "groups": ["students"], "preferred_username": "Anna", "email": "anna@example.org"}),
    );
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&t));
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["principal"], "anna@id.example.org");
    assert_eq!(caps["display"], "Anna", "{caps}");
    assert_eq!(caps["email"], "anna@example.org", "{caps}");
    assert_eq!(caps["actor"]["kind"], "absent", "{caps}");
    // 2. a key the issuer has not published: refetched, still absent, refused
    let rotated = mint(
        &key2,
        "test-2027",
        serde_json::json!({"iss": iss1, "aud": "nils", "sub": "anna", "groups": ["students"]}),
    );
    let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&rotated));
    assert_eq!(status, 401, "{doc}");
    // 3. the issuer rotates: the next call refetches and verifies
    *served.lock().unwrap() = std::fs::read_to_string(fixtures.join("jwks-rotated.json")).unwrap();
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&rotated));
    assert_eq!(status, 200, "{caps}");
    // 4. the second issuer, with its own audience, from a file
    let t2 = mint(
        &key2,
        "test-2027",
        serde_json::json!({"iss": iss2, "aud": "desk", "sub": "kit", "groups": ["neuro-ops"]}),
    );
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&t2));
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["principal"], "kit@other.example.org", "{caps}");
    assert_eq!(
        caps["roles"],
        serde_json::json!(["reader", "reviewer", "operator"]),
        "{caps}"
    );
    // 5. an audience that is the other issuer's is refused
    let crossed = mint(
        &key2,
        "test-2027",
        serde_json::json!({"iss": iss2, "aud": "nils", "sub": "kit", "groups": ["neuro-ops"]}),
    );
    let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&crossed));
    assert_eq!(status, 401, "{doc}");
    // 6. an exchanged token names its actor
    let acted = mint(
        &key2,
        "test-2027",
        serde_json::json!({"iss": iss1, "aud": "nils", "sub": "anna", "groups": ["students"], "act": {"sub": "nils-assistant"}}),
    );
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&acted));
    assert_eq!(status, 200, "{caps}");
    assert_eq!(
        caps["actor"],
        serde_json::json!({"kind": "agent", "name": "nils-assistant"}),
        "{caps}"
    );
    // 7. a ceiling narrows an operator to a reviewer, and says so
    let (status, caps) = server.request_with(
        "GET",
        "/api/capabilities",
        None,
        Some(&t2),
        &[("X-Nils-Ceiling", "reviewer")],
    );
    assert_eq!(status, 200, "{caps}");
    assert_eq!(
        caps["roles"],
        serde_json::json!(["reader", "reviewer"]),
        "{caps}"
    );
    assert_eq!(caps["ceiling"], "reviewer", "{caps}");
    assert_eq!(caps["actor"]["ceiling"], "reviewer", "{caps}");
    server.finish();
}

/// An issuer that is not up when the engine starts is a matter of order,
/// not a fault in the configuration. In a container run the desk that mints
/// the tokens usually comes up after the engine that trusts it, and an
/// engine that exits there never comes back on its own. So the engine
/// starts holding no key of that issuer, says so, and asks again when the
/// first token arrives, without waiting out the refetch floor.
#[test]
fn an_issuer_that_is_not_up_yet_does_not_stop_the_engine() {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc");
    let key = EncodingKey::from_rsa_pem(&std::fs::read(fixtures.join("signing-key.pem")).unwrap())
        .unwrap();
    // A port with nothing behind it: the issuer this engine trusts has not
    // started yet.
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    drop(held);
    let iss = "https://id.example.org/application/o/nils/";
    let home = registry();
    let trust = format!("issuer={iss},audience=nils,jwks=http://127.0.0.1:{port}/jwks");
    // Server::start panics when the engine dies before it listens, so
    // reaching the next line is half of what this test says.
    // Two requests, which is what this server is told to serve: one while
    // the issuer is down, one after it comes up.
    let server = Server::start(
        &home,
        2,
        &[
            "--auth",
            "oidc",
            "--oidc-trust",
            &trust,
            "--role",
            "students=reader",
        ],
        &[],
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-2026".to_string());
    let token = encode(
        &header,
        &serde_json::json!({"iss": iss, "aud": "nils", "sub": "anna", "groups": ["students"], "exp": now + 600, "iat": now}),
        &key,
    )
    .unwrap();
    // The issuer is still down: the token is refused, and the engine is the
    // one refusing it, which is the point.
    let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&token));
    assert_eq!(status, 401, "{doc}");
    // The issuer comes up. The engine asks again on its own, without the
    // sixty second floor this engine was given, because it holds no key of
    // that issuer at all. It does wait the short floor that replaces it,
    // which is what keeps an issuer that stays down from costing every
    // request a fetch of its own.
    let served = std::sync::Arc::new(std::sync::Mutex::new(
        std::fs::read_to_string(fixtures.join("jwks.json")).unwrap(),
    ));
    serve_jwks_on(port, served);
    std::thread::sleep(std::time::Duration::from_millis(5_200));
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&token));
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["principal"], "anna@id.example.org", "{caps}");
    server.finish();
}

/// Wave 4c §6.5: the deployment surface. Packs, batches and quarantine
/// have doors; a queued tree is named by a registered location, never by
/// a path; backup is a job whose archive verifies; the policy table names
/// every door; restore is a command that refuses without --yes.
#[test]
fn the_deployment_surface_has_doors_locations_and_an_archive_that_verifies() {
    let home = registry();
    let root = TempDir::new("a5-root");
    std::fs::create_dir_all(root.path().join("sub")).unwrap();
    let backups = TempDir::new("a5-backups");
    let root_flag = format!("src={}", root.path().display());
    let server = Server::start(
        &home,
        13,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--ingest-root",
            &root_flag,
            "--backup-dir",
            backups.path().to_str().unwrap(),
        ],
        &[],
    );
    let reader = Some("a-reader-token-of-length");
    let ops = Some("an-operator-token-of-len");
    let (status, caps) = server.request("GET", "/api/capabilities", None, ops);
    assert_eq!(status, 200, "{caps}");
    assert!(
        caps["policy"].as_array().is_some_and(|p| p.len() >= 40),
        "{caps}"
    );
    assert_eq!(caps["ingest_roots"], serde_json::json!(["src"]), "{caps}");
    assert_eq!(caps["backup_dir"], true, "{caps}");
    let (status, packs_doc) = server.request("GET", "/api/packs", None, reader);
    assert_eq!(status, 200, "{packs_doc}");
    assert_eq!(packs_doc["packs"][0]["name"], "mri", "{packs_doc}");
    let (status, pack) = server.request("GET", "/api/packs/mri", None, reader);
    assert_eq!(status, 200, "{pack}");
    assert!(
        pack["axes"].as_array().is_some_and(|a| !a.is_empty()),
        "{pack}"
    );
    let (status, batches) = server.request("GET", "/api/batches", None, reader);
    assert_eq!(status, 200, "{batches}");
    assert!(batches["count"].as_i64().unwrap() >= 1, "{batches}");
    let (status, batch) = server.request("GET", "/api/batches/1", None, reader);
    assert_eq!(status, 200, "{batch}");
    assert!(batch["report"].is_object(), "{batch}");
    let (status, refused) = server.request("GET", "/api/quarantine", None, reader);
    assert_eq!(status, 403, "{refused}");
    let (status, quarantine) = server.request("GET", "/api/quarantine", None, ops);
    assert_eq!(status, 200, "{quarantine}");
    assert_eq!(quarantine["count"], 0, "{quarantine}");
    // a path a caller composes is refused; a location is resolved
    for bad in ["/etc", "@src/../x", "@nowhere/x"] {
        let (status, doc) = server.request(
            "POST",
            "/api/jobs",
            Some(&format!(r#"{{"command": ["digest", "{bad}"]}}"#)),
            ops,
        );
        assert_eq!(status, 400, "{bad}: {doc}");
    }
    let (status, queued) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["digest", "@src/sub"]}"#),
        ops,
    );
    assert_eq!(status, 202, "{queued}");
    let digest_job = queued["job"].as_i64().unwrap();
    let (status, shown) = server.request("GET", &format!("/api/jobs/{digest_job}"), None, ops);
    assert_eq!(status, 200, "{shown}");
    let argv = shown["args"]["argv"].to_string();
    assert!(
        argv.contains(&root.path().join("sub").display().to_string()),
        "{argv}"
    );
    assert!(!argv.contains("@src"), "{argv}");
    let (status, queued) =
        server.request("POST", "/api/jobs", Some(r#"{"command": ["backup"]}"#), ops);
    assert_eq!(status, 202, "{queued}");
    server.finish();
    // a worker runs both; the archive verifies; the audit log says so
    run(&home, &["jobs", "work", "--once"], None);
    run(&home, &["jobs", "work", "--once"], None);
    let archives: Vec<_> = std::fs::read_dir(backups.path())
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(archives.len(), 1, "one archive");
    let archive = archives[0].path();
    let verified = run(
        &home,
        &["verify", archive.to_str().unwrap(), "--json"],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&verified).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["files"].as_array().unwrap().len() >= 2, "{v}");
    let audited = run(
        &home,
        &["audit", "list", "--action", "backup", "--json"],
        None,
    );
    assert!(audited.contains("\"backup\""), "{audited}");
    // restore refuses without --yes, and puts the archive back with it
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(["restore", archive.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let restored = run(
        &home,
        &["restore", archive.to_str().unwrap(), "--yes"],
        None,
    );
    assert!(restored.contains("pre-restore archive"), "{restored}");
    let status = run(&home, &["status", "--json"], None);
    assert!(status.contains("registry_id"), "{status}");
}

/// A registry of two stacks whose sequence names differ by a site word, so
/// that an overlay adding the word to the localizer bucket moves exactly one.
fn knob_registry() -> TempDir {
    let home = TempDir::new("knob-home");
    let dir = TempDir::new("knob-src");
    for (study, sop, description) in [
        ("1.2.3.A", "1.2.3.A.1.1", "t1 mprage zzgado"),
        ("1.2.3.B", "1.2.3.B.1.1", "t1 mprage"),
    ] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
        e.push(synth::text(tags::SEQUENCE_NAME, VR::SH, "tfl3d1_16"));
        e.push(synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    run(&home, &["key", "add", "k"], Some("a knob test key\n"));
    run(&home, &["init", "--key", "k"], None);
    run(
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
    run(&home, &["fingerprint"], None);
    run(
        &home,
        &["classify", "--pack-dir", packs().to_str().unwrap()],
        None,
    );
    std::mem::forget(dir);
    home
}

const SITE_OVERLAY: &str = r#"{
  "overlay": "site", "version": "1.0.0", "pack": "mri",
  "scope": {"manufacturer": "SYNTHETIC"},
  "buckets": {"contrast_positive": {"add": ["zzgado", "zzznever"]}},
  "cases": [{"name": "the site's own agent",
             "stack": {"text_series_description": "t1 mprage zzgado"},
             "axes": {"post_contrast": "1"}}]
}"#;

/// Wave 4c §6.6: the knob engine. A site word added to an overlay: `try`
/// names the stacks that would move and adopt moves exactly those; the
/// probe on a synthetic tree names the placeholder by shape and no field of
/// its answer is a seeded value or a path segment.
#[test]
fn the_knob_engine_rehearses_proposes_adopts_and_probes() {
    let home = knob_registry();
    let root = TempDir::new("knob-root");
    for (person, study) in [("AAA111", "A"), ("BBB222", "B"), ("CCC333", "C")] {
        for i in 1..=8 {
            let sop = format!("{study}.1.{i}");
            let mut e = synth::minimal_mr(study, &format!("{study}.1"), &sop);
            e.push(synth::text(tags::PATIENT_ID, VR::LO, "XXXX"));
            root.file(
                &format!("{person}/{study}/IM_{i:04}"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
    }
    let root_flag = format!("src={}", root.path().display());
    let server = Server::start(
        &home,
        17,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "a-reviewer-token-of-len=rev@lab:reviewer",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--ingest-root",
            &root_flag,
        ],
        &[],
    );
    let reader = Some("a-reader-token-of-length");
    let reviewer = Some("a-reviewer-token-of-len");
    let ops = Some("an-operator-token-of-len");

    // 1-3: the signals are a reviewer's, over a scope that parses
    let (status, doc) = server.request("GET", "/api/classify/signals?scope=batch:1", None, reader);
    assert_eq!(status, 403, "{doc}");
    let (status, doc) = server.request(
        "GET",
        "/api/classify/signals?scope=nonsense",
        None,
        reviewer,
    );
    assert_eq!(status, 400, "{doc}");
    let (status, signals) =
        server.request("GET", "/api/classify/signals?scope=batch:1", None, reviewer);
    assert_eq!(status, 200, "{signals}");
    assert!(
        signals["axes"]["technique"]["tiers"].is_object(),
        "{signals}"
    );
    assert!(signals["diagnostics"].is_object(), "{signals}");

    // 4: a rehearsal writes nothing and names what would move
    let body = format!(r#"{{"overlay": {SITE_OVERLAY}, "scope": "batch:1", "sample": 100}}"#);
    let (status, tried) = server.request("POST", "/api/classify/try", Some(&body), reviewer);
    assert_eq!(status, 200, "{tried}");
    assert_eq!(tried["sample"]["read"], 2, "{tried}");
    assert_eq!(tried["cases"]["passed"], 1, "{tried}");
    assert_eq!(tried["cases"]["failed"], 0, "{tried}");
    let moves = tried["moves"].as_array().unwrap();
    assert!(
        moves
            .iter()
            .any(|m| m["axis"] == "post_contrast" && m["to"] == "1"),
        "the site's agent moves post_contrast: {tried}"
    );
    assert!(
        moves.iter().all(|m| m["stacks"] == 1),
        "exactly the one stack with the word: {tried}"
    );
    let (status, doc) = server.request("GET", "/api/overlays", None, reader);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(
        doc["overlays"].as_array().unwrap().len(),
        0,
        "nothing was stored: {doc}"
    );

    // 6-8: a proposal is stored with its rehearsal and a review item beside it
    let body = format!(
        r#"{{"name": "site words", "overlay": {SITE_OVERLAY}, "scope": "batch:1", "why": "the site's localizer word"}}"#
    );
    let (status, doc) = server.request("POST", "/api/overlays", Some(&body), reader);
    assert_eq!(status, 403, "{doc}");
    let (status, proposed) = server.request("POST", "/api/overlays", Some(&body), reviewer);
    assert_eq!(status, 201, "{proposed}");
    let id = proposed["overlay"]["id"].as_i64().unwrap();
    let item = proposed["review_item"].as_i64().unwrap();
    assert_eq!(proposed["overlay"]["status"], "proposed", "{proposed}");
    assert_eq!(proposed["overlay"]["author_kind"], "person", "{proposed}");
    assert_eq!(
        proposed["overlay"]["tried"]["moves"], tried["moves"],
        "{proposed}"
    );
    let (status, shown) = server.request("GET", &format!("/api/review/{item}"), None, reviewer);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["kind"], "overlay.proposed", "{shown}");
    assert_eq!(shown["scope"], "overlay", "{shown}");

    // 9-11: adoption is an operator's, once, and queues the reclassify
    let (status, doc) =
        server.request("POST", &format!("/api/overlays/{id}/adopt"), None, reviewer);
    assert_eq!(status, 403, "{doc}");
    let (status, adopted) = server.request("POST", &format!("/api/overlays/{id}/adopt"), None, ops);
    assert_eq!(status, 202, "{adopted}");
    let adopt_job = adopted["job"].as_i64().unwrap();
    let (status, doc) = server.request("POST", &format!("/api/overlays/{id}/adopt"), None, ops);
    assert_eq!(status, 409, "{doc}");

    // 12-15: the probe is an operator's, over a registered location, with
    // rules that parse; the queued command line carries no path
    let rules = r#"[{"id_type": "patient-id", "from": [{"field": "PatientID"}]},
                    {"id_type": "subject-code", "code": "verbatim", "from": [{"field": "PatientID", "pattern": "^(?<id>[A-Z]{3}[0-9]{3})$"}, {"path": {"segment": 1}, "pattern": "^(?<id>.+)$"}]}]"#;
    let body = format!(r#"{{"location": "src", "sample": 50, "rules": {rules}}}"#);
    let (status, doc) = server.request("POST", "/api/ingest/probe", Some(&body), reviewer);
    assert_eq!(status, 403, "{doc}");
    let (status, doc) = server.request(
        "POST",
        "/api/ingest/probe",
        Some(&format!(r#"{{"location": "nowhere", "rules": {rules}}}"#)),
        ops,
    );
    assert_eq!(status, 400, "{doc}");
    let (status, doc) = server.request(
        "POST",
        "/api/ingest/probe",
        Some(r#"{"location": "src", "rules": [{"id_type": "patient-id", "from": [{"field": "NotAKeyword"}]}]}"#),
        ops,
    );
    assert_eq!(status, 400, "{doc}");
    let (status, queued) = server.request("POST", "/api/ingest/probe", Some(&body), ops);
    assert_eq!(status, 202, "{queued}");
    let probe_job = queued["job"].as_i64().unwrap();
    // 16
    let (status, shown) = server.request("GET", &format!("/api/jobs/{probe_job}"), None, ops);
    assert_eq!(status, 200, "{shown}");
    let argv = shown["args"]["argv"].to_string();
    assert!(argv.contains("@src"), "{argv}");
    assert!(!argv.contains(&root.path().display().to_string()), "{argv}");
    // 17
    let (status, listed) = server.request("GET", &format!("/api/overlays/{id}"), None, reader);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed["status"], "adopted", "{listed}");
    assert_eq!(listed["job"], adopt_job, "{listed}");
    server.finish();

    // a worker runs the reclassify under the adopted overlay, then the probe
    for _ in 0..2 {
        let _ = run(
            &home,
            &["jobs", "work", "--once", "--ingest-root", &root_flag],
            None,
        );
    }
    let job = run(
        &home,
        &["jobs", "show", &adopt_job.to_string(), "--json"],
        None,
    );
    let job: serde_json::Value = serde_json::from_str(&job).unwrap();
    assert_eq!(job["state"], "done", "{job}");
    // adopt moved exactly what try said: the stack whose evidence cites the
    // site's word now stores `to`, the other is unchanged. Which id is
    // which depends on the digest's walk, so the word decides, not the id.
    let explained: Vec<serde_json::Value> = [1, 2]
        .iter()
        .map(|id| {
            let text = run(&home, &["explain", &id.to_string(), "--json"], None);
            serde_json::from_str(&text).unwrap()
        })
        .collect();
    let cites = |doc: &serde_json::Value| {
        doc["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["matched"] == "zzgado")
    };
    let moved = explained
        .iter()
        .find(|d| cites(d))
        .expect("one stack cites the site's word");
    let still = explained
        .iter()
        .find(|d| !cites(d))
        .expect("and one does not");
    assert_eq!(moved["overlay"], "site@1.0.0", "{moved}");
    assert_eq!(still["overlay"], "site@1.0.0", "{still}");
    let value_of = |doc: &serde_json::Value, axis: &str| -> String {
        doc["axes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["axis"] == axis)
            .filter_map(|a| a["value"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join(",")
    };
    for m in moves {
        let axis = m["axis"].as_str().unwrap();
        assert_eq!(
            value_of(moved, axis),
            m["to"].as_str().unwrap(),
            "{axis}: {moved}"
        );
        assert_eq!(
            value_of(still, axis),
            m["from"].as_str().unwrap(),
            "{axis}: {still}"
        );
    }
    let audited = run(
        &home,
        &["audit", "list", "--action", "overlay.adopt", "--json"],
        None,
    );
    assert!(audited.contains("\"overlay.adopt\""), "{audited}");

    // the probe's answer: shapes, counts, no value and no path
    let job = run(
        &home,
        &["jobs", "show", &probe_job.to_string(), "--json"],
        None,
    );
    let job: serde_json::Value = serde_json::from_str(&job).unwrap();
    assert_eq!(job["state"], "done", "{job}");
    let result = &job["result"];
    assert_eq!(result["sample"]["files"], 24, "{result}");
    let c = &result["candidates"];
    assert_eq!(c[0]["subjects"], 1, "{result}");
    assert_eq!(c[0]["identity_constant"]["constant"], true, "{result}");
    assert_eq!(c[0]["identity_constant"]["shape"], "AAAA", "{result}");
    assert_eq!(c[1]["subjects"], 3, "{result}");
    assert_eq!(c[1]["sources"][1]["shapes"]["AAA999"], 24, "{result}");
    let text = result.to_string();
    for marker in ["XXXX", "AAA111", "BBB222", "CCC333", "IM_0001"] {
        assert!(!text.contains(marker), "{marker} escaped: {text}");
    }
    assert!(!text.contains(&root.path().display().to_string()), "{text}");
    // and the command line verbs read the same objects
    let listed = run(&home, &["overlay", "list"], None);
    assert!(
        listed.contains("adopted") && listed.contains("site words@1.0.0"),
        "{listed}"
    );
    let out = TempDir::new("knob-export");
    let wrote = run(
        &home,
        &[
            "overlay",
            "export",
            &id.to_string(),
            "--to",
            out.path().to_str().unwrap(),
        ],
        None,
    );
    assert!(wrote.contains("site-1.0.0.overlay.json"), "{wrote}");
    let exported = nils_pack::Overlay::load(&out.path().join("site-1.0.0.overlay.json")).unwrap();
    assert_eq!(exported.id, "site@1.0.0");
}

/// Wave 4c §5.3: the trust list vectors of `contracts/suite/v1` run against
/// the engine. Each case is minted with the key it names and presented; what
/// happened is compared with what the vector expects.
#[test]
fn the_trust_list_vectors_hold() {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/suite/v1/vectors");
    let t: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(vectors.join("trust-list.json")).unwrap())
            .unwrap();
    let keys: std::collections::BTreeMap<String, EncodingKey> = t["keys"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(kid, pem)| {
            let pem = std::fs::read(vectors.join(pem.as_str().unwrap())).unwrap();
            (kid.clone(), EncodingKey::from_rsa_pem(&pem).unwrap())
        })
        .collect();
    // the first entry's JWKS by URL, the second's by file
    let served = std::sync::Arc::new(std::sync::Mutex::new(
        std::fs::read_to_string(vectors.join(t["trust"][0]["jwks"].as_str().unwrap())).unwrap(),
    ));
    let port = serve_jwks(served.clone());
    let trust1 = format!(
        "issuer={},audience={},jwks=http://127.0.0.1:{port}/jwks",
        t["trust"][0]["issuer"].as_str().unwrap(),
        t["trust"][0]["audience"].as_str().unwrap()
    );
    let trust2 = format!(
        "issuer={},audience={},jwks={}",
        t["trust"][1]["issuer"].as_str().unwrap(),
        t["trust"][1]["audience"].as_str().unwrap(),
        vectors
            .join(t["trust"][1]["jwks"].as_str().unwrap())
            .display()
    );
    let mut extra: Vec<String> = vec![
        "--auth".into(),
        "oidc".into(),
        "--oidc-trust".into(),
        trust1,
        "--oidc-trust".into(),
        trust2,
        "--jwks-refetch-secs".into(),
        "0".into(),
        "--oidc-groups-claim".into(),
        t["groups_claim"].as_str().unwrap().into(),
    ];
    for (group, role) in t["roles"].as_object().unwrap() {
        extra.push("--role".into());
        extra.push(format!("{group}={}", role.as_str().unwrap()));
    }
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    let cases = t["cases"].as_array().unwrap();
    let home = registry();
    let server = Server::start(&home, cases.len(), &extra, &[]);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let mut header = Header::new(Algorithm::RS256);
        let kid = case["key"].as_str().unwrap();
        header.kid = Some(kid.to_string());
        let mut claims = case["claims"].clone();
        let expired = case["expired"].as_bool() == Some(true);
        claims["iat"] = serde_json::json!(if expired { now - 1200 } else { now });
        claims["exp"] = serde_json::json!(if expired { now - 600 } else { now + 600 });
        let token = encode(&header, &claims, &keys[kid]).unwrap();
        let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&token));
        let expect = &case["expect"];
        if expect["admitted"] == true {
            assert_eq!(status, 200, "{name}: {doc}");
            for key in ["principal", "roles", "display", "email"] {
                if !expect[key].is_null() {
                    assert_eq!(doc[key], expect[key], "{name}: {key}: {doc}");
                }
            }
            if let Some(actor) = expect["actor"].as_object() {
                for (k, v) in actor {
                    assert_eq!(&doc["actor"][k], v, "{name}: actor.{k}: {doc}");
                }
            }
        } else {
            let want = expect["status"].as_u64().unwrap_or(401) as u16;
            assert_eq!(status, want, "{name}: {doc}");
        }
    }
    server.finish();
}

/// Wave 4c §5.8: `nils login --desk` keeps a token of one day in the
/// configuration directory, every `--server` verb reads it when `--token`
/// and `NILS_TOKEN` are absent, and `nils logout` forgets it.
#[test]
fn login_keeps_a_token_the_server_verbs_read() {
    // a fake desk: one request, the command line's login
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let desk_port = listener.local_addr().unwrap().port();
    let minted = "a-cli-token-of-length-24";
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        // the headers, then the body the content length promises
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        let req = loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break String::from_utf8_lossy(&got).to_string();
            }
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if got.len() >= end + 4 + length {
                    break text;
                }
            }
        };
        assert!(req.starts_with("POST /desk/cli-login"), "{req}");
        assert!(req.contains("\"username\":\"bo\""), "{req}");
        assert!(
            req.contains("\"password\":\"another long password\""),
            "{req}"
        );
        let body = format!(
            r#"{{"token":"{minted}","expires_at":{},"issuer":"http://desk"}}"#,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 86_400
        );
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(reply.as_bytes()).unwrap();
    });
    let config = TempDir::new("login-config");
    let mut cmd = nils();
    cmd.args([
        "login",
        "--desk",
        &format!("http://127.0.0.1:{desk_port}"),
        "--username",
        "bo",
        "--password-stdin",
    ])
    .env("NILS_CONFIG_DIR", config.path())
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"another long password\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let kept = config.path().join("token.json");
    assert!(kept.is_file(), "the token is kept");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&kept).unwrap()).unwrap();
    assert_eq!(doc["token"], minted);
    assert_eq!(doc["kind"], "desk");

    // an engine that knows that token; a --server verb reads it unasked
    let home = registry();
    let server = Server::start(
        &home,
        1,
        &[
            "--auth",
            "token",
            "--token",
            &format!("{minted}=bo@lab:reader"),
        ],
        &[],
    );
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gate/fixtures/gold-a.ask.yml");
    let out = nils()
        .args([
            "ask",
            "validate",
            "--server",
            &format!("http://127.0.0.1:{}", server.port),
            "--file",
            fixture.to_str().unwrap(),
        ])
        .env("NILS_CONFIG_DIR", config.path())
        .env_remove("NILS_TOKEN")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    server.finish();

    // logout forgets it
    let out = nils()
        .args(["logout"])
        .env("NILS_CONFIG_DIR", config.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!kept.exists());
}

/// Wave 5 §12.4 and §12.8 (slice A5): adopting an overlay names the stacks
/// that move and the handles that stop reproducing; the adoption
/// invalidates them, the invalidation is a row and an event, and a stale
/// handle is refused at the server with the reason in the desk's order.
#[test]
fn an_adoption_names_the_stacks_that_move_and_the_handles_that_stop_reproducing() {
    let home = knob_registry();
    let server = Server::start(
        &home,
        14,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "a-reviewer-token-of-len=rev@lab:reviewer",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--token",
            "an-admin-token-of-length=adm@lab:admin",
        ],
        &[],
    );
    let reader = Some("a-reader-token-of-length");
    let reviewer = Some("a-reviewer-token-of-len");
    let ops = Some("an-operator-token-of-len");
    let admin = Some("an-admin-token-of-length");

    // 1: a handle over every stack, its keys named
    let ask = r#"{"document": {"ast_version": 1, "sets": {"all": {"grain": "stack"}}, "out": {"set": "all", "level": "record"}}, "name": "every stack"}"#;
    let (status, ran) = server.request("POST", "/api/ask/run", Some(ask), reader);
    assert_eq!(status, 200, "{ran}");
    let handle = ran["handle"].as_i64().unwrap();
    assert_eq!(ran["row_count"], 2, "{ran}");

    // 2-3: a kind the door does not serve, and an object it cannot find
    let (status, doc) = server.request("GET", "/api/depends/rule/1", None, reader);
    assert_eq!(status, 404, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap()
            .contains("overlay, pack, subject, stack"),
        "{doc}"
    );
    let (status, doc) = server.request("GET", "/api/depends/overlay/99", None, reader);
    assert_eq!(status, 404, "{doc}");

    // 4-5: the proposal, and its closure before anything moves
    let body = format!(
        r#"{{"name": "site words", "overlay": {SITE_OVERLAY}, "scope": "batch:1", "why": "the site's localizer word"}}"#
    );
    let (status, proposed) = server.request("POST", "/api/overlays", Some(&body), reviewer);
    assert_eq!(status, 201, "{proposed}");
    let id = proposed["overlay"]["id"].as_i64().unwrap();
    let (status, closure) =
        server.request("GET", &format!("/api/depends/overlay/{id}"), None, reader);
    assert_eq!(status, 200, "{closure}");
    assert_eq!(
        closure["stacks"]["count"], 1,
        "one stack carries the word: {closure}"
    );
    assert_eq!(
        closure["stacks"]["sample"].as_array().unwrap().len(),
        1,
        "{closure}"
    );
    let named: Vec<i64> = closure["handles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["handle"].as_i64().unwrap())
        .collect();
    assert_eq!(
        named,
        vec![handle],
        "the handle over the stacks stops reproducing: {closure}"
    );
    assert!(
        closure["handles"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("names stacks the overlay moves"),
        "{closure}"
    );
    assert_eq!(
        closure["releases"].as_array().unwrap().len(),
        0,
        "{closure}"
    );

    // 6: a subject's erasure closes over its stacks and the same handle
    let (status, subject) = server.request("GET", "/api/depends/subject/1", None, reader);
    assert_eq!(status, 200, "{subject}");
    assert!(
        subject["stacks"]["count"].as_i64().unwrap() >= 1,
        "{subject}"
    );
    assert_eq!(subject["handles"][0]["handle"], handle, "{subject}");

    // 7: the handle reproduces before the adoption
    let (status, before) =
        server.request("GET", &format!("/api/ask/handles/{handle}"), None, reader);
    assert_eq!(status, 200, "{before}");
    assert_eq!(before["stale"], false, "{before}");
    assert!(before["invalidated"].is_null(), "{before}");

    // 8: adoption records the closure and invalidates the handle
    let (status, adopted) = server.request("POST", &format!("/api/overlays/{id}/adopt"), None, ops);
    assert_eq!(status, 202, "{adopted}");
    assert_eq!(adopted["closure"]["stacks"], 1, "{adopted}");
    assert_eq!(
        adopted["invalidated"],
        serde_json::json!([handle]),
        "{adopted}"
    );

    // 9-10: the handle reads as invalidated, and its timeline says so
    let (status, after) =
        server.request("GET", &format!("/api/ask/handles/{handle}"), None, reader);
    assert_eq!(status, 200, "{after}");
    assert_eq!(after["stale"], true, "{after}");
    assert!(
        after["invalidated"]["reason"]
            .as_str()
            .unwrap()
            .contains("adopted"),
        "{after}"
    );
    assert_eq!(after["invalidated"]["kind"], "overlay", "{after}");
    let (status, timeline) = server.request(
        "GET",
        &format!("/api/timeline/handle/{handle}"),
        None,
        reader,
    );
    assert_eq!(status, 200, "{timeline}");
    assert!(
        timeline["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "invalidated"),
        "{timeline}"
    );

    // 11-12: the closure after the adoption names nothing twice; the audit row keeps the counts
    let (status, again) =
        server.request("GET", &format!("/api/depends/overlay/{id}"), None, reader);
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["handles"].as_array().unwrap().len(), 0, "{again}");
    let (status, audit) = server.request("GET", "/api/audit?action=overlay.adopt", None, admin);
    assert_eq!(status, 200, "{audit}");
    assert_eq!(
        audit["rows"][0]["details"]["closure"]["handles"], 1,
        "{audit}"
    );

    // 13-14: a stale answer is refused at the server: promote, and a page read for an export
    let (status, refused) = server.request(
        "POST",
        &format!("/api/ask/handles/{handle}/promote"),
        Some(r#"{"cohort": "x"}"#),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("a stale answer is not promoted"),
        "{refused}"
    );
    let (status, refused) = server.request(
        "GET",
        &format!("/api/ask/handles/{handle}/rows?purpose=export"),
        None,
        reader,
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("stale"),
        "{refused}"
    );
    server.finish();
}

/// Wave 5 §12.5 and §10.2: places as registry objects, and the rules at
/// the doors. A registry place without a backup is refused; a release to
/// a path outside an export place is refused at the door; a retired place
/// binds nothing.
#[test]
fn places_are_registry_objects_and_the_rules_hold_at_the_doors() {
    let home = registry();
    let export = TempDir::new("a4-export");
    let backup = TempDir::new("a4-backup");
    let elsewhere = TempDir::new("a4-elsewhere");
    let server = Server::start(
        &home,
        15,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
        &[],
    );
    let reader = Some("a-reader-token-of-length");
    let ops = Some("an-operator-token-of-len");
    // before any place is declared, nothing is enforced and the door says so
    let (status, caps) = server.request("GET", "/api/capabilities", None, reader);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["places"]["enforced"], false, "{caps}");
    assert!(
        caps["doors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == "GET /api/places"),
        "{caps}"
    );
    let (status, none) = server.request("GET", "/api/places", None, reader);
    assert_eq!(status, 200, "{none}");
    assert_eq!(none["count"], 0, "{none}");
    assert_eq!(none["enforced"], false, "{none}");
    // a reader may read, not declare
    let body = |name: &str, role: &str, path: &std::path::Path, guarantees: serde_json::Value| {
        serde_json::json!({"name": name, "role": role, "path": path.display().to_string(), "guarantees": guarantees}).to_string()
    };
    let (status, refused) = server.request(
        "POST",
        "/api/places",
        Some(&body(
            "vault",
            "backup",
            backup.path(),
            serde_json::json!({}),
        )),
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    // the registry role without a backup is refused, with the rule in the sentence
    let (status, refused) = server.request(
        "POST",
        "/api/places",
        Some(&body(
            "reg",
            "registry",
            home.path(),
            serde_json::json!({"protected": true}),
        )),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("without a backup"),
        "{refused}"
    );
    // the backup place first, then the registry place naming it
    let (status, vault) = server.request(
        "POST",
        "/api/places",
        Some(&body(
            "vault",
            "backup",
            backup.path(),
            serde_json::json!({"protected": true}),
        )),
        ops,
    );
    assert_eq!(status, 201, "{vault}");
    assert_eq!(vault["probed"]["directory"], true, "{vault}");
    assert_eq!(vault["probed"]["writable"], true, "{vault}");
    let (status, reg) = server.request(
        "POST",
        "/api/places",
        Some(&body(
            "reg",
            "registry",
            home.path(),
            serde_json::json!({"protected": true, "backup": "vault"}),
        )),
        ops,
    );
    assert_eq!(status, 201, "{reg}");
    // an unknown role, a taken name
    let (status, bad) = server.request(
        "POST",
        "/api/places",
        Some(&body("x", "attic", elsewhere.path(), serde_json::json!({}))),
        ops,
    );
    assert_eq!(status, 400, "{bad}");
    let (status, taken) = server.request(
        "POST",
        "/api/places",
        Some(&body(
            "vault",
            "backup",
            elsewhere.path(),
            serde_json::json!({}),
        )),
        ops,
    );
    assert_eq!(status, 409, "{taken}");
    // the export place; now the rules are in force
    let (status, out) = server.request(
        "POST",
        "/api/places",
        Some(&body(
            "out",
            "export",
            export.path(),
            serde_json::json!({"snapshots": true}),
        )),
        ops,
    );
    assert_eq!(status, 201, "{out}");
    let out_id = out["id"].as_i64().unwrap();
    let (status, listed) = server.request("GET", "/api/places?probe=1", None, reader);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed["count"], 3, "{listed}");
    assert_eq!(listed["enforced"], true, "{listed}");
    assert!(
        listed["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["role"] == "export"),
        "{listed}"
    );
    // a release to a path outside any export place is refused at the door, naming the rule
    let (status, refused) = server.request(
        "POST",
        "/api/releases",
        Some(&serde_json::json!({"name": "d", "out": elsewhere.path().join("tree").display().to_string()}).to_string()),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("export place"),
        "{refused}"
    );
    // and one under the export place is queued
    let (status, queued) = server.request(
        "POST",
        "/api/releases",
        Some(&serde_json::json!({"name": "d", "out": export.path().join("tree").display().to_string()}).to_string()),
        ops,
    );
    assert_eq!(status, 202, "{queued}");
    // a handover writes only to an exchange place, and none is declared
    let (status, refused) = server.request(
        "POST",
        "/api/handovers",
        Some(
            &serde_json::json!({"release": "d", "out": export.path().display().to_string()})
                .to_string(),
        ),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("exchange place"),
        "{refused}"
    );
    // retiring the export place takes the binding away
    let (status, retired) = server.request(
        "PUT",
        &format!("/api/places/{out_id}"),
        Some(r#"{"retired": true}"#),
        ops,
    );
    assert_eq!(status, 200, "{retired}");
    assert_eq!(retired["retired"], true, "{retired}");
    let (status, refused) = server.request(
        "POST",
        "/api/releases",
        Some(&serde_json::json!({"name": "d", "out": export.path().join("tree").display().to_string()}).to_string()),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    server.finish();
    // the acts are on the audit log
    let audited = run(
        &home,
        &["audit", "list", "--action", "place.add", "--json"],
        None,
    );
    assert!(audited.contains("\"place.add\""), "{audited}");
}
