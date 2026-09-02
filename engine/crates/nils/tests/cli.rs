// SPDX-License-Identifier: AGPL-3.0-only

//! The `nils` binary: `init`, `key`, `digest` and `status` round trips on a
//! SQLite home, `digest --dry-run` over a small tree, `--describe`, the exit
//! codes.

use std::io::Write as _;
use std::process::{Command, Stdio};

use nils_dicom::synth::{self, MetaFields, TempDir};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn tree() -> TempDir {
    let dir = TempDir::new("cli");
    let mr = synth::minimal_mr("1.2.3.A", "1.2.3.A.1", "1.2.3.A.1.1");
    dir.file(
        "a/IM_0001",
        &synth::part10(&MetaFields::mr("1.2.3.A.1.1"), &mr, true),
    );
    let ct = synth::minimal_ct("1.2.3.B", "1.2.3.B.1", "1.2.3.B.1.1");
    dir.file(
        "b/1.dcm",
        &synth::part10(&MetaFields::ct("1.2.3.B.1.1"), &ct, true),
    );
    dir.file("b/notes.txt", b"nothing\n");
    dir
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A home with the key `k` stored and a SQLite registry initialised.
fn home() -> TempDir {
    let home = TempDir::new("cli-home");
    let out = nils()
        .args(["--registry"])
        .arg(home.path())
        .args(["key", "add", "k"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"nils-cli-fixture-key\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let out = nils()
        .args(["--registry"])
        .arg(home.path())
        .args(["init", "--key", "k", "--display-length", "10"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    home
}

#[test]
fn dry_run_prints_the_report() {
    let dir = tree();
    let out = nils()
        .args(["digest", "--dry-run", "--workers", "2", "--name", "t"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("nils digest (dry run)   name t   root "),
        "{text}"
    );
    assert!(text.contains("3 seen   2 parsed   1 quarantined"), "{text}");
    assert!(text.contains("MR 1"), "{text}");
    assert!(text.contains("CT 1"), "{text}");
}

#[test]
fn dry_run_json_is_one_document() {
    let dir = tree();
    let out = nils()
        .args(["digest", "--dry-run", "--json", "--files", "dcm"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["seen"], 1);
    assert_eq!(report["parsed"], 1);
    assert_eq!(report["filtered"], 2);
    assert_eq!(report["files"], "dcm");
    assert_eq!(report["dry_run"], true);
    assert!(report["name"].as_str().unwrap().len() > 10);
    assert!(report.get("written").is_none());
}

#[test]
fn describe_prints_the_knobs() {
    let dir = tree();
    let out = nils()
        .args([
            "digest",
            "--describe",
            "--files",
            "no-ext",
            "--workers",
            "5",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("files              no-ext"), "{text}");
    assert!(text.contains("workers            5"), "{text}");
    assert!(text.contains("retry_quarantine"), "{text}");
}

#[test]
fn init_key_digest_and_status_go_round() {
    let home = home();
    let dir = tree();
    let registry = ["--registry", home.path().to_str().unwrap()];

    // the key store lists the key the registry uses, marked, and keeps it
    let out = nils()
        .args(registry)
        .args(["key", "list"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("* k "), "{text}");
    assert!(text.contains("20 bytes"), "{text}");
    assert!(!text.contains("nils-cli-fixture-key"), "{text}");
    let out = nils()
        .args(registry)
        .args(["key", "remove", "k"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("k"), "{}", stderr(&out));

    // a second init refuses
    let out = nils()
        .args(registry)
        .args(["init", "--key", "k"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("already"), "{}", stderr(&out));

    // a real digest writes and reports what it wrote
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "2", "--name", "first", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["seen"], 3);
    assert_eq!(report["parsed"], 2);
    assert_eq!(report["quarantined"], 1);
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["written"]["batch_id"], 1);
    assert_eq!(report["written"]["ingested"], 2);
    assert_eq!(report["written"]["subjects_created"], 2);
    let epoch = report["written"]["epoch"].as_i64().unwrap();
    assert!(epoch >= 1);

    // the same tree again is all unchanged
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "2", "--name", "second"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("nils digest   name second   root "),
        "{text}"
    );
    assert!(text.contains("3 seen"), "{text}");
    assert!(text.contains("3 unchanged"), "{text}");

    // status shows the registry and both batches
    let out = nils().args(registry).args(["status"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("nils status   registry "), "{text}");
    assert!(text.contains("backend sqlite"), "{text}");
    assert!(
        text.contains("blake2b-32 from key k, 10 characters shown"),
        "{text}"
    );
    assert!(text.contains("running jobs\n  none"), "{text}");
    assert!(text.contains("batches (last 2)"), "{text}");
    assert!(text.contains(" done     first "), "{text}");
    assert!(text.contains(" done     second "), "{text}");

    let out = nils()
        .args(registry)
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["registry"]["backend"], "sqlite");
    assert_eq!(doc["registry"]["pseudonym_key"], "k");
    assert_eq!(doc["registry"]["display_length"], 10);
    // the second run wrote its bookkeeping too, so the epoch moved on
    let epoch_now = doc["registry"]["epoch"].as_i64().unwrap();
    assert!(epoch_now > epoch, "{epoch_now} after {epoch}");
    assert_eq!(doc["jobs"].as_array().unwrap().len(), 0);
    let batches = doc["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0]["id"], 2);
    assert_eq!(batches[0]["name"], "second");
    assert_eq!(batches[0]["state"], "done");
    assert_eq!(batches[0]["seen"], 3);
    assert_eq!(batches[0]["ingested"], 0);
    assert_eq!(batches[0]["epoch_after"], epoch_now);
    assert_eq!(batches[1]["id"], 1);
    assert_eq!(batches[1]["ingested"], 2);
    assert_eq!(batches[1]["epoch_after"], epoch);

    // one batch's report comes back as it was printed
    let out = nils()
        .args(registry)
        .args(["status", "--batch", "1", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let again: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(again["seen"], report["seen"]);
    assert_eq!(again["written"], report["written"]);
    let out = nils()
        .args(registry)
        .args(["status", "--batch", "1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("nils digest   name first   root "),
        "{text}"
    );
    let out = nils()
        .args(registry)
        .args(["status", "--batch", "9"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no batch 9"), "{}", stderr(&out));

    // the home is found through the environment as well
    let out = nils()
        .env("NILS_REGISTRY", home.path())
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    // a second key can be added from a file and removed again
    let key_file = home.file("second.key", b"another-fixture-key");
    let out = nils()
        .args(registry)
        .args(["key", "add", "k2", "--from-file"])
        .arg(&key_file)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("added key k2 (19 bytes,"),
        "{}",
        stdout(&out)
    );
    let out = nils()
        .args(registry)
        .args(["key", "remove", "k2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let out = nils()
        .args(registry)
        .args(["key", "list"])
        .output()
        .unwrap();
    assert!(!stdout(&out).contains("k2"), "{}", stdout(&out));
}

#[test]
fn exit_codes_say_what_went_wrong() {
    let dir = tree();
    // a home that is no registry
    let empty = TempDir::new("cli-empty");
    let out = nils()
        .args(["--registry"])
        .arg(empty.path())
        .arg("digest")
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("no registry in"), "{}", stderr(&out));
    let out = nils()
        .args(["--registry"])
        .arg(empty.path())
        .arg("status")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    // init without its key
    let out = nils()
        .args(["--registry"])
        .arg(empty.path())
        .args(["init", "--key", "missing"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(!empty.path().join("nils.toml").exists());
    // the wrong words
    let out = nils()
        .args(["--registry"])
        .arg(empty.path())
        .args(["init", "--key", "k", "--backend", "oracle"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let out = nils()
        .args(["--registry"])
        .arg(empty.path())
        .args(["init", "--key", "k", "--scheme", "md5"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    // a bad glob
    let out = nils()
        .args(["digest", "--dry-run", "--files", "["])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // no workers at all
    let out = nils()
        .args(["digest", "--dry-run", "--batch-rows", "0"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--batch-rows"), "{}", stderr(&out));
    // a root that is not there
    let out = nils()
        .args(["digest", "--dry-run"])
        .arg(dir.path().join("nope"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("cannot list"));
    // no arguments at all
    let out = nils().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
