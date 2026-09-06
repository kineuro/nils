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
        let first = lines.next().unwrap().unwrap();
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
    let server = Server::start(&home, 16, &[], &[]);

    // C26: the capabilities name the contracts, the pack, the epoch.
    let (status, caps) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["contracts"]["openapi"], "1", "{caps}");
    assert_eq!(caps["contracts"]["review_item"], "2", "{caps}");
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
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/openapi/v1/openapi.yaml"),
    )
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
        10,
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
