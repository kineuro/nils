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

/// Three subjects with scans: their codes and stack ids, oldest first.
fn subjects(home: &TempDir, pg: Option<(&str, &str)>) -> Vec<(i64, String, Vec<i64>)> {
    let mut s = store(home, pg);
    let sql = format!(
        "SELECT su.id, su.code, st.id FROM {} su JOIN {} se ON se.subject_id = su.id \
         JOIN {} st ON st.series_id = se.id WHERE su.merged_into IS NULL ORDER BY su.id, st.id",
        s.qualified("subject"),
        s.qualified("series"),
        s.qualified("stack"),
    );
    let mut out: Vec<(i64, String, Vec<i64>)> = Vec::new();
    for r in s.query(&sql, &[]).unwrap() {
        let (id, code, stack) = (
            r.int(0).unwrap(),
            r.text(1).unwrap().to_string(),
            r.int(2).unwrap(),
        );
        match out.last_mut() {
            Some(last) if last.0 == id => last.2.push(stack),
            _ => out.push((id, code, vec![stack])),
        }
    }
    out.truncate(3);
    out
}

/// A dataset `controls` whose tree holds a file of each of these stacks;
/// and every stack of another subject sealed in a sample now.
fn plant(home: &TempDir, pg: Option<(&str, &str)>, held: &[i64], sealed: (i64, &[i64])) {
    use nils_registry::place::{self, New, Role};
    use nils_registry::schema::table;
    use nils_registry::{Insert, Param};
    let dir = home.path().join("controls");
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap().display().to_string();
    let mut s = store(home, pg);
    place::add(
        &mut s,
        &New {
            name: "controls",
            role: Role::Source,
            path: &root,
            guarantees: serde_json::json!({}),
            probed: serde_json::Value::Null,
            handling: serde_json::Value::Null,
            dataset: serde_json::Value::Null,
        },
    )
    .unwrap();
    let now = "2026-10-15T08:00:00Z";
    let source = s
        .insert(
            &Insert::new(
                table("source"),
                &["root", "root_canonical", "first_seen_at"],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(root.as_str()),
                Param::from(root.as_str()),
                Param::from(now),
            ]],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    let rows: Vec<Vec<Param>> = held
        .iter()
        .map(|st| vec![Param::Int(source), Param::Int(*st), Param::from(now)])
        .collect();
    s.insert(
        &Insert::new(
            table("source_stack"),
            &["source_id", "stack_id", "first_seen_at"],
        ),
        &rows,
    )
    .unwrap();
    let (subject, stacks) = sealed;
    let rows: Vec<Vec<Param>> = stacks
        .iter()
        .map(|st| {
            vec![
                Param::from("cert-sample@1"),
                Param::Int(*st),
                Param::Int(subject),
                Param::from("anna"),
                Param::from(now),
            ]
        })
        .collect();
    s.insert(
        &Insert::new(
            table("sealed_stack"),
            &["sample", "stack_id", "subject_id", "sealed_by", "sealed_at"],
        ),
        &rows,
    )
    .unwrap();
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
        ],
        None,
    );
    std::fs::remove_file(&csv).unwrap();
}

fn the_door_on(backend: &str, pg: Option<(&str, &str)>) {
    let home = synthetic(pg);
    let three = subjects(&home, pg);
    let (a, b, c) = (&three[0], &three[1], &three[2]);
    plant(&home, pg, &a.2[..1], (c.0, &c.2));
    // the site's ctrl-id map: Ctrl0042 is subject a, Ctrl0043 a subject the
    // map makes and no scan is of, Ctrl0044 subject b, later merged into a,
    // and Ctrl0045 subject c, every scan of whom is sealed now
    run(&home, &["linkage", "id-type", "add", "ctrl-id"], None);
    map(
        &home,
        "ctrl-id",
        &[
            ("Ctrl0042", &a.1),
            ("Ctrl0043", "ctrl-only-0012"),
            ("Ctrl0044", &b.1),
            ("Ctrl0045", &c.1),
        ],
    );
    // and an ID that identifies a person anywhere, as PatientID held it
    map(&home, "patient-id", &[("PX-0077", &a.1)]);
    run(
        &home,
        &[
            "linkage",
            "merge",
            &a.1,
            &b.1,
            "--why",
            "one person, two codes",
        ],
        None,
    );

    let server = Server::start(&home, 13);
    let ask = |value: &str| serde_json::json!({"id_type": "ctrl-id", "value": value});
    // the admin asks Nima's question's first step: Ctrl0042 is subject a
    let (status, found) = server.post("/api/linkage/resolve", &ask("Ctrl0042"), ADMIN);
    assert_eq!(status, 200, "{backend}: {found}");
    assert_eq!(found["known"], true, "{backend}: {found}");
    assert_eq!(found["subject_code"], a.1.as_str(), "{backend}: {found}");
    // a's own scans and the merged b's
    assert_eq!(found["scans"], a.2.len() + b.2.len(), "{backend}: {found}");
    assert_eq!(found["id_type"], "ctrl-id", "{backend}: {found}");
    // the dataset whose tree holds a file of one of them is named
    assert_eq!(
        found["datasets"],
        serde_json::json!(["controls"]),
        "{backend}: {found}"
    );
    // never an identifier it was not given, nor the value back
    assert!(!found.to_string().contains("Ctrl"), "{backend}: {found}");
    // a reviewer, who uses the data at detail quasi, gets the same
    let (status, same) = server.post("/api/linkage/resolve", &ask("  Ctrl0042 "), REVIEWER);
    assert_eq!(status, 200, "{backend}: {same}");
    assert_eq!(same["subject_code"], a.1.as_str(), "{backend}: {same}");
    assert_eq!(
        same["datasets"],
        serde_json::json!(["controls"]),
        "{backend}"
    );
    // a merged subject answers the one it was merged into
    let (status, merged) = server.post("/api/linkage/resolve", &ask("Ctrl0044"), ADMIN);
    assert_eq!(status, 200, "{backend}: {merged}");
    assert_eq!(merged["subject_code"], a.1.as_str(), "{backend}: {merged}");
    // a subject every scan of whom is sealed now is not known without the
    // unsealed grant, which no admin holds
    let (status, sealed) = server.post("/api/linkage/resolve", &ask("Ctrl0045"), ADMIN);
    assert_eq!(status, 200, "{backend}: {sealed}");
    assert_eq!(
        sealed,
        serde_json::json!({"id_type": "ctrl-id", "known": false}),
        "{backend}"
    );
    // a value no map names, and one whose subject holds no scan: not known
    let (status, none) = server.post("/api/linkage/resolve", &ask("Ctrl9999"), ADMIN);
    assert_eq!(status, 200, "{backend}: {none}");
    assert_eq!(none["known"], false, "{backend}: {none}");
    let (status, bare) = server.post("/api/linkage/resolve", &ask("Ctrl0043"), ADMIN);
    assert_eq!(status, 200, "{backend}: {bare}");
    assert_eq!(bare["known"], false, "{backend}: {bare}");
    // an ID that identifies a person anywhere is a re-identification: a
    // reviewer at quasi is refused, an admin, who holds everything, is not
    let pid = serde_json::json!({"id_type": "patient-id", "value": "PX-0077"});
    let (status, refused) = server.post("/api/linkage/resolve", &pid, REVIEWER);
    assert_eq!(status, 403, "{backend}: {refused}");
    assert!(!refused.to_string().contains("PX-0077"), "{backend}");
    let (status, admin) = server.post("/api/linkage/resolve", &pid, ADMIN);
    assert_eq!(status, 200, "{backend}: {admin}");
    assert_eq!(admin["subject_code"], a.1.as_str(), "{backend}: {admin}");
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
    // a value that is no ID, and one longer than an ID, counted in characters
    let (status, _) = server.post(
        "/api/linkage/resolve",
        &serde_json::json!({"id_type": "ctrl-id"}),
        ADMIN,
    );
    assert_eq!(status, 400, "{backend}");
    let (status, _) = server.post("/api/linkage/resolve", &ask(&"Ö".repeat(200)), ADMIN);
    assert_eq!(
        status, 200,
        "{backend}: 200 characters are 400 bytes and one ID"
    );
    server.finish();

    // every lookup that reached the store is audited, found or not: seven
    // ctrl-id lookups and the admin's patient-id one
    let mut linkage = match pg {
        Some((dsn, schema)) => Store::connect_postgres(dsn, &format!("{schema}_linkage")).unwrap(),
        None => Store::open_sqlite(&home.path().join("linkage.db")).unwrap(),
    };
    let read = linkage
        .query(
            &format!(
                "SELECT why, COUNT(*) FROM {} WHERE why LIKE 'resolve:%' GROUP BY why ORDER BY why",
                linkage.qualified("read_audit")
            ),
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| (r.text(0).unwrap().to_string(), r.int(1).unwrap()))
        .collect::<Vec<_>>();
    drop(linkage);
    assert_eq!(
        read,
        [
            ("resolve: found".to_string(), 4),
            ("resolve: no scan to see".to_string(), 2),
            ("resolve: not known".to_string(), 2),
        ],
        "{backend}"
    );
    let mut s = store(&home, pg);
    let logged = s
        .query(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE action = 'linkage.resolve'",
                s.qualified("audit")
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    drop(s);
    assert_eq!(logged, 8, "{backend}");
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
