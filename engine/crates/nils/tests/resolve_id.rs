// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55, ruling 24: resolving an ID to its subject code is one lookup.
//! A person who may use the data names a subject by an ID of a known type
//! (a study's own ID, `Ctrl0042`) and `POST /api/linkage/resolve` answers
//! the subject code, or that it is not known; it never answers an
//! identifier it was not given. The question that asked for it, "how many
//! SWI acquisition types does Ctrl0042 have?", is then a question of that
//! subject code. On SQLite always, and on Postgres where a test DSN is set.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;
use nils_registry::Store;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
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

fn drop_schema(dsn: &str, schema: &str) {
    if let Ok(mut store) = Store::connect_postgres(dsn, "public") {
        let _ = store.batch(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
        ));
    }
}

/// A synthetic registry with its sessions built, on one backend.
fn synthetic(pg: Option<(&str, &str)>) -> TempDir {
    let home = TempDir::new("resolve-id-home");
    run(&home, &["key", "add", "k"], Some("a resolve test key\n"));
    match pg {
        Some((dsn, schema)) => {
            drop_schema(dsn, schema);
            run(
                &home,
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
                None,
            );
        }
        None => {
            run(&home, &["init", "--key", "k"], None);
        }
    }
    run(&home, &["synth", "--seed", "11", "--subjects", "12"], None);
    run(&home, &["session", "rebuild"], None);
    home
}

fn store(home: &TempDir, pg: Option<(&str, &str)>) -> Store {
    match pg {
        Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
        None => Store::open_sqlite(&home.path().join("registry.db")).unwrap(),
    }
}

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start(home: &TempDir, requests: usize) -> Server {
        let child = nils()
            .arg("--registry")
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
            .args([
                "--auth",
                "token",
                "--token",
                "an-admin-token-of-length=chief@lab:admin",
                "--token",
                "a-reviewer-token-of-leng=rita@lab:reviewer",
                "--token",
                "a-reader-token-of-length=rolf@lab:reader",
                "--token",
                "a-rater-token-of-lengthx=rae@lab:campaigns:work",
            ])
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut held = Server { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let first = BufReader::new(stdout).lines().next().unwrap().unwrap();
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        held
    }

    fn post(&self, path: &str, body: &serde_json::Value, token: &str) -> (u16, serde_json::Value) {
        let body = body.to_string();
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = headers
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
                    panic!("nils serve still waiting for requests: the count is wrong");
                }
                None => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
    }
}

const ADMIN: &str = "an-admin-token-of-length";
const REVIEWER: &str = "a-reviewer-token-of-leng";
const READER: &str = "a-reader-token-of-length";
const RATER: &str = "a-rater-token-of-lengthx";

fn the_door_on(backend: &str, pg: Option<(&str, &str)>) {
    let home = synthetic(pg);
    // a subject with scans, and how many stacks it holds
    let (code, stacks) = {
        let mut s = store(&home, pg);
        let sql = format!(
            "SELECT su.code, COUNT(st.id) FROM {} su JOIN {} se ON se.subject_id = su.id \
             JOIN {} st ON st.series_id = se.id WHERE su.merged_into IS NULL \
             GROUP BY su.id, su.code ORDER BY su.id",
            s.qualified("subject"),
            s.qualified("series"),
            s.qualified("stack"),
        );
        let rows = s.query(&sql, &[]).unwrap();
        (
            rows[0].text(0).unwrap().to_string(),
            rows[0].int(1).unwrap(),
        )
    };
    // the site's ctrl-id map: Ctrl0042 is that subject; Ctrl0043 names a
    // subject the map makes and no scan is of
    run(&home, &["linkage", "id-type", "add", "ctrl-id"], None);
    let map = home.path().join("ctrl-map.csv");
    std::fs::write(
        &map,
        format!("identifier,code\nCtrl0042,{code}\nCtrl0043,ctrl-only-0012\n"),
    )
    .unwrap();
    run(
        &home,
        &[
            "linkage",
            "import",
            map.to_str().unwrap(),
            "--id-type",
            "ctrl-id",
        ],
        None,
    );
    std::fs::remove_file(&map).unwrap();

    let server = Server::start(&home, 8);
    let ask = |value: &str| serde_json::json!({"id_type": "ctrl-id", "value": value});
    // the admin asks Nima's question's first step: Ctrl0042 is that subject
    let (status, found) = server.post("/api/linkage/resolve", &ask("Ctrl0042"), ADMIN);
    assert_eq!(status, 200, "{backend}: {found}");
    assert_eq!(found["known"], true, "{backend}: {found}");
    assert_eq!(found["subject_code"], code.as_str(), "{backend}: {found}");
    assert_eq!(found["scans"], stacks, "{backend}: {found}");
    assert_eq!(found["id_type"], "ctrl-id", "{backend}: {found}");
    // never an identifier it was not given, nor the value back
    assert!(!found.to_string().contains("Ctrl"), "{backend}: {found}");
    // a reviewer, who uses the data at detail quasi, gets the same
    let (status, same) = server.post("/api/linkage/resolve", &ask("  Ctrl0042 "), REVIEWER);
    assert_eq!(status, 200, "{backend}: {same}");
    assert_eq!(same["subject_code"], code.as_str(), "{backend}: {same}");
    // a value no map names, and one whose subject holds no scan: not known
    let (status, none) = server.post("/api/linkage/resolve", &ask("Ctrl9999"), ADMIN);
    assert_eq!(status, 200, "{backend}: {none}");
    assert_eq!(
        none,
        serde_json::json!({"id_type": "ctrl-id", "known": false}),
        "{backend}"
    );
    let (status, bare) = server.post("/api/linkage/resolve", &ask("Ctrl0043"), ADMIN);
    assert_eq!(status, 200, "{backend}: {bare}");
    assert_eq!(bare["known"], false, "{backend}: {bare}");
    // an ID type that is not there names the types
    let (status, why) = server.post(
        "/api/linkage/resolve",
        &serde_json::json!({"id_type": "study-id", "value": "B1"}),
        ADMIN,
    );
    assert_eq!(status, 404, "{backend}: {why}");
    assert!(why.to_string().contains("ctrl-id"), "{backend}: {why}");
    // a subject code is quasi-identifying: a plain reader is refused, and so
    // is a rater who may use no data
    let (status, _) = server.post("/api/linkage/resolve", &ask("Ctrl0042"), READER);
    assert_eq!(status, 403, "{backend}");
    let (status, _) = server.post("/api/linkage/resolve", &ask("Ctrl0042"), RATER);
    assert_eq!(status, 403, "{backend}");
    // a value that is no ID
    let (status, _) = server.post(
        "/api/linkage/resolve",
        &serde_json::json!({"id_type": "ctrl-id"}),
        ADMIN,
    );
    assert_eq!(status, 400, "{backend}");
    server.finish();

    // each answer that found a subject left one read audit row
    let mut linkage = match pg {
        Some((dsn, schema)) => Store::connect_postgres(dsn, &format!("{schema}_linkage")).unwrap(),
        None => Store::open_sqlite(&home.path().join("linkage.db")).unwrap(),
    };
    let audited = linkage
        .query(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE why = 'resolve an ID to its subject code'",
                linkage.qualified("read_audit")
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    drop(linkage);
    assert_eq!(audited, 2, "{backend}");
    if let Some((dsn, schema)) = pg {
        drop_schema(dsn, schema);
    }
}

#[test]
fn an_id_resolves_to_its_subject_code_for_those_who_use_the_data() {
    the_door_on("sqlite", None);
    match std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    {
        Some(dsn) => the_door_on("postgres", Some((&dsn, "nils_resolve_id"))),
        None => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped"),
    }
}
