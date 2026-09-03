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
    assert!(text.contains(" done      first "), "{text}");
    assert!(text.contains(" done      second "), "{text}");

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

/// Twelve subjects, two studies each, two series a study, six instances a
/// series: 288 DICOM files, one file that is not DICOM, one duplicate.
fn corpus() -> TempDir {
    let dir = TempDir::new("cli-corpus");
    for s in 1..=12 {
        for st in 1..=2 {
            let study = format!("1.2.826.0.1.3680043.8.498.{s}.{st}");
            for se in 1..=2 {
                let series = format!("{study}.{se}");
                for i in 1..=6 {
                    let sop = format!("{series}.{i}");
                    let mut e = synth::minimal_mr(&study, &series, &sop);
                    e.extend(patient(&format!("S{s:02}"), None));
                    dir.file(
                        &format!("s{s:02}/st{st}/se{se}/IM_{i:04}"),
                        &synth::part10(&MetaFields::mr(&sop), &e, true),
                    );
                }
            }
        }
    }
    dir.file("s01/notes.txt", b"not a dicom file\n");
    let first = std::fs::read(dir.path().join("s01/st1/se1/IM_0001")).unwrap();
    dir.file("s01/st1/se1/IM_0001_copy", &first);
    dir
}

/// What a registry holds, as far as two runs over the same tree must agree.
#[derive(Debug, PartialEq)]
struct Snapshot {
    counts: Vec<(String, i64)>,
    statuses: Vec<(String, i64)>,
    uids: Vec<String>,
    identities: i64,
}

fn snapshot(home: &TempDir) -> Snapshot {
    use nils_registry::Store;
    let mut reg = Store::open_sqlite(&home.path().join("registry.db")).unwrap();
    let mut counts = Vec::new();
    for t in ["subject", "study", "series", "stack", "instance"] {
        let n = reg
            .query(&format!("SELECT COUNT(*) FROM {t}"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        counts.push((t.to_string(), n));
    }
    for (t, what) in [
        ("series", "n_instances"),
        ("series", "n_stacks"),
        ("stack", "n_instances"),
    ] {
        let n = reg
            .query(&format!("SELECT COALESCE(SUM({what}), 0) FROM {t}"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        counts.push((format!("{t}.{what}"), n));
    }
    let statuses = reg
        .query(
            "SELECT status, COUNT(*) FROM source_file GROUP BY status ORDER BY status",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| (r.text(0).unwrap().to_string(), r.int(1).unwrap()))
        .collect();
    let mut uids = Vec::new();
    for (t, c) in [
        ("subject", "code"),
        ("study", "study_instance_uid"),
        ("series", "series_instance_uid"),
        ("instance", "sop_instance_uid"),
    ] {
        uids.extend(
            reg.query(&format!("SELECT {c} FROM {t} ORDER BY {c}"), &[])
                .unwrap()
                .iter()
                .map(|r| format!("{t}:{}", r.text(0).unwrap())),
        );
    }
    let mut linkage = Store::open_sqlite(&home.path().join("linkage.db")).unwrap();
    let identities = linkage.query("SELECT COUNT(*) FROM identity", &[]).unwrap()[0]
        .int(0)
        .unwrap();
    Snapshot {
        counts,
        statuses,
        uids,
        identities,
    }
}

/// `nils digest` of `dir` into `home`, small batches so that a run has many
/// transactions, with a scripted stop when `stop` says so.
fn run(home: &TempDir, dir: &TempDir, stop: Option<&str>) -> std::process::Output {
    let mut cmd = nils();
    cmd.args(["--registry"])
        .arg(home.path())
        .args(["digest", "--workers", "2", "--batch-rows", "20", "--json"])
        .arg(dir.path());
    match stop {
        Some(s) => cmd.env("NILS_DEBUG_STOP", s),
        None => cmd.env_remove("NILS_DEBUG_STOP"),
    };
    cmd.output().unwrap()
}

fn status_json(home: &TempDir) -> serde_json::Value {
    let out = nils()
        .args(["--registry"])
        .arg(home.path())
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The reference: one run to the end over the corpus.
fn reference(dir: &TempDir) -> (Snapshot, serde_json::Value) {
    let home = home();
    let out = run(&home, dir, None);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["seen"], 290);
    assert_eq!(report["parsed"], 289);
    assert_eq!(report["quarantined"], 1);
    assert_eq!(report["written"]["ingested"], 288);
    assert_eq!(report["written"]["duplicate"], 1);
    assert_eq!(report["written"]["subjects_created"], 12);
    assert!(
        report["written"]["writes"].as_u64().unwrap() >= 8,
        "{report}"
    );
    let snap = snapshot(&home);
    assert_eq!(snap.counts[0], ("subject".to_string(), 12));
    assert_eq!(snap.counts[4], ("instance".to_string(), 288));
    assert_eq!(snap.identities, 12);
    (snap, report)
}

#[test]
fn a_digest_killed_after_a_commit_resumes_to_the_same_rows() {
    let dir = corpus();
    let (reference, _) = reference(&dir);
    for script in ["kill:1", "kill:3"] {
        let home = home();

        // the process ends right after a commit, before the identities of
        // that transaction reach the linkage store (§9.3)
        let out = run(&home, &dir, Some(script));
        assert!(!out.status.success(), "{script}: {}", stdout(&out));
        let doc = status_json(&home);
        assert_eq!(doc["jobs"].as_array().unwrap().len(), 1, "{script}: {doc}");
        assert_eq!(doc["batches"][0]["state"], "running", "{script}: {doc}");
        let before = snapshot(&home);
        assert!(before.counts[4].1 >= 1, "{script}: {before:?}");
        if script == "kill:1" {
            // the first transaction created subjects; none of their
            // identities made it
            assert!(before.counts[0].1 >= 1, "{before:?}");
            assert_eq!(before.identities, 0, "{before:?}");
        }

        // the next run takes the dead job over, reads the last second of its
        // batch again and goes on to the end
        let out = run(&home, &dir, None);
        assert!(out.status.success(), "{script}: {}", stderr(&out));
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["seen"], 290, "{script}");
        if script == "kill:1" {
            assert_eq!(
                report["written"]["identities_attached"], before.counts[0].1,
                "{script}: {report}"
            );
        }
        assert_eq!(snapshot(&home), reference, "{script}");
        let doc = status_json(&home);
        assert_eq!(doc["jobs"].as_array().unwrap().len(), 0, "{script}: {doc}");
        assert_eq!(doc["batches"][0]["state"], "done", "{script}: {doc}");
        assert_eq!(doc["batches"][1]["state"], "failed", "{script}: {doc}");

        // a third run finds everything in place
        let out = run(&home, &dir, None);
        assert!(out.status.success(), "{script}: {}", stderr(&out));
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["unchanged"], 290, "{script}");
        assert_eq!(snapshot(&home), reference, "{script}");
    }
}

#[test]
fn a_digest_killed_inside_a_transaction_resumes_to_the_same_rows() {
    let dir = corpus();
    let (reference, _) = reference(&dir);
    let home = home();
    let out = run(&home, &dir, Some("kill-inside:1"));
    assert!(!out.status.success(), "{}", stdout(&out));
    let before = snapshot(&home);
    assert!(before.counts[4].1 < 288, "{before:?}");

    let out = run(&home, &dir, None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(snapshot(&home), reference);
    let doc = status_json(&home);
    assert_eq!(doc["jobs"].as_array().unwrap().len(), 0, "{doc}");
    assert_eq!(doc["batches"][0]["state"], "done", "{doc}");
    assert_eq!(doc["batches"][1]["state"], "failed", "{doc}");
}

#[test]
fn a_stopped_digest_writes_what_it_read_and_resumes_to_the_same_rows() {
    let dir = corpus();
    let (reference, _) = reference(&dir);
    for (script, word) in [("stop:1", "stopped"), ("abort:1", "aborted")] {
        let home = home();
        let out = run(&home, &dir, Some(script));
        assert_eq!(out.status.code(), Some(130), "{script}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(&format!("nils: {word}:")),
            "{script}: {}",
            stderr(&out)
        );
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["cancelled"], word, "{script}");
        assert!(report["seen"].as_u64().unwrap() < 290, "{script}: {report}");
        let doc = status_json(&home);
        assert_eq!(doc["jobs"].as_array().unwrap().len(), 0, "{script}: {doc}");
        assert_eq!(doc["batches"][0]["state"], "cancelled", "{script}: {doc}");
        let partial = snapshot(&home);
        assert!(partial.counts[4].1 >= 1, "{script}: {partial:?}");
        assert!(partial.counts[4].1 < 288, "{script}: {partial:?}");

        let out = run(&home, &dir, None);
        assert!(out.status.success(), "{script}: {}", stderr(&out));
        assert_eq!(snapshot(&home), reference, "{script}");
        let doc = status_json(&home);
        assert_eq!(doc["batches"][0]["state"], "done", "{script}: {doc}");
    }
}

#[cfg(unix)]
#[test]
fn an_interrupted_digest_stops_and_resumes_to_the_same_rows() {
    let dir = corpus();
    let (reference, _) = reference(&dir);
    let home = home();
    let out = run(&home, &dir, Some("interrupt:1"));
    assert_eq!(out.status.code(), Some(130), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("nils: stopping: what is read is written"),
        "{}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["cancelled"], "stopped");
    let text_run = nils()
        .args(["--registry"])
        .arg(home.path())
        .args(["status"])
        .output()
        .unwrap();
    assert!(
        stdout(&text_run).contains(" cancelled "),
        "{}",
        stdout(&text_run)
    );

    let out = run(&home, &dir, None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(snapshot(&home), reference);
}

/// A file's path, relative to `home`, for every file under it.
fn files_under(home: &TempDir) -> Vec<String> {
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path.display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(home.path(), &mut out);
    out.sort();
    out
}

#[test]
fn custody_quarantine_review_and_purge_go_round() {
    let home = home();
    let dir = patients();
    dir.file("p1/notes.txt", b"nothing\n");
    let registry = ["--registry", home.path().to_str().unwrap()];
    let run = |args: &[&str]| {
        nils()
            .args(registry)
            .args(args)
            .env("USER", "tester")
            .env_remove("NILS_DSN")
            .output()
            .unwrap()
    };
    let json = |out: &std::process::Output| -> serde_json::Value {
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(out)))
    };

    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "1", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report = json(&out);
    assert_eq!(report["quarantined"], 1);
    assert_eq!(report["written"]["subjects_created"], 3);

    // the quarantine list names the file, its class and its batch
    let notes = dir.path().join("p1").join("notes.txt");
    let notes = notes.to_str().unwrap();
    let out = run(&["quarantine", "list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("   1 file(s)"), "{text}");
    assert!(
        text.contains(&format!("      1  not_dicom      {notes}")),
        "{text}"
    );
    let out = run(&["quarantine", "list", "--batch", "1", "--class", "not_dicom"]);
    assert!(stdout(&out).contains(notes), "{}", stdout(&out));
    let out = run(&["quarantine", "list", "--batch", "2"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("   0 file(s)   batch 2"),
        "{}",
        stdout(&out)
    );
    let out = run(&["quarantine", "list", "--class", "missing_uid", "--json"]);
    assert_eq!(json(&out)["count"], 0);
    let out = run(&["quarantine", "list", "--json"]);
    let doc = json(&out);
    assert_eq!(doc["count"], 1);
    assert_eq!(doc["files"][0]["batch_id"], 1);
    assert_eq!(doc["files"][0]["class"], "not_dicom");
    assert_eq!(doc["files"][0]["path"], notes);
    assert!(doc["files"][0]["seen_at"].as_str().unwrap().ends_with('Z'));

    // one review item groups the class, with the count and no path
    let out = run(&["review", "list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("   1 item(s)"), "{text}");
    assert!(
        text.contains("     1  ingest.quarantine    open       batch    "),
        "{text}"
    );
    assert!(
        text.contains("batch 1, class not_dicom, 1 file(s)"),
        "{text}"
    );
    assert!(!text.contains("notes.txt"), "{text}");
    let out = run(&["review", "list", "--kind", "identity.collision"]);
    assert!(stdout(&out).contains("   0 item(s)"), "{}", stdout(&out));
    let out = run(&[
        "review",
        "list",
        "--status",
        "open",
        "--kind",
        "ingest.quarantine",
        "--json",
    ]);
    let doc = json(&out);
    assert_eq!(doc["count"], 1);
    assert_eq!(doc["items"][0]["ref"]["batch_id"], 1);
    assert_eq!(doc["items"][0]["ref"]["class"], "not_dicom");
    assert_eq!(doc["items"][0]["evidence"]["count"], 1);
    assert!(doc["items"][0]["decided_at"].is_null());
    let out = run(&["review", "show", "1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("review item 1   ingest.quarantine   open\n"),
        "{text}"
    );
    assert!(
        text.contains("  about      batch 1, class not_dicom, 1 file(s)\n"),
        "{text}"
    );
    assert!(text.contains("  evidence   {\"count\":1}\n"), "{text}");
    assert!(!text.contains("decided"), "{text}");
    let out = run(&["review", "show", "1", "--json"]);
    assert_eq!(json(&out)["id"], 1);
    let out = run(&["review", "show", "9"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no review item 9"),
        "{}",
        stderr(&out)
    );

    // custody lists every file under the home and every command it names exists
    let out = run(&["custody", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["home"], home.path().to_str().unwrap());
    assert_eq!(doc["backend"], "sqlite");
    let stores = doc["stores"].as_array().unwrap();
    let names: Vec<&str> = stores
        .iter()
        .map(|s| s["store"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "configuration",
            "registry",
            "linkage store",
            "key store",
            "quarantine list",
            "classifications",
            "job records",
            "logs"
        ]
    );
    let listed: Vec<String> = stores
        .iter()
        .flat_map(|s| s["files"].as_array().unwrap())
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    for file in files_under(&home) {
        assert!(
            listed.contains(&file),
            "{file} is not in custody: {listed:?}"
        );
    }
    let by_name = |name: &str| stores.iter().find(|s| s["store"] == name).unwrap();
    assert_eq!(by_name("registry")["counts"]["subjects"], 3);
    assert_eq!(by_name("registry")["counts"]["instances"], 4);
    assert_eq!(by_name("registry")["counts"]["source_files"], 5);
    assert_eq!(by_name("linkage store")["counts"]["identities"], 3);
    assert_eq!(by_name("linkage store")["counts"]["audited_reads"], 0);
    assert_eq!(by_name("key store")["counts"]["keys"], 1);
    assert_eq!(by_name("key store")["counts"]["in_use"], "k");
    assert_eq!(by_name("quarantine list")["counts"]["files"], 1);
    assert_eq!(by_name("quarantine list")["counts"]["open_review_items"], 1);
    assert_eq!(by_name("job records")["counts"]["jobs"], 1);
    assert_eq!(by_name("logs")["where"], "nowhere");
    let mut commands = Vec::new();
    for st in stores {
        let c = &st["commands"];
        for key in ["read", "change", "export"] {
            commands.extend(
                c[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string()),
            );
        }
        commands.push(c["delete"].as_str().unwrap().to_string());
    }
    let mut checked = 0;
    for command in &commands {
        let Some(rest) = command.strip_prefix("nils ") else {
            continue;
        };
        let words: Vec<&str> = rest
            .split_whitespace()
            .take_while(|w| w.chars().next().unwrap().is_ascii_lowercase())
            .collect();
        let out = run(&[words.as_slice(), &["--help"]].concat());
        assert!(
            out.status.success(),
            "custody names {command:?} but nils {words:?} --help fails: {}",
            stderr(&out)
        );
        checked += 1;
    }
    assert!(checked >= 12, "{checked} commands checked: {commands:?}");
    let out = run(&["custody"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("nils custody   registry "), "{text}");
    assert!(
        text.contains("nothing is retained that is not listed here"),
        "{text}"
    );
    for name in names {
        assert!(text.contains(&format!("\n{name}\n")), "{name}: {text}");
    }
    assert!(
        text.contains("  delete    nils linkage purge --subject <code> | --all"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "  where     {}   ",
            home.path().join("registry.db").display()
        )),
        "{text}"
    );
    assert!(
        !text.contains("SQLite keeps"),
        "the live view lists the files instead: {text}"
    );
    assert!(
        text.contains(&format!(
            "  where     {}   directory, mode 700\n",
            home.path().join("keys").display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "            {}   20 bytes, mode 600\n",
            home.path().join("keys").join("k").display()
        )),
        "{text}"
    );
    assert!(
        text.contains("  now       3 subjects, 3 studies, 3 series, 4 instances, 5 source files"),
        "{text}"
    );
    assert!(
        text.contains("  now       1 file, 1 class, 1 open review item\n"),
        "{text}"
    );
    assert!(text.contains("  now       1 key, in use k\n"), "{text}");
    assert!(text.contains("  now       1 job, 1 batch\n"), "{text}");

    // purge says what it would delete and refuses without a yes
    let p2 = code_of("P2");
    let p3 = code_of("P3");
    let out = run(&["linkage", "link", &p2, &p3, "--evidence", "same person"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = run(&["linkage", "purge"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--subject <code> or --all"),
        "{}",
        stderr(&out)
    );
    let out = run(&["linkage", "purge", "--subject", &p2, "--all"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let out = run(&["linkage", "purge", "--subject", &p2]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("add --yes to confirm"),
        "{}",
        stderr(&out)
    );
    let out = run(&["linkage", "purge", "--subject", "nope", "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let out = run(&["linkage", "show", &p2]);
    assert!(stdout(&out).contains("P2   (identity "), "{}", stdout(&out));

    // with a yes the subject's identifiers and linkages go; the audit and the subject stay
    let out = run(&["linkage", "purge", "--subject", &p2, "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "purged 1 identifier(s) and 1 linkage(s) of subject {p2}; the read audit and the registry's subjects stay, and a file parsed again files its identifier again (an unchanged file does not)\n"
        )
    );
    let out = run(&["linkage", "show", &p2]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("  no identifiers\n"), "{text}");
    assert!(!text.contains("linkages"), "{text}");
    let out = run(&["linkage", "show", &p3]);
    assert!(stdout(&out).contains("P3   (identity "), "{}", stdout(&out));
    let out = run(&["custody", "--json"]);
    let doc = json(&out);
    let by_name = |name: &str| {
        doc["stores"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["store"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(by_name("linkage store")["counts"]["identities"], 2);
    assert_eq!(by_name("linkage store")["counts"]["open_linkages"], 0);
    assert_eq!(
        by_name("linkage store")["counts"]["audited_reads"],
        2,
        "one row per identifier revealed; a show of a subject without identifiers reads nothing"
    );
    assert_eq!(by_name("registry")["counts"]["subjects"], 3);
    assert_eq!(by_name("job records")["counts"]["jobs"], 2);

    // an unchanged digest does not file the identifier again; a parsed file does
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "1", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report = json(&out);
    assert_eq!(report["unchanged"], 5);
    assert_eq!(report["written"]["subjects_created"], 0);
    assert_eq!(report["written"]["identities_attached"], 0);
    let out = run(&["linkage", "show", &p2]);
    assert!(
        stdout(&out).contains("  no identifiers\n"),
        "{}",
        stdout(&out)
    );
    dir.file(
        "p2/IM_0001",
        &mr_of("1.2.3.B", "1.2.3.B.1.1", patient("P2", Some("revisited"))),
    );
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "1", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report = json(&out);
    assert_eq!(report["unchanged"], 4);
    assert_eq!(report["written"]["changed"], 1);
    assert_eq!(report["written"]["subjects_created"], 0);
    assert_eq!(report["written"]["identities_attached"], 1);
    let out = run(&["linkage", "show", &p2]);
    assert!(stdout(&out).contains("P2   (identity "), "{}", stdout(&out));

    // --all empties the store
    let out = run(&["linkage", "purge", "--all", "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("purged 3 identifier(s) and 0 linkage(s) of every subject;"),
        "{}",
        stdout(&out)
    );
    let mut store = nils_registry::Store::open_sqlite(&home.path().join("linkage.db")).unwrap();
    let rows = store.query("SELECT COUNT(*) FROM identity", &[]).unwrap();
    assert_eq!(rows[0].int(0).unwrap(), 0);
    let rows = store.query("SELECT COUNT(*) FROM read_audit", &[]).unwrap();
    assert_eq!(rows[0].int(0).unwrap(), 3, "the audit survives the purge");
    let out = run(&["linkage", "show", &p2]);
    assert!(
        stdout(&out).contains("  no identifiers\n"),
        "{}",
        stdout(&out)
    );
    let rows = store.query("SELECT COUNT(*) FROM id_type", &[]).unwrap();
    assert_eq!(rows[0].int(0).unwrap(), 2);
    // status lists the purges as the jobs they were
    let out = run(&["status", "--json"]);
    let doc = json(&out);
    assert_eq!(doc["jobs"].as_array().unwrap().len(), 0, "{doc}");
    let purges = doc["other_jobs"].as_array().unwrap();
    assert_eq!(purges.len(), 2, "{doc}");
    assert_eq!(purges[0]["kind"], "linkage-purge");
    assert_eq!(purges[0]["name"], "every subject");
    assert_eq!(purges[0]["state"], "done");
    assert_eq!(purges[0]["args"]["identities"], 3);
    assert_eq!(purges[0]["args"]["actor"], "tester");
    assert_eq!(purges[1]["name"], format!("subject {p2}"));
    assert_eq!(purges[1]["args"]["linkages"], 1);
    let out = run(&["status"]);
    let text = stdout(&out);
    assert!(text.contains("other jobs (last 2)\n"), "{text}");
    assert!(
        text.contains("   linkage-purge every subject   done on "),
        "{text}"
    );
}

/// The custody page under `docs/reference` is what `nils custody --markdown`
/// prints for a SQLite registry, with the home shown as `<home>`; set
/// `NILS_WRITE_REFERENCE=1` to rewrite it after a change.
#[test]
fn the_custody_page_is_current() {
    let home = home();
    let out = nils()
        .args([
            "--registry",
            home.path().to_str().unwrap(),
            "custody",
            "--markdown",
        ])
        .env_remove("NILS_DSN")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let rendered = stdout(&out).replace(home.path().to_str().unwrap(), "<home>");
    assert!(!rendered.contains("/tmp/"), "{rendered}");
    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/reference/custody.md")
        .canonicalize()
        .unwrap();
    if std::env::var_os("NILS_WRITE_REFERENCE").is_some() {
        std::fs::write(&page, &rendered).unwrap();
    }
    let current = std::fs::read_to_string(&page).unwrap();
    assert_eq!(
        current, rendered,
        "docs/reference/custody.md is stale; NILS_WRITE_REFERENCE=1 cargo test -p nils --test cli the_custody_page rewrites it"
    );
}

#[test]
fn fingerprint_derives_once_and_then_skips() {
    let dir = tree();
    let home = home();
    let registry: [&std::ffi::OsStr; 2] =
        [std::ffi::OsStr::new("--registry"), home.path().as_os_str()];
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "2", "--name", "t", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    let out = nils()
        .args(registry)
        .args(["fingerprint", "--name", "f", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let stacks = first["read"].as_i64().unwrap();
    assert!(stacks > 0, "{first}");
    assert_eq!(first["written"], stacks);
    assert_eq!(first["skipped"], 0);
    assert_eq!(first["cancelled"], false);

    // a second run derives nothing
    let out = nils()
        .args(registry)
        .args(["fingerprint", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let again: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(again["written"], 0);
    assert_eq!(again["skipped"], stacks);

    // and one modality is a subset of all of them
    let out = nils()
        .args(registry)
        .args(["fingerprint", "--modality", "CT", "--force", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let ct: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(ct["read"].as_i64().unwrap() < stacks, "{ct}");
    assert!(ct["read"].as_i64().unwrap() > 0, "{ct}");

    // the page says the same as the JSON
    let out = nils()
        .args(registry)
        .args(["fingerprint"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("stacks read"), "{}", stdout(&out));
}

#[test]
fn pack_validate_says_what_is_wrong_and_where() {
    let packs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs");

    // the pack in the repository loads, and says what it holds
    let out = nils()
        .args(["pack", "validate"])
        .arg(packs.join("mri"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let said = stdout(&out);
    assert!(said.contains("mri@"), "{said}");
    assert!(said.contains("220 predicates"), "{said}");
    assert!(said.contains("cases"), "{said}");

    // a pack that is wrong is refused, by file, line and path
    let dir = TempDir::new("cli-pack");
    dir.file(
        "pack.yml",
        b"pack: t\nversion: 1.0.0\ncontract: 1\nmodality: MR\nflags: [flags.yml]\n",
    );
    dir.file(
        "flags.yml",
        b"flags:\n  ok: {field: echo_time, gt: 0}\n  bad: {field: nope, gt: 0}\n",
    );
    dir.file(
        "corpus/c.yml",
        b"cases:\n  - {name: c, stack: {echo_time: 5}, flags: {ok: true}}\n",
    );
    let out = nils()
        .args(["pack", "validate"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let e = stderr(&out);
    assert!(e.contains("flags.yml:3:"), "{e}");
    assert!(e.contains("flags.bad"), "{e}");
    assert!(e.contains("no field named nope"), "{e}");

    // and one whose own cases do not hold does not load either
    dir.file("flags.yml", b"flags:\n  ok: {field: echo_time, gt: 0}\n");
    dir.file(
        "corpus/c.yml",
        b"cases:\n  - {name: a wrong claim, stack: {echo_time: 5}, flags: {ok: false}}\n",
    );
    let out = nils()
        .args(["pack", "validate"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let e = stderr(&out);
    assert!(e.contains("a wrong claim"), "{e}");
    assert!(e.contains("do not hold"), "{e}");
}

#[test]
fn pack_list_and_show_read_the_pack_directory() {
    let packs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs");
    let out = nils()
        .args(["pack", "list", "--json", "--pack-dir"])
        .arg(&packs)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pack"] == "mri@0.1.0"),
        "{listed}"
    );

    let out = nils()
        .args(["pack", "show", "mri", "--json", "--pack-dir"])
        .arg(&packs)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(shown["modality"], "MR");
    assert_eq!(shown["flags"], 145);
    assert_eq!(shown["contract"], 1);
    assert!(
        shown["buckets"]["diffusion_tokens"]
            .as_array()
            .unwrap()
            .len()
            >= 10
    );

    // a name that is not there says so rather than listing nothing
    let out = nils()
        .args(["pack", "show", "nope", "--pack-dir"])
        .arg(&packs)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no pack named nope"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn classify_explains_itself_and_a_decision_closes_the_question() {
    let packs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs");
    let dir = TempDir::new("cli-classify");
    let mut e = synth::minimal_mr("1.2.4.A", "1.2.4.A.1", "1.2.4.A.1.1");
    e.extend([
        synth::text(
            dicom_dictionary_std::tags::SERIES_DESCRIPTION,
            dicom_core::VR::LO,
            "sag T1 mprage",
        ),
        synth::text(
            dicom_dictionary_std::tags::SCANNING_SEQUENCE,
            dicom_core::VR::CS,
            "GR",
        ),
        synth::text(
            dicom_dictionary_std::tags::SEQUENCE_VARIANT,
            dicom_core::VR::CS,
            "SK\\SP\\MP",
        ),
        synth::text(
            dicom_dictionary_std::tags::MR_ACQUISITION_TYPE,
            dicom_core::VR::CS,
            "3D",
        ),
    ]);
    dir.file(
        "a/IM_0001",
        &synth::part10(&MetaFields::mr("1.2.4.A.1.1"), &e, true),
    );

    let home = home();
    let registry: [&std::ffi::OsStr; 2] =
        [std::ffi::OsStr::new("--registry"), home.path().as_os_str()];
    let out = nils()
        .args(registry)
        .args(["digest", "--workers", "2", "--name", "t"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let out = nils()
        .args(registry)
        .args(["fingerprint", "--name", "f"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    // Every axis below full confidence is asked about, which is every axis a
    // rule decided: the queue's size is the report's number, not a guess.
    let classify = |extra: &[&str]| -> serde_json::Value {
        let out = nils()
            .args(registry)
            .args(["classify", "--json", "--review-below", "1.0", "--pack-dir"])
            .arg(&packs)
            .args(extra)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let first = classify(&[]);
    assert_eq!(first["written"], 1, "{first}");
    assert!(first["review_items"].as_i64().unwrap() > 0, "{first}");

    // `explain` says what the pack decided and what made it decide that
    let out = nils()
        .args(registry)
        .args(["explain", "1", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let axis = |shown: &serde_json::Value, name: &str| -> serde_json::Value {
        shown["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axis"] == name)
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    assert_eq!(axis(&shown, "technique")["value"], "MPRAGE", "{shown}");
    assert!(
        shown["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["axis"] == "technique" && e["rule"].is_string()),
        "{shown}"
    );

    let items = |status: &str| -> Vec<serde_json::Value> {
        let out = nils()
            .args(registry)
            .args(["review", "list", "--json", "--status", status])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["items"]
            .as_array()
            .unwrap()
            .clone()
    };
    let open = items("open");
    let base = open
        .iter()
        .find(|i| i["kind"] == "base:low_confidence")
        .unwrap_or_else(|| panic!("{open:#?}"));
    let id = base["id"].as_i64().unwrap().to_string();

    let out = nils()
        .args(registry)
        .args([
            "review",
            "decide",
            &id,
            "--value",
            "T2w",
            "--actor",
            "a person",
            "--why",
            "checked by eye",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let said: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(said["axis"], "base", "{said}");
    assert_eq!(said["value"], "T2w", "{said}");
    assert!(
        items("open")
            .iter()
            .all(|i| i["kind"] != "base:low_confidence"),
        "the question is answered"
    );

    // and the answer survives the next run, which now disagrees out loud
    let again = classify(&[]);
    let mut kinds: Vec<String> = items("open")
        .iter()
        .map(|i| i["kind"].as_str().unwrap().to_string())
        .collect();
    kinds.sort();
    let mut once = kinds.clone();
    once.dedup();
    assert_eq!(
        kinds, once,
        "a question asked again is asked once, not twice"
    );
    assert_eq!(again["written"], 1, "{again}");
    let out = nils()
        .args(registry)
        .args(["explain", "1", "--json"])
        .output()
        .unwrap();
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(axis(&shown, "base")["value"], "T2w", "{shown}");
    assert_eq!(axis(&shown, "base")["tier"], "decision", "{shown}");
    assert!(
        items("open").iter().any(|i| i["kind"] == "base:decision"),
        "a rule that still disagrees is said out loud"
    );
}
