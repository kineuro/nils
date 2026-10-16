// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55, ruling 35 (c): every step that can live in the desk gets a
//! place in it. What `nils key`, `nils handover password` and `nils linkage
//! show`, `link`, `unlink` and `purge` do at the keyboard, at the door:
//! each refused to anyone but who may do it and to any agent or model,
//! each audited, a passphrase and a password never stored, logged, put in
//! an error or in the audit, and a value that reads as a personnummer never
//! answered.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;
use nils_registry::Store;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) -> String {
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
    match stdin {
        Some(text) => child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap(),
        None => drop(child.stdin.take()),
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

/// The registry's own key, and the passphrase of a handover's key.
const REGISTRY_KEY: &str = "a keys test key";
const SECRET: &str = "correct horse battery staple 77";

const ADMIN: &str = "an-admin-token-of-length";
const OPERATOR: &str = "an-operator-token-of-len";
const REVIEWER: &str = "a-reviewer-token-of-leng";
const AGENT: &str = "X-Nils-Actor: {\"kind\": \"agent\", \"name\": \"helper\"}\r\n";

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        // only the child this test started, by its own handle
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Answer {
    status: u16,
    headers: String,
    body: serde_json::Value,
    text: String,
}

impl Server {
    fn start(home: &TempDir, log: &Path) -> Server {
        let err = std::fs::File::create(log).unwrap();
        let child = nils()
            .arg("--registry")
            .arg(home.path())
            .args(["serve", "--bind", "127.0.0.1:0", "--workers", "1"])
            .args([
                "--auth",
                "token",
                "--token",
                "an-admin-token-of-length=chief@lab:admin",
                "--token",
                "an-operator-token-of-len=olle@lab:operator",
                "--token",
                "a-reviewer-token-of-leng=rita@lab:reviewer",
            ])
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .stdout(Stdio::piped())
            .stderr(Stdio::from(err))
            .spawn()
            .unwrap();
        let mut held = Server { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let first = lines.next().unwrap().unwrap();
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        // whatever the engine prints after its first line goes to a file
        // the test reads at the end
        let out = log.with_extension("out");
        std::thread::spawn(move || {
            let mut f = std::fs::File::create(out).unwrap();
            for l in lines.map_while(Result::ok) {
                let _ = writeln!(f, "{l}");
            }
        });
        held
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        token: &str,
        extra: &str,
    ) -> Answer {
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n{extra}\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        Answer {
            status,
            headers: headers.to_ascii_lowercase(),
            body: serde_json::from_str(text).unwrap_or(serde_json::Value::Null),
            text: text.to_string(),
        }
    }

    fn post(&self, path: &str, body: serde_json::Value, token: &str) -> Answer {
        self.call("POST", path, Some(body), token, "")
    }

    fn get(&self, path: &str, token: &str) -> Answer {
        self.call("GET", path, None, token, "")
    }
}

/// Every byte under a directory, file by file, but the key store's.
fn every_file(dir: &Path, out: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "keys") {
                continue;
            }
            every_file(&p, out);
        } else if let Ok(bytes) = std::fs::read(&p) {
            out.push((p, bytes));
        }
    }
}

fn holds(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// Two subjects' codes, oldest first.
fn two_subjects(home: &TempDir) -> (String, String) {
    let mut s = Store::open_sqlite(&home.path().join("registry.db")).unwrap();
    let sql = format!(
        "SELECT code FROM {} WHERE merged_into IS NULL ORDER BY id LIMIT 2",
        s.qualified("subject")
    );
    let rows = s.query(&sql, &[]).unwrap();
    (
        rows[0].text(0).unwrap().to_string(),
        rows[1].text(0).unwrap().to_string(),
    )
}

fn map(home: &TempDir, id_type: &str, rows: &[(&str, &str)]) {
    let csv = home.path().join("map.csv");
    let body: String = rows
        .iter()
        .map(|(id, code)| format!("{id},{code}\n"))
        .collect();
    std::fs::write(&csv, format!("identifier,code\n{body}")).unwrap();
    run(
        home,
        &[
            "linkage",
            "import",
            csv.to_str().unwrap(),
            "--id-type",
            id_type,
            "--make-types",
        ],
        None,
    );
    std::fs::remove_file(&csv).unwrap();
}

#[test]
fn keys_and_identifiers_at_the_door_are_an_admins_acts_and_no_secret_is_kept() {
    let home = TempDir::new("keys-doors-home");
    run(
        &home,
        &["key", "add", "k"],
        Some(&format!("{REGISTRY_KEY}\n")),
    );
    run(&home, &["init", "--key", "k"], None);
    run(&home, &["synth", "--seed", "5", "--subjects", "4"], None);
    let (first, second) = two_subjects(&home);
    // a hospital ID that is a personnummer in its content, a study's own ID,
    // and an ID that is the same everywhere, which is never kept
    map(&home, "patient-id", &[("198501012382", &first)]);
    map(&home, "study-id", &[("STUDY-0042", &first)]);
    map(&home, "personnummer", &[("198501012382", &second)]);
    let logs = TempDir::new("keys-doors-logs");
    let log = logs.path().join("serve.err");
    let server = Server::start(&home, &log);

    // --- the key store -------------------------------------------------
    let a = server.get("/api/keys", REVIEWER);
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.get("/api/keys", ADMIN);
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.body["pseudonym_key"], "k", "{}", a.text);
    assert_eq!(a.body["present"], true, "{}", a.text);
    assert_eq!(a.body["keys"][0]["in_use"], true, "{}", a.text);
    assert_eq!(
        a.body["keys"][0]["fingerprint"],
        nils_registry::pseudonym::fingerprint(REGISTRY_KEY.as_bytes()),
        "{}",
        a.text
    );
    let add = serde_json::json!({ "name": "partner-2026", "passphrase": SECRET });
    // an operator works on the data, not on the key store
    let a = server.post("/api/keys", add.clone(), OPERATOR);
    assert_eq!(a.status, 403, "{}", a.text);
    // an agent holds no secret, whatever its grants
    let a = server.call("POST", "/api/keys", Some(add.clone()), ADMIN, AGENT);
    assert_eq!(a.status, 403, "{}", a.text);
    assert!(a.text.contains("a person's act"), "{}", a.text);
    let a = server.post("/api/keys", add.clone(), ADMIN);
    assert_eq!(a.status, 201, "{}", a.text);
    assert!(
        !a.text.contains(SECRET),
        "the passphrase came back: {}",
        a.text
    );
    assert!(
        a.headers.contains("cache-control: no-store"),
        "{}",
        a.headers
    );
    assert_eq!(a.body["bytes"], SECRET.len(), "{}", a.text);
    assert_eq!(
        a.body["fingerprint"],
        nils_registry::pseudonym::fingerprint(SECRET.as_bytes()),
        "{}",
        a.text
    );
    assert_eq!(a.body["in_use"], false, "{}", a.text);
    // the key is the passphrase's bytes, in its own file, as key add makes it
    assert_eq!(
        std::fs::read(home.path().join("keys/partner-2026")).unwrap(),
        SECRET.as_bytes()
    );
    // a key is never replaced; a refusal never quotes the passphrase
    let a = server.post("/api/keys", add.clone(), ADMIN);
    assert_eq!(a.status, 409, "{}", a.text);
    assert!(!a.text.contains(SECRET), "{}", a.text);
    let long = format!("{SECRET}{}", "x".repeat(60));
    let a = server.post(
        "/api/keys",
        serde_json::json!({ "name": "too-long", "passphrase": long }),
        ADMIN,
    );
    assert_eq!(a.status, 400, "{}", a.text);
    assert!(!a.text.contains(SECRET), "{}", a.text);
    let a = server.post("/api/keys", serde_json::json!({ "name": "no-pass" }), ADMIN);
    assert_eq!(a.status, 400, "{}", a.text);

    // after a restore without its key, the pseudonym key is made again from
    // its passphrase, and a wrong one is refused with nothing kept
    let kept_key = home.path().join("keys/k");
    std::fs::rename(&kept_key, logs.path().join("k.away")).unwrap();
    let a = server.get("/api/keys", ADMIN);
    assert_eq!(a.body["present"], false, "{}", a.text);
    let a = server.post(
        "/api/keys",
        serde_json::json!({ "name": "k", "passphrase": "not the registry's key" }),
        ADMIN,
    );
    assert_eq!(a.status, 409, "{}", a.text);
    assert!(a.text.contains("does not open"), "{}", a.text);
    assert!(!a.text.contains("not the registry's key"), "{}", a.text);
    assert!(!kept_key.exists(), "a wrong passphrase was kept");
    let a = server.post(
        "/api/keys",
        serde_json::json!({ "name": "k", "passphrase": format!("{REGISTRY_KEY}\n") }),
        ADMIN,
    );
    assert_eq!(a.status, 201, "{}", a.text);
    assert_eq!(a.body["in_use"], true, "{}", a.text);
    assert!(
        a.body["checked"].as_str().unwrap().starts_with("it opens"),
        "{}",
        a.text
    );
    assert_eq!(std::fs::read(&kept_key).unwrap(), REGISTRY_KEY.as_bytes());

    // --- a handover's password -----------------------------------------
    let want = run(
        &home,
        &["handover", "password", "--key", "partner-2026"],
        None,
    );
    let want = want.trim();
    assert_eq!(want.len(), 64);
    let ask = serde_json::json!({ "key": "partner-2026" });
    let a = server.post("/api/handovers/password", ask.clone(), REVIEWER);
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.call(
        "POST",
        "/api/handovers/password",
        Some(ask.clone()),
        OPERATOR,
        AGENT,
    );
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.post(
        "/api/handovers/password",
        serde_json::json!({ "key": "nobody" }),
        OPERATOR,
    );
    assert_eq!(a.status, 404, "{}", a.text);
    let a = server.post("/api/handovers/password", ask.clone(), OPERATOR);
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.body["password"], want, "{}", a.text);
    assert!(
        a.headers.contains("cache-control: no-store"),
        "{}",
        a.headers
    );

    // --- a key taken away ----------------------------------------------
    let a = server.post(
        "/api/keys/partner-2026/remove",
        serde_json::json!({ "confirm": "partner" }),
        ADMIN,
    );
    assert_eq!(a.status, 400, "{}", a.text);
    let a = server.post(
        "/api/keys/k/remove",
        serde_json::json!({ "confirm": "k" }),
        ADMIN,
    );
    assert_eq!(a.status, 409, "the pseudonym key went: {}", a.text);
    let a = server.post(
        "/api/keys/partner-2026/remove",
        serde_json::json!({ "confirm": "partner-2026" }),
        OPERATOR,
    );
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.post(
        "/api/keys/partner-2026/remove",
        serde_json::json!({ "confirm": "partner-2026" }),
        ADMIN,
    );
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(!home.path().join("keys/partner-2026").exists());

    // --- a subject's identifiers ---------------------------------------
    let show = serde_json::json!({ "subject": first });
    let a = server.post("/api/linkage/show", show.clone(), OPERATOR);
    assert_eq!(a.status, 403, "an admin's act: {}", a.text);
    let a = server.call(
        "POST",
        "/api/linkage/show",
        Some(show.clone()),
        ADMIN,
        AGENT,
    );
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.post("/api/linkage/show", show.clone(), ADMIN);
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(
        a.headers.contains("cache-control: no-store"),
        "{}",
        a.headers
    );
    assert!(!a.text.contains("198501012382"), "{}", a.text);
    assert!(
        !a.text.contains("STUDY-0042"),
        "counts and types alone: {}",
        a.text
    );
    let ids = a.body["identifiers"].as_array().unwrap();
    assert!(ids.iter().all(|i| i.get("value").is_none()), "{}", a.text);
    assert!(
        ids.iter()
            .any(|i| i["id_type"] == "patient-id" && i["kept"] == true),
        "{}",
        a.text
    );
    let a = server.post(
        "/api/linkage/show",
        serde_json::json!({ "subject": first, "values": true }),
        ADMIN,
    );
    assert_eq!(a.status, 400, "values want a why: {}", a.text);
    let a = server.post(
        "/api/linkage/show",
        serde_json::json!({ "subject": first, "values": true, "why": "checking the study's own ID" }),
        ADMIN,
    );
    assert_eq!(a.status, 200, "{}", a.text);
    // the study's own ID is shown; the personnummer under patient-id is not
    assert!(a.text.contains("STUDY-0042"), "{}", a.text);
    assert!(!a.text.contains("198501012382"), "{}", a.text);
    assert!(!a.text.contains("19850101-2382"), "{}", a.text);
    let pid = a.body["identifiers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id_type"] == "patient-id")
        .unwrap()
        .clone();
    assert_eq!(pid["shape"], "999999999999", "{pid}");
    assert!(pid["withheld"].is_string(), "{pid}");
    assert!(pid.get("value").is_none(), "{pid}");
    assert_eq!(a.body["shown"], 1, "{}", a.text);
    assert_eq!(a.body["withheld"], 1, "{}", a.text);
    // an ID that is the same everywhere is never kept, so never shown
    let a = server.post(
        "/api/linkage/show",
        serde_json::json!({ "subject": second, "values": true, "why": "which IDs it holds" }),
        ADMIN,
    );
    assert_eq!(a.status, 200, "{}", a.text);
    let pn = a.body["identifiers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id_type"] == "personnummer")
        .unwrap()
        .clone();
    assert_eq!(pn["kept"], false, "{pn}");
    assert!(
        pn.get("value").is_none() && pn.get("shape").is_none(),
        "{pn}"
    );
    assert!(!a.text.contains("198501012382"), "{}", a.text);

    // --- a link, its unlink --------------------------------------------
    let a = server.post(
        "/api/linkage/links",
        serde_json::json!({ "a": first, "b": second, "evidence": "one person" }),
        OPERATOR,
    );
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.post(
        "/api/linkage/links",
        serde_json::json!({ "a": first, "b": first, "evidence": "one person" }),
        ADMIN,
    );
    assert_eq!(a.status, 400, "{}", a.text);
    let a = server.post(
        "/api/linkage/links",
        serde_json::json!({ "a": first, "b": second, "evidence": "one person, by the clinic's letter" }),
        ADMIN,
    );
    assert_eq!(a.status, 202, "{}", a.text);
    let linkage = a.body["linkage"].as_i64().unwrap();
    assert!(a.body["job"].as_i64().is_some(), "{}", a.text);
    let a = server.post("/api/linkage/show", show.clone(), ADMIN);
    let links = a.body["linkages"].as_array().unwrap();
    assert!(
        links
            .iter()
            .any(|l| l["id"] == linkage && l["role"] == "canonical" && l["other"] == second),
        "{}",
        a.text
    );
    let path = format!("/api/linkage/links/{linkage}/unlink");
    let a = server.post(&path, serde_json::json!({}), OPERATOR);
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.post(&path, serde_json::json!({}), ADMIN);
    assert_eq!(a.status, 200, "{}", a.text);
    let a = server.post(&path, serde_json::json!({}), ADMIN);
    assert_eq!(a.status, 404, "reversed once: {}", a.text);

    // --- a purge, typed --------------------------------------------------
    let a = server.post(
        "/api/linkage/purge",
        serde_json::json!({ "subject": first, "dry_run": true }),
        ADMIN,
    );
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.body["identities"], 2, "{}", a.text);
    assert_eq!(a.body["confirm"], first, "{}", a.text);
    let a = server.post(
        "/api/linkage/purge",
        serde_json::json!({ "subject": first, "confirm": "yes" }),
        ADMIN,
    );
    assert_eq!(a.status, 400, "{}", a.text);
    let a = server.post(
        "/api/linkage/purge",
        serde_json::json!({ "subject": first, "confirm": first }),
        OPERATOR,
    );
    assert_eq!(a.status, 403, "{}", a.text);
    let a = server.post(
        "/api/linkage/purge",
        serde_json::json!({ "subject": first, "confirm": first }),
        ADMIN,
    );
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.body["identities"], 2, "{}", a.text);
    let a = server.post("/api/linkage/show", show.clone(), ADMIN);
    assert_eq!(a.body["identifiers"], serde_json::json!([]), "{}", a.text);
    let a = server.post(
        "/api/linkage/purge",
        serde_json::json!({ "all": true, "confirm": "everyone" }),
        ADMIN,
    );
    assert_eq!(a.status, 400, "{}", a.text);

    // --- every act audited, and no secret in the audit ------------------
    let a = server.get("/api/audit?limit=200", ADMIN);
    assert_eq!(a.status, 200, "{}", a.text);
    for act in [
        "key.add",
        "key.remove",
        "handover.password",
        "linkage.reveal",
        "linkage.link",
        "linkage.unlink",
        "linkage.purge",
    ] {
        assert!(a.text.contains(act), "{act} not audited: {}", a.text);
    }
    assert!(
        a.text.contains("checking the study's own ID"),
        "the why: {}",
        a.text
    );
    assert!(!a.text.contains(SECRET), "{}", a.text);
    assert!(!a.text.contains(want), "{}", a.text);
    // the policy says which doors carry a secret
    let a = server.get("/api/capabilities", ADMIN);
    let row = |door: &str| {
        a.body["policy"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["door"] == door)
            .cloned()
            .unwrap_or_else(|| panic!("{door} has no policy row"))
    };
    assert_eq!(row("POST /api/keys")["secret"], true);
    assert_eq!(row("POST /api/handovers/password")["secret"], true);
    assert_eq!(row("POST /api/linkage/purge")["person_only"], true);
    drop(server);

    // --- no secret anywhere but the key's own file -----------------------
    let mut files = Vec::new();
    every_file(home.path(), &mut files);
    every_file(logs.path(), &mut files);
    assert!(files.iter().any(|(p, _)| p.ends_with("registry.db")));
    assert!(files.iter().any(|(p, _)| p == &log));
    for (p, bytes) in &files {
        for secret in [SECRET, want, "198501012382"] {
            if secret == "198501012382" && !p.ends_with("serve.err") && !p.ends_with("serve.out") {
                // the personnummer is a test input; only the engine's own
                // output is held to never print it
                continue;
            }
            assert!(
                !holds(bytes, secret),
                "{} holds a secret it must not",
                p.display()
            );
        }
    }
}
