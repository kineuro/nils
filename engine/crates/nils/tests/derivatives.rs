// SPDX-License-Identifier: AGPL-3.0-only
//! Record 42 S4, derivatives: with no working place the capability is off
//! and the registering door says so; with one, an upload is hashed on the
//! way in, refused when its digest is not the one named, and read back
//! byte for byte with its digest; a caller without the grant is refused;
//! every registration and download is audited; and the command line
//! registers, lists and shows the same rows.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
    let mut child = nils()
        .arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .env_remove("NILS_DSN")
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

fn ok(home: &TempDir, args: &[&str]) -> String {
    let (good, out, err) = run(home, args, None);
    assert!(good, "nils {args:?} failed: {err}");
    out
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

const READER: &str = "a-reader-token-of-length";
const REVIEWER: &str = "a-reviewer-token-of-len";
const OPERATOR: &str = "an-operator-token-of-len";

struct Server {
    child: Child,
    port: u16,
}

impl Server {
    /// A server that answers exactly `requests` requests and exits.
    fn start(home: &TempDir, requests: usize) -> Server {
        let mut child = nils()
            .arg("--registry")
            .arg(home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "1",
                "--requests",
            ])
            .arg(requests.to_string())
            .args([
                "--auth",
                "token",
                "--token",
                &format!("{READER}=lou@lab:reader"),
                "--token",
                &format!("{REVIEWER}=rev@lab:reviewer"),
                "--token",
                &format!("{OPERATOR}=ops@lab:operator"),
            ])
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            let _ = child.kill();
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        Server { child, port }
    }

    /// One request: the status, the headers, the body's bytes.
    fn call(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<(&str, &[u8])>,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n"
        );
        if let Some((content_type, bytes)) = body {
            head.push_str(&format!(
                "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
                bytes.len()
            ));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        if let Some((_, bytes)) = body {
            // a refusal may answer before the body is read
            let _ = stream.write_all(bytes);
        }
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let split = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a header block");
        let head = String::from_utf8_lossy(&response[..split]).to_string();
        let status: u16 = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = head
            .lines()
            .skip(1)
            .filter_map(|l| {
                l.split_once(": ")
                    .map(|(k, v)| (k.to_lowercase(), v.to_string()))
            })
            .collect();
        (status, headers, response[split + 4..].to_vec())
    }

    fn json(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<(&str, &[u8])>,
    ) -> (u16, serde_json::Value) {
        let (status, _, bytes) = self.call(method, path, token, body);
        let text = String::from_utf8_lossy(&bytes).to_string();
        (
            status,
            serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)),
        )
    }

    fn finish(mut self) {
        let status = self.child.wait().unwrap();
        assert!(status.success(), "nils serve exited {status}");
    }
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn registry() -> TempDir {
    let home = TempDir::new("derivatives-home");
    let (good, _, err) = run(
        &home,
        &["key", "add", "k"],
        Some("a derivatives test key\n"),
    );
    assert!(good, "{err}");
    ok(&home, &["init", "--key", "k"]);
    ok(&home, &["synth", "--seed", "5", "--subjects", "2"]);
    home
}

#[test]
fn a_derivative_goes_round_and_is_refused_without_its_grant_or_its_place() {
    let home = registry();
    let mask: Vec<u8> = (0u32..70_000).map(|i| (i * 31 % 251) as u8).collect();
    let digest = sha256(&mask);
    let upload = format!("/api/derivatives?kind=mask&stack=1&sha256={digest}&name=m.nii.gz");

    // No working place: the capability is off, and the door says so before
    // it reads a byte.
    let server = Server::start(&home, 2);
    let (status, caps) = server.json("GET", "/api/capabilities", OPERATOR, None);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["derivatives"]["enabled"], false, "{caps}");
    assert!(
        caps["derivatives"]["reason"]
            .as_str()
            .unwrap()
            .contains("no working place is bound"),
        "{caps}"
    );
    let (status, doc) = server.json(
        "POST",
        &upload,
        OPERATOR,
        Some(("application/octet-stream", &mask)),
    );
    assert_eq!(status, 409, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap()
            .contains("derivatives are off"),
        "{doc}"
    );
    server.finish();
    let (good, _, err) = run(
        &home,
        &[
            "derivative",
            "add",
            "/dev/null",
            "--kind",
            "mask",
            "--stack",
            "1",
        ],
        None,
    );
    assert!(!good);
    assert!(err.contains("derivatives are off"), "{err}");

    // A working place turns it on.
    let work = TempDir::new("derivatives-work");
    ok(
        &home,
        &[
            "place",
            "add",
            "scratch",
            work.path().to_str().unwrap(),
            "--role",
            "working",
            "--fast",
        ],
    );
    let server = Server::start(&home, 9);
    let (_, caps) = server.json("GET", "/api/capabilities", OPERATOR, None);
    assert_eq!(caps["derivatives"]["enabled"], true, "{caps}");
    assert_eq!(caps["derivatives"]["place"], "scratch", "{caps}");
    assert!(
        caps["doors"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("POST /api/derivatives")),
        "{caps}"
    );

    // Without the grant: a reader holds no pipelines grant, a reviewer
    // sees but does not register.
    let body = Some(("application/x-nifti", mask.as_slice()));
    let (status, doc) = server.json("POST", &upload, READER, body);
    assert_eq!(status, 403, "{doc}");
    let (status, doc) = server.json("POST", &upload, REVIEWER, body);
    assert_eq!(status, 403, "{doc}");
    assert!(
        doc["error"].as_str().unwrap().contains("pipelines:work"),
        "{doc}"
    );

    // A body whose digest is not the one named keeps nothing.
    let wrong = format!(
        "/api/derivatives?kind=mask&stack=1&sha256={}",
        "0".repeat(64)
    );
    let (status, doc) = server.json("POST", &wrong, OPERATOR, body);
    assert_eq!(status, 422, "{doc}");
    assert!(doc["error"].as_str().unwrap().contains(&digest), "{doc}");

    // The round trip.
    let (status, made) = server.json("POST", &upload, OPERATOR, body);
    assert_eq!(status, 201, "{made}");
    assert_eq!(made["sha256"], digest.as_str(), "{made}");
    assert_eq!(made["bytes"], mask.len() as i64, "{made}");
    assert_eq!(made["kind"], "mask");
    assert_eq!(made["scope"], "stack");
    assert_eq!(made["stack_id"], 1);
    assert_eq!(made["media_type"], "application/x-nifti");
    assert_eq!(made["place"], "scratch");
    assert_eq!(made["registered_by"], "ops@lab");
    assert_eq!(made["model_id"], serde_json::Value::Null);
    let path = made["path"].as_str().unwrap();
    assert_eq!(
        path,
        format!("derivatives/mask/{}/{digest}.nii.gz", &digest[..2])
    );
    assert_eq!(std::fs::read(work.path().join(path)).unwrap(), mask);
    let id = made["id"].as_i64().unwrap();

    let (status, shown) = server.json("GET", &format!("/api/derivatives/{id}"), REVIEWER, None);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["sha256"], digest.as_str());
    let (status, headers, bytes) = server.call(
        "GET",
        &format!("/api/derivatives/{id}/content"),
        REVIEWER,
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(bytes, mask, "the bytes come back as they went in");
    assert_eq!(sha256(&bytes), digest);
    assert_eq!(header(&headers, "x-nils-sha256"), Some(digest.as_str()));
    assert_eq!(
        header(&headers, "content-type"),
        Some("application/x-nifti")
    );
    let (status, doc) = server.json(
        "GET",
        &format!("/api/derivatives/{id}/content"),
        READER,
        None,
    );
    assert_eq!(status, 403, "{doc}");
    let (status, listed) = server.json("GET", "/api/derivatives?stack=1", REVIEWER, None);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(
        listed["derivatives"].as_array().unwrap().len(),
        1,
        "{listed}"
    );
    server.finish();

    // Only the registered file is in the place: the refused ones left nothing.
    let files = walk(&work.path().join("derivatives"));
    assert_eq!(files, [path.to_string()], "{files:?}");

    // One registration and one download in the audit log.
    let audit: serde_json::Value = serde_json::from_str(&ok(
        &home,
        &["audit", "list", "--action", "derivative.", "--json"],
    ))
    .unwrap();
    let text = audit.to_string();
    assert_eq!(text.matches("derivative.register").count(), 1, "{text}");
    assert_eq!(text.matches("derivative.read").count(), 1, "{text}");

    // The command line: the same row, a second one superseding it, and
    // the custody row counting both.
    let listed: serde_json::Value =
        serde_json::from_str(&ok(&home, &["derivative", "list", "--json"])).unwrap();
    assert_eq!(listed[0]["id"], id);
    let file = work.path().join("again.nii.gz");
    std::fs::write(&file, b"a corrected mask").unwrap();
    let added: serde_json::Value = serde_json::from_str(&ok(
        &home,
        &[
            "derivative",
            "add",
            file.to_str().unwrap(),
            "--kind",
            "mask",
            "--stack",
            "1",
            "--supersedes",
            &id.to_string(),
            "--sha256",
            &sha256(b"a corrected mask"),
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(added["supersedes_id"], id, "{added}");
    assert_eq!(added["registered_by"], "anna@ward-3", "{added}");
    let shown = ok(&home, &["derivative", "show", &id.to_string()]);
    assert!(shown.contains(&digest), "{shown}");
    let (good, _, err) = run(
        &home,
        &[
            "derivative",
            "add",
            file.to_str().unwrap(),
            "--kind",
            "embedding",
            "--stack",
            "1",
            "--supersedes",
            &id.to_string(),
        ],
        None,
    );
    assert!(!good);
    assert!(err.contains("does not supersede"), "{err}");

    // A derivative a model made names it in the model table: an unknown
    // model is refused before a byte is kept, a registered one is kept on
    // the row by id.
    let (good, _, err) = run(
        &home,
        &[
            "derivative",
            "add",
            file.to_str().unwrap(),
            "--kind",
            "mask",
            "--stack",
            "1",
            "--model",
            "nobody@1",
        ],
        None,
    );
    assert!(!good);
    assert!(
        err.contains("no registered model answers to nobody@1"),
        "{err}"
    );
    let weights = work.path().join("segmenter.onnx");
    std::fs::write(&weights, b"not really a segmenter").unwrap();
    let card = work.path().join("segmenter.json");
    std::fs::write(
        &card,
        r#"{"name": "seg", "version": "1", "kind": "segmenter", "task": "segment:brain"}"#,
    )
    .unwrap();
    let model: serde_json::Value = serde_json::from_str(&ok(
        &home,
        &[
            "model",
            "register",
            "--card",
            card.to_str().unwrap(),
            "--artifact",
            weights.to_str().unwrap(),
            "--json",
        ],
    ))
    .unwrap();
    let by_model = work.path().join("by-model.nii.gz");
    std::fs::write(&by_model, b"a model's mask").unwrap();
    let made: serde_json::Value = serde_json::from_str(&ok(
        &home,
        &[
            "derivative",
            "add",
            by_model.to_str().unwrap(),
            "--kind",
            "mask",
            "--stack",
            "1",
            "--model",
            "seg@1",
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(made["model_id"], model["id"], "{made}");
    let custody: serde_json::Value =
        serde_json::from_str(&ok(&home, &["custody", "--json"])).unwrap();
    let row = custody["stores"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["store"] == "derivatives")
        .unwrap();
    assert_eq!(row["counts"]["derivatives"], 3, "{row}");
    assert_eq!(
        row["counts"]["bytes"],
        (mask.len() + b"a corrected mask".len() + b"a model's mask".len()) as i64,
        "{row}"
    );
}

fn walk(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(
                    p.strip_prefix(dir.parent().unwrap())
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    out.sort();
    out
}
