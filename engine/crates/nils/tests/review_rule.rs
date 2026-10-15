// SPDX-License-Identifier: AGPL-3.0-only
//! System 1 in the review rule, end to end (record 55 D3, D6 and D7 with
//! record 56; spec §11; gate tests T20 and T21's engine half): System 1 runs
//! as a pipeline given the model chosen for its task, and its opinions are
//! kept, never decisions; on a dataset flagged as having damaged headers
//! the gate raises nothing until a recorded certificate names the model,
//! then accepts only agreement above both thresholds and asks about the
//! rest; a blind-first question hides NILS's answers until the person's
//! provisional answer, then shows both suggestions and both reasons, and
//! its decision is marked blind only when it kept that answer; a round
//! reports what it accepted and sent to Review; the audit draws a seeded
//! share of the accepted stacks into a blind campaign of purpose audit,
//! and its grade is the accepted error with its exact bound.
//!
//! No container runtime is needed: a stand-in for podman runs the stand-in
//! pipeline's command on the host. SQLite.

use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packs")
        .to_str()
        .unwrap()
        .to_string()
}

/// A stand-in for podman: it answers `--version` and `info` as a rootless
/// podman does, keeps the words of a `run`, and runs the command after the
/// image on the host, every container path in it written as the host
/// folder mounted there; with `FAKE_PODMAN_EXIT` it fails as an image that
/// is not there does.
const FAKE_PODMAN: &str = r#"#!/usr/bin/env python3
import json, os, re, subprocess, sys
a = sys.argv[1:]
if not a or a[0] == "--version":
    print("podman version 9.9.9-test"); sys.exit(0)
if a[0] == "info":
    print("/fake/containers/storage" if "GraphRoot" in a[-1] else "true"); sys.exit(0)
if a[0] != "run":
    sys.exit(0)
with open(os.environ["FAKE_PODMAN_ARGS"], "a") as log:
    log.write(json.dumps(a) + "\n")
if os.environ.get("FAKE_PODMAN_EXIT"):
    print("Error: the image is not known here", file=sys.stderr)
    sys.exit(int(os.environ["FAKE_PODMAN_EXIT"]))
mounts, env, i = {}, {}, 1
while i < len(a):
    w = a[i]
    if w == "--volume":
        host, ctr = a[i + 1].split(":")[:2]; mounts[ctr] = host; i += 2
    elif w == "--env":
        k, v = a[i + 1].split("=", 1); env[k] = v; i += 2
    elif w in ("--name", "--network", "--pull", "--userns", "--cap-drop", "--security-opt", "--device", "--user", "--gpus"):
        i += 2
    elif "@sha256:" in w:
        i += 1; break
    else:
        i += 1
keys = sorted(mounts, key=len, reverse=True)
pattern = re.compile("(" + "|".join(re.escape(k) for k in keys) + r")(?=/|$|[^A-Za-z0-9_])")
argv = [pattern.sub(lambda m: mounts[m.group(1)], w) for w in a[i:]]
env = {k: pattern.sub(lambda m: mounts[m.group(1)], v) for k, v in env.items()}
sys.exit(subprocess.call(argv, env=dict(os.environ, **env)))
"#;

struct Home {
    dir: TempDir,
    /// The Postgres DSN and schema, when the registry lives there.
    pg: Option<(String, String)>,
}

impl Home {
    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        let mut child = nils()
            .arg("--registry")
            .arg(self.dir.path())
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
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
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (good, out, err) = self.run(args, None);
        assert!(good, "nils {args:?} failed: {err}\n{out}");
        out
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap(),
        }
    }
}

const OPS: &str = "an-operator-token-of-length";
const READS: &str = "a-reader-token-of-its-length";
/// Pipelines work at detail plain: a run reads pixels, so it is refused.
const PLAIN: &str = "a-plain-pipelines-token-long";
const ADMIN: &str = "an-administrator-token-long";
const RATER_A: &str = "the-first-agent-rater-token";
const RATER_B: &str = "the-second-agent-rater-tokn";
const SETTLER: &str = "the-settling-agent-the-token";

/// A `nils serve` on a search path, with its worker or without one, killed
/// when dropped.
struct Served {
    child: std::process::Child,
    port: u16,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Served {
    fn start(home: &Home, path: &OsStr, worker: bool, env: &[(&str, &OsStr)]) -> Served {
        use std::io::BufRead as _;
        let tokens = [
            format!("{OPS}=ops@lab:operator"),
            format!("{READS}=lou@lab:reader"),
            format!("{PLAIN}=pat@lab:pipelines:work,pipelines:see,data:see"),
            format!("{ADMIN}=adm@lab:admin"),
            format!("{RATER_A}=rater-a@lab:campaigns:work,campaigns:see"),
            format!("{RATER_B}=rater-b@lab:campaigns:work,campaigns:see"),
            format!("{SETTLER}=settler@lab:campaigns:work,campaigns:see"),
        ]
        .join(",");
        let mut child = nils()
            .arg("--registry")
            .arg(home.dir.path())
            .args(["serve", "--bind", "127.0.0.1:0", "--workers", "1"])
            .args(if worker { &["--worker"][..] } else { &[][..] })
            .args(["--auth", "token", "--pack-dir", &packs()])
            .env("NILS_TOKENS", tokens)
            .env("NILS_PACK_DIR", packs())
            .env("PATH", path)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .envs(env.iter().copied())
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            let _ = child.kill();
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        std::thread::spawn(move || for _ in lines {});
        Served { child, port }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        use std::io::Read as _;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let response = String::from_utf8_lossy(&bytes).to_string();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            serde_json::from_str(text).unwrap_or(Value::String(text.to_string())),
        )
    }

    fn get(&self, path: &str, token: &str) -> Value {
        let (status, doc) = self.call("GET", path, None, token);
        assert_eq!(status, 200, "{path}: {doc}");
        doc
    }

    /// A step's run asked for at its door.
    fn run(&self, scope: &str, step: &str, token: &str) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/api/{scope}/steps/{step}/run"),
            None,
            token,
        )
    }

    /// A job once it is over, two minutes at most.
    fn over(&self, job: i64) -> Value {
        for _ in 0..1200 {
            let doc = self.get(&format!("/api/jobs/{job}"), OPS);
            if matches!(doc["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return doc;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("job {job} did not end");
    }
}

/// A search path whose first folder holds the stand-in podman and a card
/// reader that finds no card, so a host's own runtime or GPU is never used.
fn fake_path(bin: &TempDir) -> OsString {
    let fake = bin.file("podman", FAKE_PODMAN.as_bytes());
    let no_gpu = bin.file("nvidia-smi", b"#!/bin/sh\nexit 1\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for f in [&fake, &no_gpu] {
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let mut path = bin.path().as_os_str().to_owned();
    if let Some(p) = std::env::var_os("PATH") {
        path.push(":");
        path.push(p);
    }
    path
}

/// Register a model by its card and its file; answers its id and digest.
fn register(home: &Home, files: &TempDir, card: Value, bytes: &[u8]) -> (i64, String) {
    let name = card["name"].as_str().unwrap().to_string();
    let artifact = files.file(&format!("{name}.bin"), bytes);
    let card_file = files.file(&format!("{name}.json"), card.to_string().as_bytes());
    let m = home.json(&[
        "model",
        "register",
        "--card",
        card_file.to_str().unwrap(),
        "--artifact",
        artifact.to_str().unwrap(),
        "--json",
    ]);
    (
        m["id"].as_i64().unwrap(),
        m["digest"].as_str().unwrap().to_string(),
    )
}
/// What a series is, by its physics and its name.
#[derive(Clone, Copy)]
enum Kind {
    T1,
    Flair,
}

/// One series of two images of one session on 2024-03-10, the second image
/// 30 seconds after the first, at `hhmmss`.
fn series(dir: &TempDir, study: &str, number: i64, kind: Kind, hhmmss: u32) {
    let (name, scanning, variant, acq, tr, te, ti) = match kind {
        Kind::T1 => (
            "t1_mprage_sag",
            "GR\\IR",
            "SP\\MP",
            "3D",
            "2300",
            "3",
            "900",
        ),
        Kind::Flair => ("AX FLAIR", "SE\\IR", "SK\\SP", "2D", "9000", "90", "2500"),
    };
    let clock = |s: u32| format!("{:02}{:02}{:02}", s / 10_000, (s / 100) % 100, s % 100);
    let series = format!("{study}.{number}");
    for i in 0..2u32 {
        let sop = format!("{series}.{}", i + 1);
        let mut e = synth::minimal_mr(study, &series, &sop);
        let at = clock(hhmmss + 30 * i);
        for (tag, vr, v) in [
            (tags::PATIENT_ID, VR::LO, "S-ONE".to_string()),
            (tags::STUDY_DATE, VR::DA, "20240310".to_string()),
            (tags::STUDY_TIME, VR::TM, "075000".to_string()),
            (tags::SERIES_DATE, VR::DA, "20240310".to_string()),
            (tags::ACQUISITION_DATE, VR::DA, "20240310".to_string()),
            (tags::ACQUISITION_TIME, VR::TM, at.clone()),
            (tags::CONTENT_DATE, VR::DA, "20240310".to_string()),
            (tags::CONTENT_TIME, VR::TM, at),
            (tags::MANUFACTURER, VR::LO, "SIEMENS".into()),
            (tags::MAGNETIC_FIELD_STRENGTH, VR::DS, "3".into()),
            (tags::BODY_PART_EXAMINED, VR::CS, "HEAD".into()),
            (tags::SERIES_DESCRIPTION, VR::LO, name.into()),
            (tags::PROTOCOL_NAME, VR::LO, name.into()),
            (tags::SERIES_NUMBER, VR::IS, number.to_string()),
            (tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND".into()),
            (tags::SCANNING_SEQUENCE, VR::CS, scanning.into()),
            (tags::SEQUENCE_VARIANT, VR::CS, variant.into()),
            (tags::MR_ACQUISITION_TYPE, VR::CS, acq.into()),
            (tags::REPETITION_TIME, VR::DS, tr.into()),
            (tags::ECHO_TIME, VR::DS, te.into()),
            (tags::INVERSION_TIME, VR::DS, ti.into()),
            (tags::SLICE_THICKNESS, VR::DS, "1".into()),
            (tags::PIXEL_SPACING, VR::DS, "1\\1".into()),
            (
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                "1\\0\\0\\0\\1\\0".into(),
            ),
            (tags::IMAGE_POSITION_PATIENT, VR::DS, format!("0\\0\\{i}")),
            (tags::SLICE_LOCATION, VR::DS, format!("{i}")),
            (tags::FRAME_OF_REFERENCE_UID, VR::UI, format!("{study}.0")),
            (tags::INSTANCE_NUMBER, VR::IS, (i + 1).to_string()),
        ] {
            e.push(synth::text(tag, vr, &v));
        }
        e.push(synth::num(tags::ROWS, VR::US, 64.0));
        e.push(synth::num(tags::COLUMNS, VR::US, 64.0));
        dir.file(
            &format!("s{number}/IM_{i:04}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
}

/// The stack of the session's series `number`.
fn stack_of(home: &Home, number: i64) -> i64 {
    let mut store = home.store();
    let sql = format!(
        "SELECT f.stack_id FROM {} f WHERE f.series_number = {number}",
        store.qualified("stack_fingerprint")
    );
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

/// System 1's stand-in: it checks the rows it is given and answers in its
/// own vocabulary, saying how it read the rules (after D2): technique 3D-TSE
/// where the pack stores a vendor's name, base T2starw on series 2 where the
/// pack stores another word, modifier null read as null; base as the rules
/// did on series 1, below the threshold, and T2w against the rules on
/// series 5.
fn system1_stand_in() -> String {
    format!(
        r#"# SPDX-License-Identifier: AGPL-3.0-only
name: system1-score
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/system1-stand-in@sha256:{}"
command-line: |
  python3 -c '
  import json, os, sys
  m = json.load(open(sys.argv[1])); out = sys.argv[2]
  man = json.load(open(sys.argv[3] + "/manifest.json"))
  ids = [x["model_id"] for x in man["models"] if x["input"] == "model"]
  assert len(ids) == 1, man
  rows = m["rows"]
  assert rows["votes"][0] == "axis", rows
  units, opinions = [], []
  for s in m["stacks"]:
      r = rows["stacks"][str(s["stack_id"])]
      assert r["scored"] and r["fingerprint"]["text_series_description"], r
      assert "earliest_acquisition_date" not in r["fingerprint"], r
      sn = r["fingerprint"]["series_number"]
      rule = (s["header"]["classification"].get("base") or [{{}}])[0].get("value")
      value, read, p = rule, rule, 0.99
      if sn == 2:
          value, read = "T2starw", "T2starw"
      if sn == 5:
          value = "T2w"
      if sn == 1:
          p = 0.5
      d = os.path.join(out, "system1"); os.makedirs(d, exist_ok=True)
      json.dump({{"value": value}}, open(os.path.join(d, "%d.json" % s["stack_id"]), "w"))
      units.append({{"unit_id": s["unit"], "status": "succeeded", "derivatives": ["system1/%d.json" % s["stack_id"]]}})
      opinions.append({{"stack_id": s["stack_id"], "axis": "base", "value": value, "p": p, "reliability": 0.995,
                        "fields": [{{"field": "text_series_description", "effect": 0.4}}], "model_id": ids[0],
                        "rules_read": {{"value": read}}}})
      opinions.append({{"stack_id": s["stack_id"], "axis": "technique", "value": "3D-TSE", "p": 0.99, "reliability": 0.995,
                        "model_id": ids[0], "rules_read": {{"value": "3D-TSE"}}}})
      opinions.append({{"stack_id": s["stack_id"], "axis": "modifier", "value": None, "p": 0.99, "reliability": 0.995,
                        "model_id": ids[0], "rules_read": {{"value": None}}}})
      opinions.append({{"stack_id": s["stack_id"], "axis": "flavour", "value": "x", "p": 0.9, "model_id": ids[0]}})
  json.dump({{"schema_version": "1", "units": units, "opinions": opinions}}, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation] [Inputs]
x-nils:
  analysis-level: stack
  input: {{layout: stacks, header: true, rows: true}}
  inputs:
    - {{id: model, type: model}}
  outputs:
    - id: opinion
      kind: output
      path-template: "system1/{{stack}}.json"
      media-type: application/json
  needs: {{gpu: none}}
  opinions: [{{axis: base}}, {{axis: technique}}, {{axis: modifier}}]
"#,
        "e".repeat(64)
    )
}

/// The decisions a person gave, by id, with their blind mark.
fn blind_of(home: &Home, decision: i64) -> Option<i64> {
    let mut store = home.store();
    let sql = format!(
        "SELECT blind FROM {} WHERE id = {decision}",
        store.qualified("decision")
    );
    store.query(&sql, &[]).unwrap()[0].opt_int(0).unwrap()
}

/// The open items of a kind.
fn open_items(server: &Served, kind: &str) -> Vec<Value> {
    let doc = server.get(
        &format!("/api/review?status=open&kind={kind}&limit=200"),
        OPS,
    );
    doc["items"].as_array().cloned().unwrap_or_default()
}

#[test]
fn system1_runs_beside_the_rules_the_gate_acts_on_the_chosen_model_and_the_audit_grades() {
    let home = Home {
        dir: TempDir::new("s1-home"),
        pg: None,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a review rule test key\n"));
    assert!(good, "{err}");
    home.ok(&["init", "--key", "k"]);
    let dir = TempDir::new("s1-ds");
    let study = "1.2.826.0.1.3680043.8.497.9.2";
    series(&dir, study, 1, Kind::Flair, 95_500);
    series(&dir, study, 2, Kind::T1, 100_000);
    series(&dir, study, 5, Kind::T1, 102_000);
    home.ok(&[
        "place",
        "add",
        "ds",
        dir.path().to_str().unwrap(),
        "--role",
        "source",
        "--move-into",
        "anon",
        "--confirm-move",
        "--patient-id",
        "id-type:patient-id",
        "--subjects",
        "generated",
    ]);
    let work = TempDir::new("s1-work");
    home.ok(&[
        "place",
        "add",
        "work",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);
    let bin = TempDir::new("s1-bin");
    let path = fake_path(&bin);
    let args_log = bin.path().join("args.log");
    let logged: [(&str, &OsStr); 1] = [("FAKE_PODMAN_ARGS", args_log.as_os_str())];
    let server = Served::start(&home, &path, true, &logged);
    let (status, queued) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["digest", "@ds", "--name", "first"]})),
        OPS,
    );
    assert_eq!(status, 202, "{queued}");
    let read = server.over(queued["job"].as_i64().unwrap());
    assert_eq!(read["state"], "done", "{read}");
    drop(server);
    home.ok(&["fingerprint"]);
    home.ok(&["classify", "--pack-dir", &packs()]);
    let stacks: Vec<i64> = [1, 2, 5].iter().map(|n| stack_of(&home, *n)).collect();

    // no System 1 model yet: the step says why
    let server = Served::start(&home, &path, false, &logged);
    let (status, refused) = server.run("datasets/ds", "system1", OPS);
    assert_eq!(status, 409, "{refused}");
    assert_eq!(refused["reason"], "no_pipeline", "{refused}");
    drop(server);

    let descriptor = work.file("system1-score.yml", system1_stand_in().as_bytes());
    let added = home.json(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
    std::fs::remove_file(&descriptor).unwrap();
    assert_eq!(added["label"], "system1-score@1", "{added}");
    // two System 1 models; the one the site chooses runs
    let files = TempDir::new("s1-models");
    let gate = json!({"axes": {"base": {"lambda1": 0.9, "lambda2": 0.99},
                               "technique": {"lambda1": 0.9, "lambda2": 0.99},
                               "modifier": {"lambda1": 0.9, "lambda2": 0.99}}, "evidence": "run:test"});
    let (first, _) = register(
        &home,
        &files,
        json!({"name": "s1-first", "version": "t1", "kind": "pass", "task": "system1", "gate": gate}),
        b"the first system one",
    );
    let (second, _) = register(
        &home,
        &files,
        json!({"name": "s1-second", "version": "t1", "kind": "pass", "task": "system1", "gate": gate}),
        b"the second system one",
    );
    let check = files.file(
        "check.json",
        json!({"suite": "s1", "passed": true, "checks": [{"name": "equivalence", "passed": true}]})
            .to_string()
            .as_bytes(),
    );
    for m in [first, second] {
        home.ok(&[
            "model",
            "admit",
            &m.to_string(),
            "--check",
            check.to_str().unwrap(),
        ]);
        home.ok(&[
            "model",
            "keep",
            &m.to_string(),
            "--artifact",
            files
                .path()
                .join(if m == first {
                    "s1-first.bin"
                } else {
                    "s1-second.bin"
                })
                .to_str()
                .unwrap(),
        ]);
    }
    let server = Served::start(&home, &path, true, &logged);
    let (status, chose) = server.call(
        "POST",
        "/api/models/choose",
        Some(json!({"model": first, "why": "the trial model"})),
        OPS,
    );
    assert_eq!(status, 200, "{chose}");
    let (status, chose) = server.call(
        "POST",
        "/api/models/choose",
        Some(json!({"model": second, "why": "the frozen model"})),
        OPS,
    );
    assert_eq!(status, 200, "{chose}");
    let (status, started) = server.run("datasets/ds", "system1", OPS);
    assert_eq!(status, 202, "{started}");
    let models: Vec<String> = started["command"]
        .as_array()
        .unwrap()
        .windows(2)
        .filter(|w| w[0] == "--model")
        .map(|w| w[1].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        models,
        [second.to_string()],
        "the chosen model runs: {started}"
    );
    let done = server.over(started["job"].as_i64().unwrap());
    assert_eq!(done["state"], "done", "{done}");

    // the opinions are kept on the declared axis, never as decisions
    {
        let mut store = home.store();
        let n = store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE model_id = {second} AND axis = 'base'",
                    store.qualified("opinion")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(n, 3);
        let undeclared = store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE axis = 'flavour'",
                    store.qualified("opinion")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(
            undeclared, 0,
            "an opinion on an axis the descriptor does not declare is not taken"
        );
        let by_model = store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE author_kind = 'model'",
                    store.qualified("decision")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(by_model, 0, "an opinion is never a decision");
    }
    // the normal dataset: no gate question, whatever System 1 says
    assert!(open_items(&server, "base:disagree").is_empty());

    // flagged as damaged: the gate acts on the chosen, admitted model
    let (status, flag) = server.call(
        "PUT",
        "/api/datasets/ds/headers",
        Some(json!({"damaged": true})),
        OPS,
    );
    assert_eq!(status, 400, "a flag says why: {flag}");
    let (status, flag) = server.call(
        "PUT",
        "/api/datasets/ds/headers",
        Some(json!({"damaged": true, "why": "an export with stripped names"})),
        OPS,
    );
    assert_eq!(status, 200, "{flag}");
    assert_eq!(flag["damaged"], true, "{flag}");
    assert_eq!(flag["by"], "person", "{flag}");
    // record 55 ruling 27: the model is admitted and chosen, so System 1
    // stands with no certificate recorded
    let s = server.get("/api/review/settings", OPS);
    assert_eq!(s["system1"]["confirmed"], true, "{s}");
    assert_eq!(s["system1"]["certificate_id"], Value::Null, "{s}");
    drop(server);

    // the gate accepts only agreement above both thresholds, and asks about
    // the rest
    let server = Served::start(&home, &path, false, &logged);
    // every question is read blind first from here, for the test
    let (status, set) = server.call(
        "PUT",
        "/api/settings",
        Some(json!({"review": {"blind_share": 1.0}})),
        ADMIN,
    );
    assert_eq!(status, 200, "{set}");
    assert_eq!(set["review"]["blind_share"], 1.0, "{set}");
    let (status, gated) = server.call("POST", "/api/review/gate", Some(json!({})), OPS);
    assert_eq!(status, 200, "{gated}");
    let g = &gated["rounds"][0]["gated"];
    // vendor names count as agreement where the model read the rules so:
    // a technique the pack names by its vendor (3D-TSE), base T2starw where
    // the pack stores another word, and no modifier read as null
    assert_eq!(g["outcomes"]["accepted"], 7, "{gated}");
    assert_eq!(g["outcomes"]["disagree"], 1, "{gated}");
    assert_eq!(g["outcomes"]["unsure"], 1, "{gated}");
    assert_eq!(g["accepted_stacks"], 1, "{gated}");
    {
        // the pack never stores the model's word: the agreement is its reading
        let mut store = home.store();
        let n = store
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE value IN ('3D-TSE', 'T2starw')",
                    store.qualified("classification_axis")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(n, 0);
    }
    let disagree = open_items(&server, "base:disagree");
    assert_eq!(disagree.len(), 1, "{disagree:?}");
    let unsure = open_items(&server, "base:unsure");
    assert_eq!(unsure.len(), 1, "{unsure:?}");
    // a gate run again changes nothing
    let (_, again) = server.call("POST", "/api/review/gate", Some(json!({})), OPS);
    assert_eq!(open_items(&server, "base:disagree").len(), 1, "{again}");

    // blind first: NILS's answers are hidden until the provisional answer
    let item = disagree[0]["id"].as_i64().unwrap();
    assert_eq!(disagree[0]["mode"], "blind_first", "{disagree:?}");
    assert_eq!(disagree[0]["evidence"]["blind_first"], true, "{disagree:?}");
    assert!(
        disagree[0]["evidence"].get("system1").is_none(),
        "{disagree:?}"
    );
    let one = server.get(&format!("/api/review/{item}"), OPS);
    assert_eq!(one["mode"], "blind_first", "{one}");
    assert!(one["evidence"].get("rules").is_none(), "{one}");
    let (status, early) = server.call("GET", &format!("/api/review/{item}/suggestions"), None, OPS);
    assert_eq!(status, 409, "{early}");
    let (status, given) = server.call(
        "POST",
        &format!("/api/review/{item}/provisional"),
        Some(json!({"value": "T1w"})),
        OPS,
    );
    assert_eq!(status, 200, "{given}");
    // both suggestions and both reasons now
    let sug = server.get(&format!("/api/review/{item}/suggestions"), OPS);
    assert_eq!(sug["system1"]["status"], "confirmed", "{sug}");
    let axis = &sug["stacks"][0]["axes"][0];
    assert_eq!(axis["axis"], "base", "{sug}");
    assert_eq!(axis["system1"]["value"], "T2w", "{sug}");
    assert_eq!(axis["system1"]["agrees"], false, "{sug}");
    assert_eq!(
        axis["system1"]["fields"][0]["field"], "text_series_description",
        "{sug}"
    );
    assert!(
        axis["rules"]["line"].as_str().unwrap().starts_with("base:"),
        "{sug}"
    );
    assert!(axis["system1"]["similar"].is_array(), "{sug}");
    // the item now shows its evidence to the person who answered
    let one = server.get(&format!("/api/review/{item}"), OPS);
    assert!(one["evidence"]["system1"].is_object(), "{one}");
    // kept the provisional answer: the decision is blind
    let (status, applied) = server.call(
        "POST",
        &format!("/api/review/{item}/apply"),
        Some(json!({"value": "T1w"})),
        OPS,
    );
    assert_eq!(status, 200, "{applied}");
    assert_eq!(applied["answer"]["blind"], true, "{applied}");
    assert_eq!(applied["answer"]["mode"], "blind_first", "{applied}");
    assert_eq!(
        blind_of(&home, applied["decision"].as_i64().unwrap()),
        Some(1)
    );
    // changed after the suggestions showed: a decision, not blind
    let other = unsure[0]["id"].as_i64().unwrap();
    let (status, _) = server.call(
        "POST",
        &format!("/api/review/{other}/provisional"),
        Some(json!({"value": "PDw"})),
        OPS,
    );
    assert_eq!(status, 200);
    let (status, applied) = server.call(
        "POST",
        &format!("/api/review/{other}/apply"),
        Some(json!({"value": "FLAIR"})),
        OPS,
    );
    assert_eq!(status, 200, "{applied}");
    assert_eq!(applied["answer"]["blind"], false, "{applied}");
    assert_eq!(
        blind_of(&home, applied["decision"].as_i64().unwrap()),
        Some(0)
    );
    // and only the blind one is a training label
    {
        let mut store = home.store();
        let (rows, left_out) =
            nils_registry::labels::training_labels(&mut store, Some("base"), None, &[], None)
                .unwrap();
        assert!(
            rows.iter().all(|l| l.value.as_deref() != Some("FLAIR")),
            "a decision given after the suggestions showed never trains"
        );
        assert!(
            rows.iter().any(|l| l.value.as_deref() == Some("T1w")),
            "the blind one does"
        );
        assert!(left_out >= 1);
    }

    // the round reports what it accepted and sent to Review
    let rounds = server.get("/api/review/rounds", OPS);
    let r = &rounds["rounds"][0];
    assert_eq!(r["round"]["model_id"], second, "{rounds}");
    assert_eq!(r["round"]["confirmed"], true, "{rounds}");
    assert_eq!(r["scans"], 3, "{rounds}");
    assert_eq!(r["system1"]["scans_with_opinion"], 3, "{rounds}");
    for k in ["conflict", "low_confidence", "body_part", "post_contrast"] {
        assert_eq!(r["never_asked"][k], 0, "{k}: {rounds}");
    }
    assert!(
        r["answers"]["blind"]["decisions"].as_i64().unwrap() >= 1,
        "{rounds}"
    );

    // the audit: the accepted stacks, a seeded draw keyed by DICOM identity
    let (status, drawn) = server.call(
        "POST",
        "/api/audit/draws",
        Some(json!({"seed": "test-round", "share": 1.0, "axes": ["base"],
                    "raters": ["rater-a@lab", "rater-b@lab"], "adjudicators": ["settler@lab"]})),
        ADMIN,
    );
    assert_eq!(status, 200, "{drawn}");
    let draw = drawn["draw"]["id"].as_i64().unwrap();
    assert_eq!(drawn["campaign"]["purpose"], "audit", "{drawn}");
    assert_eq!(drawn["campaign"]["trains"], false, "{drawn}");
    let frame = drawn["draw"]["frame"]["stacks"].as_i64().unwrap();
    assert!(frame >= 1, "{drawn}");
    let campaign = drawn["campaign"]["id"].as_i64().unwrap();
    let c = server.get(&format!("/api/campaigns/{campaign}"), ADMIN);
    assert_eq!(c["purpose"], "audit", "{c}");
    assert_eq!(c["suggest"], "none", "{c}");
    let shown = server.get(&format!("/api/audit/draws/{draw}"), OPS);
    let accepted: std::collections::BTreeMap<i64, Value> = shown["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["stack_id"].as_i64().unwrap(),
                s["accepted"]["base"].clone(),
            )
        })
        .collect();
    assert_eq!(
        accepted.len() as i64,
        drawn["draw"]["drawn"].as_i64().unwrap()
    );
    assert!(
        !accepted.contains_key(&stacks[2]),
        "a gate-held stack is never in the frame"
    );
    // the two raters read blind and agree; on the first stack they both
    // say the rules were wrong
    let wrong_stack = *accepted.keys().next().unwrap();
    for token in [RATER_A, RATER_B] {
        loop {
            let (status, claimed) = server.call(
                "POST",
                &format!("/api/campaigns/{campaign}/claim"),
                Some(json!({"role": "rater"})),
                token,
            );
            assert_eq!(status, 200, "{claimed}");
            if claimed["assignment"].is_null() {
                break;
            }
            let stack = claimed["item"]["stack_id"].as_i64().unwrap();
            let item = claimed["item"]["id"].as_i64().unwrap();
            let why = server.get(
                &format!("/api/campaigns/{campaign}/items/{item}/why"),
                token,
            );
            assert_eq!(why["blind"], true, "an audit is read blind: {why}");
            let value = if stack == wrong_stack {
                json!("PDw")
            } else {
                accepted[&stack].clone()
            };
            let a = claimed["assignment"]["id"].as_i64().unwrap();
            let (status, answered) = server.call(
                "POST",
                &format!("/api/campaigns/{campaign}/assignments/{a}/answer"),
                Some(json!({"value": json!({"base": value}).to_string()})),
                token,
            );
            assert_eq!(status, 200, "{answered}");
        }
    }
    let (status, closed) = server.call(
        "POST",
        &format!("/api/campaigns/{campaign}/close"),
        Some(json!({})),
        ADMIN,
    );
    assert_eq!(status, 200, "{closed}");
    let (status, graded) = server.call(
        "POST",
        &format!("/api/audit/draws/{draw}/grade"),
        Some(json!({})),
        OPS,
    );
    assert_eq!(status, 200, "{graded}");
    let g = &graded["grade"];
    assert_eq!(g["read"], accepted.len(), "{graded}");
    assert_eq!(g["wrong"], 1, "{graded}");
    assert_eq!(g["trains"], false, "{graded}");
    assert!(
        g["bound"].as_f64().unwrap() >= g["error"].as_f64().unwrap(),
        "{graded}"
    );
    assert_eq!(g["method"], "exact one-sided Clopper-Pearson", "{graded}");
    drop(server);
}
