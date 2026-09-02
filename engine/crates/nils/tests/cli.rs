// SPDX-License-Identifier: AGPL-3.0-only

//! The `nils` binary: `digest --dry-run` over a small tree, `--describe`, the
//! exit codes.

use std::process::Command;

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

#[test]
fn dry_run_prints_the_report() {
    let dir = tree();
    let out = nils()
        .args(["digest", "--dry-run", "--workers", "2", "--name", "t"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
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
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("files              no-ext"), "{text}");
    assert!(text.contains("workers            5"), "{text}");
    assert!(text.contains("retry_quarantine"), "{text}");
}

#[test]
fn exit_codes_say_what_went_wrong() {
    let dir = tree();
    // the writer is not here yet
    let out = nils().arg("digest").arg(dir.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--dry-run"));
    // a bad glob
    let out = nils()
        .args(["digest", "--dry-run", "--files", "["])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // a root that is not there
    let out = nils()
        .args(["digest", "--dry-run"])
        .arg(dir.path().join("nope"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot list"));
    // no arguments at all
    let out = nils().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
