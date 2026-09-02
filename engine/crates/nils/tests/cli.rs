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

/// The elements a patient adds to a minimal dataset.
fn patient(id: &str, comments: Option<&str>) -> Vec<synth::Elem> {
    use dicom_core::VR;
    use dicom_dictionary_std::tags;
    let mut e = vec![synth::text(tags::PATIENT_ID, VR::LO, id)];
    if let Some(c) = comments {
        e.push(synth::text(tags::PATIENT_COMMENTS, VR::LT, c));
    }
    e
}

fn mr_of(study: &str, sop: &str, elems: Vec<synth::Elem>) -> Vec<u8> {
    let mut e = synth::minimal_mr(study, &format!("{study}.1"), sop);
    e.extend(elems);
    synth::part10(&MetaFields::mr(sop), &e, true)
}

/// Three patients: P1 with two files, P2, and P3 whose comments carry another id.
fn patients() -> TempDir {
    let dir = TempDir::new("cli-patients");
    dir.file(
        "p1/IM_0001",
        &mr_of("1.2.3.A", "1.2.3.A.1.1", patient("P1", None)),
    );
    dir.file(
        "p1/IM_0002",
        &mr_of("1.2.3.A", "1.2.3.A.1.2", patient("P1", None)),
    );
    dir.file(
        "p2/IM_0001",
        &mr_of("1.2.3.B", "1.2.3.B.1.1", patient("P2", None)),
    );
    dir.file(
        "p3/IM_0001",
        &mr_of(
            "1.2.3.C",
            "1.2.3.C.1.1",
            patient("P3", Some("id=42 (moved)")),
        ),
    );
    dir
}

/// The code the CLI's registry derives for an identifier.
fn code_of(id: &str) -> String {
    nils_registry::pseudonym::code(
        nils_registry::Scheme::Blake2b32,
        b"nils-cli-fixture-key",
        id,
        10,
    )
    .code
}

#[test]
fn import_digest_show_and_link_go_round() {
    let home = home();
    let dir = patients();
    let registry = ["--registry", home.path().to_str().unwrap()];
    let run = |args: &[&str]| {
        nils()
            .args(registry)
            .args(args)
            .env("USER", "tester")
            .output()
            .unwrap()
    };

    // the store starts with the two id types of §7.1
    let out = run(&["linkage", "id-type", "list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("  1  patient-id"), "{text}");
    assert!(text.contains("  2  study-instance-uid"), "{text}");
    let out = run(&[
        "linkage",
        "id-type",
        "add",
        "mrn",
        "--description",
        "hospital number",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "added id type mrn (id 3)\n");
    let out = run(&["linkage", "id-type", "list"]);
    assert!(
        stdout(&out).contains("  3  mrn                      hospital number"),
        "{}",
        stdout(&out)
    );
    let out = run(&["linkage", "id-type", "add", "Bad Name"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    // a CSV of legacy codes comes in under patient-id
    let csv = home.file("legacy.csv", b"code,identifier\nlegacy-0001,P1\n");
    let out = run(&["linkage", "import", csv.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "imported 1 row(s) as patient-id: 1 subject(s) created, 1 identifier(s) filed, 0 already filed\n"
    );
    let out = run(&["linkage", "import", csv.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("0 subject(s) created, 0 identifier(s) filed, 1 already filed"),
        "{}",
        stdout(&out)
    );

    // a column the header lacks is a usage error; refused rows fail whole
    let out = run(&[
        "linkage",
        "import",
        csv.to_str().unwrap(),
        "--id-column",
        "pid",
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out)
            .contains("no column \"pid\" in the header (code, identifier); --id-column names it"),
        "{}",
        stderr(&out)
    );
    let bad = home.file(
        "bad.csv",
        b"identifier,code\nP1,legacy-0002\n,legacy-0003\nP5,legacy-0005\n",
    );
    let out = run(&["linkage", "import", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stderr(&out);
    assert!(
        text.contains("2 row(s) refused; nothing was written:"),
        "{text}"
    );
    assert!(
        text.contains("line 2: the identifier already maps to subject legacy-0001"),
        "{text}"
    );
    assert!(
        text.contains("line 3: an empty identifier or code"),
        "{text}"
    );
    assert!(!text.contains("P1"), "{text}");
    let out = run(&["linkage", "show", "legacy-0005"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        stderr(&out).trim(),
        "nils: no subject with code legacy-0005"
    );

    // the digest keeps the imported code and derives the others
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "1", "--name", "first", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["parsed"], 4);
    assert_eq!(report["written"]["subjects_created"], 2);
    assert_eq!(report["written"]["subjects_matched"], 1);
    assert_eq!(report["written"]["identities_attached"], 0);
    assert_eq!(report["written"]["studies_created"], 3);
    let p2 = code_of("P2");
    let p3 = code_of("P3");
    assert_ne!(p2, p3);
    assert_eq!(p2.len(), 10);

    // show reveals the identifier and says where it came from
    let out = run(&["linkage", "show", "legacy-0001", "--why", "a test"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("subject legacy-0001 (id 1)\n"), "{text}");
    assert!(
        text.contains("  patient-id               P1   (identity 1, from csv)\n"),
        "{text}"
    );
    assert!(!text.contains("linkages"), "{text}");
    let out = run(&["linkage", "show", &p2]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("P2   (identity "), "{text}");
    assert!(text.contains(", from dicom)"), "{text}");
    let out = run(&["linkage", "show", "nope"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no subject with code nope"),
        "{}",
        stderr(&out)
    );

    // the same tree again meets every identifier
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "1", "--name", "second", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["unchanged"], 4);
    assert_eq!(report["written"]["subjects_created"], 0);

    // a linkage joins two subjects and can be reversed once
    let out = run(&[
        "linkage",
        "link",
        "legacy-0001",
        &p2,
        "--evidence",
        "the clinic renamed P1 to P2",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("linked {p2} to legacy-0001 (linkage 1)\n")
    );
    let out = run(&["linkage", "show", "legacy-0001"]);
    let text = stdout(&out);
    assert!(text.contains("linkages\n"), "{text}");
    assert!(
        text.contains(&format!("     1  canonical of {p2}   ")),
        "{text}"
    );
    assert!(text.contains("the clinic renamed P1 to P2"), "{text}");
    assert!(text.contains("   by tester at "), "{text}");
    assert!(text.trim_end().ends_with("   open"), "{text}");
    let out = run(&["linkage", "show", &p2]);
    assert!(
        stdout(&out).contains("     1  alias of legacy-0001   "),
        "{}",
        stdout(&out)
    );
    let out = run(&["linkage", "unlink", "1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "reversed linkage 1\n");
    let out = run(&["linkage", "unlink", "1"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no open linkage 1"),
        "{}",
        stderr(&out)
    );
    let out = run(&["linkage", "show", "legacy-0001"]);
    let text = stdout(&out);
    assert!(text.contains("   reversed "), "{text}");
    assert!(text.contains(" by tester\n"), "{text}");
    let out = run(&["linkage", "link", "legacy-0001", "nope", "--evidence", "x"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
}

#[test]
fn an_identity_rule_comes_from_a_file() {
    let home = home();
    let dir = patients();
    let registry = ["--registry", home.path().to_str().unwrap()];
    let rule = home.file(
        "rule.yaml",
        b"identity:\n  id_type: patient-id\n  from:\n    - field: PatientComments\n      pattern: 'id=(?<id>[0-9]+)'\n    - field: PatientID\n",
    );
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "1", "--json", "--identity-rule"])
        .arg(&rule)
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["written"]["subjects_created"], 3);
    let out = nils()
        .args(registry)
        .args(["linkage", "show", &code_of("42"), "--why", "a test"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("  patient-id               42   (identity "),
        "{text}"
    );
    let out = nils()
        .args(registry)
        .args(["linkage", "show", &code_of("P3")])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    // the rule is checked before anything runs
    let bad = home.file("bad.yaml", b"identity:\n  id_type: patient-id\n  from:\n    - field: PatientComments\n      pattern: 'id=([0-9]+)'\n");
    let out = nils()
        .args(registry)
        .args(["digest", "--dry-run", "--identity-rule"])
        .arg(&bad)
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let text = stderr(&out);
    assert!(text.contains("--identity-rule"), "{text}");
    assert!(text.contains("bad.yaml"), "{text}");
    let out = nils()
        .args(registry)
        .args(["digest", "--dry-run", "--identity-rule"])
        .arg(home.path().join("missing.yaml"))
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("missing.yaml"), "{}", stderr(&out));
}

#[test]
fn a_blake2b_8_registry_gives_the_v0_code() {
    let home = TempDir::new("cli-v0");
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
                .write_all(b"nils-fixture-key\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let out = nils()
        .args(["--registry"])
        .arg(home.path())
        .args(["init", "--key", "k", "--scheme", "blake2b-8"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let dir = TempDir::new("cli-v0-tree");
    dir.file(
        "s/IM_0001",
        &mr_of("1.2.3.A", "1.2.3.A.1.1", patient("PID-0001", None)),
    );
    dir.file(
        "s/IM_0002",
        &mr_of("1.2.3.B", "1.2.3.B.1.1", patient(" PID-0001 ", None)),
    );
    let out = nils()
        .args(["--registry"])
        .arg(home.path())
        .args(["digest", "--workers", "1", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["written"]["subjects_created"], 1);
    assert_eq!(report["written"]["studies_created"], 2);
    // the fixture's code under v0's scheme (§7.1)
    let out = nils()
        .args(["--registry"])
        .arg(home.path())
        .args(["linkage", "show", "771c4326c89c082c", "--why", "a test"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("subject 771c4326c89c082c (id 1)\n"),
        "{text}"
    );
    assert!(
        text.contains("  patient-id               PID-0001   (identity 1, from dicom)\n"),
        "{text}"
    );
}
