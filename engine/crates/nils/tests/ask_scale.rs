// SPDX-License-Identifier: AGPL-3.0-only

//! The documents of the scale fixture (`engine/benches/ask-scale`) answer
//! what their reference keeps. The reference was taken before the compiler
//! was changed for speed, on the scale registry of `nils ask-scale build`
//! at a size a test builds in seconds, and it holds for every later
//! compiler: a change to the statement an ask compiles to may make it
//! faster, and may not change one row of its answer, on either backend.
//! The same documents on the registry of production size are timed by hand
//! with `nils ask-scale time`, against `reference.json`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use nils_dicom::synth::TempDir;

const SCHEMA: &str = "nils_ask_scale_test";

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn engine() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
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

fn drop_schema(dsn: &str) {
    if let Ok(mut store) = nils_registry::Store::connect_postgres(dsn, "public") {
        let _ = store.batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ));
    }
}

/// Build the scale registry at the reference's size and hold every
/// document's answer against the reference.
fn answers_as_kept(dsn: Option<&str>) {
    let home = TempDir::new("ask-scale-home");
    run(&home, dsn, &["key", "add", "k"], Some("an ask scale key\n"));
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
        &["ask-scale", "build", "--seed", "7", "--subjects", "150"],
        None,
    );
    let fixtures = engine().join("benches/ask-scale");
    let out = run(
        &home,
        dsn,
        &[
            "ask-scale",
            "time",
            "--pack-dir",
            engine().join("../packs").to_str().unwrap(),
            "--fixtures",
            fixtures.to_str().unwrap(),
            "--runs",
            "1",
            "--reference",
            fixtures.join("reference-small.json").to_str().unwrap(),
        ],
        None,
    );
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let shapes = doc["shapes"].as_array().unwrap();
    assert!(shapes.len() >= 14, "{out}");
    for s in shapes {
        assert_eq!(s["same_as_reference"], true, "{s}");
        assert!(s["error"].is_null(), "{s}");
    }
}

#[test]
fn the_scale_documents_answer_as_their_reference_keeps_them_on_sqlite() {
    answers_as_kept(None);
}

#[test]
fn the_scale_documents_answer_as_their_reference_keeps_them_on_postgres() {
    let dsn = match std::env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => dsn,
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            return;
        }
    };
    drop_schema(&dsn);
    answers_as_kept(Some(&dsn));
    drop_schema(&dsn);
}
