// SPDX-License-Identifier: AGPL-3.0-only

//! A subject's sessions are a cache, and a merge drops the cache rows of
//! both its subjects. The subject that is left has its sessions again when
//! the merge ends, with no reader in between; a question brings the cache
//! up to date before it reads a session, so a subject whose rows are gone
//! is never missing from an answer; and `nils repair sessions` names the
//! subjects that hold a study and have no session, and builds them. On a
//! made-up registry, on SQLite and, where `NILS_TEST_POSTGRES_DSN` names a
//! server, on Postgres.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use nils_dicom::synth::TempDir;
use serde_json::Value;

const SCHEMA: &str = "nils_merge_sessions_test";

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn run(home: &TempDir, dsn: Option<&str>, args: &[&str], stdin: Option<&str>) -> String {
    let mut cmd = nils();
    cmd.arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match dsn {
        Some(dsn) => cmd.env("NILS_DSN", dsn),
        None => cmd.env_remove("NILS_DSN"),
    };
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
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(
        out.status.success(),
        "nils {args:?} failed ({}): {stderr}{stdout}",
        out.status
    );
    stdout
}

fn json(home: &TempDir, dsn: Option<&str>, args: &[&str]) -> Value {
    let out = run(home, dsn, args, None);
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("nils {args:?} is not JSON ({e}): {out}"))
}

fn drop_schema(dsn: &str) {
    if let Ok(mut store) = nils_registry::Store::connect_postgres(dsn, "public") {
        let _ = store.batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ));
    }
}

/// The registry's own store, for what a test does behind the engine's back.
fn store(home: &TempDir, dsn: Option<&str>) -> nils_registry::Store {
    match dsn {
        Some(dsn) => nils_registry::Store::connect_postgres(dsn, SCHEMA).unwrap(),
        None => nils_registry::Store::open_sqlite(&home.path().join("registry.db")).unwrap(),
    }
}

/// A made-up registry with its sessions built.
fn synthetic(dsn: Option<&str>) -> TempDir {
    let home = TempDir::new("merge-sessions-home");
    run(&home, dsn, &["key", "add", "k"], Some("a made-up key\n"));
    match dsn {
        Some(_) => run(
            &home,
            dsn,
            &[
                "init",
                "--key",
                "k",
                "--backend",
                "postgres",
                "--schema",
                SCHEMA,
            ],
            None,
        ),
        None => run(&home, dsn, &["init", "--key", "k"], None),
    };
    run(
        &home,
        dsn,
        &["synth", "--seed", "11", "--subjects", "16"],
        None,
    );
    run(&home, dsn, &["session", "rebuild"], None);
    home
}

/// Every subject's session days under the default scheme, by code, as
/// `nils session list` says them. A list is a reader: it builds what is
/// missing, so no test calls it between a write and what it checks.
fn days(home: &TempDir, dsn: Option<&str>) -> BTreeMap<String, BTreeSet<String>> {
    let doc = json(home, dsn, &["session", "list", "--json"]);
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in doc["rows"].as_array().unwrap() {
        out.entry(r["subject"].as_str().unwrap().to_string())
            .or_default()
            .insert(r["first"].as_str().unwrap().to_string());
    }
    out
}

/// What a count of sessions answers: its rows and its subjects. Every
/// subject's sessions, or one subject's by code.
fn sessions_counted(home: &TempDir, dsn: Option<&str>, code: Option<&str>) -> (i64, i64) {
    let text = match code {
        Some(code) => format!(
            "ast_version: 1\n\
             name: the sessions of one subject\n\
             sets:\n\
             \x20 who: {{grain: subject, where: [[\"=\", {{}}, [\"field\", {{}}, \"code\"], \"{code}\"]]}}\n\
             \x20 visits: {{grain: session, of: who}}\n\
             out: {{set: visits, level: count}}\n"
        ),
        None => "ast_version: 1\n\
                 name: every session\n\
                 sets:\n\
                 \x20 visits: {grain: session}\n\
                 out: {set: visits, level: count}\n"
            .to_string(),
    };
    let path = home.path().join("sessions.ask.yml");
    std::fs::write(&path, text).unwrap();
    let packs = packs();
    let ran = json(
        home,
        dsn,
        &[
            "ask",
            "run",
            "--file",
            path.to_str().unwrap(),
            "--pack-dir",
            packs.to_str().unwrap(),
            "--json",
        ],
    );
    let handle = ran["handle"].as_i64().unwrap().to_string();
    let csv = run(
        home,
        dsn,
        &["ask", "handles", "export", "--handle", &handle],
        None,
    );
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("rows,subjects"), "{csv}");
    let mut cells = lines.next().unwrap().split(',');
    (
        cells.next().unwrap().parse().unwrap(),
        cells.next().unwrap().parse().unwrap(),
    )
}

/// What `nils repair sessions --dry-run` finds, and writes nothing.
fn sessionless(home: &TempDir, dsn: Option<&str>) -> Value {
    json(home, dsn, &["repair", "sessions", "--dry-run", "--json"])
}

/// The cache rows of one subject taken away behind the engine's back, as
/// an engine that merged and built nothing left them.
fn drop_sessions(home: &TempDir, dsn: Option<&str>, code: &str) {
    let mut store = store(home, dsn);
    let subject = format!("SELECT id FROM subject WHERE code = '{code}'");
    let ids = format!("SELECT id FROM session_cache WHERE subject_id IN ({subject})");
    store
        .batch(&format!(
            "DELETE FROM session_label WHERE session_id IN ({ids}); \
             DELETE FROM session_cache_study WHERE session_id IN ({ids}); \
             DELETE FROM session_cache WHERE subject_id IN ({subject})"
        ))
        .unwrap();
}

/// Two subjects merged at the command line: the one that is left has its
/// sessions when the merge ends, every day of both, and the next question
/// counts them.
fn a_merge_leaves_the_subject_its_sessions(dsn: Option<&str>) {
    let home = synthetic(dsn);
    let before = days(&home, dsn);
    let canonical = before.keys().next().unwrap().clone();
    // one that came in on a day the first did not, so the merge adds a session
    let alias = before
        .iter()
        .find(|(code, days)| **code != canonical && !days.is_subset(&before[&canonical]))
        .map(|(code, _)| code.clone())
        .expect("a second subject with a day of its own");
    let merged: BTreeSet<&String> = before[&canonical].union(&before[&alias]).collect();
    let all: usize = before.values().map(BTreeSet::len).sum();
    let all_after = all - before[&canonical].len() - before[&alias].len() + merged.len();
    assert_eq!(
        sessions_counted(&home, dsn, None),
        (all as i64, before.len() as i64)
    );

    run(
        &home,
        dsn,
        &[
            "linkage",
            "merge",
            &canonical,
            &alias,
            "--why",
            "one person",
        ],
        None,
    );

    // nothing has read since: the merge built them
    let found = sessionless(&home, dsn);
    assert_eq!(found["without_sessions"], 0, "{found}");
    let mut store = store(&home, dsn);
    let held = store
        .query(
            &format!(
                "SELECT COUNT(*) FROM session_cache c JOIN subject s ON s.id = c.subject_id \
                 WHERE s.code = '{canonical}' AND c.window_days = 0"
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(held as usize, merged.len());
    drop(store);

    // and a question at session grain counts them
    assert_eq!(
        sessions_counted(&home, dsn, Some(&canonical)),
        (merged.len() as i64, 1)
    );
    assert_eq!(sessions_counted(&home, dsn, Some(&alias)), (0, 0));
    assert_eq!(
        sessions_counted(&home, dsn, None),
        (all_after as i64, before.len() as i64 - 1)
    );
}

/// A subject whose cache rows are gone, with nothing that builds them
/// between: the repair names it and writes nothing on a dry run, a question
/// builds its sessions before it reads one, and the repair builds what a
/// question has not.
fn a_question_and_the_repair_find_a_subject_without_sessions(dsn: Option<&str>) {
    let home = synthetic(dsn);
    let before = days(&home, dsn);
    let code = before.keys().next().unwrap().clone();
    let own = before[&code].len() as i64;
    let all: i64 = before.values().map(|d| d.len() as i64).sum();
    assert_eq!(sessionless(&home, dsn)["without_sessions"], 0);

    drop_sessions(&home, dsn, &code);
    for _ in 0..2 {
        let found = sessionless(&home, dsn);
        assert_eq!(found["without_sessions"], 1, "{found}");
        assert_eq!(found["subjects"][0]["code"], code.as_str(), "{found}");
        assert!(found["subjects"][0]["studies"].as_i64().unwrap() >= own);
        assert_eq!(found["built"], false, "{found}");
    }
    let said = run(&home, dsn, &["repair", "sessions", "--dry-run"], None);
    assert!(
        said.starts_with("1 subject holds a dated study with series and no session"),
        "{said}"
    );
    assert!(said.contains("nothing was written"), "{said}");
    assert!(said.contains(&code), "{said}");

    // the question finds the subject all the same, and leaves it whole
    assert_eq!(sessions_counted(&home, dsn, Some(&code)), (own, 1));
    assert_eq!(sessionless(&home, dsn)["without_sessions"], 0);
    assert_eq!(
        sessions_counted(&home, dsn, None),
        (all, before.len() as i64)
    );

    // the repair builds them where no question has
    drop_sessions(&home, dsn, &code);
    let mended = json(&home, dsn, &["repair", "sessions", "--json"]);
    assert_eq!(mended["without_sessions"], 1, "{mended}");
    assert_eq!(mended["built"], true, "{mended}");
    assert_eq!(mended["sessions"], own, "{mended}");
    let mut store = store(&home, dsn);
    let audited = store
        .query(
            "SELECT COUNT(*) FROM audit WHERE action = 'registry.repair' AND epoch IS NOT NULL",
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(audited, 1);
    drop(store);
    assert_eq!(sessionless(&home, dsn)["without_sessions"], 0);
    let said = run(&home, dsn, &["repair", "sessions"], None);
    assert!(said.starts_with("every subject"), "{said}");
    assert_eq!(days(&home, dsn), before);
}

/// A command that may fail: whether it succeeded, and what it wrote.
fn tried(home: &TempDir, dsn: Option<&str>, args: &[&str]) -> (bool, String, String) {
    let mut cmd = nils();
    cmd.arg("--registry")
        .arg(home.path())
        .args(args)
        .env("USER", "anna")
        .env("HOSTNAME", "ward-3")
        .stdin(Stdio::null());
    match dsn {
        Some(dsn) => cmd.env("NILS_DSN", dsn),
        None => cmd.env_remove("NILS_DSN"),
    };
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn epoch(home: &TempDir, dsn: Option<&str>) -> i64 {
    store(home, dsn)
        .query("SELECT value FROM registry_meta WHERE key = 'epoch'", &[])
        .unwrap()[0]
        .text(0)
        .unwrap()
        .parse()
        .unwrap()
}

/// A repair builds under the default scheme, commits, and then builds under
/// every kept scheme. Where a kept scheme cannot be built, what the default
/// scheme's build wrote is there all the same: the repair says what it
/// built, audits it and moves the epoch for it, and ends as a failure that
/// names the scheme. Run again, it finds nothing left to build.
fn a_repair_that_fails_under_a_kept_scheme_accounts_for_what_it_built(dsn: Option<&str>) {
    let home = synthetic(dsn);
    let before = days(&home, dsn);
    let code = before.keys().next().unwrap().clone();
    let own = before[&code].len() as i64;
    // a kept scheme whose label pattern does not compile, as a row written
    // by another version or by hand: it parses, and no build under it runs
    let mut definition = serde_json::to_value(nils_registry::session::Scheme::default()).unwrap();
    definition["said"] = serde_json::json!({"segment": 1, "pattern": "(?P<label>["});
    store(&home, dsn)
        .batch(&format!(
            "INSERT INTO session_scheme (name, definition, created_at) \
             VALUES ('broken', '{definition}', '2026-01-01T00:00:00Z')"
        ))
        .unwrap();
    drop_sessions(&home, dsn, &code);
    let at = epoch(&home, dsn);

    let (ok, stdout, stderr) = tried(&home, dsn, &["repair", "sessions", "--json"]);
    assert!(!ok, "the repair succeeded: {stdout}{stderr}");
    let said: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    assert_eq!(said["built"], true, "{said}");
    assert_eq!(said["sessions"], own, "{said}");
    assert!(
        said["failed"]
            .as_str()
            .is_some_and(|w| w.contains("broken")),
        "{said}"
    );
    assert!(stderr.contains("the scheme broken"), "{stderr}");
    // built, audited with the failure, and the epoch moved
    assert_eq!(sessionless(&home, dsn)["without_sessions"], 0);
    assert!(epoch(&home, dsn) > at);
    let mut reg = store(&home, dsn);
    let audited = reg
        .query(
            "SELECT COUNT(*) FROM audit WHERE action = 'registry.repair' AND epoch IS NOT NULL \
             AND CAST(details AS TEXT) LIKE '%broken%'",
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(audited, 1);
    drop(reg);
    // in words too
    drop_sessions(&home, dsn, &code);
    let (ok, stdout, stderr) = tried(&home, dsn, &["repair", "sessions"]);
    assert!(!ok, "{stdout}{stderr}");
    assert!(stdout.contains("built"), "{stdout}");
    assert!(stderr.contains("the scheme broken"), "{stderr}");
    // nothing is left to build, so nothing fails
    let again = run(&home, dsn, &["repair", "sessions"], None);
    assert!(again.starts_with("every subject"), "{again}");
    assert_eq!(days(&home, dsn), before);
}

#[test]
fn a_repair_that_fails_under_a_kept_scheme_accounts_for_what_it_built_on_sqlite() {
    a_repair_that_fails_under_a_kept_scheme_accounts_for_what_it_built(None);
}

#[test]
fn a_merge_leaves_the_subject_its_sessions_on_sqlite() {
    a_merge_leaves_the_subject_its_sessions(None);
}

#[test]
fn a_question_and_the_repair_find_a_subject_without_sessions_on_sqlite() {
    a_question_and_the_repair_find_a_subject_without_sessions(None);
}

/// The same on Postgres, one after the other: the two share a schema.
#[test]
fn a_merge_and_a_subject_without_sessions_on_postgres() {
    let Ok(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN") else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    if dsn.is_empty() {
        eprintln!("NILS_TEST_POSTGRES_DSN is empty; the Postgres half is skipped");
        return;
    }
    drop_schema(&dsn);
    a_merge_leaves_the_subject_its_sessions(Some(&dsn));
    drop_schema(&dsn);
    a_question_and_the_repair_find_a_subject_without_sessions(Some(&dsn));
    drop_schema(&dsn);
    a_repair_that_fails_under_a_kept_scheme_accounts_for_what_it_built(Some(&dsn));
    drop_schema(&dsn);
}
