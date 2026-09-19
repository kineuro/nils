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
    let server = Server::start(&home, 18, &[], &[]);

    // C26: the capabilities name the contracts, the pack, the epoch.
    let (status, caps) = server.request("GET", "/api/capabilities", None, None);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["contracts"]["openapi"], "6", "{caps}");
    assert_eq!(caps["contracts"]["review_item"], "4", "{caps}");
    // Wave 4c §4.5: the engine's document is the `engine` part of the
    // deployment capabilities document, and carries what the suite requires.
    assert_eq!(caps["contracts"]["suite"], "2", "{caps}");
    assert_eq!(caps["contracts"]["mcp"], "2", "{caps}");
    let suite: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../contracts/suite/v2/capabilities.schema.json"),
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
    // the suite contract, version 2: off holds every grant and detail sensitive
    assert_eq!(caps["grants"].as_array().unwrap().len(), 24, "{caps}");
    assert_eq!(caps["detail"], "sensitive", "{caps}");
    assert_eq!(
        caps["roles"],
        serde_json::json!(["reader", "reviewer", "operator"]),
        "{caps}"
    );
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

    // Record 28: the engine serves the tag policy it owns, so that nothing
    // reading it has to keep a copy. A hundred elements in four categories,
    // never the times, which are a release's.
    assert!(doors.contains(&"GET /api/pseudonymize/tags"), "{doors:?}");
    let (status, policy) = server.request("GET", "/api/pseudonymize/tags", None, None);
    assert_eq!(status, 200, "{policy}");
    assert_eq!(policy["count"], 100, "{policy}");
    assert_eq!(
        policy["categories"],
        serde_json::json!([
            {"category": "patient", "count": 34},
            {"category": "trial", "count": 23},
            {"category": "provider", "count": 38},
            {"category": "institution", "count": 5},
        ]),
        "{policy}"
    );
    assert!(!policy.to_string().contains("times"), "{policy}");
    assert_eq!(policy["code"]["tag"], "0010,0020", "{policy}");
    assert_eq!(policy["code"]["fate"], "replaced", "{policy}");
    assert_eq!(policy["mandatory"][0]["tag"], "0008,0016", "{policy}");
    assert_eq!(policy["mandatory"][1]["tag"], "0008,0018", "{policy}");
    assert_eq!(
        policy["covariates"]["opt_out"], "keep_demographics",
        "{policy}"
    );

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

/// `NILS_TOKENS` holds the entries `--token` takes, and an entry's own list
/// of grants survives the commas between the entries.
#[test]
fn nils_tokens_keeps_an_entry_s_own_list() {
    let home = registry();
    let server = Server::start(
        &home,
        2,
        &["--auth", "token"],
        &[(
            "NILS_TOKENS",
            "a-listed-token-of-length=bo@lab-2:reader,kvasir:see, a-machine-token-of-length=cy@lab-2",
        )],
    );
    let (status, doc) = server.request(
        "GET",
        "/api/capabilities",
        None,
        Some("a-listed-token-of-length"),
    );
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["principal"], "bo@lab-2", "{doc}");
    assert_eq!(
        doc["grants"],
        serde_json::json!(["data:see", "kvasir:see", "query:see", "query:work"]),
        "{doc}"
    );
    assert_eq!(doc["detail"], "plain", "{doc}");
    let (status, doc) = server.request(
        "GET",
        "/api/capabilities",
        None,
        Some("a-machine-token-of-length"),
    );
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["principal"], "cy@lab-2", "{doc}");
    assert_eq!(doc["grants"].as_array().unwrap().len(), 24, "{doc}");
    assert_eq!(doc["detail"], "sensitive", "{doc}");
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
/// the issuer's keys and audience, maps groups to grants, makes the subject
/// the audit principal, and keeps no user table beyond a cache of claims.
#[test]
fn under_oidc_the_subject_is_the_principal_and_groups_give_grants() {
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
        12,
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

    // Wave 4b §12.4: a token whose groups map to nothing holds no grant and
    // is refused at every door, never defaulted to a reader.
    let unmapped = token("kit", &["guests"], "nils", now + 600, Some("test-2026"));
    let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&unmapped));
    assert_eq!(status, 403, "{doc}");
    assert!(doc["error"].as_str().unwrap().contains("no grant"), "{doc}");

    // A reader: the subject at the issuer's node, the reader's set only, and
    // a door that needs more says which grant with 403.
    let reader = token("anna", &["students"], "nils", now + 600, Some("test-2026"));
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&reader));
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["auth"], "oidc", "{caps}");
    assert_eq!(caps["principal"], "anna@id.example.org", "{caps}");
    assert_eq!(caps["roles"], serde_json::json!(["reader"]), "{caps}");
    assert_eq!(
        caps["grants"],
        serde_json::json!(["data:see", "query:see", "query:work"]),
        "{caps}"
    );
    assert_eq!(caps["detail"], "plain", "{caps}");
    let (status, doc) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["fingerprint"]}"#),
        Some(&reader),
    );
    assert_eq!(status, 403, "{doc}");
    assert!(
        doc["error"].as_str().unwrap().contains("pipelines:work"),
        "{doc}"
    );
    let (status, doc) = server.request("GET", "/api/review?status=open", None, Some(&reader));
    assert_eq!(status, 403, "{doc}");
    assert!(
        doc["error"].as_str().unwrap().contains("review:see"),
        "{doc}"
    );

    // A reviewer reads what waits and decides, and the audit row carries
    // the subject.
    let reviewer = token(
        "bo",
        &["neuro-reviewers"],
        "nils",
        now + 600,
        Some("test-2026"),
    );
    let (status, listed) = server.request("GET", "/api/review?status=open", None, Some(&reviewer));
    assert_eq!(status, 200, "{listed}");
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
    // An operator reads the audit? No: audit:see is the admin's; an
    // operator queues work. The operator's set holds the reader's and the
    // reviewer's, so the operator reads and decides too.
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
        14,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--token",
            "an-admin-token-of-length=adm@lab:admin",
            "--ingest-root",
            &root_flag,
            "--backup-dir",
            backups.path().to_str().unwrap(),
        ],
        &[],
    );
    let reader = Some("a-reader-token-of-length");
    let ops = Some("an-operator-token-of-len");
    let admin = Some("an-admin-token-of-length");
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
    // record 26: every axis with its values, their words and how else they
    // are reached, the flags count, the thresholds and the amendable lists
    let technique = pack["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == "technique")
        .unwrap_or_else(|| panic!("{pack}"));
    assert_eq!(
        technique["count"],
        technique["values"].as_array().unwrap().len()
    );
    let tse = technique["values"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "TSE")
        .unwrap_or_else(|| panic!("{technique}"));
    assert_eq!(tse["label"], "TSE", "{tse}");
    assert_eq!(tse["family"], "SE", "{tse}");
    assert_eq!(tse["tried"], true, "{tse}");
    assert_eq!(tse["list"], "technique.TSE", "{tse}");
    assert_eq!(tse["keywords"][0], "tse", "{tse}");
    assert!(tse["site"].is_null(), "no overlay adopted: {tse}");
    let mprage = technique["values"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "MPRAGE")
        .unwrap();
    assert_eq!(mprage["detection"]["exclusive"], "is_mprage", "{mprage}");
    assert!(pack["flags"].as_i64().unwrap() > 100, "{pack}");
    assert_eq!(pack["review"]["low_confidence"]["default"], 0.7, "{pack}");
    assert_eq!(
        pack["review"]["low_confidence"]["per_axis"]["body_part"], 0.65,
        "{pack}"
    );
    assert_eq!(
        pack["review"]["missing"],
        serde_json::json!(["technique"]),
        "{pack}"
    );
    let lists = pack["lists"].as_array().unwrap();
    assert!(lists.len() > 100, "{}", lists.len());
    assert!(
        lists.iter().any(|l| l == "base.T1w"),
        "a longhand rule's words"
    );
    assert_eq!(packs_doc["packs"][0]["lists"], lists.len(), "{packs_doc}");
    assert_eq!(packs_doc["packs"][0]["contract"], 4, "{packs_doc}");
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
    // a backup is the database page's work, which an operator does not hold
    let (status, doc) =
        server.request("POST", "/api/jobs", Some(r#"{"command": ["backup"]}"#), ops);
    assert_eq!(status, 403, "{doc}");
    let (status, queued) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["backup"]}"#),
        admin,
    );
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

/// Wave 5 §10.3: the archives are listed, kept to a number and rehearsed; the
/// schedule and the registry's calendar are an admin's, and the schedule is
/// read in the registry's timezone.
#[test]
fn backups_are_listed_kept_and_rehearsed_and_the_schedule_and_the_calendar_are_an_admins() {
    let home = registry();
    let backups = TempDir::new("w5-backups");
    let dir = backups.path().to_str().unwrap();
    let flags = [
        "--auth",
        "token",
        "--token",
        "a-reader-token-of-length=reader@lab:reader",
        "--token",
        "an-admin-token-of-length=admin@lab:operator,admin",
        "--backup-dir",
        dir,
    ];
    let reader = Some("a-reader-token-of-length");
    let admin = Some("an-admin-token-of-length");
    let server = Server::start(&home, 12, &flags, &[]);
    let (status, _) = server.request("GET", "/api/backups", None, reader);
    assert_eq!(status, 403);
    let (status, doc) = server.request("GET", "/api/backups", None, admin);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["count"], 0, "{doc}");
    assert_eq!(doc["schedule"]["every"], "off", "{doc}");
    assert!(doc["schedule"]["next"].is_null(), "{doc}");
    let (status, _) = server.request(
        "PUT",
        "/api/backups/schedule",
        Some(r#"{"every": "day"}"#),
        reader,
    );
    assert_eq!(status, 403);
    let (status, refused) = server.request(
        "PUT",
        "/api/backups/schedule",
        Some(r#"{"every": "hourly"}"#),
        admin,
    );
    assert_eq!(status, 400, "{refused}");
    let (status, schedule) = server.request(
        "PUT",
        "/api/backups/schedule",
        Some(r#"{"every": "day", "at": "02:00", "keep": 2}"#),
        admin,
    );
    assert_eq!(status, 200, "{schedule}");
    assert_eq!(schedule["keep"], 2, "{schedule}");
    assert!(
        schedule["next_local"]
            .as_str()
            .is_some_and(|n| n.ends_with("T02:00")),
        "{schedule}"
    );
    let (status, refused) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["backup", "--everything"]}"#),
        admin,
    );
    assert_eq!(status, 400, "{refused}");
    let (status, queued) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["backup", "--keep", "2", "--rehearse"]}"#),
        admin,
    );
    assert_eq!(status, 202, "{queued}");
    let job = queued["job"].as_i64().unwrap();
    let (status, calendar) = server.request("GET", "/api/settings", None, reader);
    assert_eq!(status, 200, "{calendar}");
    assert_eq!(calendar["timezone"], "UTC", "{calendar}");
    assert!(
        calendar["timezones"]
            .as_array()
            .unwrap()
            .iter()
            .any(|z| z == "Europe/Stockholm"),
        "{calendar}"
    );
    let epoch = calendar["epoch"].as_i64().unwrap();
    let (status, _) = server.request(
        "PUT",
        "/api/settings",
        Some(r#"{"timezone": "Europe/Stockholm"}"#),
        reader,
    );
    assert_eq!(status, 403);
    let (status, refused) = server.request(
        "PUT",
        "/api/settings",
        Some(r#"{"timezone": "Mars/Olympus_Mons"}"#),
        admin,
    );
    assert_eq!(status, 400, "{refused}");
    let (status, calendar) = server.request(
        "PUT",
        "/api/settings",
        Some(r#"{"timezone": "Europe/Stockholm", "week_start": "sunday"}"#),
        admin,
    );
    assert_eq!(status, 200, "{calendar}");
    assert_eq!(calendar["timezone"], "Europe/Stockholm", "{calendar}");
    assert_eq!(calendar["week_start"], "sunday", "{calendar}");
    assert_eq!(calendar["epoch"], epoch + 1, "{calendar}");
    let (status, doc) = server.request("GET", "/api/backups", None, admin);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["schedule"]["timezone"], "Europe/Stockholm", "{doc}");
    assert!(
        doc["schedule"]["next_local"]
            .as_str()
            .is_some_and(|n| n.ends_with("T02:00")),
        "{doc}"
    );
    server.finish();

    // the worker runs the queued backup; two more by hand keep the newest two
    run(&home, &["jobs", "work", "--once"], None);
    for _ in 0..2 {
        // an archive is named to the second
        std::thread::sleep(std::time::Duration::from_millis(1100));
        run(
            &home,
            &[
                "backup",
                "--dir",
                dir,
                "--keep",
                "2",
                "--rehearse",
                "--json",
            ],
            None,
        );
    }
    let archives: Vec<_> = std::fs::read_dir(backups.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(archives.len(), 2, "{archives:?}");
    for archive in &archives {
        let checked: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(archive.join("checked.json")).unwrap())
                .unwrap();
        assert_eq!(checked["ok"], true, "{checked}");
        assert_eq!(checked["rehearsed"], true, "{checked}");
    }
    let rehearsed = run(
        &home,
        &[
            "verify",
            archives[0].to_str().unwrap(),
            "--rehearse",
            "--json",
        ],
        None,
    );
    let rehearsed: serde_json::Value = serde_json::from_str(&rehearsed).unwrap();
    let opened = rehearsed["opened"].as_array().unwrap();
    assert_eq!(opened.len(), 2, "{rehearsed}");
    assert!(opened.iter().all(|o| o["state"] == "opens"), "{rehearsed}");
    for action in ["backup.schedule", "settings.set"] {
        let audited = run(
            &home,
            &["audit", "list", "--action", action, "--json"],
            None,
        );
        assert!(audited.contains(action), "{audited}");
    }
    let out = nils()
        .arg("--registry")
        .arg(home.path())
        .args(["settings", "set", "timezone", "Mars/Olympus_Mons"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let server = Server::start(&home, 2, &flags, &[]);
    let (status, doc) = server.request("GET", "/api/backups", None, admin);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["count"], 2, "{doc}");
    let newest = &doc["archives"][0];
    assert_eq!(newest["ours"], true, "{doc}");
    assert_eq!(newest["checked"]["rehearsed"], true, "{doc}");
    assert!(
        newest["bytes"].as_u64().is_some_and(|b| b > 0) && newest["seconds"].is_u64(),
        "{doc}"
    );
    let (status, shown) = server.request("GET", &format!("/api/jobs/{job}"), None, admin);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["result"]["checked"]["ok"], true, "{shown}");
    server.finish();
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
    // record 26: the same by value, and the origins for the scope chips
    let by_value = signals["by_value"]["technique"]
        .as_object()
        .unwrap_or_else(|| panic!("{signals}"));
    assert_eq!(
        by_value
            .values()
            .map(|v| v["decided"].as_i64().unwrap())
            .sum::<i64>(),
        2,
        "{signals}"
    );
    assert!(
        signals["origins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["name"] == "SYNTHETIC" && o["kind"] == "manufacturer" && o["stacks"] == 2),
        "{signals}"
    );

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
    // the overlays are the Review page's, which a reader does not see
    let (status, doc) = server.request("GET", "/api/overlays", None, reviewer);
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
    let (status, listed) = server.request("GET", &format!("/api/overlays/{id}"), None, reviewer);
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
    // record 26 section 11: the evidence sits under each axis
    let cites = |doc: &serde_json::Value| {
        doc["axes"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|a| a["evidence"].as_array().into_iter().flatten())
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

/// Wave 4c §5.3: the trust list vectors of `contracts/suite/v2` run against
/// the engine. Each case is minted with the key it names and presented; what
/// happened is compared with what the vector expects.
#[test]
fn the_trust_list_vectors_hold() {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/suite/v2/vectors");
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

#[test]
fn a_job_queued_at_the_door_runs_when_serve_runs_its_queue() {
    let home = registry();
    // every request counts toward the limit, and the ones not spent polling
    // are spent at the end so the server stops
    const LIMIT: usize = 90;
    let server = Server::start(&home, LIMIT, &["--worker"], &[]);
    let (status, queued) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["fingerprint", "--name", "by-the-worker"], "name": "worker"}"#),
        None,
    );
    assert_eq!(status, 202, "{queued}");
    let job = queued["job"].as_i64().unwrap();
    let mut used = 1;
    let mut shown = serde_json::Value::Null;
    while used < LIMIT - 1 {
        let (status, now) = server.request("GET", &format!("/api/jobs/{job}"), None, None);
        used += 1;
        assert_eq!(status, 200, "{now}");
        shown = now;
        if matches!(shown["state"].as_str(), Some("done" | "failed")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert_eq!(
        shown["state"], "done",
        "no worker was started by hand: {shown}"
    );
    while used < LIMIT {
        let _ = server.request("GET", "/api/capabilities", None, None);
        used += 1;
    }
    server.finish();
}

/// The Data page's door: a source place lists its digests, what they added
/// and how it is handled, and a handling that is not one is refused.
#[test]
fn a_source_lists_its_digests_what_they_added_and_how_it_is_handled() {
    let home = TempDir::new("sources-home");
    let dir = TempDir::new("sources-src");
    for (study, sop) in [("1.2.3.A", "1.2.3.A.1.1"), ("1.2.3.B", "1.2.3.B.1.1")] {
        let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "P1"));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        dir.file(
            &format!("{study}/{sop}"),
            &synth::part10(&MetaFields::mr(sop), &e, true),
        );
    }
    let tree = dir.path().to_str().unwrap();
    run(&home, &["key", "add", "k"], Some("a serve test key\n"));
    run(&home, &["init", "--key", "k"], None);
    run(
        &home,
        &["place", "add", "incoming", tree, "--role", "source"],
        None,
    );
    // record 26: the map makes the subject and the digest meets it, which
    // is what a dataset with a map looks like; the door counts the subjects
    // whose files the tree holds, not the ones a digest made
    let map = home.file("map.csv", b"PatientID,subject_code\nP1,mapped-0001\n");
    run(
        &home,
        &[
            "linkage",
            "import",
            map.to_str().unwrap(),
            "--id-column",
            "PatientID",
            "--code-column",
            "subject_code",
        ],
        None,
    );
    run(
        &home,
        &["digest", "--name", "first", "--no-private", tree],
        None,
    );
    run(&home, &["fingerprint"], None);
    run(
        &home,
        &[
            "classify",
            "--review-below",
            "1.0",
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
        None,
    );
    let server = Server::start(
        &home,
        6,
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

    let (status, doc) = server.request("GET", "/api/sources", None, reader);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["count"], 1, "{doc}");
    let source = &doc["sources"][0];
    assert_eq!(source["name"], "incoming", "{doc}");
    // record 26: a source declared with nothing said is a dataset reading
    // its folder itself, de-identified, and the handling mirrors it
    assert_eq!(source["handling"]["arrives"], "deidentified", "{doc}");
    assert_eq!(source["handling_declared"], false, "{doc}");
    assert_eq!(source["arrives"], "deidentified", "{doc}");
    assert_eq!(
        source["trees"]["originals"],
        serde_json::Value::Null,
        "{doc}"
    );
    assert_eq!(source["trees"]["anon"]["path"], tree, "{doc}");
    assert_eq!(
        source["held"],
        serde_json::json!({"files": 0, "identifiers": 0}),
        "{doc}"
    );
    assert_eq!(source["unmapped"], "code", "{doc}");
    assert_eq!(source["originals_kept"], "kept", "{doc}");
    assert_eq!(source["dataset"]["arrives"], "deidentified", "{doc}");
    assert_eq!(source["digests"]["count"], 1, "{doc}");
    assert_eq!(source["digests"]["last"]["name"], "first", "{doc}");
    let digest = &source["digests"]["recent"][0];
    assert_eq!(digest["files"]["seen"], 2, "{doc}");
    assert_eq!(digest["pseudonymised"], serde_json::Value::Null, "{doc}");
    assert_eq!(digest["stacks_added"], 2, "{doc}");
    assert_eq!(digest["classified"], 2, "{doc}");
    assert_eq!(source["totals"]["stacks"], 2, "{doc}");
    assert_eq!(source["totals"]["studies"], 2, "{doc}");
    assert_eq!(source["totals"]["subjects"], 1, "{doc}");
    let id = source["id"].as_i64().unwrap();
    let path = format!("/api/places/{id}");

    // a reader reads the sources and declares nothing
    let (status, refused) = server.request(
        "PUT",
        &path,
        Some(r#"{"handling": {"arrives": "deidentified"}}"#),
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    // dates that move cannot keep the original UIDs
    let (status, refused) = server.request(
        "PUT",
        &path,
        Some(r#"{"handling": {"on_release": {"dates": "shift", "uids": "preserve"}}}"#),
        ops,
    );
    assert_eq!(status, 400, "{refused}");
    let (status, place) = server.request(
        "PUT",
        &path,
        Some(r#"{"handling": {"arrives": "deidentified", "on_release": {"dates": "year"}}}"#),
        ops,
    );
    assert_eq!(status, 200, "{place}");
    assert_eq!(
        place["handling"],
        serde_json::json!({"arrives": "deidentified", "on_release": {"dates": "year", "uids": "remap", "deface": false}}),
        "{place}"
    );
    assert_eq!(place["handling_declared"], true, "{place}");
    let (_, doc) = server.request("GET", "/api/sources", None, reader);
    assert_eq!(
        doc["sources"][0]["handling"]["arrives"], "deidentified",
        "{doc}"
    );
    assert_eq!(doc["sources"][0]["arrives"], "deidentified", "{doc}");
    let (status, caps) = server.request("GET", "/api/capabilities", None, reader);
    assert_eq!(status, 200, "{caps}");
    assert!(
        caps["doors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == "GET /api/sources"),
        "{caps}"
    );
    assert!(
        caps["policy"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["door"] == "GET /api/sources"),
        "{caps}"
    );
    server.finish();
}

/// Record 26: a source place is a dataset. Declaring one looks at its
/// folder: an identified dataset's loose entries move into the originals and
/// an empty pseudonymised tree is made; a v0 cohort folder has `dcm-raw`
/// renamed; a de-identified folder moves into the tree only when asked, and
/// reads itself otherwise. The dataset fields need `data:work` beside
/// `places:work`, a folder that is another dataset's tree is refused,
/// `@name` is the pseudonymised tree and its originals are never digested,
/// and the sources and places doors show the dataset.
#[test]
fn a_dataset_is_declared_on_a_source_place_and_named_by_its_name() {
    let home = registry();
    let identified = TempDir::new("ds-identified");
    let v0 = TempDir::new("ds-v0");
    let plain = TempDir::new("ds-plain");
    let bare = TempDir::new("ds-bare");
    let dicom = |study: &str, sop: &str| {
        let e = synth::minimal_mr(study, &format!("{study}.1"), sop);
        synth::part10(&MetaFields::mr(sop), &e, true)
    };
    identified.file("sub-1/ses-1/a.dcm", &dicom("1.2.3.C", "1.2.3.C.1.1"));
    identified.file("notes.txt", b"n");
    v0.file(
        "derivatives/dcm-original/sub-1/a.dcm",
        &dicom("1.2.3.D", "1.2.3.D.1.1"),
    );
    v0.file(
        "derivatives/dcm-raw/sub-1/a.dcm",
        &dicom("1.2.3.D", "1.2.3.D.1.1"),
    );
    plain.file("sub-1/a.dcm", &dicom("1.2.3.E", "1.2.3.E.1.1"));
    const LIMIT: usize = 42;
    let used = std::cell::Cell::new(0usize);
    let server = Server::start(
        &home,
        LIMIT,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--token",
            "a-places-token-of-length=pl@lab:places:work,data:see",
            "--ingest-root",
            &format!("ds={}", identified.path().display()),
            "--ingest-root",
            &format!("old={}", v0.path().display()),
        ],
        &[],
    );
    let reader = Some("a-reader-token-of-length");
    let ops = Some("an-operator-token-of-len");
    let places_only = Some("a-places-token-of-length");
    let ask = |method: &str, path: &str, body: Option<&str>, token: Option<&str>| {
        used.set(used.get() + 1);
        server.request(method, path, body, token)
    };
    let ends = |v: &serde_json::Value, tail: &str| v.as_str().is_some_and(|s| s.ends_with(tail));

    // before anything is declared, a look says what the folder holds
    let (status, look) = ask("POST", "/api/ingest/look", Some(r#"{"at": "@old"}"#), ops);
    assert_eq!(status, 200, "{look}");
    assert_eq!(look["layout"]["v0"]["original_files"], 1, "{look}");
    assert_eq!(look["layout"]["v0"]["raw_files"], 1, "{look}");
    assert_eq!(look["layout"]["v0"]["renamed"], false, "{look}");
    let (_, look) = ask("POST", "/api/ingest/look", Some(r#"{"at": "@ds"}"#), ops);
    assert_eq!(look["layout"]["v0"], serde_json::Value::Null, "{look}");
    assert_eq!(look["layout"]["loose"], 2, "{look}");
    // record 26: a folder under no location, by its absolute path, which is
    // how the desk looks at what it is about to declare; the same look,
    // bounded the same way, and the answer names no location
    let (status, look) = ask(
        "POST",
        "/api/ingest/look",
        Some(&serde_json::json!({"path": plain.path().display().to_string()}).to_string()),
        ops,
    );
    assert_eq!(status, 200, "{look}");
    assert_eq!(look["at"], serde_json::Value::Null, "{look}");
    assert_eq!(look["root"], serde_json::Value::Null, "{look}");
    assert_eq!(look["layout"]["v0"], serde_json::Value::Null, "{look}");
    assert_eq!(look["layout"]["loose"], 1, "{look}");
    assert_eq!(look["folders"][0]["name"], "sub-1", "{look}");
    assert_eq!(look["folders"][0]["dicom"], 1, "{look}");
    // a path that is not absolute is refused in words
    let (status, refused) = ask(
        "POST",
        "/api/ingest/look",
        Some(r#"{"path": "sub-1"}"#),
        ops,
    );
    assert_eq!(status, 400, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains("absolute path"),
        "{refused}"
    );

    // the dataset fields need data:work beside places:work, and a source place
    let body = |name: &str, role: &str, path: &std::path::Path, more: serde_json::Value| {
        let mut doc =
            serde_json::json!({"name": name, "role": role, "path": path.display().to_string()});
        for (k, v) in more.as_object().into_iter().flatten() {
            doc[k] = v.clone();
        }
        doc.to_string()
    };
    let (status, refused) = ask(
        "POST",
        "/api/places",
        Some(&body(
            "ds",
            "source",
            identified.path(),
            serde_json::json!({"arrives": "identified"}),
        )),
        places_only,
    );
    assert_eq!(status, 403, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("data:work"),
        "{refused}"
    );
    let (status, refused) = ask(
        "POST",
        "/api/places",
        Some(&body(
            "out",
            "export",
            bare.path(),
            serde_json::json!({"arrives": "coded"}),
        )),
        ops,
    );
    assert_eq!(status, 400, "{refused}");
    assert!(identified.path().join("sub-1").is_dir());

    // an identified dataset: the loose entries move into the originals
    let (status, ds) = ask(
        "POST",
        "/api/places",
        Some(&body(
            "ds",
            "source",
            identified.path(),
            serde_json::json!({
                "arrives": "identified",
                "identity": {"id_type": "study-id", "from": [{"field": "PatientID"}]},
                "cohort": "study-a",
                "tags": {"remove": ["0010,1010"]},
            }),
        )),
        ops,
    );
    assert_eq!(status, 201, "{ds}");
    let ds_id = ds["id"].as_i64().unwrap();
    assert_eq!(ds["dataset"]["arrives"], "identified", "{ds}");
    assert_eq!(ds["handling"]["arrives"], "identified", "{ds}");
    assert_eq!(ds["dataset"]["unmapped"], "hold", "{ds}");
    assert_eq!(ds["dataset"]["cohort"], "study-a", "{ds}");
    assert_eq!(ds["dataset"]["identity"]["id_type"], "study-id", "{ds}");
    assert!(
        ends(
            &ds["dataset"]["trees"]["originals"]["path"],
            "derivatives/dcm-original"
        ),
        "{ds}"
    );
    assert!(
        ends(
            &ds["dataset"]["trees"]["anon"]["path"],
            "derivatives/dcm-anon"
        ),
        "{ds}"
    );
    assert_eq!(ds["dataset"]["trees"]["originals"]["files"], 2, "{ds}");
    assert_eq!(ds["dataset"]["trees"]["anon"]["files"], 0, "{ds}");
    assert_eq!(ds["layout"]["v0"], serde_json::Value::Null, "{ds}");
    assert_eq!(ds["layout"]["loose"], 0, "{ds}");
    assert!(
        identified
            .path()
            .join("derivatives/dcm-original/sub-1/ses-1/a.dcm")
            .is_file()
    );
    assert!(
        identified
            .path()
            .join("derivatives/dcm-original/notes.txt")
            .is_file()
    );
    assert!(identified.path().join("derivatives/dcm-anon").is_dir());
    assert!(!identified.path().join("sub-1").exists());

    // a v0 cohort folder: dcm-raw is renamed, nothing else touched
    let (status, old) = ask(
        "POST",
        "/api/places",
        Some(&body("old", "source", v0.path(), serde_json::json!({}))),
        ops,
    );
    assert_eq!(status, 201, "{old}");
    assert_eq!(old["dataset"]["arrives"], "deidentified", "{old}");
    assert_eq!(
        old["layout"]["v0"],
        serde_json::json!({"original_files": 1, "raw_files": 1, "partial": false, "renamed": true}),
        "{old}"
    );
    assert!(
        ends(
            &old["dataset"]["trees"]["originals"]["path"],
            "derivatives/dcm-original"
        ),
        "{old}"
    );
    assert!(
        ends(
            &old["dataset"]["trees"]["anon"]["path"],
            "derivatives/dcm-anon"
        ),
        "{old}"
    );
    assert!(!v0.path().join("derivatives/dcm-raw").exists());
    assert!(v0.path().join("derivatives/dcm-anon/sub-1/a.dcm").is_file());

    // a de-identified folder moves into the tree when asked; a bare one reads itself
    let (status, moved) = ask(
        "POST",
        "/api/places",
        Some(&body(
            "plain",
            "source",
            plain.path(),
            serde_json::json!({"arrives": "coded", "move_into_anon": true}),
        )),
        ops,
    );
    assert_eq!(status, 201, "{moved}");
    assert_eq!(
        moved["dataset"]["trees"]["originals"],
        serde_json::Value::Null,
        "{moved}"
    );
    assert!(
        ends(
            &moved["dataset"]["trees"]["anon"]["path"],
            "derivatives/dcm-anon"
        ),
        "{moved}"
    );
    assert!(
        plain
            .path()
            .join("derivatives/dcm-anon/sub-1/a.dcm")
            .is_file()
    );
    let (status, bare_place) = ask(
        "POST",
        "/api/places",
        Some(&body("bare", "source", bare.path(), serde_json::json!({}))),
        ops,
    );
    assert_eq!(status, 201, "{bare_place}");
    assert_eq!(
        bare_place["dataset"]["trees"]["anon"]["path"],
        bare.path().display().to_string(),
        "{bare_place}"
    );
    assert_eq!(
        bare_place["dataset"]["trees"]["originals"],
        serde_json::Value::Null,
        "{bare_place}"
    );

    // a folder that is another dataset's tree is refused
    let (status, refused) = ask(
        "POST",
        "/api/places",
        Some(&body(
            "inside",
            "source",
            &identified.path().join("derivatives/dcm-original"),
            serde_json::json!({}),
        )),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("originals of the dataset ds"),
        "{refused}"
    );
    let (status, refused) = ask(
        "POST",
        "/api/places",
        Some(&body(
            "deeper",
            "source",
            &v0.path().join("derivatives/dcm-anon/sub-1"),
            serde_json::json!({}),
        )),
        ops,
    );
    assert_eq!(status, 409, "{refused}");

    // @name is the pseudonymised tree, and the originals are never digested
    let (status, queued) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["digest", "@ds"]}"#),
        ops,
    );
    assert_eq!(status, 202, "{queued}");
    assert!(
        ends(&queued["command"][1], "derivatives/dcm-anon"),
        "{queued}"
    );
    let (status, refused) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["digest", "@ds/originals"]}"#),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("pseudonymiser"),
        "{refused}"
    );
    let (status, refused) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["digest", "@old/originals/sub-1"]}"#),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    // the picker reads the trees the same way
    let (status, roots) = ask("POST", "/api/ingest/folders", Some("{}"), ops);
    assert_eq!(status, 200, "{roots}");
    let ds_root = roots["roots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "ds")
        .unwrap();
    assert!(ends(&ds_root["path"], "derivatives/dcm-anon"), "{roots}");
    assert!(
        ends(&ds_root["originals"], "derivatives/dcm-original"),
        "{roots}"
    );
    assert_eq!(ds_root["place"]["name"], "ds", "{roots}");
    let (status, look) = ask(
        "POST",
        "/api/ingest/look",
        Some(r#"{"at": "@ds/originals"}"#),
        ops,
    );
    assert_eq!(status, 200, "{look}");
    assert!(ends(&look["path"], "derivatives/dcm-original"), "{look}");
    assert_eq!(look["here"]["files"]["count"], 1, "{look}");
    let (_, look) = ask("POST", "/api/ingest/look", Some(r#"{"at": "@ds"}"#), ops);
    assert!(ends(&look["path"], "derivatives/dcm-anon"), "{look}");

    // the sources door shows the dataset, with the counts the probe kept
    let (status, doc) = ask("GET", "/api/sources?probe=1", None, reader);
    assert_eq!(status, 200, "{doc}");
    let source = doc["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "ds")
        .unwrap();
    assert_eq!(source["arrives"], "identified", "{doc}");
    assert_eq!(source["trees"]["originals"]["files"], 2, "{doc}");
    assert_eq!(source["trees"]["anon"]["files"], 0, "{doc}");
    assert_eq!(
        source["trees"]["anon"]["last_written"],
        serde_json::Value::Null,
        "{doc}"
    );
    assert_eq!(source["identity"]["from"][0]["field"], "PatientID", "{doc}");
    assert_eq!(source["unmapped"], "hold", "{doc}");
    assert_eq!(source["cohort"], "study-a", "{doc}");
    assert_eq!(
        source["tags"],
        serde_json::json!({"keep_demographics": true, "remove": ["0010,1010"], "keep": []}),
        "{doc}"
    );
    assert_eq!(source["originals_kept"], "kept", "{doc}");
    assert_eq!(
        source["held"],
        serde_json::json!({"files": 0, "identifiers": 0}),
        "{doc}"
    );

    // changing the dataset needs data:work too; the rest of a place does not
    let path = format!("/api/places/{ds_id}");
    let (status, refused) = ask("PUT", &path, Some(r#"{"cohort": null}"#), places_only);
    assert_eq!(status, 403, "{refused}");
    let (status, kept) = ask(
        "PUT",
        &path,
        Some(r#"{"guarantees": {"snapshots": true}}"#),
        places_only,
    );
    assert_eq!(status, 200, "{kept}");
    assert_eq!(kept["dataset"]["cohort"], "study-a", "{kept}");
    let (status, changed) = ask(
        "PUT",
        &path,
        Some(r#"{"cohort": null, "unmapped": "code", "tags": {"keep_demographics": false}}"#),
        ops,
    );
    assert_eq!(status, 200, "{changed}");
    assert_eq!(
        changed["dataset"]["cohort"],
        serde_json::Value::Null,
        "{changed}"
    );
    assert_eq!(changed["dataset"]["unmapped"], "code", "{changed}");
    assert_eq!(changed["dataset"]["originals_kept"], "kept", "{changed}");
    // lab 26c, finding 4: what became of the originals is the act's to
    // write, and this is the door the desk's Change form sends to. A
    // declaration naming it is refused, in words naming the act.
    let (status, refused) = ask("PUT", &path, Some(r#"{"originals_kept": "purged"}"#), ops);
    assert_eq!(status, 400, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("nils place originals"),
        "{refused}"
    );
    assert_eq!(
        changed["dataset"]["tags"]["keep_demographics"], false,
        "{changed}"
    );
    assert_eq!(
        changed["dataset"]["tags"]["remove"],
        serde_json::json!(["0010,1010"]),
        "{changed}"
    );
    assert_eq!(
        changed["dataset"]["identity"]["id_type"], "study-id",
        "{changed}"
    );
    let (status, refused) = ask("PUT", &path, Some(r#"{"unmapped": "ask"}"#), ops);
    assert_eq!(status, 400, "{refused}");

    // the places door shows the same, and the capabilities say what the fields need
    let (status, listed) = ask("GET", "/api/places", None, reader);
    assert_eq!(status, 200, "{listed}");
    let shown = listed["places"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "ds")
        .unwrap();
    assert_eq!(shown["dataset"]["arrives"], "identified", "{listed}");
    assert!(
        ends(
            &shown["dataset"]["trees"]["anon"]["path"],
            "derivatives/dcm-anon"
        ),
        "{listed}"
    );
    let (status, caps) = ask("GET", "/api/capabilities", None, reader);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["places"]["dataset"]["grant"], "data:work", "{caps}");
    assert_eq!(
        caps["places"]["dataset"]["trees"]["anon"], "derivatives/dcm-anon",
        "{caps}"
    );
    let row = caps["policy"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["door"] == "POST /api/places")
        .unwrap();
    assert_eq!(row["grant"], "places:work", "{row}");
    assert_eq!(row["dataset"], "data:work", "{row}");
    while used.get() < LIMIT {
        ask("GET", "/api/capabilities", None, reader);
    }
    server.finish();
}

/// Lab 26c, finding 1 at the door: a dataset one of whose originals changed
/// after its copy was written. GET says a purge is not ready and counts the
/// file as changed, POST refuses in the same sentence rather than queueing a
/// job that fails later, and every original is still on disk afterwards.
#[test]
fn the_originals_door_refuses_a_purge_when_an_original_changed_after_its_copy() {
    let home = TempDir::new("originals-door-home");
    let dir = TempDir::new("originals-door-ds");
    let patient = "199001011234";
    for instance in 1..=2u32 {
        let study = format!("1.2.826.0.1.3680043.8.498.{patient}.1");
        let series = format!("{study}.1");
        let sop = format!("{series}.{instance}");
        let mut e = synth::minimal_mr(&study, &series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::PATIENT_NAME, VR::PN, "Doe^Jane"));
        e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
        e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
        e.push(synth::text(
            tags::INSTANCE_NUMBER,
            VR::IS,
            &instance.to_string(),
        ));
        e.push(synth::bytes(
            tags::PIXEL_DATA,
            VR::OW,
            (0..4000u32).map(|i| (i % 251) as u8).collect(),
        ));
        dir.file(
            &format!("sub-0/IM_{instance:04}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    run(&home, &["key", "add", "k"], Some("an originals door key\n"));
    run(&home, &["init", "--key", "k"], None);
    run(
        &home,
        &[
            "place",
            "add",
            "ds",
            dir.path().to_str().unwrap(),
            "--role",
            "source",
            "--arrives",
            "identified",
            "--unmapped",
            "code",
        ],
        None,
    );
    run(&home, &["pseudonymize", "@ds"], None);
    let places: serde_json::Value =
        serde_json::from_str(&run(&home, &["place", "list", "--json"], None)).unwrap();
    let id = places[0]["id"].as_i64().unwrap();

    let server = Server::start(
        &home,
        3,
        &[
            "--auth",
            "token",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
        &[],
    );
    let ops = Some("an-operator-token-of-len");
    let door = format!("/api/places/{id}/originals");

    let (status, ready) = server.request("GET", &door, None, ops);
    assert_eq!(status, 200, "{ready}");
    assert_eq!(ready["files"], 2, "{ready}");
    assert_eq!(ready["verified"], 2, "{ready}");
    assert_eq!(ready["ready"], true, "{ready}");

    // one original changed in place, to other bytes of exactly its length,
    // its modification time moving with them
    let changed = dir.path().join("derivatives/dcm-original/sub-0/IM_0001");
    let mut bytes = std::fs::read(&changed).unwrap();
    let n = bytes.len();
    for b in &mut bytes[n - 64..] {
        *b ^= 0xFF;
    }
    let moved = std::fs::metadata(&changed).unwrap().modified().unwrap()
        + std::time::Duration::from_secs(1);
    std::fs::write(&changed, &bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&changed)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(moved))
        .unwrap();

    let (status, looked) = server.request("GET", &door, None, ops);
    assert_eq!(status, 200, "{looked}");
    assert_eq!(looked["ready"], false, "{looked}");
    assert_eq!(looked["verified"], 1, "{looked}");
    assert_eq!(looked["changed"], 1, "{looked}");
    assert_eq!(looked["copy_unverified"], 0, "{looked}");
    assert_eq!(looked["no_copy"], 0, "{looked}");
    let why = looked["why"].as_str().unwrap().to_string();
    assert!(why.contains("changed after being copied"), "{why}");
    assert!(why.contains("nils pseudonymize @ds"), "{why}");

    let (status, refused) = server.request(
        "POST",
        &door,
        Some(r#"{"do": "purge", "why": "the study is over"}"#),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    assert_eq!(
        refused["error"].as_str().unwrap(),
        why,
        "the door refuses in the sentence it answered with"
    );
    assert!(changed.is_file(), "a refused purge deletes nothing");
    assert!(
        dir.path()
            .join("derivatives/dcm-original/sub-0/IM_0002")
            .is_file()
    );
    server.finish();
}

/// The desk's picker: the ingest roots with the places that hold them, a page
/// of the folders inside a folder, filtered and paged after a name, and a
/// look inside a few of them, for an operator and never outside the roots.
#[cfg(unix)]
#[test]
fn an_operator_pages_through_the_ingest_roots_and_looks_inside_them_never_outside() {
    use serde_json::json;
    use std::os::unix::fs::symlink;
    let home = TempDir::new("browse-home");
    let scans = TempDir::new("browse-scans");
    let archive = TempDir::new("browse-archive");
    let elsewhere = TempDir::new("browse-elsewhere");
    for name in ["alpha", "Beta", "gamma"] {
        std::fs::create_dir_all(scans.path().join(name)).unwrap();
    }
    scans.file("notes/readme.txt", b"not dicom");
    scans.file("list.csv", b"a,b");
    for (i, (series, sop)) in [
        ("anat", "1.2.3.1.1.1"),
        ("anat", "1.2.3.1.1.2"),
        ("func", "1.2.3.2.1.1"),
    ]
    .into_iter()
    .enumerate()
    {
        let mr = synth::minimal_mr("1.2.3", &format!("1.2.3.{}", i + 1), sop);
        scans.file(
            &format!("sub-001/ses-1/{series}/IM_{i}"),
            &synth::part10(&MetaFields::mr(sop), &mr, true),
        );
    }
    // a link that leaves the root, and one that stays inside it: neither is listed
    symlink(elsewhere.path(), scans.path().join("outside")).unwrap();
    symlink(scans.path().join("alpha"), scans.path().join("alias")).unwrap();
    // a folder of twenty thousand folders, and forty folders of one DICOM file each
    for i in 0..20_000 {
        std::fs::create_dir_all(archive.path().join("many").join(format!("f{i:05}"))).unwrap();
    }
    for i in 0..40 {
        let sop = format!("1.2.4.{i}.1");
        let mr = synth::minimal_mr("1.2.4", "1.2.4.1", &sop);
        archive.file(
            &format!("looks/l{i:02}/IM"),
            &synth::part10(&MetaFields::mr(&sop), &mr, true),
        );
    }
    run(&home, &["key", "add", "k"], Some("a serve test key\n"));
    run(&home, &["init", "--key", "k"], None);
    let scans_path = scans.path().to_str().unwrap();
    let looks_path = archive.path().join("looks");
    run(
        &home,
        &["place", "add", "incoming", scans_path, "--role", "source"],
        None,
    );
    run(
        &home,
        &[
            "place",
            "add",
            "looked",
            looks_path.to_str().unwrap(),
            "--role",
            "source",
        ],
        None,
    );
    const LIMIT: usize = 45;
    let scans_flag = format!("scans={scans_path}");
    let archive_flag = format!("archive={}", archive.path().display());
    let server = Server::start(
        &home,
        LIMIT,
        &[
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--ingest-root",
            &scans_flag,
            "--ingest-root",
            &archive_flag,
        ],
        &[],
    );
    let reader = Some("a-reader-token-of-length");
    let ops = Some("an-operator-token-of-len");
    // every request counts toward the limit, and the ones not spent are spent
    // at the end so the server stops
    let used = std::cell::Cell::new(0usize);
    let call = |door: &str, body: serde_json::Value, token: Option<&str>| {
        used.set(used.get() + 1);
        server.request("POST", door, Some(&body.to_string()), token)
    };
    let listed = |body: serde_json::Value| {
        let (status, doc) = call("/api/ingest/folders", body, ops);
        assert_eq!(status, 200, "{doc}");
        assert_eq!(doc["timed_out"], false, "{doc}");
        doc
    };
    let names = |doc: &serde_json::Value| -> Vec<String> {
        doc["folders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_string())
            .collect()
    };

    used.set(used.get() + 1);
    let (status, caps) = server.request("GET", "/api/capabilities", None, ops);
    assert_eq!(status, 200, "{caps}");
    for door in ["POST /api/ingest/folders", "POST /api/ingest/look"] {
        assert!(
            caps["doors"].as_array().unwrap().iter().any(|d| d == door),
            "{door}: {caps}"
        );
        assert!(
            caps["policy"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["door"] == door && p["grant"] == "data:work"),
            "{door}: {caps}"
        );
    }

    // a reader is refused; an operator starts at the roots, sources first
    let (status, refused) = call("/api/ingest/folders", json!({}), reader);
    assert_eq!(status, 403, "{refused}");
    let roots = listed(json!({}));
    assert_eq!(roots["at"], serde_json::Value::Null, "{roots}");
    assert_eq!(roots["roots"][0]["name"], "scans", "{roots}");
    assert_eq!(roots["roots"][0]["path"], scans_path, "{roots}");
    assert_eq!(
        roots["roots"][0]["place"],
        json!({"name": "incoming", "role": "source"}),
        "{roots}"
    );
    assert_eq!(roots["roots"][1]["name"], "archive", "{roots}");
    assert_eq!(
        roots["roots"][1]["place"],
        serde_json::Value::Null,
        "{roots}"
    );

    // a folder's folders, sorted without regard to case, links left out, files counted
    let top = listed(json!({"at": "@scans"}));
    assert_eq!(names(&top), ["alpha", "Beta", "gamma", "notes", "sub-001"]);
    assert_eq!(top["at"], "@scans", "{top}");
    assert_eq!(top["rel"], "", "{top}");
    assert_eq!(top["parent"], serde_json::Value::Null, "{top}");
    assert_eq!(top["path"], scans_path, "{top}");
    assert_eq!(top["files"], json!({"count": 1, "more": false}), "{top}");
    assert_eq!(top["total"], 5, "{top}");
    assert_eq!(top["next"], serde_json::Value::Null, "{top}");
    assert_eq!(top["partial"], false, "{top}");
    assert_eq!(top["place"]["name"], "incoming", "{top}");
    assert_eq!(top["folders"][0]["readable"], true, "{top}");
    assert_eq!(top["folders"][0]["place"]["role"], "source", "{top}");
    // a page, the page after it, and a filter
    let first = listed(json!({"at": "@scans", "limit": 2}));
    assert_eq!(names(&first), ["alpha", "Beta"]);
    assert_eq!(first["next"], "Beta", "{first}");
    assert_eq!(first["total"], 5, "{first}");
    let second = listed(json!({"at": "@scans", "after": "Beta", "limit": 2}));
    assert_eq!(names(&second), ["gamma", "notes"]);
    assert_eq!(second["next"], "notes", "{second}");
    let filtered = listed(json!({"at": "@scans", "filter": "A"}));
    assert_eq!(names(&filtered), ["alpha", "Beta", "gamma"]);
    assert_eq!(filtered["total"], 3, "{filtered}");
    let deeper = listed(json!({"at": "@scans/sub-001/ses-1/"}));
    assert_eq!(names(&deeper), ["anat", "func"]);
    assert_eq!(deeper["at"], "@scans/sub-001/ses-1", "{deeper}");
    assert_eq!(deeper["parent"], "@scans/sub-001", "{deeper}");
    // a folder inside a root that is a place of its own says so; its neighbour does not
    let archived = listed(json!({"at": "@archive"}));
    assert_eq!(names(&archived), ["looks", "many"]);
    assert_eq!(archived["place"], serde_json::Value::Null, "{archived}");
    assert_eq!(
        archived["folders"][0]["place"],
        json!({"name": "looked", "role": "source"}),
        "{archived}"
    );
    assert_eq!(
        archived["folders"][1]["place"],
        serde_json::Value::Null,
        "{archived}"
    );

    // twenty thousand folders: the first page, then every page after it, quickly
    let began = std::time::Instant::now();
    let mut seen: Vec<String> = Vec::new();
    let mut after = serde_json::Value::Null;
    loop {
        let page = listed(json!({"at": "@archive/many", "limit": 1000, "after": after}));
        assert_eq!(page["total"], 20_000, "{}", page["total"]);
        seen.extend(names(&page));
        after = page["next"].clone();
        if after.is_null() {
            break;
        }
        assert!(seen.len() < 20_000, "a next past the last page");
    }
    assert_eq!(seen.len(), 20_000);
    assert_eq!(seen[0], "f00000");
    assert_eq!(seen[19_999], "f19999");
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "in order, none twice");
    assert!(
        began.elapsed() < std::time::Duration::from_secs(20),
        "twenty pages took {:?}",
        began.elapsed()
    );

    // nothing outside the roots: a parent step, a leading slash, an unknown
    // root, a link that leaves the root and a path of the host
    for bad in [
        "@scans/../x",
        "@scans//etc",
        "@nowhere/x",
        "@scans/outside",
        "/etc",
    ] {
        let (status, doc) = call("/api/ingest/folders", json!({"at": bad}), ops);
        assert_eq!(status, 400, "{bad}: {doc}");
    }
    let nowhere = listed(json!({"at": "@scans/nothing-here"}));
    assert_eq!(nowhere["exists"], false, "{nowhere}");
    let file = listed(json!({"at": "@scans/list.csv"}));
    assert_eq!(file["exists"], true, "{file}");
    assert_eq!(file["directory"], false, "{file}");

    // a look: DICOM found in a nested tree, a folder of other files, an empty
    // one and a name that is not there, each in the order named
    let (status, refused) = call("/api/ingest/look", json!({"at": "@scans"}), reader);
    assert_eq!(status, 403, "{refused}");
    let (status, seen) = call(
        "/api/ingest/look",
        json!({"at": "@scans", "names": ["sub-001", "notes", "alpha", "gone"]}),
        ops,
    );
    assert_eq!(status, 200, "{seen}");
    assert_eq!(seen["timed_out"], false, "{seen}");
    assert_eq!(names(&seen), ["sub-001", "notes", "alpha", "gone"]);
    let sub = &seen["folders"][0];
    assert_eq!(sub["looked"], true, "{seen}");
    assert_eq!(sub["sampled"], 3, "{seen}");
    assert_eq!(sub["dicom"], 3, "{seen}");
    assert_eq!(sub["modalities"]["MR"], 3, "{seen}");
    assert_eq!(sub["files"], json!({"count": 3, "more": false}), "{seen}");
    assert_eq!(seen["folders"][1]["sampled"], 1, "{seen}");
    assert_eq!(seen["folders"][1]["dicom"], 0, "{seen}");
    assert_eq!(seen["folders"][2]["sampled"], 0, "{seen}");
    assert_eq!(
        seen["folders"][2]["files"],
        json!({"count": 0, "more": false}),
        "{seen}"
    );
    assert_eq!(seen["folders"][3]["directory"], false, "{seen}");
    assert_eq!(
        seen["here"]["files"],
        json!({"count": 1, "more": false}),
        "{seen}"
    );
    assert_eq!(seen["here"]["dicom"], 0, "{seen}");
    // past a tiny budget the folders not reached say only that
    let (status, tiny) = call(
        "/api/ingest/look",
        json!({"at": "@archive/looks", "budget_ms": 1}),
        ops,
    );
    assert_eq!(status, 200, "{tiny}");
    let looks = tiny["folders"].as_array().unwrap();
    assert_eq!(looks.len(), 40, "every folder, as none were named: {tiny}");
    assert_eq!(looks[0]["name"], "l00", "{tiny}");
    let unreached: Vec<&serde_json::Value> =
        looks.iter().filter(|f| f["looked"] == false).collect();
    assert!(!unreached.is_empty(), "{tiny}");
    for f in unreached {
        assert_eq!(f.as_object().unwrap().len(), 2, "{f}");
    }
    for body in [
        json!({"at": "@scans/../x"}),
        json!({"at": "@scans", "names": ["../x"]}),
    ] {
        let (status, doc) = call("/api/ingest/look", body.clone(), ops);
        assert_eq!(status, 400, "{body}: {doc}");
    }
    assert!(used.get() <= LIMIT, "{} requests", used.get());
    while used.get() < LIMIT {
        used.set(used.get() + 1);
        let _ = server.request("GET", "/api/capabilities", None, ops);
    }
    server.finish();
}

/// The suite contract, version 2: the door table. A door of every row is
/// passed by a token that holds its grant and refused, naming the grant, to
/// a token that does not; a queued verb needs its own grant and a cancel
/// the grant of the job's verb; adopting a rule needs two grants; the detail
/// gates refuse below their detail; a queued job records the caller's
/// detail; the policy rows name the grant of their door; a ceiling can leave
/// a caller with no grant; and a name that is neither a ladder name nor a
/// grant stops the engine before it listens.
#[test]
fn every_door_needs_its_grant_and_a_refusal_names_it() {
    let home = registry();
    // what each token holds, by the name it is known by
    let holds: &[(&str, &str)] = &[
        ("none", ""),
        ("assist", "assist"),
        ("query-see", "query:see"),
        ("query-work", "query:work"),
        ("data-see", "data:see"),
        ("data-work", "data:work"),
        ("places-see", "places:see"),
        ("places-work", "places:work"),
        ("review-see", "review:see"),
        ("review-work", "review:work"),
        ("both-work", "review:work,data:work"),
        ("release-see", "release:see"),
        ("release-work", "release:work"),
        ("pipelines-see", "pipelines:see"),
        ("pipelines-work", "pipelines:work"),
        ("database-see", "database:see"),
        ("database-work", "database:work"),
        ("audit-see", "audit:see"),
        ("reader", "reader"),
        ("reviewer", "reviewer"),
        ("operator", "operator"),
    ];
    let token = |name: &str| format!("a-token-that-holds-{name}");
    // a door of every row: who passes it, who is refused, and what the
    // refusal names
    let rows: &[(&str, &str, &str, &str, &str, &str)] = &[
        ("GET", "/api/status", "", "assist", "none", "no grant"),
        (
            "GET",
            "/api/ask/schema",
            "",
            "query-see",
            "data-see",
            "query:see",
        ),
        (
            "GET",
            "/api/instances/1/manifest",
            "",
            "query-see",
            "data-see",
            "query:see",
        ),
        (
            "GET",
            "/api/timeline/stack/1",
            "",
            "data-see",
            "places-see",
            "data:see, query:see",
        ),
        (
            "POST",
            "/api/ask/documents",
            "{}",
            "query-work",
            "query-see",
            "query:work",
        ),
        (
            "PUT",
            "/api/ask/selections/kept",
            "{}",
            "reviewer",
            "query-work",
            "detail quasi",
        ),
        (
            "POST",
            "/api/ask/values",
            "{}",
            "reviewer",
            "reader",
            "detail quasi",
        ),
        (
            "POST",
            "/api/ask/start",
            r#"{"from": {"values": "nowhere"}}"#,
            "reviewer",
            "reader",
            "detail quasi",
        ),
        ("GET", "/api/packs", "", "data-see", "query-see", "data:see"),
        (
            "GET",
            "/api/places",
            "",
            "places-see",
            "query-see",
            "data:see, places:see",
        ),
        (
            "POST",
            "/api/ingest/folders",
            "{}",
            "data-work",
            "data-see",
            "data:work",
        ),
        (
            "GET",
            "/api/review",
            "",
            "review-see",
            "reader",
            "review:see",
        ),
        (
            "POST",
            "/api/classify/try",
            "{}",
            "review-work",
            "review-see",
            "review:work",
        ),
        (
            "POST",
            "/api/overlays/999/adopt",
            "",
            "both-work",
            "review-work",
            "review:work and data:work",
        ),
        (
            "POST",
            "/api/overlays/999/adopt",
            "",
            "both-work",
            "data-work",
            "review:work and data:work",
        ),
        (
            "GET",
            "/api/releases",
            "",
            "release-see",
            "query-work",
            "release:see",
        ),
        (
            "POST",
            "/api/handovers",
            "{}",
            "release-work",
            "release-see",
            "release:work",
        ),
        (
            "GET",
            "/api/jobs",
            "",
            "pipelines-see",
            "reader",
            "pipelines:see",
        ),
        (
            "POST",
            "/api/sessions/rebuild",
            "{}",
            "pipelines-work",
            "pipelines-see",
            "pipelines:work",
        ),
        (
            "POST",
            "/api/places",
            "{}",
            "places-work",
            "places-see",
            "places:work",
        ),
        (
            "GET",
            "/api/backups",
            "",
            "database-see",
            "pipelines-work",
            "database:see",
        ),
        (
            "PUT",
            "/api/settings",
            "{}",
            "database-work",
            "database-see",
            "database:work",
        ),
        (
            "GET",
            "/api/audit",
            "",
            "audit-see",
            "database-work",
            "audit:see",
        ),
    ];
    // a queued command: the door needs one of the verbs' grants, and each
    // verb its own
    let jobs: &[(&str, &str, u16, &str)] = &[
        ("query-see", r#"["fingerprint"]"#, 403, "one of the grants"),
        ("data-work", r#"["fingerprint"]"#, 403, "pipelines:work"),
        (
            "pipelines-work",
            r#"["digest", "@nowhere"]"#,
            403,
            "data:work",
        ),
        (
            "data-work",
            r#"["digest", "@nowhere"]"#,
            400,
            "is not a registered ingest location",
        ),
        (
            "data-work",
            r#"["linkage", "import", "@nowhere/codes.csv"]"#,
            403,
            "detail sensitive",
        ),
        (
            "operator",
            r#"["linkage", "import", "@nowhere/codes.csv"]"#,
            400,
            "is not a registered ingest location",
        ),
        ("pipelines-work", r#"["backup"]"#, 403, "database:work"),
        ("database-work", r#"["backup"]"#, 409, "no backup directory"),
        // record 26 section 9: a promotion is a cohort act, Data work
        (
            "query-work",
            r#"["ask", "promote", "--handle", "1", "--cohort", "c"]"#,
            403,
            "data:work",
        ),
        (
            "query-work",
            r#"["ask", "gate"]"#,
            400,
            "ask run and ask promote",
        ),
        (
            "release-see",
            r#"["release", "--name", "x"]"#,
            403,
            "release:work",
        ),
    ];
    // the rows twice, the jobs, then the cancels, the recorded detail, the
    // capabilities and the ceiling below
    let requests = rows.len() * 2 + jobs.len() + 8;
    let mut extra: Vec<String> = vec!["--auth".into(), "token".into()];
    for (name, list) in holds {
        extra.push("--token".into());
        extra.push(format!("{}={name}@lab:{list}", token(name)));
    }
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    let server = Server::start(&home, requests, &extra, &[]);
    for (method, path, body, passes, refused, names) in rows {
        let body = (!body.is_empty()).then_some(*body);
        let (status, doc) = server.request(method, path, body, Some(&token(passes)));
        assert_ne!(status, 403, "{method} {path} as {passes}: {doc}");
        let (status, doc) = server.request(method, path, body, Some(&token(refused)));
        assert_eq!(status, 403, "{method} {path} as {refused}: {doc}");
        assert!(
            doc["error"].as_str().unwrap_or_default().contains(names),
            "{method} {path} as {refused} names {names}: {doc}"
        );
    }
    for (holder, command, want, says) in jobs {
        let (status, doc) = server.request(
            "POST",
            "/api/jobs",
            Some(&format!(r#"{{"command": {command}}}"#)),
            Some(&token(holder)),
        );
        assert_eq!(status, *want, "{command} as {holder}: {doc}");
        assert!(
            doc["error"].as_str().unwrap_or_default().contains(says),
            "{command} as {holder} says {says}: {doc}"
        );
    }
    // a cancel needs the grant of the job's verb
    let (status, queued) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["fingerprint"]}"#),
        Some(&token("pipelines-work")),
    );
    assert_eq!(status, 202, "{queued}");
    let cancel = format!("/api/jobs/{}/cancel", queued["job"]);
    let (status, doc) = server.request("POST", &cancel, None, Some(&token("data-work")));
    assert_eq!(status, 403, "{doc}");
    assert!(
        doc["error"].as_str().unwrap().contains("pipelines:work"),
        "{doc}"
    );
    let (status, doc) = server.request("POST", &cancel, None, Some(&token("pipelines-work")));
    assert_eq!(status, 200, "{doc}");
    let (status, doc) = server.request(
        "POST",
        "/api/jobs/99999/cancel",
        None,
        Some(&token("pipelines-work")),
    );
    assert_eq!(status, 404, "{doc}");
    // a queued job records the caller's detail, which its verb runs under
    let (status, queued) = server.request(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["ask", "run", "--document", "1"]}"#),
        Some(&token("reviewer")),
    );
    assert_eq!(status, 202, "{queued}");
    let (status, shown) = server.request(
        "GET",
        &format!("/api/jobs/{}", queued["job"]),
        None,
        Some(&token("pipelines-see")),
    );
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["args"]["detail"], "quasi", "{shown}");
    assert_eq!(shown["args"]["actor"]["kind"], "absent", "{shown}");
    // the capabilities: the caller's grants, detail and steps, and a policy
    // row that names the grant of its door, the four doors where the policy
    // and the code once disagreed at the stricter side
    let (status, caps) = server.request("GET", "/api/capabilities", None, Some(&token("reviewer")));
    assert_eq!(status, 200, "{caps}");
    assert_eq!(
        caps["grants"],
        serde_json::json!([
            "data:see",
            "pipelines:see",
            "query:see",
            "query:work",
            "review:see",
            "review:work"
        ]),
        "{caps}"
    );
    assert_eq!(caps["detail"], "quasi", "{caps}");
    assert_eq!(
        caps["roles"],
        serde_json::json!(["reader", "reviewer"]),
        "{caps}"
    );
    let row = |door: &str| -> serde_json::Value {
        caps["policy"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["door"] == door)
            .cloned()
            .unwrap_or_else(|| panic!("no policy row for {door}"))
    };
    assert!(
        caps["policy"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["role"].is_null() && !r["grant"].is_null()),
        "{caps}"
    );
    assert_eq!(
        row("GET /api/status")["grant"].as_array().unwrap().len(),
        24
    );
    assert_eq!(row("GET /api/review")["grant"], "review:see");
    assert_eq!(row("GET /api/review/{id}")["grant"], "review:see");
    assert_eq!(row("POST /api/ask/values")["grant"], "query:work");
    assert_eq!(row("POST /api/ask/values")["detail"], "quasi");
    let tiles = row("GET /api/instances/{stack}/tiles/{level}/{z}");
    assert_eq!(tiles["grant"], "query:see", "{tiles}");
    assert_eq!(tiles["detail"], "quasi", "{tiles}");
    assert_eq!(
        row("GET /api/places")["grant"],
        serde_json::json!(["data:see", "places:see"])
    );
    let adopt = row("POST /api/overlays/{id}/adopt");
    assert_eq!(adopt["grant"], "review:work", "{adopt}");
    assert_eq!(adopt["also"], "data:work", "{adopt}");
    assert_eq!(
        row("POST /api/jobs")["grant"],
        serde_json::json!([
            "data:work",
            "database:work",
            "pipelines:work",
            "query:work",
            "release:work"
        ])
    );
    // a ceiling keeps only what its step's set holds, which can be nothing
    let (status, doc) = server.request_with(
        "GET",
        "/api/status",
        None,
        Some(&token("places-work")),
        &[("X-Nils-Ceiling", "reader")],
    );
    assert_eq!(status, 403, "{doc}");
    assert!(doc["error"].as_str().unwrap().contains("no grant"), "{doc}");
    server.finish();

    // a name that is neither a ladder name nor a grant stops the engine
    // before it listens, in a token's list and in a binding, and so does a
    // trust entry whose keep_subject is neither true nor false
    let jwks = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oidc/jwks.json");
    let trust = format!(
        "issuer=https://id.example.org/,audience=nils,jwks={}",
        jwks.display()
    );
    let keeps = format!("{trust},keep_subject=yes");
    for (args, says) in [
        (
            vec![
                "--auth",
                "token",
                "--token",
                "sixteen-characters-long=bo@lab:reader,coffee:work",
            ],
            "neither a ladder name nor a grant",
        ),
        (
            vec![
                "--auth",
                "oidc",
                "--oidc-trust",
                trust.as_str(),
                "--role",
                "students=assistant:see",
            ],
            "neither a ladder name nor a grant",
        ),
        (
            vec!["--auth", "oidc", "--oidc-trust", keeps.as_str()],
            "keep_subject is true or false",
        ),
    ] {
        let out = nils()
            .arg("--registry")
            .arg(home.path())
            .args(["serve", "--bind", "127.0.0.1:0"])
            .args(&args)
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {err}");
        assert!(err.contains(says), "{args:?}: {err}");
    }
}

/// The suite contract, version 2: the grants vectors run against the
/// engine. The claims and the ceilings are tokens of the trust list's first
/// issuer, bound by the vectors' `--role` bindings; a principal is a token
/// of the issuer its case names, whose entry keeps subjects when the case
/// says so; a named case is a token of `--auth token`.
/// What the capabilities say is compared with what each case expects.
#[test]
fn the_grants_vectors_hold() {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/suite/v2/vectors");
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(vectors.join(name)).unwrap()).unwrap()
    };
    let g = read("grants.json");
    let t = read("trust-list.json");
    let kid = "test-2026";
    let key = EncodingKey::from_rsa_pem(
        &std::fs::read(vectors.join(t["keys"][kid].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    let jwks = vectors.join(t["trust"][0]["jwks"].as_str().unwrap());
    let issuer = t["trust"][0]["issuer"].as_str().unwrap();
    let audience = t["trust"][0]["audience"].as_str().unwrap();
    let mut extra: Vec<String> = vec![
        "--auth".into(),
        "oidc".into(),
        "--oidc-groups-claim".into(),
        g["groups_claim"].as_str().unwrap().into(),
        "--oidc-trust".into(),
        format!(
            "issuer={issuer},audience={audience},jwks={}",
            jwks.display()
        ),
    ];
    let claims = g["claims"].as_array().unwrap();
    let ceilings = g["ceilings"].as_array().unwrap();
    let principals = g["principals"].as_array().unwrap();
    // every issuer a principal case names, trusted with the same keys, one
    // entry each, keeping subjects when the case's entry does; an entry that
    // keeps none says nothing, so the default is what is tested
    let mut entries: Vec<(&str, bool)> = principals
        .iter()
        .map(|c| {
            (
                c["iss"].as_str().unwrap(),
                c["keep_subject"].as_bool().unwrap_or(false),
            )
        })
        .filter(|(i, _)| *i != issuer)
        .collect();
    entries.sort();
    entries.dedup();
    assert!(
        entries.windows(2).all(|w| w[0].0 != w[1].0),
        "one entry per issuer: {entries:?}"
    );
    for (iss, keep) in entries {
        extra.push("--oidc-trust".into());
        extra.push(format!(
            "issuer={iss},audience={audience},jwks={}{}",
            jwks.display(),
            if keep { ",keep_subject=true" } else { "" }
        ));
    }
    for (group, bound) in g["roles"].as_object().unwrap() {
        extra.push("--role".into());
        extra.push(format!("{group}={}", bound.as_str().unwrap()));
    }
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    let home = registry();
    let server = Server::start(
        &home,
        claims.len() + ceilings.len() + principals.len(),
        &extra,
        &[],
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mint = |mut claims: serde_json::Value, iss: &str| -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        claims["iss"] = iss.into();
        claims["aud"] = audience.into();
        if claims.get("sub").is_none() {
            claims["sub"] = "anna".into();
        }
        claims["iat"] = now.into();
        claims["exp"] = (now + 600).into();
        encode(&header, &claims, &key).unwrap()
    };
    let compare = |name: &str, status: u16, doc: &serde_json::Value, expect: &serde_json::Value| {
        if expect["refused"] == true {
            assert_eq!(status, 403, "{name}: {doc}");
            assert!(
                doc["error"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("no grant"),
                "{name}: {doc}"
            );
        } else {
            assert_eq!(status, 200, "{name}: {doc}");
            assert_eq!(doc["grants"], expect["grants"], "{name}: {doc}");
            assert_eq!(doc["detail"], expect["detail"], "{name}: {doc}");
        }
    };
    for case in claims {
        let name = case["name"].as_str().unwrap();
        let token = mint(case["claims"].clone(), issuer);
        let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&token));
        compare(name, status, &doc, &case["expect"]);
    }
    for case in ceilings {
        let name = case["name"].as_str().unwrap();
        let token = mint(
            serde_json::json!({"grants": case["grants"], "detail": case["detail"]}),
            issuer,
        );
        let ceiling = case["ceiling"].as_str().unwrap();
        let (status, doc) = server.request_with(
            "GET",
            "/api/capabilities",
            None,
            Some(&token),
            &[("X-Nils-Ceiling", ceiling)],
        );
        compare(name, status, &doc, &case["expect"]);
        assert_eq!(doc["ceiling"], ceiling, "{name}: {doc}");
        assert_eq!(doc["actor"]["ceiling"], ceiling, "{name}: {doc}");
    }
    for case in principals {
        let name = case["name"].as_str().unwrap();
        let token = mint(
            serde_json::json!({"sub": case["sub"], "grants": ["query:see"]}),
            case["iss"].as_str().unwrap(),
        );
        let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&token));
        assert_eq!(status, 200, "{name}: {doc}");
        assert_eq!(doc["principal"], case["expect"], "{name}: {doc}");
    }
    server.finish();

    // the named cases: one token of `--auth token` each
    let named = g["named"].as_array().unwrap();
    let tokens: Vec<String> = named
        .iter()
        .enumerate()
        .map(|(i, case)| {
            format!(
                "a-named-vector-token-{i}=named{i}@lab:{}",
                case["roles"].as_str().unwrap()
            )
        })
        .collect();
    let mut extra: Vec<&str> = vec!["--auth", "token"];
    for t in &tokens {
        extra.push("--token");
        extra.push(t.as_str());
    }
    let server = Server::start(&home, named.len(), &extra, &[]);
    for (i, case) in named.iter().enumerate() {
        let name = case["name"].as_str().unwrap();
        let token = format!("a-named-vector-token-{i}");
        let (status, doc) = server.request("GET", "/api/capabilities", None, Some(&token));
        compare(name, status, &doc, &case["expect"]);
    }
    server.finish();
}

/// Record 26 §7 and §14 through the doors: `bring-in @dataset` queues the
/// thread of a dataset as a chain the serve worker runs step by step,
/// each job naming the one before and after it; the batch page reads the
/// five stages off the thread and the timeline serves the batch; the
/// sources door fills the pseudonymise step in and the machine's rates; a
/// step the caller who queued the chain may not queue ends the chain and
/// the job says why; a chain that is not one is refused at the door; and
/// a dataset's originals are never digested.
#[test]
fn a_chain_runs_through_the_jobs_door_and_a_refused_step_ends_it() {
    let home = registry();
    let dir = TempDir::new("chain-ds");
    let identified = |patient: &str, instance: u32| {
        let study = format!("1.2.826.0.1.3680043.8.498.{patient}.1");
        let series = format!("{study}.1");
        let sop = format!("{series}.{instance}");
        let mut e = synth::minimal_mr(&study, &series, &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        e.push(synth::text(tags::PATIENT_NAME, VR::PN, "Doe^Jane"));
        e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
        e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
        e.push(synth::text(
            tags::INSTANCE_NUMBER,
            VR::IS,
            &instance.to_string(),
        ));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        e.push(synth::bytes(
            tags::PIXEL_DATA,
            VR::OW,
            (0..2000u32).map(|i| (i % 251) as u8).collect(),
        ));
        synth::part10(&MetaFields::mr(&sop), &e, true)
    };
    for (p, patient) in ["199001011234", "198502023456"].iter().enumerate() {
        for instance in 1..=3 {
            dir.file(
                &format!("sub-{p}/IM_{instance:04}"),
                &identified(patient, instance),
            );
        }
    }
    run(
        &home,
        &[
            "place",
            "add",
            "ds",
            dir.path().to_str().unwrap(),
            "--role",
            "source",
            "--arrives",
            "identified",
            "--unmapped",
            "code",
        ],
        None,
    );
    const LIMIT: usize = 320;
    let used = std::cell::Cell::new(0usize);
    let server = Server::start(
        &home,
        LIMIT,
        &[
            "--worker",
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--token",
            "a-data-token-of-length-x=data@lab:reviewer,data:work",
            "--ingest-root",
            &format!("ds={}", dir.path().display()),
        ],
        &[("NILS_PACK_DIR", packs().to_str().unwrap())],
    );
    let reader = Some("a-reader-token-of-length");
    let ops = Some("an-operator-token-of-len");
    let data = Some("a-data-token-of-length-x");
    let ask = |method: &str, path: &str, body: Option<&str>, token: Option<&str>| {
        used.set(used.get() + 1);
        server.request(method, path, body, token)
    };
    let wait = |job: i64| -> serde_json::Value {
        let mut shown = serde_json::Value::Null;
        for _ in 0..150 {
            let (status, now) = ask("GET", &format!("/api/jobs/{job}"), None, ops);
            assert_eq!(status, 200, "{now}");
            shown = now;
            if matches!(
                shown["state"].as_str(),
                Some("done" | "failed" | "cancelled")
            ) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        shown
    };

    // the originals are the pseudonymiser's: a digest of them is refused
    let (status, refused) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["digest", "@ds/originals"]}"#),
        ops,
    );
    assert_eq!(status, 409, "{refused}");
    // a chain that is not one, and a verb the door does not queue
    let (status, refused) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["digest", "@ds"], "then": [["bring-in", "@ds"]]}"#),
        ops,
    );
    assert_eq!(status, 400, "{refused}");
    let (status, refused) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["digest", "@ds"], "then": [["restore", "x"]]}"#),
        ops,
    );
    assert_eq!(status, 400, "{refused}");
    let (status, refused) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["pseudonymize", "@ds"]}"#),
        reader,
    );
    assert_eq!(status, 403, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("data:work"),
        "{refused}"
    );
    let (status, refused) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["pseudonymize", "@ds"]}"#),
        data,
    );
    assert_eq!(
        status, 403,
        "the pseudonymiser reads identifiers: {refused}"
    );
    assert!(
        refused["error"].as_str().unwrap().contains("sensitive"),
        "{refused}"
    );

    // bring-in: the thread queued as a chain
    let (status, queued) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["bring-in", "@ds", "--name", "chain-1"], "name": "chain-1"}"#),
        ops,
    );
    assert_eq!(status, 202, "{queued}");
    assert_eq!(queued["command"][0], "pseudonymize", "{queued}");
    assert_eq!(queued["command"][1], "@ds", "{queued}");
    let then = queued["then"].as_array().unwrap();
    assert_eq!(then.len(), 3, "{queued}");
    assert_eq!(then[0][0], "digest", "{queued}");
    assert!(
        then[0][1]
            .as_str()
            .unwrap()
            .ends_with("derivatives/dcm-anon"),
        "the digest's tree located: {queued}"
    );
    assert_eq!(then[1], serde_json::json!(["fingerprint"]), "{queued}");
    assert_eq!(then[2], serde_json::json!(["classify"]), "{queued}");
    let first = queued["job"].as_i64().unwrap();
    let mut ids = vec![first];
    let mut job = wait(first);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["kind"], "pseudonymize", "{job}");
    assert_eq!(job["result"]["files"]["written"], 6, "{job}");
    assert_eq!(job["then"].as_array().unwrap().len(), 3, "{job}");
    assert_eq!(job["chain"]["before"], serde_json::Value::Null, "{job}");
    // the chain, step by step, each job naming the one before
    for expected in ["digest", "fingerprint", "classify"] {
        let mut next = job["chain"]["after"].as_i64();
        for _ in 0..50 {
            if next.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            let (_, again) = ask(
                "GET",
                &format!("/api/jobs/{}", ids.last().unwrap()),
                None,
                ops,
            );
            next = again["chain"]["after"].as_i64();
        }
        let next = next.unwrap_or_else(|| panic!("no job after {expected}: {job}"));
        job = wait(next);
        assert_eq!(job["state"], "done", "{job}");
        assert_eq!(job["kind"], expected, "{job}");
        assert_eq!(
            job["chain"]["before"],
            serde_json::json!(ids.last().unwrap()),
            "{job}"
        );
        assert_eq!(job["args"]["principal"], "ops@lab", "{job}");
        assert_eq!(job["args"]["detail"], "sensitive", "{job}");
        ids.push(next);
    }
    assert_eq!(job["then"], serde_json::json!([]), "{job}");
    assert_eq!(job["chain"]["after"], serde_json::Value::Null, "{job}");

    // the batch is the thread: the two batches of one name, the stages
    let (status, batches) = ask("GET", "/api/batches", None, reader);
    assert_eq!(status, 200, "{batches}");
    let of = |kind: &str| {
        batches["batches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "chain-1" && b["kind"] == kind)
            .cloned()
            .unwrap_or_else(|| panic!("no {kind} batch chain-1: {batches}"))
    };
    let pseudonymise = of("pseudonymize");
    let digest = of("digest");
    assert_eq!(pseudonymise["seen"], 6, "{pseudonymise}");
    let (status, page) = ask(
        "GET",
        &format!("/api/batches/{}", digest["id"]),
        None,
        reader,
    );
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["kind"], "digest", "{page}");
    let stages = &page["stages"];
    assert_eq!(stages["pseudonymised"]["files"], 6, "{page}");
    assert_eq!(stages["pseudonymised"]["changed"], 6, "{page}");
    assert_eq!(stages["pseudonymised"]["held"], 0, "{page}");
    assert_eq!(stages["pseudonymised"]["job"], first, "{page}");
    assert_eq!(
        stages["pseudonymised"]["batch"], pseudonymise["id"],
        "{page}"
    );
    assert_eq!(stages["walked"]["files"], 6, "{page}");
    assert_eq!(stages["walked"]["new"], 6, "{page}");
    assert_eq!(stages["walked"]["job"], ids[1], "{page}");
    assert_eq!(stages["digested"]["stacks"], 2, "{page}");
    // record 26 §14: the subjects whose files the batch holds, which the
    // pseudonymiser made and the digest found by their codes
    assert_eq!(stages["digested"]["subjects"], 2, "{page}");
    assert_eq!(stages["classified"]["of"], 2, "{page}");
    assert_eq!(stages["classified"]["stacks"], 2, "{page}");
    assert_eq!(
        stages["classified"]["jobs"],
        serde_json::json!([ids[3]]),
        "{page}"
    );
    assert!(
        stages["classified"]["pack"]
            .as_str()
            .unwrap()
            .starts_with("mri"),
        "{page}"
    );
    assert!(stages["classified"]["by_base"].is_object(), "{page}");
    assert!(stages["reviewed"]["of"].as_u64().is_some(), "{page}");
    let (status, page) = ask(
        "GET",
        &format!("/api/batches/{}", pseudonymise["id"]),
        None,
        reader,
    );
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["kind"], "pseudonymize", "{page}");
    assert_eq!(
        page["stages"]["pseudonymised"]["batch"], pseudonymise["id"],
        "{page}"
    );
    assert_eq!(page["stages"]["walked"]["batch"], digest["id"], "{page}");
    assert_eq!(page["report"]["files"]["written"], 6, "{page}");

    // the timeline of a batch
    let (status, line) = ask(
        "GET",
        &format!("/api/timeline/batch/{}", digest["id"]),
        None,
        reader,
    );
    assert_eq!(status, 200, "{line}");
    let kinds: Vec<&str> = line["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    for k in ["started", "finished", "pseudonymised", "classified"] {
        assert!(kinds.contains(&k), "{k} missing: {line}");
    }
    let (status, _) = ask("GET", "/api/timeline/batch/999999", None, reader);
    assert_eq!(status, 404);

    // the sources door: the pseudonymise step on the digest, the rates
    let (status, sources) = ask("GET", "/api/sources", None, reader);
    assert_eq!(status, 200, "{sources}");
    let source = sources["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "ds")
        .unwrap();
    let recent = &source["digests"]["recent"][0];
    assert_eq!(recent["name"], "chain-1", "{sources}");
    assert_eq!(recent["pseudonymised"]["files"], 6, "{sources}");
    assert_eq!(recent["pseudonymised"]["changed"], 6, "{sources}");
    assert_eq!(recent["pseudonymised"]["job"], first, "{sources}");
    // record 26 §14: the jobs of the thread by stage, the whole way from
    // the pseudonymise step to the runs that sorted the stacks
    assert_eq!(recent["chain"]["pseudonymize"], first, "{sources}");
    assert_eq!(recent["chain"]["digest"], ids[1], "{sources}");
    assert_eq!(
        recent["chain"]["classify"],
        serde_json::json!([ids[3]]),
        "{sources}"
    );
    assert_eq!(
        source["digests"]["count"], 1,
        "a pseudonymise step is not a digest: {sources}"
    );
    // record 26 §14: a rate says what it was measured over, so that a page
    // can say what the number is worth
    for step in ["pseudonymize", "digest"] {
        let rate = &sources["rates"][step];
        assert!(rate["files_per_s"].as_f64().is_some(), "{step}: {sources}");
        assert_eq!(rate["files"], 6, "{step}: {sources}");
    }

    // the jobs read as queued (lab 26, defect 19): the command line the
    // door located, what ran beside it, and the queue's worker left out
    // of the list unless asked for
    let (_, shown) = ask("GET", &format!("/api/jobs/{first}"), None, ops);
    assert_eq!(shown["args"]["queued"][0], "pseudonymize", "{shown}");
    assert_eq!(shown["args"]["queued"][2], "--name", "{shown}");
    assert!(
        shown["args"]["argv"][0].as_str().unwrap().ends_with("nils"),
        "what ran: {shown}"
    );
    assert_eq!(job["args"]["queued"][0], "classify", "{job}");
    let (_, open) = ask("GET", "/api/jobs", None, ops);
    assert!(
        !open["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["kind"] == "worker"),
        "{open}"
    );
    let (_, every) = ask("GET", "/api/jobs?all=1", None, ops);
    assert!(
        every["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["kind"] == "worker"),
        "{every}"
    );

    // a dry run at the door answers its report as the job's result (lab
    // 26, defect 17)
    let (status, queued) = ask(
        "POST",
        "/api/jobs",
        Some(r#"{"command": ["pseudonymize", "@ds", "--dry-run"]}"#),
        ops,
    );
    assert_eq!(status, 202, "{queued}");
    let dry = wait(queued["job"].as_i64().unwrap());
    assert_eq!(dry["state"], "done", "{dry}");
    assert_eq!(dry["result"]["dry_run"], true, "{dry}");
    assert_eq!(dry["result"]["files"]["seen"], 6, "{dry}");
    assert_eq!(dry["result"]["files"]["unchanged"], 6, "{dry}");

    // a step the caller may not queue ends the chain, and the job says why
    let (status, queued) = ask(
        "POST",
        "/api/jobs",
        Some(
            r#"{"command": ["digest", "@ds", "--name", "chain-2"], "then": [["fingerprint"], ["classify"]]}"#,
        ),
        data,
    );
    assert_eq!(status, 202, "{queued}");
    let second = queued["job"].as_i64().unwrap();
    let job = wait(second);
    assert_eq!(job["state"], "done", "{job}");
    let mut stopped = job["result"]["chain_stopped"].clone();
    for _ in 0..50 {
        if !stopped.is_null() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let (_, again) = ask("GET", &format!("/api/jobs/{second}"), None, ops);
        stopped = again["result"]["chain_stopped"].clone();
    }
    assert_eq!(stopped["step"], serde_json::json!(["fingerprint"]), "{job}");
    assert!(
        stopped["why"].as_str().unwrap().contains("pipelines:work"),
        "{stopped}"
    );
    let (_, again) = ask("GET", &format!("/api/jobs/{second}"), None, ops);
    assert_eq!(again["chain"]["after"], serde_json::Value::Null, "{again}");
    let (_, jobs) = ask("GET", "/api/jobs?all=1", None, ops);
    assert!(
        !jobs["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["chain"]["before"] == second),
        "nothing queued after the refused step: {jobs}"
    );

    while used.get() < LIMIT {
        ask("GET", "/api/capabilities", None, reader);
    }
    server.finish();
}

const LIST_OVERLAY: &str = r#"{
  "overlay": "site-lists", "version": "1.0.0", "pack": "mri",
  "scope": {"manufacturer": "SYNTHETIC"},
  "lists": {"technique.TSE": {"add": ["zzgado"]}},
  "cases": [{"name": "the site's own turbo word",
             "stack": {"text_series_description": "zzgado"},
             "axes": {"technique": "TSE"}}]
}"#;

/// Record 26, decision 12 (pack contract 5): a word added to an axis
/// value's list through an overlay moves a verdict in a rehearsal and after
/// adoption exactly as a bucket's does, the packs door then names the
/// site's term on that list, the overlay commands print and export it, and
/// the exported document loads on the command line.
#[test]
fn a_list_on_an_axis_value_rehearses_adopts_and_is_named_on_the_pack() {
    let home = knob_registry();
    let server = Server::start(
        &home,
        8,
        &[
            "--auth",
            "token",
            "--token",
            "a-reviewer-token-of-len=rev@lab:reviewer",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
        ],
        &[],
    );
    let reviewer = Some("a-reviewer-token-of-len");
    let ops = Some("an-operator-token-of-len");

    // 1: the rehearsal moves the one stack that carries the word, from the
    // technique the pack's own words decided to the one the site's word does
    let body = format!(r#"{{"overlay": {LIST_OVERLAY}, "scope": "batch:1", "sample": 100}}"#);
    let (status, tried) = server.request("POST", "/api/classify/try", Some(&body), reviewer);
    assert_eq!(status, 200, "{tried}");
    assert_eq!(tried["cases"]["passed"], 1, "{tried}");
    assert_eq!(tried["cases"]["failed"], 0, "{tried}");
    let moves = tried["moves"].as_array().unwrap();
    assert!(
        moves
            .iter()
            .any(|m| m["axis"] == "technique" && m["to"] == "TSE" && m["stacks"] == 1),
        "the site's word moves technique: {tried}"
    );
    // 2: a list an overlay names that the pack cannot reach is refused with why
    let bad = LIST_OVERLAY.replace("technique.TSE", "provenance.RawRecon");
    let body = format!(r#"{{"overlay": {bad}, "scope": "batch:1"}}"#);
    let (status, refused) = server.request("POST", "/api/classify/try", Some(&body), reviewer);
    assert_eq!(status, 400, "{refused}");
    assert!(
        refused
            .to_string()
            .contains("no word of the pack's reaches RawRecon on provenance"),
        "{refused}"
    );
    // 3-4: proposed with its rehearsal, adopted by an operator
    let body = format!(
        r#"{{"name": "site lists", "overlay": {LIST_OVERLAY}, "scope": "batch:1", "why": "the site's turbo word"}}"#
    );
    let (status, proposed) = server.request("POST", "/api/overlays", Some(&body), reviewer);
    assert_eq!(status, 201, "{proposed}");
    let id = proposed["overlay"]["id"].as_i64().unwrap();
    assert_eq!(
        proposed["overlay"]["document"]["lists"]["technique.TSE"]["add"],
        serde_json::json!(["zzgado"]),
        "{proposed}"
    );
    let (status, adopted) = server.request("POST", &format!("/api/overlays/{id}/adopt"), None, ops);
    assert_eq!(status, 202, "{adopted}");
    let adopt_job = adopted["job"].as_i64().unwrap();
    // 5: the packs door names the site's term on the list, on the value and
    // per list, and the overlay it came from
    let (status, pack) = server.request("GET", "/api/packs/mri", None, reviewer);
    assert_eq!(status, 200, "{pack}");
    assert_eq!(
        pack["site"]["technique.TSE"]["add"],
        serde_json::json!(["zzgado"]),
        "{}",
        pack["site"]
    );
    assert_eq!(
        pack["site"]["technique.TSE"]["overlays"],
        serde_json::json!([id])
    );
    let tse = pack["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == "technique")
        .and_then(|a| a["values"].as_array())
        .unwrap()
        .iter()
        .find(|v| v["name"] == "TSE")
        .cloned()
        .unwrap();
    assert_eq!(tse["site"]["add"], serde_json::json!(["zzgado"]), "{tse}");
    assert_eq!(
        tse["keywords"][0], "tse",
        "the pack's own words are the pack's, unamended on disk: {tse}"
    );
    assert_eq!(pack["adopted"][0]["id"], id, "{}", pack["adopted"]);
    // 6-8: the overlay row keeps the document; the signals by value after
    // the rehearsal are unchanged, since a rehearsal writes nothing
    let (status, row) = server.request("GET", &format!("/api/overlays/{id}"), None, reviewer);
    assert_eq!(status, 200, "{row}");
    assert_eq!(
        row["document"]["lists"]["technique.TSE"]["add"][0], "zzgado",
        "{row}"
    );
    let (status, signals) =
        server.request("GET", "/api/classify/signals?scope=batch:1", None, reviewer);
    assert_eq!(status, 200, "{signals}");
    assert!(
        signals["by_value"]["technique"]["TSE"].is_null(),
        "{signals}"
    );
    let (status, doc) = server.request("GET", "/api/overlays", None, reviewer);
    assert_eq!(status, 200, "{doc}");
    server.finish();

    // the worker runs the reclassify under the adopted overlay, and the
    // stack citing the site's word is TSE now, the other unchanged
    run(&home, &["jobs", "work", "--once"], None);
    let job = run(
        &home,
        &["jobs", "show", &adopt_job.to_string(), "--json"],
        None,
    );
    let job: serde_json::Value = serde_json::from_str(&job).unwrap();
    assert_eq!(job["state"], "done", "{job}");
    let explained: Vec<serde_json::Value> = [1, 2]
        .iter()
        .map(|id| {
            let text = run(&home, &["explain", &id.to_string(), "--json"], None);
            serde_json::from_str(&text).unwrap()
        })
        .collect();
    let technique_of = |doc: &serde_json::Value| -> String {
        doc["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axis"] == "technique")
            .and_then(|a| a["value"].as_str())
            .unwrap_or("")
            .to_string()
    };
    let cites = |doc: &serde_json::Value| {
        doc["axes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["axis"] == "technique")
            .flat_map(|a| a["evidence"].as_array().cloned().unwrap_or_default())
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
    assert_eq!(technique_of(moved), "TSE", "{moved}");
    assert_eq!(moved["overlay"], "site-lists@1.0.0", "{moved}");
    assert_ne!(technique_of(still), "TSE", "{still}");
    assert_eq!(still["overlay"], "site-lists@1.0.0", "{still}");

    // `nils overlay show` prints the list, and the export loads on the
    // command line as `nils classify --overlay` loads it
    let shown = run(&home, &["overlay", "show", &id.to_string()], None);
    assert!(shown.contains("list technique.TSE: +zzgado"), "{shown}");
    let out = TempDir::new("overlay-export");
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
    assert!(wrote.contains("site-lists-1.0.0.overlay.json"), "{wrote}");
    let file = out.path().join("site-lists-1.0.0.overlay.json");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"lists\""), "{text}");
    let pack = run(
        &home,
        &[
            "pack",
            "show",
            "mri",
            "--json",
            "--overlay",
            file.to_str().unwrap(),
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
        None,
    );
    let pack: serde_json::Value = serde_json::from_str(&pack).unwrap();
    assert_eq!(pack["overlay"], "site-lists@1.0.0", "{}", pack["overlay"]);
    let tse = pack["axes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["axis"] == "technique")
        .and_then(|a| a["values"].as_array())
        .unwrap()
        .iter()
        .find(|v| v["name"] == "TSE")
        .cloned()
        .unwrap();
    assert!(
        tse["keywords"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "zzgado"),
        "loaded under the overlay, the value carries the site's word: {tse}"
    );
    let classified = run(
        &home,
        &[
            "classify",
            "--overlay",
            file.to_str().unwrap(),
            "--pack-dir",
            packs().to_str().unwrap(),
            "--json",
        ],
        None,
    );
    let classified: serde_json::Value = serde_json::from_str(&classified).unwrap();
    assert_eq!(classified["pack"], "mri@0.1.5", "{classified}");
}
