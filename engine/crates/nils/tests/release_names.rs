// SPDX-License-Identifier: AGPL-3.0-only
//! A release names its subjects by an ID type, through the doors and the
//! command line. The rehearsal door answers the plan with the values only
//! for a caller who reads identifiers, and with counts and subject codes
//! otherwise; the release door rehearses before it queues and answers a
//! plan that is not ready with 409 and no job; a type that keeps no value,
//! a type nobody holds and a type that is not one are refused in words; a
//! type whose IDs identify a person names a release only for a caller who
//! may read identifiers. The released tree spells the names and never the
//! codes they stand for, the releases list and the history show the types
//! and the counts, and the choice a person made is kept for the release's
//! next version.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

/// Every grant, at detail sensitive.
const ADMIN: &str = "an-admin-token-of-length";
/// The release's own grants and nothing of the Data page: no identifier.
const RELEASER: &str = "a-releaser-token-of-leng";

fn run(home: &Path, args: &[&str]) -> Output {
    nils()
        .arg("--registry")
        .arg(home)
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .env("NILS_PACK_DIR", packs())
        .env_remove("NILS_DSN")
        .env_remove("NILS_JOB_ID")
        .env_remove("NILS_JOB_DETAIL")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(home: &Path, args: &[&str]) -> String {
    let out = run(home, args);
    assert!(
        out.status.success(),
        "nils {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A command that must fail: its exit code and what it said.
fn refused(home: &Path, args: &[&str]) -> (i32, String) {
    let out = run(home, args);
    assert!(!out.status.success(), "nils {} succeeded", args.join(" "));
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

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

impl Server {
    fn start(home: &Path, log: &Path) -> Server {
        let err = std::fs::File::create(log).unwrap();
        let child = nils()
            .arg("--registry")
            .arg(home)
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "1",
                "--worker",
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .args([
                "--auth",
                "token",
                "--token",
                "an-admin-token-of-length=chief@lab:admin",
                "--token",
                "a-releaser-token-of-leng=rita@lab:release:work,release:see,pipelines:see",
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
        let out = log.with_extension("out");
        std::thread::spawn(move || {
            let mut f = std::fs::File::create(out).unwrap();
            for l in lines.map_while(Result::ok) {
                let _ = writeln!(f, "{l}");
            }
        });
        held
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n\r\n",
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
        (status, serde_json::from_str(text).unwrap_or(Value::Null))
    }

    fn post(&self, path: &str, body: Value, token: &str) -> (u16, Value) {
        self.call("POST", path, Some(body), token)
    }

    /// Until the job is over, then its state.
    fn over(&self, job: i64) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let (_, body) = self.call("GET", &format!("/api/jobs/{job}"), None, ADMIN);
            let state = body["state"].as_str().unwrap_or("").to_string();
            if matches!(state.as_str(), "done" | "failed" | "cancelled") {
                return state;
            }
            assert!(std::time::Instant::now() < deadline, "job {job}: {body}");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

fn every_file(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            every_file(&p, out);
        } else if let Ok(bytes) = std::fs::read(&p) {
            out.push((p, bytes));
        }
    }
}

/// The files under `root` whose path or bytes spell `word`.
fn spelled(root: &Path, word: &str) -> Vec<String> {
    let mut files = Vec::new();
    every_file(root, &mut files);
    files
        .into_iter()
        .filter(|(p, bytes)| {
            p.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .contains(word)
                || bytes.windows(word.len()).any(|w| w == word.as_bytes())
        })
        .map(|(p, _)| p.display().to_string())
        .collect()
}

/// The subject folders of a released tree, sorted.
fn subject_dirs(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("sub-"))
        .collect();
    out.sort();
    out
}

/// These folder names, sorted as [`subject_dirs`] answers them.
fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

/// Files an ID under a type for the subject of a code, the way a site's own
/// list comes in.
fn file_id(home: &Path, work: &Path, id_type: &str, code: &str, value: &str) {
    let csv = work.join(format!("{id_type}-{code}-{}.csv", value.len()));
    std::fs::write(&csv, format!("code,id\n{code},{value}\n")).unwrap();
    let role = format!("id=identifier:{id_type}");
    ok(
        home,
        &[
            "linkage",
            "import",
            csv.to_str().unwrap(),
            "--column",
            "code=code",
            "--column",
            &role,
            "--make-types",
        ],
    );
}

#[test]
fn a_release_is_named_by_an_id_type_through_the_doors_and_the_command_line() {
    let home = TempDir::new("release-names-home");
    let work = TempDir::new("release-names-work");
    let home = home.path();
    let tree = work.path().join("tree");
    for i in 1..=3 {
        let study = format!("1.2.7.{i}");
        let sop = format!("{study}.1.1");
        let mut e = synth::minimal_mr(&study, &format!("{study}.1"), &sop);
        e.push(synth::text(tags::PATIENT_ID, VR::LO, &format!("P-00{i}")));
        e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
        let file = tree.join(&study).join(&sop);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, synth::part10(&MetaFields::mr(&sop), &e, true)).unwrap();
    }
    let key = work.path().join("key");
    std::fs::write(&key, "a release names test key\n").unwrap();
    let out = nils()
        .arg("--registry")
        .arg(home)
        .args(["key", "add", "k"])
        .stdin(Stdio::from(std::fs::File::open(&key).unwrap()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    ok(home, &["init", "--key", "k"]);
    ok(
        home,
        &[
            "digest",
            "--name",
            "a",
            "--no-private",
            tree.to_str().unwrap(),
        ],
    );

    // the three subjects, by their codes: the first holds one study ID, the
    // second two, the third none
    let codes: Vec<String> = {
        let mut store =
            nils_registry::store::Store::open_sqlite(&home.join("registry.db")).unwrap();
        store
            .query("SELECT code FROM subject ORDER BY id", &[])
            .unwrap()
            .iter()
            .map(|r| r.text(0).unwrap().to_string())
            .collect()
    };
    assert_eq!(codes.len(), 3, "{codes:?}");
    let (one, two, none) = (&codes[0], &codes[1], &codes[2]);
    file_id(home, work.path(), "study-id", one, "ST_001");
    file_id(home, work.path(), "study-id", two, "ST-002a");
    file_id(home, work.path(), "study-id", two, "ST-002bb");
    // a type whose IDs identify a person, and a type nobody holds a value of
    ok(
        home,
        &["linkage", "id-type", "add", "ward-id", "--identifying"],
    );
    file_id(home, work.path(), "ward-id", one, "W-17");
    ok(home, &["linkage", "id-type", "add", "empty-id"]);

    let server = Server::start(home, &work.path().join("serve.err"));
    let out_dir = work.path().join("out");
    let out_text = out_dir.display().to_string();

    // --- the rehearsal: the plan, with the values for who reads identifiers
    let asked = json!({"name": "r", "name_by": ["study-id"], "on_unknown": "write"});
    let (status, body) = server.post("/api/releases/names", asked.clone(), ADMIN);
    assert_eq!(status, 200, "{body}");
    let names = &body["names"];
    assert_eq!(names["by"], json!(["study-id"]), "{names}");
    assert_eq!(names["subjects"], 3, "{names}");
    assert_eq!(names["ready"], false, "{names}");
    assert_eq!(names["values"], "shown", "{names}");
    assert_eq!(names["code"], 1, "{names}");
    assert_eq!(names["types"][0]["subjects"], 1, "{names}");
    assert_eq!(names["types"][0]["identifying"], false, "{names}");
    let needs = names["needs_choice"].as_array().unwrap();
    assert_eq!(needs.len(), 1, "{names}");
    assert_eq!(needs[0]["subject"], json!(two), "{names}");
    assert_eq!(needs[0]["type"], "study-id", "{names}");
    assert_eq!(needs[0]["count"], 2, "{names}");
    let mut values: Vec<&str> = needs[0]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    values.sort();
    assert_eq!(values, ["ST-002a", "ST-002bb"], "{names}");

    // ... and counts and subject codes alone for who does not
    let (status, body) = server.post("/api/releases/names", asked.clone(), RELEASER);
    assert_eq!(status, 200, "{body}");
    let names = &body["names"];
    assert_eq!(names["values"], "withheld", "{names}");
    assert_eq!(names["needs_choice"][0]["subject"], json!(two), "{names}");
    assert_eq!(names["needs_choice"][0]["count"], 2, "{names}");
    assert!(names["needs_choice"][0].get("values").is_none(), "{names}");
    assert!(!body.to_string().contains("002"), "{body}");

    // a body that names by nothing has no plan
    let (status, body) = server.post("/api/releases/names", json!({"name": "r"}), RELEASER);
    assert_eq!((status, &body["names"]), (200, &Value::Null), "{body}");

    // --- what cannot name is refused in words, and nothing is queued
    for (by, word) in [
        (json!(["personnummer"]), "a personnummer is never kept"),
        (
            json!(["study-id", "personnummer"]),
            "a personnummer is never kept",
        ),
        (
            json!(["empty-id"]),
            "no subject holds a kept value of empty-id",
        ),
        (json!(["no-such-id"]), "no ID type is named so"),
        (json!(["study-id", "study-id"]), "in the list twice"),
    ] {
        for door in ["/api/releases/names", "/api/releases"] {
            let (status, body) = server.post(
                door,
                json!({"name": "r", "out": out_text, "name_by": by, "on_unknown": "write"}),
                ADMIN,
            );
            let said = body["error"].as_str().unwrap_or_default();
            assert_eq!(status, 400, "{door} {by}: {body}");
            assert!(said.contains(word), "{door} {by}: {said}");
            assert!(
                said.ends_with("Nothing was written."),
                "{door} {by}: {said}"
            );
            assert!(body.get("job").is_none(), "{body}");
        }
    }
    let (status, body) = server.post(
        "/api/releases/names",
        json!({"name": "r", "name_choices": [{"subject": two, "value": "ST-002a"}]}),
        ADMIN,
    );
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("names by no ID type"),
        "{body}"
    );
    let (status, body) = server.post(
        "/api/releases/names",
        json!({"name": "r", "name_by": ["study-id"], "name_choices": [{"subject": two}]}),
        ADMIN,
    );
    assert_eq!(status, 400, "{body}");
    // a choice of a value the subject does not hold names the subject and
    // never the value
    let (status, body) = server.post(
        "/api/releases/names",
        json!({"name": "r", "name_by": ["study-id"],
               "name_choices": [{"subject": two, "value": "ST-999"}]}),
        ADMIN,
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["disclosure"], "gated", "{body}");
    let said = body["error"].as_str().unwrap();
    assert!(
        said.contains(two.as_str()) && !said.contains("999"),
        "{said}"
    );

    // --- a type whose IDs identify a person names a release only for a
    // caller who may read identifiers
    let identifying = json!({"name": "w", "out": out_text, "name_by": ["ward-id"]});
    for door in ["/api/releases/names", "/api/releases"] {
        let (status, body) = server.post(door, identifying.clone(), RELEASER);
        assert_eq!(status, 403, "{door}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains("data:work"),
            "{body}"
        );
    }
    let (status, body) = server.post("/api/releases/names", identifying, ADMIN);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["names"]["types"][0]["identifying"], true, "{body}");
    assert_eq!(body["names"]["ready"], true, "{body}");

    // --- the release door rehearses before it queues: a plan that is not
    // ready is 409 with the plan, and no job
    let release = json!({
        "name": "r", "out": out_text, "name_by": ["study-id"], "on_unknown": "write",
    });
    let (status, body) = server.post("/api/releases", release.clone(), RELEASER);
    assert_eq!(status, 409, "{body}");
    assert!(body.get("job").is_none(), "{body}");
    assert_eq!(body["disclosure"], "gated", "{body}");
    assert_eq!(body["names"]["ready"], false, "{body}");
    assert_eq!(body["names"]["values"], "withheld", "{body}");
    let said = body["error"].as_str().unwrap();
    assert!(said.contains(two.as_str()), "{said}");
    assert!(said.ends_with("Nothing was written."), "{said}");
    assert!(!body.to_string().contains("002"), "{body}");
    assert!(!out_dir.exists(), "a refused release wrote");

    // with the choice it is queued, and the tree spells the names
    let mut chosen = release.clone();
    chosen["name_choices"] = json!([{"subject": two, "value": "ST-002bb"}]);
    let (status, body) = server.post("/api/releases", chosen, ADMIN);
    assert_eq!(status, 202, "{body}");
    let job = body["job"].as_i64().unwrap();
    assert_eq!(server.over(job), "done", "job {job}");
    assert_eq!(
        subject_dirs(&out_dir),
        sorted(vec![
            "sub-ST001".to_string(),
            "sub-ST002bb".to_string(),
            format!("sub-{none}")
        ]),
    );
    for code in [one, two] {
        assert_eq!(spelled(&out_dir, code), Vec::<String>::new(), "{code}");
    }
    assert!(!spelled(&out_dir, "ST002bb").is_empty());

    // --- the releases list says the types and the counts, never a name
    let (status, body) = server.call("GET", "/api/releases", None, RELEASER);
    assert_eq!(status, 200, "{body}");
    let row = &body["releases"][0];
    assert_eq!(row["name_by"], json!(["study-id"]), "{row}");
    assert_eq!(
        row["names"],
        json!({"types": [{"type": "study-id", "subjects": 2}], "code": 1, "chosen": 1}),
        "{row}"
    );
    assert!(!body.to_string().contains("ST002"), "{body}");
    drop(server);

    // --- the command line: the kept choice answers the rehearsal of the
    // next version, which is written without asking again
    let plan: Value = serde_json::from_str(&ok(
        home,
        &[
            "release",
            "--name",
            "r",
            "--name-by",
            "study-id",
            "--name-plan",
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(plan["names"]["ready"], true, "{plan}");
    assert_eq!(plan["names"]["chosen"], 1, "{plan}");
    let said = ok(
        home,
        &[
            "release",
            "--name",
            "r",
            "--on-unknown",
            "write",
            "--out",
            &out_text,
            "--name-by",
            "study-id",
        ],
    );
    assert!(
        said.contains("subjects named   2 by study-id, 1 by the subject code; 1 by a choice"),
        "{said}"
    );
    assert_eq!(subject_dirs(&out_dir).len(), 3);
    let history = ok(home, &["release", "--history", "--name", "r"]);
    assert!(
        history.contains("named        2 by study-id, 1 by the subject code; 1 by a choice"),
        "{history}"
    );
    let listed: Value = serde_json::from_str(&ok(
        home,
        &["release", "--history", "--name", "r", "--json"],
    ))
    .unwrap();
    assert_eq!(
        listed["releases"][0]["name_by"],
        json!(["study-id"]),
        "{listed}"
    );

    // a release of another name asks: the plan says who, the release is
    // refused with nothing written, and a file of choices answers it
    let other = work.path().join("other");
    let other_text = other.display().to_string();
    let said = ok(
        home,
        &[
            "release",
            "--name",
            "s",
            "--name-by",
            "study-id",
            "--name-plan",
        ],
    );
    assert!(said.contains("needs a choice (1)"), "{said}");
    assert!(said.contains("ST-002a | ST-002bb"), "{said}");
    assert!(said.contains("not ready"), "{said}");
    let (_, said) = refused(
        home,
        &[
            "release",
            "--name",
            "s",
            "--on-unknown",
            "write",
            "--out",
            &other_text,
            "--name-by",
            "study-id",
        ],
    );
    assert!(said.contains(two.as_str()), "{said}");
    assert!(said.contains("Nothing was written."), "{said}");
    assert!(!said.contains("002"), "{said}");
    assert!(!other.exists(), "a refused release wrote");
    let choices = work.path().join("choices.tsv");
    std::fs::write(&choices, format!("# who is who\n{two}\tST-002a\n")).unwrap();
    ok(
        home,
        &[
            "release",
            "--name",
            "s",
            "--on-unknown",
            "write",
            "--out",
            &other_text,
            "--name-by",
            "study-id",
            "--name-choices",
            choices.to_str().unwrap(),
        ],
    );
    assert_eq!(
        subject_dirs(&other),
        sorted(vec![
            "sub-ST001".to_string(),
            "sub-ST002a".to_string(),
            format!("sub-{none}")
        ]),
    );

    // what keeps no value is refused by name at the command line too, and
    // a choice needs the types it chooses under
    let (code, said) = refused(
        home,
        &[
            "release",
            "--name",
            "t",
            "--on-unknown",
            "write",
            "--out",
            &other_text,
            "--name-by",
            "personnummer",
        ],
    );
    assert_eq!(code, 2, "{said}");
    assert!(said.contains("a personnummer is never kept"), "{said}");
    let (code, _) = refused(
        home,
        &[
            "release",
            "--name",
            "t",
            "--out",
            &other_text,
            "--name-choice",
            "x=y",
        ],
    );
    assert_eq!(code, 2);
}
