// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4b §12.3, slice 10: `nils ask` as a person runs it. Every verb
//! answers on a standalone registry; the same document produces the same
//! content hash standalone and against a running engine (bar 7 of §13.4);
//! `explain --dialect` prints both texts; a handle exports to CSV with no
//! database driver; a refused document names its issues and exits 2. Record
//! 35 S8: the standing predicate of §4.4 rule 10 is in the query, not only
//! in the description.

use std::collections::BTreeSet;
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

/// Every stack of the registry, at the record level and at the count level:
/// one set, two readings, so the number and the rows can be held against
/// each other.
const STACKS_RECORD: &str = "ast_version: 1\n\
     name: which stacks the archive holds\n\
     scheme: default\n\
     sets:\n  acquisitions: {grain: stack}\n\
     keep: [acquisitions]\n\
     out: {set: acquisitions, level: record}\n";
const STACKS_COUNT: &str = "ast_version: 1\n\
     name: which stacks the archive holds\n\
     scheme: default\n\
     sets:\n  acquisitions: {grain: stack}\n\
     keep: [acquisitions]\n\
     out: {set: acquisitions, level: count}\n";

/// A document written beside the registry, as a person would keep one.
fn document(home: &TempDir, name: &str, text: &str) -> String {
    let path = home.path().join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

/// The stacks the pack ruled out. The generator writes none, so a registry
/// has nothing for the standing predicate to bite on until a run of the
/// classifier would have ruled some out; this is that run's effect, and the
/// ids it returns are the ones no answer may carry.
fn rule_out(home: &TempDir, how_many: usize) -> Vec<i64> {
    let mut store = nils_registry::Store::open_sqlite(&home.path().join("registry.db")).unwrap();
    let ids: Vec<i64> = store
        .query(
            "SELECT stack_id FROM classification_axis \
             WHERE axis = 'disposition' AND value = 'scout' ORDER BY stack_id",
            &[],
        )
        .unwrap()
        .iter()
        .take(how_many)
        .map(|r| r.int(0).unwrap())
        .collect();
    assert_eq!(ids.len(), how_many, "the registry holds fewer scouts");
    let list = ids
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    store
        .batch(&format!(
            "UPDATE classification_axis SET value = 'excluded' \
             WHERE axis = 'disposition' AND stack_id IN ({list})"
        ))
        .unwrap();
    ids
}

/// The rows a run says it answered, from its own line.
fn rows_of(text: &str) -> usize {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let at = fields
        .iter()
        .position(|f| *f == "rows")
        .unwrap_or_else(|| panic!("no row count in {text}"));
    fields[at - 1].parse().unwrap()
}

/// The keys of an exported answer.
fn keys_of(path: &Path) -> BTreeSet<i64> {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .skip(1)
        .map(|l| l.split(',').next().unwrap().parse().unwrap())
        .collect()
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
    // the selection's hash is of what it selects: the yardstick binds its
    // parameters, so the values are in it and the question's hash is not
    // (record 43)
    let out = run(
        &home,
        &["ask", "validate", "--file", y, "--pack-dir", p, "--json"],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(out.ok("validate --json")).unwrap();
    assert_eq!(v["hash"], hash.as_str());
    let bound = v["bound_hash"].as_str().unwrap();
    assert_ne!(bound, hash, "the yardstick binds values");
    assert!(
        text.contains(bound),
        "the selection stores the hash of what it selects: {text}"
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

/// §4.4 rule 10 and §14.2: a stack the pack ruled out is not a stack any set
/// sees, and a document cannot switch that off. The ask used to read the
/// whole registry while describe printed the exclusion, so its stack counts
/// were the release's plus every excluded stack.
#[test]
fn an_ask_whose_selection_excludes_rows_answers_without_them() {
    let home = synthetic();
    let packs = packs();
    let p = packs.to_str().unwrap();
    let doc = document(&home, "stacks-record.ask.yml", STACKS_RECORD);

    let out = run(
        &home,
        &["ask", "run", "--file", &doc, "--pack-dir", p],
        None,
    );
    let before = rows_of(out.ok("run over every stack"));
    assert!(
        before > 100,
        "the synthetic registry holds stacks: {before}"
    );

    let ruled_out = rule_out(&home, 20);
    let out = run(
        &home,
        &["ask", "run", "--file", &doc, "--pack-dir", p],
        None,
    );
    let after = rows_of(out.ok("run once twenty stacks are ruled out"));
    assert_eq!(
        after,
        before - ruled_out.len(),
        "the answer drops exactly the stacks the pack ruled out"
    );

    // and the rows themselves, not only their number
    let csv = home.path().join("stacks.csv");
    run(
        &home,
        &[
            "ask",
            "handles",
            "export",
            "--handle",
            "2",
            "--out",
            csv.to_str().unwrap(),
        ],
        None,
    )
    .ok("export");
    let keys = keys_of(&csv);
    assert_eq!(keys.len(), after);
    for id in &ruled_out {
        assert!(!keys.contains(id), "an excluded stack is in the answer");
    }
}

/// The count level and the record level of one document are one query with
/// one set of predicates: the number a run prints is the number of rows it
/// hands back, and a reader may hold either against the other.
#[test]
fn the_number_it_prints_and_the_rows_it_returns_come_from_one_place() {
    let home = synthetic();
    let packs = packs();
    let p = packs.to_str().unwrap();
    rule_out(&home, 20);
    let record = document(&home, "stacks-record.ask.yml", STACKS_RECORD);
    let counted = document(&home, "stacks-count.ask.yml", STACKS_COUNT);

    let out = run(
        &home,
        &["ask", "run", "--file", &record, "--pack-dir", p],
        None,
    );
    let printed = rows_of(out.ok("run at the record level"));
    let csv = home.path().join("record.csv");
    run(
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
    )
    .ok("export the record");
    let keys = keys_of(&csv);
    assert_eq!(keys.len(), printed, "the print and the rows are one answer");

    run(
        &home,
        &["ask", "run", "--file", &counted, "--pack-dir", p],
        None,
    )
    .ok("run at the count level");
    let csv = home.path().join("count.csv");
    run(
        &home,
        &[
            "ask",
            "handles",
            "export",
            "--handle",
            "2",
            "--out",
            csv.to_str().unwrap(),
        ],
        None,
    )
    .ok("export the count");
    let text = std::fs::read_to_string(&csv).unwrap();
    let line = text.lines().nth(1).unwrap();
    let counted: usize = line.split(',').next().unwrap().parse().unwrap();
    assert_eq!(
        counted, printed,
        "counting the set and listing it answer the same number"
    );
}

/// Rule 10 again, from the reader's side: describe names the standing
/// predicate, explain's SQL carries it, and a person who writes that
/// predicate out by hand against the registry gets the ask's own number.
#[test]
fn what_the_ask_prints_as_its_predicate_is_what_a_reader_can_reproduce() {
    let home = synthetic();
    let packs = packs();
    let p = packs.to_str().unwrap();
    rule_out(&home, 20);
    let doc = document(&home, "stacks-record.ask.yml", STACKS_RECORD);

    let out = run(
        &home,
        &["ask", "describe", "--file", &doc, "--pack-dir", p],
        None,
    );
    let text = out.ok("describe").to_string();
    assert!(
        text.contains("stack sets exclude the excluded disposition"),
        "{text}"
    );

    let out = run(
        &home,
        &[
            "ask",
            "explain",
            "--file",
            &doc,
            "--pack-dir",
            p,
            "--dialect",
            "sqlite",
        ],
        None,
    );
    let sql = out.ok("explain").to_string();
    assert!(
        sql.contains("NOT EXISTS") && sql.contains("classification_axis"),
        "the printed query carries the standing predicate: {sql}"
    );

    let out = run(
        &home,
        &["ask", "run", "--file", &doc, "--pack-dir", p],
        None,
    );
    let answered = rows_of(out.ok("run"));
    let mut store = nils_registry::Store::open_sqlite(&home.path().join("registry.db")).unwrap();
    let readers_own = store
        .query(
            "SELECT COUNT(*) FROM stack st WHERE NOT EXISTS \
             (SELECT 1 FROM classification_axis a WHERE a.stack_id = st.id \
             AND a.axis = 'disposition' AND a.value = 'excluded')",
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(
        readers_own as usize, answered,
        "the predicate a reader reproduces is the one the ask applied"
    );
}

/// The handle a run printed.
fn handle_of(text: &str) -> String {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let at = fields
        .iter()
        .position(|f| *f == "handle")
        .unwrap_or_else(|| panic!("no handle in {text}"));
    fields[at + 1].to_string()
}

/// A run's answer as rows of text, the header left out.
fn answer(home: &TempDir, doc: &str, p: &str) -> Vec<Vec<String>> {
    let out = run(home, &["ask", "run", "--file", doc, "--pack-dir", p], None);
    let handle = handle_of(out.ok("run"));
    let out = run(
        home,
        &["ask", "handles", "export", "--handle", &handle],
        None,
    );
    out.ok("export").lines().skip(1).map(csv_fields).collect()
}

/// One CSV line's fields, a quoted field's comma kept.
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    out.push(field);
    out
}

/// A group's counts by the key in its third column, `_rows` in its fourth.
fn counts(rows: &[Vec<String>]) -> std::collections::BTreeMap<String, usize> {
    rows.iter()
        .map(|r| (r[2].clone(), r[3].parse().unwrap()))
        .collect()
}

/// Record 38 S4: a group keyed by an axis. Single-valued, every stack counts
/// once under its value. Multi-valued, the default key is a stack's values
/// as one sorted list, so every stack still counts once and a stack with two
/// values is its own key; `each` counts it under both. A stack with no value
/// falls under the empty key, and a stack the pack ruled out is in none.
#[test]
fn a_group_keyed_by_an_axis_counts_every_stack_it_should_and_says_how() {
    let home = synthetic();
    let packs = packs();
    let p = packs.to_str().unwrap();
    let ruled_out = rule_out(&home, 20);

    // Ten FLAIR stacks also say FatSat: the generator writes one modifier
    // per stack at most, and the case that needs saying is two.
    let mut store = nils_registry::Store::open_sqlite(&home.path().join("registry.db")).unwrap();
    let flair: Vec<i64> = store
        .query(
            "SELECT stack_id FROM classification_axis \
             WHERE axis = 'modifier' AND value = 'FLAIR' ORDER BY stack_id LIMIT 10",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect();
    assert_eq!(flair.len(), 10);
    for id in &flair {
        store
            .batch(&format!(
                "INSERT INTO classification_axis (stack_id, axis, value, confidence, tier) \
                 VALUES ({id}, 'modifier', 'FatSat', 0.95, 'exclusive')"
            ))
            .unwrap();
    }
    let count = |sql: &str| -> usize {
        nils_registry::Store::open_sqlite(&home.path().join("registry.db"))
            .unwrap()
            .query(sql, &[])
            .unwrap()[0]
            .int(0)
            .unwrap() as usize
    };
    let not_excluded = "NOT EXISTS (SELECT 1 FROM classification_axis x WHERE x.stack_id = st.id \
                        AND x.axis = 'disposition' AND x.value = 'excluded')";
    let stacks = count(&format!(
        "SELECT COUNT(*) FROM stack st WHERE {not_excluded}"
    ));
    let with_flair = count(&format!(
        "SELECT COUNT(*) FROM stack st WHERE {not_excluded} AND EXISTS (SELECT 1 FROM \
         classification_axis a WHERE a.stack_id = st.id AND a.axis = 'modifier' AND a.value = 'FLAIR')"
    ));
    let scouts = count(
        "SELECT COUNT(*) FROM classification_axis WHERE axis = 'disposition' AND value = 'scout'",
    );
    assert!(scouts > 0 && with_flair > 10, "{scouts} {with_flair}");

    let group = |name: &str, by: &str, key: &str| -> String {
        document(
            &home,
            name,
            &format!(
                "ast_version: 1\n\
                 sets:\n  acquisitions: {{grain: stack}}\n  per:\n    grain: group\n    \
                 group: {{of: acquisitions, by: [{by}]}}\n\
                 out:\n  set: per\n  level: record\n  columns:\n    - [\"field\", {{}}, \"{key}\"]\n    \
                 - [\"field\", {{}}, \"_rows\"]\n  order:\n    - [[\"field\", {{}}, \"{key}\"], asc]\n"
            ),
        )
    };

    // a single-valued axis: one row per value, and the rows sum to the stacks
    let doc = group(
        "per-technique.ask.yml",
        "[\"axis\", {}, \"technique\"]",
        "technique",
    );
    let per = counts(&answer(&home, &doc, p));
    assert_eq!(per.values().sum::<usize>(), stacks, "{per:?}");
    let gre = count(
        "SELECT COUNT(*) FROM classification_axis WHERE axis = 'technique' AND value = 'GRE'",
    );
    // every scout is a GRE localizer, and the twenty ruled out are not read
    assert_eq!(per["GRE"], gre - ruled_out.len(), "{per:?}");

    // a multi-valued axis, the default reading: one key per stack
    let doc = group(
        "per-modifier.ask.yml",
        "[\"axis\", {}, \"modifier\"]",
        "modifier",
    );
    let per = counts(&answer(&home, &doc, p));
    assert_eq!(per.values().sum::<usize>(), stacks, "{per:?}");
    assert_eq!(per.len(), 3, "FLAIR, FLAIR with FatSat, and none: {per:?}");
    let two: Vec<(&String, &usize)> = per.iter().filter(|(k, _)| k.contains(',')).collect();
    assert_eq!(two.len(), 1, "{per:?}");
    assert_eq!(*two[0].1, 10, "{per:?}");
    assert!(two[0].0.contains("FLAIR") && two[0].0.contains("FatSat"));
    assert_eq!(per["FLAIR"], with_flair - 10, "{per:?}");
    assert_eq!(
        per[""],
        stacks - with_flair,
        "no modifier, one key: {per:?}"
    );

    // `each`: a stack with two values counts under both
    let doc = group(
        "per-modifier-each.ask.yml",
        "[\"axis\", {each: true}, \"modifier\"]",
        "modifier",
    );
    let rows = answer(&home, &doc, p);
    let per = counts(&rows);
    assert_eq!(per["FLAIR"], with_flair, "{per:?}");
    assert_eq!(per["FatSat"], 10, "{per:?}");
    assert_eq!(per[""], stacks - with_flair, "{per:?}");
    assert_eq!(per.values().sum::<usize>(), stacks + 10, "{per:?}");
    // the empty key is numbered last
    assert_eq!(rows.last().unwrap()[2], "", "{rows:?}");

    // describe says which reading a document asked for
    let out = run(
        &home,
        &["ask", "describe", "--file", &doc, "--pack-dir", p],
        None,
    );
    assert!(
        out.ok("describe").contains(
            "each value of the axis modifier (a stack with two values counts under both)"
        ),
        "{}",
        out.stdout
    );

    // and an axis takes no option but each
    let bad = group(
        "per-modifier-bad.ask.yml",
        "[\"axis\", {every: true}, \"modifier\"]",
        "modifier",
    );
    let out = run(
        &home,
        &["ask", "validate", "--file", &bad, "--pack-dir", p],
        None,
    );
    assert_eq!(out.status.code(), Some(2), "{}", out.stdout);
    assert!(
        out.stderr.contains("takes no option every"),
        "{}",
        out.stderr
    );
}

/// Record 38 S4: `ask run` finds the installed pack as `classify` does, and
/// `--pack-dir` still overrides it. The corpus run's own command line, with
/// no pack directory, exited 2 before this.
#[test]
fn ask_run_finds_the_installed_pack_without_being_told() {
    let home = synthetic();
    let packs = packs();
    let p = packs.to_str().unwrap();
    let doc = document(&home, "stacks-count.ask.yml", STACKS_COUNT);
    let told = run(
        &home,
        &["ask", "run", "--file", &doc, "--pack-dir", p],
        None,
    );
    let hash = hash_of_run(told.ok("run with --pack-dir"));

    // the first place `classify` looks: the registry's own packs
    std::os::unix::fs::symlink(packs.canonicalize().unwrap(), home.path().join("packs")).unwrap();
    let bare = |args: &[&str]| -> Out {
        let out = nils()
            .arg("--registry")
            .arg(home.path())
            .args(args)
            .env_remove("NILS_PACK_DIR")
            .env("HOME", home.path())
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        Out {
            status: out.status,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    };
    let found = bare(&["ask", "run", "--file", &doc]);
    assert_eq!(
        hash_of_run(found.ok("run with no --pack-dir")),
        hash,
        "the same pack answers the same hash"
    );

    // the flag still wins, and a wrong one is an error, not a fall through
    let missing = home.path().join("no-packs-here");
    let wrong = bare(&[
        "ask",
        "run",
        "--file",
        &doc,
        "--pack-dir",
        missing.to_str().unwrap(),
    ]);
    assert!(!wrong.status.success(), "{}", wrong.stdout);
}

/// The hash a run printed on its one line.
fn hash_of_run(text: &str) -> String {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let at = fields
        .iter()
        .position(|f| *f == "hash")
        .unwrap_or_else(|| panic!("no hash in {text}"));
    fields[at + 1].to_string()
}

/// The reported fault (record 43): two selections whose documents differ
/// only in the list a parameter binds, the ids they select, were saved
/// under one hash, so a promotion's match by hash and the run cache could
/// take one for the other. A parameter's value stays out of the question's
/// hash (§4.4 rule 13); a saved selection's hash is of what it selects,
/// with its values bound.
#[test]
fn selections_that_bind_different_ids_have_different_hashes() {
    let home = synthetic();
    let p = packs();
    let p = p.to_str().unwrap();
    let doc = |ids: &str| {
        format!(
            r#"{{"ast_version": 1, "params": {{"ids": {{"type": "list", "value": [{ids}]}}}},
               "sets": {{"s": {{"grain": "stack", "where": [["in", {{}}, ["field", {{}}, "id"], ["param", {{}}, "ids"]]]}}}},
               "out": {{"set": "s", "level": "record"}}}}"#
        )
    };
    let mut hashes = Vec::new();
    let mut questions = Vec::new();
    for (name, ids) in [("first", "1, 2"), ("second", "3"), ("again", "1, 2")] {
        let file = home.path().join(format!("{name}.ask.json"));
        std::fs::write(&file, doc(ids)).unwrap();
        let f = file.to_str().unwrap();
        let out = run(
            &home,
            &[
                "ask",
                "selections",
                "save",
                "--name",
                name,
                "--file",
                f,
                "--pack-dir",
                p,
                "--json",
            ],
            None,
        );
        let saved: serde_json::Value = serde_json::from_str(out.ok("selections save")).unwrap();
        hashes.push(saved["hash"].as_str().unwrap().to_string());
        let out = run(
            &home,
            &["ask", "validate", "--file", f, "--pack-dir", p, "--json"],
            None,
        );
        let v: serde_json::Value = serde_json::from_str(out.ok("validate")).unwrap();
        questions.push(v["hash"].as_str().unwrap().to_string());
    }
    assert_ne!(hashes[0], hashes[1], "different ids, different selections");
    assert_eq!(hashes[0], hashes[2], "the same ids, the same selection");
    // the question itself is one, whatever its parameter is bound to
    assert_eq!(questions[0], questions[1]);
}
