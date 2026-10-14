// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55 ruling 22 (b) and ruling 20, end to end on both backends: a
//! certificate may cite its graded run instead of a sample sealed in the
//! same registry. Recorded by one person at the engine's door (here through
//! `nils model certify` under the person's token), it names the run, the
//! sha256 of its manifest, the plan's commit and the artifact digest of the
//! model it graded, keeps the graded result with the bound the engine works
//! out again, and unseals nothing; the audit keeps it. It confirms System 1,
//! whose gate then acts on a dataset flagged as having damaged headers and
//! stages its answer where the rules name nothing, for a person to confirm,
//! and does nothing on a dataset without the flag. A certificate on a
//! sealed sample still takes two people.
//!
//! No container runtime is needed: a stand-in for podman runs the stand-in
//! pipeline's command on the host. SQLite, and Postgres where
//! `NILS_TEST_POSTGRES_DSN` names one.

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

fn sha256(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// A stand-in for podman: it answers `--version` and `info` as a rootless
/// podman does, and runs the command after the image on the host, every
/// container path in it written as the host folder mounted there.
const FAKE_PODMAN: &str = r#"#!/usr/bin/env python3
import os, re, subprocess, sys
a = sys.argv[1:]
if not a or a[0] == "--version":
    print("podman version 9.9.9-test"); sys.exit(0)
if a[0] == "info":
    print("/fake/containers/storage" if "GraphRoot" in a[-1] else "true"); sys.exit(0)
if a[0] != "run":
    sys.exit(0)
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
    fn run_env(
        &self,
        args: &[&str],
        stdin: Option<&str>,
        env: &[(&str, &OsStr)],
    ) -> (bool, String, String) {
        let mut child = nils()
            .arg("--registry")
            .arg(self.dir.path())
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .env_remove("NILS_TOKEN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .envs(env.iter().copied())
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

    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        self.run_env(args, stdin, &[])
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

    /// Rows of a query whose `{table}` names are qualified here.
    fn rows(&self, sql: &str) -> Vec<nils_registry::store::Row> {
        let mut store = self.store();
        let mut sql = sql.to_string();
        for t in [
            "decision",
            "review_item",
            "classification_axis",
            "stack_fingerprint",
            "audit",
        ] {
            sql = sql.replace(&format!("{{{t}}}"), &store.qualified(t));
        }
        store.query(&sql, &[]).unwrap()
    }

    fn int(&self, sql: &str) -> i64 {
        self.rows(sql)[0].int(0).unwrap()
    }
}

fn init(home: &Home) {
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a certificate test key\n"));
    assert!(good, "{err}");
    match &home.pg {
        Some((dsn, schema)) => {
            home.ok(&[
                "init",
                "--backend",
                "postgres",
                "--dsn",
                dsn,
                "--schema",
                schema,
                "--key",
                "k",
            ]);
        }
        None => {
            home.ok(&["init", "--key", "k"]);
        }
    }
}

const OPS: &str = "an-operator-token-of-length";
const ADMIN: &str = "an-administrator-token-long";
/// The person who records the certificate, and another person.
const NIMA: &str = "the-first-person-of-models";
const RUSS: &str = "the-other-person-of-models";

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
    fn start(home: &Home, path: &OsStr, worker: bool) -> Served {
        use std::io::BufRead as _;
        let tokens = [
            format!("{OPS}=ops@lab:operator"),
            format!("{ADMIN}=adm@lab:admin"),
            format!("{NIMA}=nima@lab:admin"),
            format!("{RUSS}=russ@lab:admin"),
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

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn call_with(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        token: &str,
        extra: &str,
    ) -> (u16, Value) {
        use std::io::Read as _;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\n{extra}\r\n",
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

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        self.call_with(method, path, body, token, "")
    }

    fn ok(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> Value {
        let (status, doc) = self.call(method, path, body, token);
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {doc}"
        );
        doc
    }

    /// A job once it is over, two minutes at most.
    fn over(&self, job: i64) -> Value {
        for _ in 0..1200 {
            let doc = self.ok("GET", &format!("/api/jobs/{job}"), None, OPS);
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

/// What a series is: a T1 the rules name, or a bare one whose name and
/// physics say nothing, so the rules name no base.
#[derive(Clone, Copy)]
enum Kind {
    T1,
    Bare,
}

/// One series of two images of one session on 2024-03-10, the second image
/// 30 seconds after the first, at `hhmmss`.
fn series(dir: &TempDir, patient: &str, study: &str, number: i64, kind: Kind, hhmmss: u32) {
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
        Kind::Bare => ("series", "RM", "NONE", "2D", "", "", ""),
    };
    let clock = |s: u32| format!("{:02}{:02}{:02}", s / 10_000, (s / 100) % 100, s % 100);
    let series = format!("{study}.{number}");
    for i in 0..2u32 {
        let sop = format!("{series}.{}", i + 1);
        let mut e = synth::minimal_mr(study, &series, &sop);
        let at = clock(hhmmss + 30 * i);
        for (tag, vr, v) in [
            (tags::PATIENT_ID, VR::LO, patient.to_string()),
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
            if v.is_empty() {
                continue;
            }
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

/// System 1's stand-in: on base it agrees with the rules where they name a
/// value, and where they name nothing it says T2starw, in the vocabulary's
/// identity (the pack stores the label T2*w), reading the rules as none.
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
  units, opinions = [], []
  for s in m["stacks"]:
      rule = (s["header"]["classification"].get("base") or [{{}}])[0].get("value")
      if rule:
          value, read, p = rule, rule, 0.99
      else:
          value, read, p = "T2starw", "none", 0.97
      d = os.path.join(out, "system1"); os.makedirs(d, exist_ok=True)
      json.dump({{"value": value}}, open(os.path.join(d, "%d.json" % s["stack_id"]), "w"))
      units.append({{"unit_id": s["unit"], "status": "succeeded", "derivatives": ["system1/%d.json" % s["stack_id"]]}})
      opinions.append({{"stack_id": s["stack_id"], "axis": "base", "value": value, "p": p, "reliability": 0.995,
                        "model_id": ids[0], "rules_read": {{"value": read}}}})
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
  opinions: [{{axis: base}}]
"#,
        "e".repeat(64)
    )
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

/// The stack of a patient's series.
fn stack_of(home: &Home, series_uid: &str) -> i64 {
    let mut store = home.store();
    let sql = format!(
        "SELECT st.id FROM {} st JOIN {} se ON se.id = st.series_id WHERE se.series_instance_uid = '{series_uid}'",
        store.qualified("stack"),
        store.qualified("series"),
    );
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

/// The base the rules stored on a stack: none where they name nothing.
fn base_of(home: &Home, stack: i64) -> Option<String> {
    home.rows(&format!(
        "SELECT value FROM {{classification_axis}} WHERE stack_id = {stack} AND axis = 'base' AND value IS NOT NULL AND value <> ''"
    ))
    .first()
    .map(|r| r.text(0).unwrap().to_string())
}

/// System 1's decisions on base: id, stack, value, whether committed.
fn model_decisions(home: &Home) -> Vec<(i64, String, Option<String>, bool, bool)> {
    home.rows(
        "SELECT id, ref, value, CASE WHEN committed_at IS NULL THEN 0 ELSE 1 END, \
         CASE WHEN withdrawn_at IS NULL THEN 0 ELSE 1 END FROM {decision} \
         WHERE axis = 'base' AND author_kind = 'model' ORDER BY id",
    )
    .iter()
    .map(|r| {
        (
            r.int(0).unwrap(),
            r.text(1).unwrap().to_string(),
            r.opt_text(2).unwrap().map(str::to_string),
            r.int(3).unwrap() != 0,
            r.int(4).unwrap() != 0,
        )
    })
    .collect()
}

/// The gate's questions on base for a stack, by status.
fn questions(home: &Home, stack: i64) -> Vec<(String, String)> {
    home.rows(&format!(
        "SELECT kind, status FROM {{review_item}} WHERE group_key = 'gate:base:stack:{stack}' ORDER BY id"
    ))
    .iter()
    .map(|r| (r.text(0).unwrap().to_string(), r.text(1).unwrap().to_string()))
    .collect()
}

#[test]
fn a_certificate_cites_its_graded_run_and_system_1_stages_answers_on_sqlite() {
    flow(None);
}

#[test]
fn a_certificate_cites_its_graded_run_and_system_1_stages_answers_on_postgres() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_certificate_evidence";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    flow(Some((dsn.clone(), schema.to_string())));
    drop();
}

fn flow(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("cert-home"),
        pg,
    };
    init(&home);
    // two datasets of one session each: a T1 the rules name and a bare
    // series whose base they leave unnamed
    let damaged = TempDir::new("cert-damaged");
    let normal = TempDir::new("cert-normal");
    let (study_d, study_n) = (
        "1.2.826.0.1.3680043.8.497.21.1",
        "1.2.826.0.1.3680043.8.497.21.2",
    );
    series(&damaged, "S-ONE", study_d, 1, Kind::T1, 100_000);
    series(&damaged, "S-ONE", study_d, 3, Kind::Bare, 101_000);
    series(&normal, "S-TWO", study_n, 1, Kind::T1, 100_000);
    series(&normal, "S-TWO", study_n, 3, Kind::Bare, 101_000);
    for (name, dir) in [("dmg", &damaged), ("ok", &normal)] {
        home.ok(&[
            "place",
            "add",
            name,
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
    }
    let work = TempDir::new("cert-work");
    home.ok(&[
        "place",
        "add",
        "work",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);
    let bin = TempDir::new("cert-bin");
    let path = fake_path(&bin);
    let server = Served::start(&home, &path, true);
    for name in ["dmg", "ok"] {
        let queued = server.ok(
            "POST",
            "/api/jobs",
            Some(json!({"command": ["digest", &format!("@{name}"), "--name", name]})),
            OPS,
        );
        let read = server.over(queued["job"].as_i64().unwrap());
        assert_eq!(read["state"], "done", "{read}");
    }
    drop(server);
    home.ok(&["fingerprint"]);
    home.ok(&["classify", "--pack-dir", &packs()]);
    let t1_d = stack_of(&home, &format!("{study_d}.1"));
    let bare_d = stack_of(&home, &format!("{study_d}.3"));
    let t1_n = stack_of(&home, &format!("{study_n}.1"));
    let bare_n = stack_of(&home, &format!("{study_n}.3"));
    assert_eq!(base_of(&home, t1_d).as_deref(), Some("T1w"));
    assert_eq!(base_of(&home, bare_d), None, "the rules name no base");
    assert_eq!(base_of(&home, bare_n), None, "the rules name no base");

    // System 1 as the site's model, its pipeline in the catalog
    let descriptor = work.file("system1-score.yml", system1_stand_in().as_bytes());
    home.ok(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
    std::fs::remove_file(&descriptor).unwrap();
    let files = TempDir::new("cert-models");
    let gate = json!({"axes": {"base": {"lambda1": 0.9, "lambda2": 0.99}}, "evidence": "run:test"});
    let (s1, digest) = register(
        &home,
        &files,
        json!({"name": "s1", "version": "t1", "kind": "pass", "task": "system1", "gate": gate}),
        b"the frozen system one",
    );
    let (other, other_digest) = register(
        &home,
        &files,
        json!({"name": "other", "version": "t1", "kind": "pass", "task": "system1", "gate": gate}),
        b"another system one",
    );
    let check = files.file(
        "check.json",
        json!({"suite": "s1", "passed": true, "checks": [{"name": "equivalence", "passed": true}]})
            .to_string()
            .as_bytes(),
    );
    for m in [s1, other] {
        home.ok(&[
            "model",
            "admit",
            &m.to_string(),
            "--check",
            check.to_str().unwrap(),
        ]);
    }
    home.ok(&[
        "model",
        "keep",
        &s1.to_string(),
        "--artifact",
        files.path().join("s1.bin").to_str().unwrap(),
    ]);
    let server = Served::start(&home, &path, true);
    server.ok(
        "POST",
        "/api/models/choose",
        Some(json!({"model": s1, "why": "the frozen model"})),
        ADMIN,
    );
    for name in ["dmg", "ok"] {
        let (status, started) = server.call(
            "POST",
            &format!("/api/datasets/{name}/steps/system1/run"),
            None,
            OPS,
        );
        assert_eq!(status, 202, "{started}");
        let done = server.over(started["job"].as_i64().unwrap());
        assert_eq!(done["state"], "done", "{done}");
    }
    let flagged = server.ok(
        "PUT",
        "/api/datasets/dmg/headers",
        Some(json!({"damaged": true, "why": "an export with stripped names"})),
        OPS,
    );
    assert_eq!(flagged["damaged"], true, "{flagged}");

    // not confirmed yet: the gate waits, and nothing is staged
    let gated = server.ok("POST", "/api/review/gate", Some(json!({})), OPS);
    assert!(
        gated["rounds"][0]["gated"]["waiting"]
            .as_str()
            .unwrap_or_default()
            .contains("not confirmed"),
        "{gated}"
    );
    assert!(model_decisions(&home).is_empty());
    let settings = server.ok("GET", "/api/review/settings", None, OPS);
    assert_eq!(settings["system1"]["confirmed"], false, "{settings}");

    // the graded run's manifest, as the evidence archive writes one
    let evidence_dir = TempDir::new("cert-evidence");
    let manifest_bytes =
        json!({"run": "s1e-test", "files": 2, "sha256": {"grade/grade.json": {"bytes": 10, "sha256": "a".repeat(64)}}})
            .to_string()
            .into_bytes();
    let manifest = evidence_dir.file("MANIFEST.json", &manifest_bytes);
    let manifest_sha = format!("sha256:{}", sha256(&manifest_bytes));
    let plan_commit = "83a75e5875ce4e1b8fe22714dc4fc3e20deffaff";
    let cited = |run: &str, model_digest: &str| json!({"run": run, "manifest_sha256": manifest_sha, "plan_commit": plan_commit, "model_digest": model_digest});
    let result = json!({"bar": 0.01, "n": 5482, "errors": 1, "bound": 0.0008650574506395371,
                        "coverage": 0.892, "test": "T19"});

    // the door's refusals
    let refused = |body: Value, token: &str, extra: &str, want: u16| {
        let (status, doc) = server.call_with(
            "POST",
            "/api/certificates",
            Some(body.clone()),
            token,
            extra,
        );
        assert_eq!(status, want, "{body}: {doc}");
        doc
    };
    let agent = "X-Nils-Actor: {\"kind\": \"agent\", \"name\": \"certifier\"}\r\n";
    refused(
        json!({"models": [s1], "evidence": cited("s1e-test", &digest), "result": result}),
        NIMA,
        agent,
        403,
    );
    refused(
        json!({"sample": "selection:x@1", "models": [s1], "evidence": cited("s1e-test", &digest), "result": result}),
        NIMA,
        "",
        400,
    );
    let doc = refused(
        json!({"models": [s1], "evidence": cited("s1e-test", &other_digest), "result": result}),
        NIMA,
        "",
        400,
    );
    assert!(
        doc["error"].as_str().unwrap().contains("one model"),
        "{doc}"
    );
    refused(
        json!({"models": [s1], "evidence": cited("s1e-test", &format!("sha256:{}", "c".repeat(64))), "result": result}),
        NIMA,
        "",
        404,
    );
    let mut off = result.clone();
    off["bound"] = json!(0.0001);
    let doc = refused(
        json!({"models": [s1], "evidence": cited("s1e-test", &digest), "result": off}),
        NIMA,
        "",
        400,
    );
    assert!(
        doc["error"].as_str().unwrap().contains("Clopper-Pearson"),
        "{doc}"
    );
    let doc = refused(
        json!({"models": [s1], "evidence": cited("s1e-test", &digest),
               "result": {"bar": 0.01, "n": 299, "errors": 1}}),
        NIMA,
        "",
        409,
    );
    assert!(
        doc["error"].as_str().unwrap().contains("above the bar"),
        "{doc}"
    );
    let mut stray = result.clone();
    stray["wrong"] = json!(1);
    refused(
        json!({"models": [s1], "evidence": cited("s1e-test", &digest), "result": stray}),
        NIMA,
        "",
        400,
    );
    assert_eq!(
        server.ok("GET", "/api/certificates", None, OPS)["count"],
        0,
        "nothing refused was recorded"
    );

    // the keyboard path: the door under the person's token, and without one
    let empty = TempDir::new("cert-config");
    let certify = |token: Option<&str>| {
        let mut args = vec![
            "model".to_string(),
            "certify".to_string(),
            "s1@t1".to_string(),
            "--server".to_string(),
            server.url(),
            "--evidence".to_string(),
            manifest.to_str().unwrap().to_string(),
            "--plan-commit".to_string(),
            plan_commit.to_string(),
            "--model-digest".to_string(),
            digest.clone(),
            "--bar".to_string(),
            "0.01".to_string(),
            "--n".to_string(),
            "5482".to_string(),
            "--errors".to_string(),
            "1".to_string(),
            "--bound".to_string(),
            "0.0008650574506395371".to_string(),
            "--coverage".to_string(),
            "0.892".to_string(),
            "--test".to_string(),
            "T19".to_string(),
            "--json".to_string(),
        ];
        if let Some(t) = token {
            args.push("--token".to_string());
            args.push(t.to_string());
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        home.run_env(
            &args,
            None,
            &[("NILS_CONFIG_DIR", empty.path().as_os_str())],
        )
    };
    let (good, _, err) = certify(None);
    assert!(!good && err.contains("nils login"), "{err}");
    let (good, out, err) = certify(Some(NIMA));
    assert!(good, "{err}\n{out}");
    let cert: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(cert["cites"], "evidence", "{cert}");
    assert_eq!(cert["unseals"], false, "{cert}");
    assert_eq!(cert["sample"], "evidence:s1e-test", "{cert}");
    assert_eq!(cert["created_by"], "nima@lab", "{cert}");
    assert_eq!(cert["model_ids"], json!([s1]), "{cert}");
    assert_eq!(cert["evidence"]["run"], "s1e-test", "{cert}");
    assert_eq!(
        cert["evidence"]["manifest_sha256"],
        manifest_sha.as_str(),
        "{cert}"
    );
    assert_eq!(cert["evidence"]["plan_commit"], plan_commit, "{cert}");
    assert_eq!(cert["evidence"]["model_digest"], digest.as_str(), "{cert}");
    assert_eq!(cert["result"]["passes"], true, "{cert}");
    assert_eq!(cert["result"]["confidence"], 0.95, "{cert}");
    assert_eq!(
        cert["result"]["method"], "exact one-sided Clopper-Pearson",
        "{cert}"
    );
    let bound = cert["result"]["bound"].as_f64().unwrap();
    assert!((bound - 0.000_865_057_450_639_537_1).abs() < 1e-9, "{cert}");
    let id = cert["id"].as_i64().unwrap();
    // a second time, the manifest and the served artifact's run named by
    // their sha256 where the files are not at hand: refused as the same run
    let (url, served_sha) = (server.url(), "6".repeat(64));
    let mut again: Vec<String> = [
        "model",
        "certify",
        "s1@t1",
        "--server",
        url.as_str(),
        "--manifest-sha256",
        manifest_sha.as_str(),
        "--run",
        "s1e-test",
        "--plan-commit",
        plan_commit,
        "--model-digest",
        digest.as_str(),
        "--bar",
        "0.01",
        "--n",
        "5482",
        "--errors",
        "1",
        "--equivalence-run",
        "serve-2",
        "--equivalence-sha256",
        served_sha.as_str(),
        "--equivalence-class",
        "I",
        "--token",
        NIMA,
    ]
    .iter()
    .map(|w| w.to_string())
    .collect();
    let args: Vec<&str> = again.iter().map(String::as_str).collect();
    let (good, _, err) = home.run(&args, None);
    assert!(!good && err.contains("already cites"), "{err}");
    // the class goes with the served artifact's run
    again.truncate(again.len() - 8);
    again.extend(["--equivalence-class", "I", "--token", NIMA].map(str::to_string));
    let args: Vec<&str> = again.iter().map(String::as_str).collect();
    let (good, _, err) = home.run(&args, None);
    assert!(!good && err.contains("--equivalence-class"), "{err}");
    // and the unseal door: refused
    let (status, doc) = server.call(
        "POST",
        &format!("/api/certificates/{id}/unseal"),
        None,
        RUSS,
    );
    assert_eq!(status, 409, "{doc}");
    assert!(
        doc["error"].as_str().unwrap().contains("unseals nothing"),
        "{doc}"
    );

    // the model's card shows it as certified, with the citation
    let model = server.ok("GET", &format!("/api/models/{s1}"), None, OPS);
    assert_eq!(model["certificate"]["id"], id, "{model}");
    assert_eq!(
        model["certificate"]["evidence"]["run"], "s1e-test",
        "{model}"
    );
    assert!(
        model["certificate"]["words"]
            .as_str()
            .unwrap()
            .contains("cites the graded run s1e-test"),
        "{model}"
    );
    let listed = server.ok("GET", "/api/models?task=system1", None, OPS);
    let certified: Vec<(i64, bool)> = listed["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["id"].as_i64().unwrap(), m["certificate"].is_object()))
        .collect();
    assert!(
        certified.contains(&(s1, true)) && certified.contains(&(other, false)),
        "{certified:?}"
    );
    let shown = home.ok(&["model", "show", "s1@t1"]);
    assert!(
        shown.contains("certified") && shown.contains("s1e-test"),
        "{shown}"
    );
    let all = home.ok(&["labels", "certificates"]);
    assert!(all.contains("cites its graded run"), "{all}");
    let settings = server.ok("GET", "/api/review/settings", None, OPS);
    assert_eq!(settings["system1"]["confirmed"], true, "{settings}");
    assert_eq!(
        settings["system1"]["certificate"]["cites"], "evidence",
        "{settings}"
    );

    // the audit keeps it: who, the citation and the result
    let audit = home.rows(
        "SELECT principal, CAST(scope AS TEXT), CAST(details AS TEXT) FROM {audit} WHERE action = 'labels.certificate' ORDER BY id",
    );
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].text(0).unwrap(), "nima@lab");
    let scope: Value = serde_json::from_str(audit[0].text(1).unwrap()).unwrap();
    assert_eq!(scope["evidence"], "s1e-test", "{scope}");
    let details: Value = serde_json::from_str(audit[0].text(2).unwrap()).unwrap();
    assert_eq!(details["unseals"], false, "{details}");
    assert_eq!(details["evidence"]["plan_commit"], plan_commit, "{details}");
    assert_eq!(details["result"]["errors"], 1, "{details}");

    // confirmed, the gate acts on the flagged dataset: the T1 is accepted,
    // and where the rules name nothing System 1's answer is staged, as the
    // pack stores it, for a person to confirm
    let gated = server.ok("POST", "/api/review/gate", Some(json!({})), OPS);
    let rounds = gated["rounds"].as_array().unwrap();
    assert_eq!(
        rounds.len(),
        1,
        "only the flagged dataset is gated: {gated}"
    );
    let g = &rounds[0]["gated"];
    assert_eq!(g["waiting"], Value::Null, "{gated}");
    assert_eq!(g["outcomes"]["accepted"], 1, "{gated}");
    assert_eq!(g["outcomes"]["disagree"], 1, "{gated}");
    assert_eq!(g["staged"]["count"], 1, "{gated}");
    let staged = model_decisions(&home);
    assert_eq!(staged.len(), 1, "{staged:?}");
    let (decision, at, value, committed, withdrawn) = staged[0].clone();
    assert_eq!(
        (at.as_str(), value.as_deref(), committed, withdrawn),
        (bare_d.to_string().as_str(), Some("T2*w"), false, false),
        "{staged:?}"
    );
    assert_eq!(
        home.int(&format!(
            "SELECT model_id FROM {{decision}} WHERE id = {decision}"
        )),
        s1
    );
    assert_eq!(
        questions(&home, bare_d),
        [("base:disagree".to_string(), "staged".to_string())]
    );
    // the dataset without the flag: no question, nothing staged
    assert!(questions(&home, bare_n).is_empty());
    assert!(questions(&home, t1_n).is_empty());
    assert!(base_of(&home, bare_n).is_none());
    // a gate run again keeps what it staged
    let again = server.ok("POST", "/api/review/gate", Some(json!({})), OPS);
    assert_eq!(again["rounds"][0]["gated"]["staged"]["count"], 0, "{again}");
    assert_eq!(again["rounds"][0]["gated"]["staged"]["kept"], 1, "{again}");
    assert_eq!(model_decisions(&home).len(), 1);
    assert_eq!(questions(&home, bare_d).len(), 1);

    // a person confirms it; the gate asks no more about it
    let committed = server.ok(
        "POST",
        &format!("/api/decisions/{decision}/commit"),
        Some(json!({})),
        OPS,
    );
    assert_eq!(committed["committed"], json!([decision]), "{committed}");
    let after = server.ok("POST", "/api/review/gate", Some(json!({})), OPS);
    let g = &after["rounds"][0]["gated"];
    assert_eq!(g["outcomes"]["decided"], 1, "{after}");
    assert_eq!(g["staged"]["count"], 0, "{after}");
    let staged = model_decisions(&home);
    assert_eq!(staged.len(), 1, "{staged:?}");
    assert!(staged[0].3, "committed: {staged:?}");
    assert_eq!(
        questions(&home, bare_d),
        [("base:disagree".to_string(), "accepted".to_string())]
    );
    drop(server);

    // a certificate on a sealed sample still takes two people
    {
        let mut registry = nils_registry::home::Home::new(home.dir.path())
            .open()
            .unwrap();
        nils_registry::labels::seal(
            &mut registry,
            "selection:sealed@1",
            None,
            &[t1_n],
            "ops@lab",
        )
        .unwrap();
    }
    let digest_of_sample = {
        let mut store = home.store();
        nils_registry::labels::sample_digest(&mut store, "selection:sealed@1").unwrap()
    };
    let server = Served::start(&home, &path, false);
    let sealed = server.ok(
        "POST",
        "/api/certificates",
        Some(
            json!({"sample": "selection:sealed@1", "models": ["other@t1"],
                    "result": {"sample": "selection:sealed@1", "sample_digest": digest_of_sample,
                               "risk": 0.05, "n": 1, "errors": 0}}),
        ),
        NIMA,
    );
    assert_eq!(sealed["cites"], "sample", "{sealed}");
    assert_eq!(sealed["unseals"], true, "{sealed}");
    let sid = sealed["id"].as_i64().unwrap();
    let (status, _) = server.call(
        "POST",
        &format!("/api/certificates/{sid}/unseal"),
        None,
        NIMA,
    );
    assert_eq!(status, 409, "who recorded it does not unseal it");
    let done = server.ok(
        "POST",
        &format!("/api/certificates/{sid}/unseal"),
        None,
        RUSS,
    );
    assert_eq!(done["stacks"], 1, "{done}");
    let listed = server.ok("GET", "/api/certificates", None, OPS);
    assert_eq!(listed["count"], 2, "{listed}");
    let actions: Vec<String> = home
        .rows("SELECT action FROM {audit} WHERE action IN ('labels.certificate', 'labels.unseal') ORDER BY id")
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
    assert_eq!(
        actions,
        ["labels.certificate", "labels.certificate", "labels.unseal"]
    );
    drop(server);
}
