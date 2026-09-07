// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4b §12.3, slice 10: `nils ask` as a person runs it. Every verb
//! answers on a standalone registry; the same document produces the same
//! content hash standalone and against a running engine (bar 7 of §13.4);
//! `explain --dialect` prints both texts; a handle exports to CSV with no
//! database driver; a refused document names its issues and exits 2.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../nils-ask/fixtures/{name}.ask.yml"))
}

struct Out {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(&self, what: &str) -> &str {
        assert!(
            self.status.success(),
            "{what} failed ({}): {}{}",
            self.status,
            self.stderr,
            self.stdout
        );
        &self.stdout
    }
}

fn run(home: &TempDir, args: &[&str], stdin: Option<&str>) -> Out {
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
    Out {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A synthetic registry with its sessions built.
fn synthetic() -> TempDir {
    let home = TempDir::new("ask-cli-home");
    run(&home, &["key", "add", "k"], Some("an ask cli test key\n")).ok("key add");
    run(&home, &["init", "--key", "k"], None).ok("init");
    run(&home, &["synth", "--seed", "11", "--subjects", "48"], None).ok("synth");
    run(&home, &["session", "rebuild"], None).ok("session rebuild");
    home
}

struct Server {
    child: Child,
    url: String,
}

impl Server {
    fn start(home: &TempDir, requests: usize) -> Server {
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
            ])
            .arg(requests.to_string())
            .args(["--pack-dir", packs().to_str().unwrap()])
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
            url: format!("http://{addr}"),
        }
    }

    /// The engine goes when the test is done with it: `--requests` bounds
    /// a run, but a test that counts requests breaks the moment a verb
    /// makes one more, and a server nobody stops outlives the run.
    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn hash_of(text: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix("hash "))
        .unwrap_or_else(|| panic!("no hash in {text}"))
        .trim()
        .to_string()
}

#[test]
fn every_verb_answers_on_a_standalone_registry() {
    let home = synthetic();
    let packs = packs();
    let (p, y) = (packs.to_str().unwrap(), fixture("yardstick"));
    let y = y.to_str().unwrap();

    // validate: the hash, the parameters pinned, no warning that refuses
    let out = run(
        &home,
        &["ask", "validate", "--file", y, "--pack-dir", p],
        None,
    );
    let hash = hash_of(out.ok("validate"));
    assert_eq!(hash.len(), 64, "{hash}");

    // explain: both texts, each naming its own placeholder style
    let out = run(
        &home,
        &["ask", "explain", "--file", y, "--pack-dir", p],
        None,
    );
    let text = out.ok("explain").to_string();
    assert!(
        text.contains("-- sqlite") && text.contains("-- postgres"),
        "{text}"
    );
    assert!(text.contains("?1") && text.contains("$1::"), "{text}");
    assert_eq!(hash_of(&text), hash);
    let only = run(
        &home,
        &[
            "ask",
            "explain",
            "--file",
            y,
            "--pack-dir",
            p,
            "--dialect",
            "sqlite",
        ],
        None,
    );
    let text = only.ok("explain --dialect sqlite").to_string();
    assert!(
        text.contains("-- sqlite") && !text.contains("-- postgres"),
        "{text}"
    );
    let bad = run(
        &home,
        &[
            "ask",
            "explain",
            "--file",
            y,
            "--pack-dir",
            p,
            "--dialect",
            "duckdb",
        ],
        None,
    );
    assert_eq!(bad.status.code(), Some(2), "{}", bad.stderr);

    // describe: one sentence per set, in the order of rule 5, and the
    // conventions that always apply
    let out = run(
        &home,
        &["ask", "describe", "--file", y, "--pack-dir", p],
        None,
    );
    let text = out.ok("describe").to_string();
    assert!(
        text.contains("converted: subjects; transition = the course changes from"),
        "{text}"
    );
    assert!(text.contains("a month is 31 days and a year 366"), "{text}");
    assert!(text.contains("disclosure local"), "{text}");

    // options: the moves of one set, with their fillers, and no query
    let out = run(
        &home,
        &[
            "ask",
            "options",
            "--file",
            y,
            "--pack-dir",
            p,
            "--set",
            "good",
        ],
        None,
    );
    let text = out.ok("options").to_string();
    assert!(text.contains("good (sessions)"), "{text}");
    assert!(
        text.contains("read {relation} within {preset} each way"),
        "{text}"
    );
    assert!(text.contains("near:edss"), "{text}");

    // diagnose: the funnel, stage by stage, and the answer's count
    let out = run(
        &home,
        &["ask", "diagnose", "--file", y, "--pack-dir", p],
        None,
    );
    let text = out.ok("diagnose").to_string();
    assert!(text.contains("funnel"), "{text}");
    assert!(text.contains("converted/source"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.contains("answer/where comparable.largest") && l.contains("14 subjects")),
        "{text}"
    );

    // run: the handle, then the handle's own doors
    let out = run(
        &home,
        &[
            "ask",
            "run",
            "--file",
            y,
            "--pack-dir",
            p,
            "--name",
            "converters",
            "--keep",
        ],
        None,
    );
    let text = out.ok("run").to_string();
    assert!(text.contains("14 rows"), "{text}");
    assert!(
        text.contains(&hash),
        "the run hashes the same document: {text}"
    );
    let listed = run(&home, &["ask", "handles", "list"], None);
    let text = listed.ok("handles list").to_string();
    assert_eq!(text.lines().count(), 4, "{text}");
    assert!(text.contains("converters/good"), "{text}");
    let shown = run(&home, &["ask", "handles", "show", "--handle", "1"], None);
    let text = shown.ok("handles show").to_string();
    assert!(text.contains("handle 1   subject   14 rows"), "{text}");
    assert!(text.contains("named converters"), "{text}");
    assert!(text.contains("columns _key (integer)"), "{text}");

    // export: the rows as CSV, from the pages, with no driver
    let csv = home.path().join("converters.csv");
    let out = run(
        &home,
        &[
            "ask",
            "handles",
            "export",
            "--handle",
            "1",
            "--out",
            csv.to_str().unwrap(),
        ],
        None,
    );
    assert!(out.ok("export").contains("14 rows"), "{}", out.stdout);
    let text = std::fs::read_to_string(&csv).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 15, "a header and fourteen rows: {text}");
    assert_eq!(
        lines[0],
        "_key,_subject,code,transition.to_date,transition.precision,n_good,comparable.largest,comparable.groups"
    );
    assert!(lines[1].contains("SYN000"), "{}", lines[1]);
    // to standard output when no file is named
    let out = run(&home, &["ask", "handles", "export", "--handle", "1"], None);
    assert_eq!(out.ok("export to stdout").lines().count(), 15);

    // selections: saved, listed, shown as the document it stores
    let out = run(
        &home,
        &[
            "ask",
            "selections",
            "save",
            "--name",
            "converters",
            "--file",
            y,
            "--pack-dir",
            p,
            "--note",
            "the study's converters",
        ],
        None,
    );
    let text = out.ok("selections save").to_string();
    assert!(text.contains("at version 1"), "{text}");
    assert!(
        text.contains(&hash),
        "the selection stores the same hash: {text}"
    );
    let listed = run(&home, &["ask", "selections", "list"], None);
    assert!(
        listed.ok("selections list").contains("converters"),
        "{}",
        listed.stdout
    );
    let shown = run(&home, &["ask", "selections", "show", "converters@1"], None);
    let text = shown.ok("selections show").to_string();
    assert!(text.contains("version 1 of 1"), "{text}");
    assert!(text.contains("ast_version"), "the document itself: {text}");

    // a refused document names every issue and exits 2
    let broken = home.path().join("broken.ask.json");
    std::fs::write(
        &broken,
        r#"{"ast_version": 1, "sets": {"a": {"grain": "subject", "where": [["=", {}, ["field", {}, "nowhere"], 1]]}}, "out": {"set": "a", "level": "count"}}"#,
    )
    .unwrap();
    let out = run(
        &home,
        &[
            "ask",
            "validate",
            "--file",
            broken.to_str().unwrap(),
            "--pack-dir",
            p,
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(2), "{}", out.stdout);
    assert!(
        out.stderr.contains("unknown_field at sets.a.where[0]"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("GET /api/ask/options"),
        "{}",
        out.stderr
    );
}

/// Bar 7 of §13.4: the same document produces the same content hash
/// standalone and against the server, and the verbs read alike either way.
#[test]
fn the_same_document_hashes_alike_standalone_and_against_the_server() {
    let home = synthetic();
    let packs = packs();
    let p = packs.to_str().unwrap();
    let y = fixture("yardstick");
    let y = y.to_str().unwrap();
    let server = Server::start(&home, 64);

    let local = run(
        &home,
        &["ask", "validate", "--file", y, "--pack-dir", p],
        None,
    );
    let remote = run(
        &home,
        &["ask", "validate", "--file", y, "--server", &server.url],
        None,
    );
    let hash = hash_of(local.ok("validate here"));
    assert_eq!(hash, hash_of(remote.ok("validate there")));

    // explain: the same two texts and the same hash
    let local = run(
        &home,
        &["ask", "explain", "--file", y, "--pack-dir", p],
        None,
    );
    let remote = run(
        &home,
        &["ask", "explain", "--file", y, "--server", &server.url],
        None,
    );
    assert_eq!(local.ok("explain here"), remote.ok("explain there"));

    // describe: the same sentences
    let local = run(
        &home,
        &["ask", "describe", "--file", y, "--pack-dir", p],
        None,
    );
    let remote = run(
        &home,
        &["ask", "describe", "--file", y, "--server", &server.url],
        None,
    );
    assert_eq!(local.ok("describe here"), remote.ok("describe there"));

    // run: the engine leaves the handle, and the answer hashes the same
    let out = run(
        &home,
        &[
            "ask",
            "run",
            "--file",
            y,
            "--server",
            &server.url,
            "--name",
            "through-the-door",
        ],
        None,
    );
    let text = out.ok("run there").to_string();
    assert!(text.contains("14 rows"), "{text}");
    assert!(
        text.contains(&hash),
        "the engine hashes the document the same: {text}"
    );
    server.stop();

    // the handle the door left is this registry's, and exports here
    let shown = run(&home, &["ask", "handles", "show", "--handle", "1"], None);
    let text = shown.ok("handles show").to_string();
    assert!(text.contains("named through-the-door"), "{text}");
    let out = run(&home, &["ask", "handles", "export", "--handle", "1"], None);
    assert_eq!(out.ok("export").lines().count(), 15);
}

/// A URL the command line will not speak to says so, and names what to do.
#[test]
fn tls_belongs_to_the_front() {
    let home = synthetic();
    let out = run(
        &home,
        &[
            "ask",
            "validate",
            "--file",
            fixture("gold-c").to_str().unwrap(),
            "--server",
            "https://nils.example.org",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(2), "{}", out.stdout);
    assert!(
        out.stderr.contains("TLS belongs to whatever sits in front"),
        "{}",
        out.stderr
    );
}
