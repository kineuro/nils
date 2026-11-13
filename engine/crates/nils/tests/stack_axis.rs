// SPDX-License-Identifier: AGPL-3.0-only

//! `nils repair stack-axis`: the axes of every stack as one row, which a
//! question reads, held against the axis rows they are made of. A registry
//! the engine wrote agrees. A write that went around the engine (here an
//! axis row changed by hand, which rules a stack out) is found and counted
//! with `--dry-run`, which writes nothing, so a question still answers from
//! the row as it was; the repair makes every row again, moves the epoch and
//! leaves an audit row, and the question then leaves the stack out.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use nils_dicom::synth::TempDir;
use serde_json::Value;

const SCHEMA: &str = "nils_stack_axis_cli_test";

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
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
}

fn store(home: &TempDir, dsn: Option<&str>) -> nils_registry::Store {
    match dsn {
        Some(dsn) => nils_registry::Store::connect_postgres(dsn, SCHEMA).unwrap(),
        None => nils_registry::Store::open_sqlite(&home.path().join("registry.db")).unwrap(),
    }
}

fn drop_schema(dsn: &str) {
    if let Ok(mut store) = nils_registry::Store::connect_postgres(dsn, "public") {
        let _ = store.batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ));
    }
}

/// How many stacks a question over every stack sees, from the line a run
/// says its rows in.
fn stacks_seen(home: &TempDir, dsn: Option<&str>) -> i64 {
    let file = home.file(
        "all-stacks.ask.yml",
        b"ast_version: 1\nname: every stack\nscheme: default\nsets:\n  scans: {grain: stack}\nout: {set: scans, level: record}\n",
    );
    let text = run(
        home,
        dsn,
        &[
            "ask",
            "run",
            "--file",
            file.to_str().unwrap(),
            "--pack-dir",
            packs().to_str().unwrap(),
        ],
        None,
    );
    let fields: Vec<&str> = text.split_whitespace().collect();
    let at = fields
        .iter()
        .position(|f| *f == "rows")
        .unwrap_or_else(|| panic!("no row count in {text}"));
    fields[at - 1].parse().unwrap()
}

fn the_repair_finds_and_mends(dsn: Option<&str>) {
    let home = TempDir::new("stack-axis-cli");
    run(&home, dsn, &["key", "add", "k"], Some("a stack axes key\n"));
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
        &["synth", "--seed", "11", "--subjects", "48"],
        None,
    );
    run(&home, dsn, &["session", "rebuild"], None);

    // what the engine wrote agrees, and a repair of it writes nothing
    let found = json(&home, dsn, &["repair", "stack-axis", "--dry-run", "--json"]);
    assert_eq!(found["agrees"], true, "{found}");
    assert!(found["stacks"].as_i64().unwrap() > 100, "{found}");
    let found = json(&home, dsn, &["repair", "stack-axis", "--json"]);
    assert_eq!(
        (&found["agrees"], &found["rebuilt"]),
        (&true.into(), &false.into())
    );
    let seen = stacks_seen(&home, dsn);

    // a hand rules a stack out in the axis rows, around the engine
    let stack = {
        let mut store = store(&home, dsn);
        let axis = store.qualified("classification_axis");
        let stack = store
            .query(
                &format!(
                    "SELECT MIN(stack_id) FROM {axis} WHERE axis = 'disposition' AND value = 'acquisition'"
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        store
            .execute(
                &format!(
                    "UPDATE {axis} SET value = 'excluded' WHERE axis = 'disposition' AND stack_id = {stack}"
                ),
                &[],
            )
            .unwrap();
        stack
    };
    let found = json(&home, dsn, &["repair", "stack-axis", "--dry-run", "--json"]);
    assert_eq!(found["agrees"], false, "{found}");
    assert_eq!(found["different"], 1, "{found}");
    assert_eq!(found["sample"], serde_json::json!([stack]), "{found}");
    assert_eq!(found["rebuilt"], false, "{found}");
    // nothing was written: the question reads the row as it was
    assert_eq!(stacks_seen(&home, dsn), seen);

    let mended = json(&home, dsn, &["repair", "stack-axis", "--json"]);
    assert_eq!(mended["rebuilt"], true, "{mended}");
    let after = json(&home, dsn, &["repair", "stack-axis", "--dry-run", "--json"]);
    assert_eq!(after["agrees"], true, "{after}");
    // and the stack is in no set now
    assert_eq!(stacks_seen(&home, dsn), seen - 1);
    let mut store = store(&home, dsn);
    let audited = store
        .query(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE action = 'registry.repair'",
                store.qualified("audit")
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(audited, 1);
}

#[test]
fn the_repair_finds_a_write_around_the_engine_and_mends_it_on_sqlite() {
    the_repair_finds_and_mends(None);
}

#[test]
fn the_repair_finds_a_write_around_the_engine_and_mends_it_on_postgres() {
    let dsn = match std::env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => dsn,
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            return;
        }
    };
    drop_schema(&dsn);
    the_repair_finds_and_mends(Some(&dsn));
    drop_schema(&dsn);
}
