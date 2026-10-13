// SPDX-License-Identifier: AGPL-3.0-only
//! Post-contrast served, end to end (record 55 E3 and F5; spec §12.2 and
//! §12.3; gate tests T25 and T27): a dataset's post-contrast step runs
//! `postcontrast-cascade` with the promoted cascade's parts, each input
//! filled by the part of its name; the run's `stacks.json` carries the
//! session facts the model reads (`x-nils.input.contrast`) and the
//! datasets that hold each stack; the stacks the cascade defers are queued
//! to `postcontrast-full`; the answers become staged decisions authored by
//! the cascade, which anchor the time rule, so after the run every stack of
//! the session carries its post-contrast label, on the step, on the
//! stack's evidence line and at `nils contrast show`.
//!
//! No container runtime is needed and no real image is run: a stand-in for
//! podman runs the stand-in pipelines' commands on the host. On SQLite
//! always, and on Postgres where a test DSN is set.

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

fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
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

/// The cascade's stand-in: its name, its three model inputs in order, the
/// contrast facts asked for, a table per stack with its `reason`, and
/// proposals by the cascade. It answers the pre T1 (series 2) `not_given`,
/// defers the post T1 (series 5) and says `not-t1-brain` of the rest.
fn cascade_stand_in() -> String {
    format!(
        r#"# SPDX-License-Identifier: AGPL-3.0-only
name: postcontrast-cascade
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/postcontrast-stand-in@sha256:{}"
command-line: |
  python3 -c '
  import json, os, sys
  m = json.load(open(sys.argv[1])); out = sys.argv[2]
  man = json.load(open(sys.argv[3] + "/manifest.json"))
  ids = {{x["input"]: x["model_id"] for x in man["models"]}}
  assert list(ids) == ["student", "fusion", "cascade"], ids
  units, proposals = [], []
  for s in m["stacks"]:
      c, h = s["contrast"], s["header"]
      assert h["datasets"] == ["ds"], h
      assert c["acquisition_time"] and c["series_number"] is not None and "post_contrast_rules" in c, c
      sn = c["series_number"]
      if sn == 2:
          row = {{"stack": s["stack_id"], "p_given": 0.01, "reason": "", "stage": "fs3"}}
      elif sn == 5:
          row = {{"stack": s["stack_id"], "p_given": 0.5, "reason": "deferred", "stage": ""}}
      else:
          row = {{"stack": s["stack_id"], "p_given": 0.5, "reason": "not-t1-brain", "stage": ""}}
      d = os.path.join(out, "postcontrast"); os.makedirs(d, exist_ok=True)
      json.dump(row, open(os.path.join(d, "%d.json" % s["stack_id"]), "w"))
      units.append({{"unit_id": s["unit"], "status": "succeeded", "derivatives": ["postcontrast/%d.json" % s["stack_id"]]}})
      if sn == 2:
          proposals.append({{"stack_id": s["stack_id"], "axis": "post_contrast", "value": "not_given", "probabilities": {{"given": 0.01, "not_given": 0.99}}, "model_id": ids["cascade"]}})
  for e in m.get("context", []):
      assert "contrast" in e and "header" in e and "series_id" in e, e
  json.dump({{"schema_version": "1", "units": units, "proposals": proposals}}, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation] [Inputs]
x-nils:
  analysis-level: stack
  input: {{layout: stacks, header: true, geometry: true, contrast: true}}
  inputs:
    - {{id: student, type: model}}
    - {{id: fusion, type: model}}
    - {{id: cascade, type: model}}
  outputs:
    - id: scores
      kind: table
      format: json
      path-template: "postcontrast/{{stack}}.json"
      media-type: application/json
      columns:
        - {{name: p_given}}
        - {{name: reason, type: text}}
        - {{name: stage, type: text}}
  needs: {{gpu: none}}
  proposals: [{{axis: post_contrast}}]
"#,
        "c".repeat(64)
    )
}

/// The full pipeline as it ships (`pipelines/nils-postcontrast/postcontrast-full`),
/// its descriptor unchanged but for the memory it asks, which a test
/// machine may not have (the runner refuses a unit that asks more memory
/// than the machine holds); the image's program is the stand-in
/// `nils-postcontrast` on the search path ([`FULL_PROGRAM`]).
fn full_shipped() -> String {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../pipelines/nils-postcontrast/postcontrast-full/nils.job.yml"),
    )
    .unwrap();
    assert!(text.contains("    memory-gb: 64\n"), "the shipped need moved");
    text.replace("    memory-gb: 64\n", "    memory-gb: 1\n")
}

/// A stand-in for the full image's program: it takes the shipped command
/// line, checks the model inputs it is given are the fusion, the arms and
/// the cascade in that order, and answers `given` for every stack.
const FULL_PROGRAM: &str = r#"#!/usr/bin/env python3
import json, os, sys
a = sys.argv[1:]
assert a[0] == "full", a
arg = lambda k: a[a.index(k) + 1]
m = json.load(open(arg("--stacks"))); out = arg("--output")
man = json.load(open(os.path.join(arg("--inputs"), "manifest.json")))
ids = {x["input"]: x["model_id"] for x in man["models"]}
assert list(ids) == ["fusion", "arms", "cascade"], ids
for k in ("--threads", "--memory-gb", "--synthseg-procs"):
    assert k in a, a
units, proposals = [], []
for s in m["stacks"]:
    u = s["unit"]; d = os.path.join(out, "postcontrast"); os.makedirs(d, exist_ok=True)
    open(os.path.join(d, f"{s['stack_id']}.json"), "w").write("{}")
    units.append({"unit_id": u, "status": "succeeded", "derivatives": [f"postcontrast/{s['stack_id']}.json"]})
    proposals.append({"stack_id": s["stack_id"], "axis": "post_contrast", "value": "given", "probabilities": {"given": 0.99, "not_given": 0.01}, "model_id": ids["cascade"]})
json.dump({"schema_version": "1", "units": units, "proposals": proposals}, open(os.path.join(out, "results.json"), "w"))
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

/// One step of a summary's or a cohort document's steps.
fn step(doc: &Value, name: &str) -> Value {
    doc["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no step {name}: {doc}"))
}

/// A search path whose first folder holds the stand-in podman and a card
/// reader that finds no card, so a host's own runtime or GPU is never used.
fn fake_path(bin: &TempDir) -> OsString {
    let fake = bin.file("podman", FAKE_PODMAN.as_bytes());
    let no_gpu = bin.file("nvidia-smi", b"#!/bin/sh\nexit 1\n");
    let full = bin.file("nils-postcontrast", FULL_PROGRAM.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for f in [&fake, &no_gpu, &full] {
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

fn round(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("pc-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a post-contrast test key\n"));
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
    // one session: a FLAIR, the pre T1, a FLAIR, the post T1, a FLAIR
    let dir = TempDir::new("pc-ds");
    let study = "1.2.826.0.1.3680043.8.497.9.1";
    series(&dir, study, 1, Kind::Flair, 95_500);
    series(&dir, study, 2, Kind::T1, 100_000);
    series(&dir, study, 3, Kind::Flair, 100_500);
    series(&dir, study, 5, Kind::T1, 102_000);
    series(&dir, study, 6, Kind::Flair, 103_000);
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
    let work = TempDir::new("pc-work");
    home.ok(&[
        "place",
        "add",
        "work",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);
    let bin = TempDir::new("pc-bin");
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
    // labelled, and no session anchored: no T1 answer yet
    let stacks: Vec<i64> = [1, 2, 3, 5, 6]
        .iter()
        .map(|n| stack_of(&home, *n))
        .collect();
    let show = |stack: i64| home.json(&["contrast", "show", &stack.to_string(), "--json"]);
    let first = show(stacks[0]);
    assert_eq!(first["contrast_state"], "undetermined", "{first}");
    assert_eq!(first["reason"], "C7_no_t1", "{first}");
    assert_eq!(first["anchored"], false, "{first}");

    // no pipeline answers the step yet
    let server = Served::start(&home, &path, false, &logged);
    let s = server.get("/api/datasets/ds/summary", READS);
    let pc = step(&s, "post_contrast");
    assert_eq!(pc["refusal"]["reason"], "no_pipeline", "{s}");
    assert_eq!(pc["label"]["labelled"], 5, "{s}");
    assert_eq!(pc["label"]["in_anchored_sessions"], 0, "{s}");
    drop(server);

    // the cascade and the full pipeline in the catalog
    for (name, text) in [
        ("postcontrast-cascade", cascade_stand_in()),
        ("postcontrast-full", full_shipped()),
    ] {
        let descriptor = work.file(&format!("{name}.yml"), text.as_bytes());
        let added = home.json(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
        std::fs::remove_file(&descriptor).unwrap();
        assert_eq!(added["label"], format!("{name}@1"), "{added}");
    }
    // the cascade's parts and the cascade, admitted
    let files = TempDir::new("pc-models");
    let (student, student_digest) = register(
        &home,
        &files,
        json!({"name": "pc-student", "version": "t1", "kind": "encoder", "task": "features:postcontrast_slices"}),
        b"three seeds of a student",
    );
    let (fusion, fusion_digest) = register(
        &home,
        &files,
        json!({"name": "pc-fusion", "version": "t1", "kind": "head", "task": "axis:post_contrast",
               "encoder": {"digest": student_digest}, "threshold": 0.9}),
        b"model r1's fusion",
    );
    let (arms, arms_digest) = register(
        &home,
        &files,
        json!({"name": "pc-arms", "version": "t1", "kind": "head", "task": "features:postcontrast_arms",
               "encoder": {"digest": student_digest}}),
        b"model r1's arms",
    );
    let (cascade, _) = register(
        &home,
        &files,
        json!({"name": "pc-cascade", "version": "t1", "kind": "head", "task": "axis:post_contrast",
               "encoders": [{"digest": student_digest}], "threshold": 0.9,
               "parts": {"student": student_digest, "fusion": fusion_digest, "arms": arms_digest}}),
        b"the cascade c1-fs3-m",
    );
    let check = files.file(
        "check.json",
        json!({"suite": "pc", "passed": true, "checks": [{"name": "class", "passed": true}]})
            .to_string()
            .as_bytes(),
    );
    for m in [student, fusion, arms, cascade] {
        home.ok(&[
            "model",
            "admit",
            &m.to_string(),
            "--check",
            check.to_str().unwrap(),
        ]);
    }
    // admitted, the cascade waits for a person to promote it
    let server = Served::start(&home, &path, false, &logged);
    let s = server.get("/api/datasets/ds/summary", READS);
    let refusal = step(&s, "post_contrast")["refusal"].clone();
    assert_eq!(refusal["reason"], "no_model", "{s}");
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("answers the step once it is chosen or promoted"),
        "{refusal}"
    );
    drop(server);
    home.ok(&["model", "promote", &cascade.to_string()]);

    // the step's run: the cascade with its parts, in the descriptor's order
    let server = Served::start(&home, &path, true, &logged);
    let (status, started) = server.run("datasets/ds", "post_contrast", OPS);
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
        [student, fusion, cascade].map(|m| m.to_string()),
        "{started}"
    );
    let done = server.over(started["job"].as_i64().unwrap());
    assert_eq!(done["state"], "done", "{done}");
    // the deferred post T1 went to the full pipeline, queued under the step
    let fallback = {
        let mut store = home.store();
        let sql = format!(
            "SELECT id FROM {} WHERE kind = 'pipeline' ORDER BY id DESC",
            store.qualified("job")
        );
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    };
    assert_ne!(
        fallback,
        started["job"].as_i64().unwrap(),
        "a second job was queued"
    );
    let full = server.over(fallback);
    assert_eq!(full["state"], "done", "{full}");
    let argv = full["args"]["queued"].to_string();
    assert!(argv.contains("postcontrast-full@1"), "{full}");
    assert_eq!(full["args"]["step"], "post_contrast", "{full}");

    // T27: every stack of the session carries its label, the answers of
    // the cascade and of the full pipeline anchoring the time rule
    let want = [
        (stacks[0], "pre", None, "t1_anchor_time_rule"),
        (stacks[1], "pre", None, "image"),
        (stacks[2], "undetermined", None, "t1_anchor_time_rule"),
        (stacks[3], "post", Some(0.0), "image"),
        (stacks[4], "post", Some(10.0), "t1_anchor_time_rule"),
    ];
    for (stack, state, minutes, source) in want {
        let l = show(stack);
        assert_eq!(l["contrast_state"], state, "{l}");
        assert_eq!(l["minutes_since_injection"].as_f64(), minutes, "{l}");
        assert_eq!(l["contrast_source"], source, "{l}");
        assert_eq!(l["contrast_prior_72h"], "no", "{l}");
        assert_eq!(l["anchored"], true, "{l}");
    }
    assert_eq!(show(stacks[2])["reason"], "R_window");
    assert_eq!(show(stacks[4])["injection_ref"], "first_post_t1");
    // on the step, and on the stack's evidence line
    let s = server.get("/api/datasets/ds/summary", READS);
    let pc = step(&s, "post_contrast");
    assert_eq!(pc["label"]["in_anchored_sessions"], 5, "{s}");
    assert_eq!(pc["label"]["by_state"]["post"], 2, "{s}");
    assert_eq!(pc["label"]["by_state"]["pre"], 2, "{s}");
    let why = server.get(&format!("/api/stacks/{}/why", stacks[4]), OPS);
    assert_eq!(why["contrast_label"]["contrast_state"], "post", "{why}");
    assert_eq!(
        why["contrast_label"]["minutes_since_injection"], 10.0,
        "{why}"
    );
    drop(server);
    drop(files);
    drop(bin);
    drop(work);
    drop(dir);
}

#[test]
fn post_contrast_is_served_through_its_cascade_and_labels_every_stack() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    round(None);
}

#[test]
fn post_contrast_is_served_on_postgres_too() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
        return;
    };
    let schema = "nils_post_contrast";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    round(Some((dsn.clone(), schema.to_string())));
    drop();
}
