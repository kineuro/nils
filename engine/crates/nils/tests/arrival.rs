// SPDX-License-Identifier: AGPL-3.0-only
//! Scoring at arrival and the slice cache (record 55 E5; spec section
//! 10.4; gate test T26 as ruling 35 (b) wrote it beside the original): a
//! dataset brought in is scored by System 1 without a manual run; body part
//! and post-contrast are optional models a person runs from the dataset's
//! step, and a re-score after a model update reads the slice cache instead
//! of the files.
//!
//! A bring-in ends with `score`, which queues System 1's run over the
//! batch's new stacks with the chosen model, since its pipeline declares
//! `x-nils.on-arrival`, and queues no body part, though its pipeline
//! declares it too: the score names it under `optional`, and the dataset's
//! body-part step waits, saying how many scans have no body part (`hint`).
//! A person runs it from the step's door; the run is given the working
//! place's `slice-cache/` and a `stacks.json` that names each file's SOP
//! Instance UID, and the sha256 of each file a run reported reading whole
//! is kept. After a new head is chosen, the step's run from its door finds
//! every file's sha256 in `stacks.json`, and the stand-in image reads its
//! inputs from the cache: the files it reads are counted in a log, and none
//! is read. A bring-in of nothing new queues no score, and a caller who may
//! not run a pipeline gets the score skipped, with the reason.
//!
//! No container runtime is needed and no real image is run: a stand-in for
//! podman runs the stand-in pipeline's command on the host. On SQLite
//! always, and on Postgres where a test DSN is set.

use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::{Path, PathBuf};
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
/// folder mounted there, and the mounts in `FAKE_MOUNTS` for what a
/// manifest names; with `FAKE_PODMAN_EXIT` it fails as an image that
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
env["FAKE_MOUNTS"] = json.dumps(mounts)
sys.exit(subprocess.call(argv, env=dict(os.environ, **env)))
"#;

/// System 1's stand-in, which scores at arrival: it reads the registry's
/// rows and no file, and says T1w on base for every stack.
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
      d = os.path.join(out, "system1"); os.makedirs(d, exist_ok=True)
      json.dump({{"value": "T1w"}}, open(os.path.join(d, "%d.json" % s["stack_id"]), "w"))
      units.append({{"unit_id": s["unit"], "status": "succeeded", "derivatives": ["system1/%d.json" % s["stack_id"]]}})
      opinions.append({{"stack_id": s["stack_id"], "axis": "base", "value": "T1w", "p": 0.9, "reliability": 0.9,
                        "model_id": ids[0], "rules_read": {{"value": "none"}}}})
  json.dump({{"schema_version": "1", "units": units, "opinions": opinions}}, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation] [Inputs]
x-nils:
  analysis-level: stack
  on-arrival: true
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

/// The body-part pipeline's stand-in, which declares scoring at arrival and keeps
/// its input in the slice cache: per stack, where every file's sha256 is in
/// `stacks.json` and the cache holds the entry those identities key, it
/// reads no file; otherwise it reads each file whole, logs the read, keeps
/// the entry and reports each file's sha256 as `read`. It proposes `brain`
/// and `head` by the head and the coarse mode the run was given.
fn stand_in() -> String {
    format!(
        r#"# SPDX-License-Identifier: AGPL-3.0-only
name: bodypart-infer-fusion
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/bodypart-stand-in@sha256:{}"
command-line: |
  python3 -c '
  import hashlib, json, os, sys
  m = json.load(open(sys.argv[1])); out = sys.argv[2]; cache = sys.argv[4]
  man = json.load(open(sys.argv[3] + "/manifest.json"))
  ids = {{x["input"]: x["model_id"] for x in man["models"]}}
  assert list(ids) == ["encoder", "head", "coarse"], ids
  mounts = json.loads(os.environ["FAKE_MOUNTS"])
  src = {{s["id"]: mounts[s["mount"]] for s in m["sources"]}}
  v = os.path.join(cache, "stand-in-planes:v1"); os.makedirs(v, exist_ok=True)
  log = open(os.environ["STANDIN_READS"], "a")
  key = lambda pairs: hashlib.sha256(chr(10).join(sorted(u + " " + h for u, h in pairs)).encode()).hexdigest()
  units, proposals = [], []
  for s in m["stacks"]:
      assert s["identity"]["study_instance_uid"], s
      idents = [f["identity"] for f in s["files"]]
      u = s["unit"]; d = os.path.join(out, u); os.makedirs(d, exist_ok=True)
      unit = {{"unit_id": u, "status": "succeeded", "derivatives": [u + "/answer.json"]}}
      known = all(i["sha256"] for i in idents)
      if known and os.path.exists(os.path.join(v, key([(i["sop_instance_uid"], i["sha256"]) for i in idents]) + ".json")):
          unit["metrics"] = {{"files_read": 0}}
      else:
          read = []
          for n, f in enumerate(s["files"]):
              b = open(os.path.join(src[f["source"]], f["path"]), "rb").read()
              log.write(f["path"] + chr(10))
              read.append({{"file": n, "sha256": "sha256:" + hashlib.sha256(b).hexdigest()}})
          k = key([(i["sop_instance_uid"], r["sha256"]) for i, r in zip(idents, read)])
          json.dump({{"files": len(read)}}, open(os.path.join(v, k + ".json"), "w"))
          unit["read"] = read; unit["metrics"] = {{"files_read": len(read)}}
      open(os.path.join(d, "answer.json"), "w").write(json.dumps({{"stack": s["stack_id"], "head": ids["head"]}}))
      units.append(unit)
      proposals.append({{"stack_id": s["stack_id"], "axis": "body_part", "value": "brain", "probabilities": {{"brain": 0.9, "spine": 0.1}}, "model_id": ids["head"]}})
      proposals.append({{"stack_id": s["stack_id"], "axis": "body_region", "value": "head", "probabilities": {{"head": 0.9, "spine": 0.1}}, "model_id": ids["coarse"]}})
  json.dump({{"schema_version": "1", "units": units, "proposals": proposals}}, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation] [Inputs] [Cache]
x-nils:
  analysis-level: stack
  input: {{layout: stacks}}
  on-arrival: true
  cache: {{inputs: [stand-in-planes:v1]}}
  inputs:
    - {{id: encoder, type: model}}
    - {{id: head, type: model}}
    - {{id: coarse, type: model, optional: true}}
  outputs:
    - id: answer
      kind: output
      path-template: "stack-{{stack}}/answer.json"
      media-type: application/json
  needs: {{gpu: none}}
  proposals: [{{axis: body_part}}, {{axis: body_region}}]
"#,
        "b".repeat(64)
    )
}

struct Home {
    dir: TempDir,
    /// The Postgres DSN and schema, when the registry lives there.
    pg: Option<(String, String)>,
}

impl Home {
    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        self.run_with(args, stdin, &[])
    }

    /// [`Home::run`] with more of the environment: a worker's search path,
    /// what its stand-ins log to.
    fn run_with(
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
            .envs(env.iter().copied())
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
/// Data and Pipelines work at detail plain: a bring-in and its sort, and
/// no run, which reads pixels.
const CLERK: &str = "a-plain-clerk-token-of-length";

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
            format!("{CLERK}=cal@lab:data:work,data:see,pipelines:work,pipelines:see"),
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

    /// A dataset's summary with its counts read now (2026-10-31): the door
    /// answers the kept counts at once and reads them again apart when what
    /// they count moved, saying `counting`, so the summary is asked again
    /// until it no longer counts, a minute at most.
    fn summary(&self, path: &str, token: &str) -> Value {
        let mut doc = self.get(path, token);
        for _ in 0..600 {
            if doc["counting"] != true {
                return doc;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            doc = self.get(path, token);
        }
        panic!("{path} never stopped counting: {doc}");
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

/// A dataset of `subjects` subjects, each one session of two series of two
/// files, under its own study UIDs (`root` tells two datasets apart).
fn dataset(dir: &TempDir, root: u32, subjects: u32) {
    for p in 0..subjects {
        let patient = format!("S-{root}-{p}");
        let study = format!("1.2.826.0.1.3680043.8.497.{root}.{}", p + 1);
        for (n, description) in ["t1 mprage", "t2 flair"].iter().enumerate() {
            let series = format!("{study}.{}", n + 1);
            for instance in 1..=2u32 {
                let sop = format!("{series}.{instance}");
                let mut e = synth::minimal_mr(&study, &series, &sop);
                e.push(synth::text(tags::PATIENT_ID, VR::LO, &patient));
                e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
                e.push(synth::text(
                    tags::SERIES_NUMBER,
                    VR::IS,
                    &(n + 1).to_string(),
                ));
                e.push(synth::text(
                    tags::INSTANCE_NUMBER,
                    VR::IS,
                    &instance.to_string(),
                ));
                e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
                dir.file(
                    &format!("sub-{p}/s{n}/IM_{instance:04}"),
                    &synth::part10(&MetaFields::mr(&sop), &e, true),
                );
            }
        }
    }
}

/// The ids of the jobs of a kind, oldest first.
fn jobs_of(home: &Home, kind: &str) -> Vec<i64> {
    let mut store = home.store();
    let sql = format!(
        "SELECT id FROM {} WHERE kind = '{kind}' ORDER BY id",
        store.qualified("job")
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect()
}

/// The first job of a kind once there is one, two minutes at most.
fn first_of(home: &Home, kind: &str, after: usize) -> i64 {
    for _ in 0..1200 {
        if let Some(id) = jobs_of(home, kind).get(after) {
            return *id;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("no job of kind {kind} was queued");
}

/// The lines of the stand-in's read log: one per file it read.
fn reads(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

/// How many files' content the registry knows now.
fn known(home: &Home) -> i64 {
    let mut store = home.store();
    nils_registry::content::known(&mut store).unwrap()
}

/// What a round starts from: a registry with the dataset `ds` of three
/// sessions (six scans, twelve files), the working place `work`, the
/// body-part and System 1 stand-ins with their models, admitted, and a
/// search path whose first folder holds the stand-in podman.
struct Lab {
    home: Home,
    dir: TempDir,
    work: TempDir,
    files: TempDir,
    bin: TempDir,
    path: OsString,
    args_log: PathBuf,
    read_log: PathBuf,
    s1: i64,
    encoder_digest: String,
    check: PathBuf,
}

fn lab(pg: Option<(String, String)>) -> Lab {
    lab_with(pg, &system1_stand_in())
}

/// [`lab`] with System 1's pipeline added from `system1`'s text.
fn lab_with(pg: Option<(String, String)>, system1: &str) -> Lab {
    let home = Home {
        dir: TempDir::new("arrival-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("an arrival test key\n"));
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
    // a dataset of three sessions, six scans, twelve files
    let dir = TempDir::new("arrival-ds");
    dataset(&dir, 7, 3);
    {
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
    }
    let work = TempDir::new("arrival-work");
    home.ok(&[
        "place",
        "add",
        "work",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);
    // the body-part pipeline, which declares scoring at arrival, and its
    // model; System 1's pipeline, which scores at arrival, and its model
    let descriptor = work.file("bodypart-infer-fusion.yml", stand_in().as_bytes());
    let added = home.json(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
    std::fs::remove_file(&descriptor).unwrap();
    assert_eq!(added["label"], "bodypart-infer-fusion@1", "{added}");
    let descriptor = work.file("system1-score.yml", system1.as_bytes());
    let added = home.json(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
    std::fs::remove_file(&descriptor).unwrap();
    assert_eq!(added["label"], "system1-score@1", "{added}");
    let files = TempDir::new("arrival-models");
    let (s1, _) = register(
        &home,
        &files,
        json!({"name": "s1", "version": "t1", "kind": "pass", "task": "system1",
               "gate": {"axes": {"base": {"lambda1": 0.9, "lambda2": 0.99}}, "evidence": "run:test"}}),
        b"a system one",
    );
    let (_, encoder_digest) = register(
        &home,
        &files,
        json!({"name": "bp-enc", "version": "t1", "kind": "encoder", "task": "features:bodypart_image"}),
        b"an encoder's weights",
    );
    let (head, head_digest) = register(
        &home,
        &files,
        json!({"name": "bp-head", "version": "t1", "kind": "head", "task": "axis:body_part",
               "encoder": {"digest": encoder_digest}, "threshold": 0.5}),
        b"a head",
    );
    let (coarse, _) = register(
        &home,
        &files,
        json!({"name": "bp-coarse", "version": "t1", "kind": "head", "task": "axis:body_region",
               "encoder": {"digest": encoder_digest}, "threshold": 0.5,
               "params": {"head": {"digest": head_digest}}}),
        b"a coarse mode",
    );
    let check = files.file(
        "check.json",
        json!({"suite": "arrival", "passed": true, "checks": [{"name": "ece", "passed": true}]})
            .to_string()
            .as_bytes(),
    );
    for m in [s1, head, coarse] {
        home.ok(&[
            "model",
            "admit",
            &m.to_string(),
            "--check",
            check.to_str().unwrap(),
        ]);
    }
    let bin = TempDir::new("arrival-bin");
    let path = fake_path(&bin);
    let args_log = bin.path().join("args.log");
    let read_log = bin.path().join("reads.log");
    Lab {
        home,
        dir,
        work,
        files,
        bin,
        path,
        args_log,
        read_log,
        s1,
        encoder_digest,
        check,
    }
}

fn round(pg: Option<(String, String)>) {
    let Lab {
        home,
        dir,
        work,
        files,
        bin,
        path,
        args_log,
        read_log,
        s1,
        encoder_digest,
        check,
    } = lab(pg);
    let env: [(&str, &OsStr); 2] = [
        ("FAKE_PODMAN_ARGS", args_log.as_os_str()),
        ("STANDIN_READS", read_log.as_os_str()),
    ];

    // T26, first half: the dataset brought in, and nobody runs anything
    let server = Served::start(&home, &path, true, &env);
    let (status, queued) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["bring-in", "@ds", "--name", "first"]})),
        OPS,
    );
    assert_eq!(status, 202, "{queued}");
    let score = server.over(first_of(&home, "score", 0));
    assert_eq!(score["state"], "done", "{score}");
    let result = &score["result"];
    assert_eq!(result["batch"], "first", "{score}");
    assert_eq!(result["new_stacks"], 6, "{score}");
    // ruling 35 (b): System 1 alone scores at arrival
    let queued_runs = result["queued"].as_array().unwrap();
    assert_eq!(queued_runs.len(), 1, "{score}");
    assert_eq!(queued_runs[0]["step"], "system1", "{score}");
    assert_eq!(queued_runs[0]["stacks"], 6, "{score}");
    assert_eq!(result["skipped"], json!([]), "{score}");
    // body part and post-contrast are a person's runs, never the arrival's
    assert_eq!(
        result["optional"],
        json!(["body_part", "post_contrast"]),
        "{score}"
    );
    // the run with the chosen model, for the dataset, under the chain's
    // principal, and it ran, reading no file
    let run = server.over(queued_runs[0]["job"].as_i64().unwrap());
    assert_eq!(run["state"], "done", "{run}");
    assert_eq!(run["args"]["step"], "system1", "{run}");
    assert_eq!(run["args"]["for"], "dataset:ds", "{run}");
    assert_eq!(run["args"]["arrival"]["batch"], "first", "{run}");
    let argv = run["args"]["queued"].to_string();
    assert!(argv.contains(&format!("\"--model\",\"{s1}\"")), "{run}");
    // six new stacks are far fewer than a part of System 1's holds
    assert!(!argv.contains("--chunk"), "{run}");
    assert_eq!(reads(&read_log), 0, "System 1 reads rows, no file");
    // every scan has System 1's opinion
    let opinions = {
        let mut store = home.store();
        let sql = format!(
            "SELECT COUNT(DISTINCT stack_id) FROM {} WHERE model_id = {s1}",
            store.qualified("opinion")
        );
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    };
    assert_eq!(opinions, 6);
    // the body-part step waits for a person, and says what it would add
    let s = server.summary("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "waiting", "{s}");
    assert_eq!(bp["answered"], 0, "{s}");
    assert_eq!(
        bp["hint"],
        json!({"kind": "no_body_part", "scans": 6}),
        "{s}"
    );
    let pc = step(&s, "post_contrast");
    assert_eq!(pc["hint"]["kind"], "t1_brain_contrast_unknown", "{s}");
    assert_eq!(jobs_of(&home, "pipeline").len(), 1, "System 1's run only");

    // a person runs body part from its step: every scan has its image
    // answer, and every file was read once
    let (status, started) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 202, "{started}");
    let first_run = server.over(started["job"].as_i64().unwrap());
    assert_eq!(first_run["state"], "done", "{first_run}");
    let s = server.summary("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "done", "{s}");
    assert_eq!(bp["answered"], 6, "{s}");
    assert_eq!(bp["hint"]["scans"], 0, "{s}");
    assert_eq!(reads(&read_log), 12);
    // the content of each file read whole is known now
    assert_eq!(known(&home), 12);
    // the slice cache is in the working place
    assert!(
        work.path().join("slice-cache/stand-in-planes:v1").is_dir(),
        "the cache's version folder"
    );

    // T26, second half: a new head, chosen; the step's run reads the cache
    let (head2, _) = register(
        &home,
        &files,
        json!({"name": "bp-head", "version": "t2", "kind": "head", "task": "axis:body_part",
               "encoder": {"digest": encoder_digest}, "threshold": 0.5}),
        b"a better head",
    );
    home.ok(&[
        "model",
        "admit",
        &head2.to_string(),
        "--check",
        check.to_str().unwrap(),
    ]);
    home.ok(&[
        "model",
        "choose",
        &head2.to_string(),
        "--why",
        "a better head",
    ]);
    let (status, started) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 202, "{started}");
    let models: Vec<String> = started["command"]
        .as_array()
        .unwrap()
        .windows(2)
        .filter(|w| w[0] == "--model")
        .map(|w| w[1].as_str().unwrap().to_string())
        .collect();
    assert!(models.contains(&head2.to_string()), "{started}");
    let rescore = server.over(started["job"].as_i64().unwrap());
    assert_eq!(rescore["state"], "done", "{rescore}");
    assert_eq!(reads(&read_log), 12, "the re-score read no file");
    let s = server.summary("/api/datasets/ds/summary", READS);
    assert_eq!(step(&s, "body_part")["state"], "done", "{s}");

    // a bring-in of nothing new queues no score
    let (status, again) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["bring-in", "@ds", "--name", "again"]})),
        OPS,
    );
    assert_eq!(status, 202, "{again}");
    let mut read = server.over(again["job"].as_i64().unwrap());
    assert_eq!(read["state"], "done", "{read}");
    // the worker writes where the chain ended once the read is done
    for _ in 0..100 {
        if read["result"]["chain_ended"].is_object() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        read = server.get(&format!("/api/jobs/{}", again["job"]), OPS);
    }
    let skipped_steps = read["result"]["chain_ended"]["skipped"].to_string();
    assert!(skipped_steps.contains("score"), "{read}");
    assert_eq!(jobs_of(&home, "score").len(), 1);
    assert_eq!(jobs_of(&home, "pipeline").len(), 3);

    // a caller who may sort but not run a pipeline: the score is skipped,
    // and says why
    let other = TempDir::new("arrival-ds2");
    dataset(&other, 8, 1);
    drop(server);
    home.ok(&[
        "place",
        "add",
        "ds2",
        other.path().to_str().unwrap(),
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
    let server = Served::start(&home, &path, true, &env);
    let (status, queued) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["bring-in", "@ds2", "--name", "clerk"]})),
        CLERK,
    );
    assert_eq!(status, 202, "{queued}");
    let score = server.over(first_of(&home, "score", 1));
    assert_eq!(score["state"], "done", "{score}");
    assert_eq!(score["result"]["new_stacks"], 2, "{score}");
    let skipped = score["result"]["score_skipped"].as_array().unwrap();
    assert!(skipped.iter().all(|s| s["reason"] == "no_grant"), "{score}");
    assert!(
        skipped[0]["why"].as_str().unwrap().contains("detail quasi"),
        "{score}"
    );
    assert_eq!(jobs_of(&home, "pipeline").len(), 3, "no run was queued");
    drop(server);

    // the cache as `nils cache show` reads it
    let shown = home.json(&["cache", "show", "--json"]);
    let versions = shown["places"][0]["versions"].as_array().unwrap();
    assert_eq!(versions[0]["version"], "stand-in-planes:v1", "{shown}");
    assert_eq!(
        versions[0]["read_by"][0], "bodypart-infer-fusion@1",
        "{shown}"
    );
    assert_eq!(versions[0]["files"], 6, "{shown}");
    drop(files);
    drop(bin);
    drop(work);
    drop(other);
    drop(dir);
}

/// The runs a job ran, oldest first, each with how many stacks its handle
/// holds.
fn runs_of(home: &Home, job: i64) -> Vec<(i64, i64)> {
    let mut store = home.store();
    let sql = format!(
        "SELECT r.id, (SELECT COUNT(*) FROM {} m WHERE m.handle_id = r.handle_id) \
         FROM {} r WHERE r.job_id = {job} ORDER BY r.id",
        store.qualified("handle_member"),
        store.qualified("pipeline_run")
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| (r.int(0).unwrap(), r.int(1).unwrap()))
        .collect()
}

/// A job as `nils jobs show` gives it.
fn job_doc(home: &Home, id: i64) -> Value {
    home.json(&["jobs", "show", &id.to_string(), "--json"])
}

/// How many stacks have an opinion of the model.
fn opinions_of(home: &Home, model: i64) -> i64 {
    let mut store = home.store();
    let sql = format!(
        "SELECT COUNT(DISTINCT stack_id) FROM {} WHERE model_id = {model}",
        store.qualified("opinion")
    );
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

/// 2026-10-17, on a production install: a dataset's bring-in queued at the
/// keyboard a step at a time (`nils jobs enqueue`) recorded no grants, so
/// its score skipped System 1 for want of `pipelines:work`. A job queued at
/// the keyboard records the keyboard's reach, so the score queues System
/// 1's run over the batch's new stacks, and the worker runs it. The same
/// day's run by hand put 456,110 stacks in one container, which was killed
/// for memory: with parts of four stacks (as the tests set them), the six
/// new stacks are queued with `--chunk 4` and run as two runs in the one
/// job, and so is a step's run from its door.
fn keyboard_round(pg: Option<(String, String)>) {
    let Lab {
        home,
        dir,
        work,
        files,
        bin,
        path,
        args_log,
        read_log,
        s1,
        ..
    } = lab(pg);
    let packs = packs();
    for step in [
        &["pseudonymize", "@ds", "--name", "kb"][..],
        &["digest", "@ds", "--name", "kb"],
        &["fingerprint"],
        &["classify", "--pack-dir", &packs],
        &["score", "@ds", "--name", "kb", "--pack-dir", &packs],
    ] {
        let mut args = vec!["jobs", "enqueue", "--"];
        args.extend_from_slice(step);
        home.ok(&args);
    }
    // each job records the keyboard's reach: every grant but the
    // certificate's, which only --unsealed-access gives, at sensitive
    let queued = job_doc(&home, jobs_of(&home, "score")[0]);
    let grants = queued["args"]["grants"].as_array().unwrap();
    assert!(grants.iter().any(|g| g == "pipelines:work"), "{queued}");
    assert!(!grants.iter().any(|g| g == "sealed:see"), "{queued}");
    assert_eq!(queued["args"]["detail"], "sensitive", "{queued}");
    let env: [(&str, &OsStr); 5] = [
        ("PATH", path.as_os_str()),
        ("NILS_PACK_DIR", OsStr::new(&packs)),
        ("FAKE_PODMAN_ARGS", args_log.as_os_str()),
        ("STANDIN_READS", read_log.as_os_str()),
        ("NILS_TEST_PART_STACKS", OsStr::new("4")),
    ];
    let (good, out, err) = home.run_with(&["jobs", "work", "--once"], None, &env);
    assert!(good, "{err}\n{out}");
    let score = job_doc(&home, jobs_of(&home, "score")[0]);
    assert_eq!(score["state"], "done", "{score}");
    let result = &score["result"];
    assert_eq!(result["new_stacks"], 6, "{score}");
    assert_eq!(result["skipped"], json!([]), "{score}");
    assert!(result.get("score_skipped").is_none(), "{score}");
    let queued_runs = result["queued"].as_array().unwrap();
    assert_eq!(queued_runs.len(), 1, "{score}");
    assert_eq!(queued_runs[0]["step"], "system1", "{score}");
    assert_eq!(queued_runs[0]["stacks"], 6, "{score}");
    let command: Vec<&str> = queued_runs[0]["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert!(command.windows(2).any(|w| w == ["--chunk", "4"]), "{score}");
    // the run, under the reach the score's job recorded, ran in two parts,
    // each a run of its own in the one job, and gave every scan its opinion
    let job = queued_runs[0]["job"].as_i64().unwrap();
    let run = job_doc(&home, job);
    assert_eq!(run["state"], "done", "{run}");
    assert_eq!(run["args"]["arrival"]["batch"], "kb", "{run}");
    let parts = runs_of(&home, job);
    assert_eq!(
        parts.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
        [4, 2],
        "{parts:?}"
    );
    assert_eq!(opinions_of(&home, s1), 6);

    // a step's run from its door, over the dataset's six scans, in parts too
    let server = Served::start(
        &home,
        &path,
        true,
        &[
            ("FAKE_PODMAN_ARGS", args_log.as_os_str()),
            ("STANDIN_READS", read_log.as_os_str()),
            ("NILS_TEST_PART_STACKS", OsStr::new("4")),
        ],
    );
    let (status, started) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 202, "{started}");
    let command: Vec<&str> = started["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert!(
        command.windows(2).any(|w| w == ["--chunk", "4"]),
        "{started}"
    );
    let job = started["job"].as_i64().unwrap();
    let done = server.over(job);
    assert_eq!(done["state"], "done", "{done}");
    let parts = runs_of(&home, job);
    assert_eq!(
        parts.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
        [4, 2],
        "{parts:?}"
    );
    // every file was read once, over the two parts
    assert_eq!(reads(&read_log), 12);
    drop(server);
    drop(files);
    drop(bin);
    drop(work);
    drop(dir);
}

#[test]
fn a_bring_in_queued_at_the_keyboard_is_scored_by_system_one() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    keyboard_round(None);
}

#[test]
fn a_bring_in_queued_at_the_keyboard_is_scored_on_postgres_too() {
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
    let schema = "nils_arrival_keyboard";
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    keyboard_round(Some((dsn.clone(), schema.to_string())));
    drop();
}

#[test]
fn a_dataset_brought_in_is_scored_and_a_rescore_reads_the_slice_cache() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    round(None);
}

#[test]
fn a_dataset_brought_in_is_scored_on_postgres_too() {
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
    let schema = "nils_arrival";
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

/// The image's command, `nils-system1`, as a stand-in on the host: like a
/// bundle that brings its own code, it refuses unless the run says it
/// trusts the code (`--trust-code true`), and it says base T1w for every
/// stack. It keeps its words in `S1_ARGS`.
const NILS_SYSTEM1: &str = r#"#!/usr/bin/env python3
import json, os, sys
a = sys.argv[1:]
with open(os.environ["S1_ARGS"], "a") as log:
    log.write(json.dumps(a) + "\n")
assert a[0] == "score", a
o = dict(zip(a[1::2], a[2::2]))
if o.get("--trust-code") != "true":
    print("the bundle's predictor is code the site's model brings; the run must say it trusts it", file=sys.stderr)
    sys.exit(2)
m = json.load(open(o["--stacks"])); out = o["--output"]
man = json.load(open(os.path.join(o["--inputs"], "manifest.json")))
ids = [x["model_id"] for x in man["models"] if x["input"] == "model"]
assert len(ids) == 1, man
mounts = json.loads(os.environ["FAKE_MOUNTS"])
artifact = man["models"][0]["artifact"]
assert artifact and os.path.isfile(mounts["/inputs/model"] + "/" + os.path.basename(artifact)), man
units, opinions = [], []
for s in m["stacks"]:
    d = os.path.join(out, "system1"); os.makedirs(d, exist_ok=True)
    json.dump({"value": "T1w"}, open(os.path.join(d, "%d.json" % s["stack_id"]), "w"))
    units.append({"unit_id": s["unit"], "status": "succeeded", "derivatives": ["system1/%d.json" % s["stack_id"]]})
    opinions.append({"stack_id": s["stack_id"], "axis": "base", "value": "T1w", "p": 0.9, "reliability": 0.9,
                     "model_id": ids[0], "rules_read": {"value": "none"}})
json.dump({"schema_version": "1", "units": units, "opinions": opinions}, open(os.path.join(out, "results.json"), "w"))
"#;

/// The files under a folder whose bytes are `bytes`.
fn files_holding(dir: &Path, bytes: &[u8]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else if std::fs::read(&p).is_ok_and(|b| b == bytes) {
                out.push(p);
            }
        }
    }
    out
}

/// Wave 7b §18, System 1 at arrival on a fresh install: the System 1
/// pipeline the engine ships, as it is, and an admitted model whose bundle
/// brings code. The bring-in's score queues System 1, the run is given
/// `--trust-code true` by the model's admission with no hand-registered
/// version, and every scan gets its opinion. A model file that is no longer
/// the bytes admitted is refused before its code runs.
#[test]
fn the_shipped_system_one_scores_at_arrival_on_a_fresh_install() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    if std::thread::available_parallelism().map_or(1, |n| n.get()) < 4 {
        eprintln!("System 1's pipeline asks 4 cores of the lane; this machine has fewer");
        return;
    }
    let shipped = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../pipelines/nils-system1/system1-score/nils.job.yml"),
    )
    .unwrap();
    let Lab {
        home,
        dir,
        work,
        files,
        bin,
        path,
        args_log,
        s1,
        ..
    } = lab_with(None, &shipped);
    // the model kept where a run finds it, as a site keeps its own
    home.ok(&[
        "model",
        "keep",
        &s1.to_string(),
        "--artifact",
        files.path().join("s1.bin").to_str().unwrap(),
    ]);
    let stand_in = bin.file("nils-system1", NILS_SYSTEM1.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let s1_args = bin.path().join("s1-args.log");
    let packs = packs();
    for step in [
        &["pseudonymize", "@ds", "--name", "fresh"][..],
        &["digest", "@ds", "--name", "fresh"],
        &["fingerprint"],
        &["classify", "--pack-dir", &packs],
        &["score", "@ds", "--name", "fresh", "--pack-dir", &packs],
    ] {
        let mut args = vec!["jobs", "enqueue", "--"];
        args.extend_from_slice(step);
        home.ok(&args);
    }
    let env: [(&str, &OsStr); 4] = [
        ("PATH", path.as_os_str()),
        ("NILS_PACK_DIR", OsStr::new(&packs)),
        ("FAKE_PODMAN_ARGS", args_log.as_os_str()),
        ("S1_ARGS", s1_args.as_os_str()),
    ];
    let (good, out, err) = home.run_with(&["jobs", "work", "--once"], None, &env);
    assert!(good, "{err}\n{out}");
    let score = job_doc(&home, jobs_of(&home, "score")[0]);
    assert_eq!(score["state"], "done", "{score}");
    let queued = score["result"]["queued"].as_array().unwrap();
    assert_eq!(queued.len(), 1, "{score}");
    assert_eq!(queued[0]["step"], "system1", "{score}");
    // no hand step: the command names no trust, the run is given it
    assert!(
        !queued[0]["command"].to_string().contains("trust_code"),
        "{score}"
    );
    let run = job_doc(&home, queued[0]["job"].as_i64().unwrap());
    assert_eq!(run["state"], "done", "{run}");
    assert_eq!(opinions_of(&home, s1), 6);
    let words = std::fs::read_to_string(&s1_args).unwrap();
    assert!(words.contains(r#""--trust-code", "true""#), "{words}");

    // the model's file changed after its admission: its code is not run
    let kept = files_holding(work.path(), b"a system one");
    assert_eq!(kept.len(), 1, "{kept:?}");
    std::fs::write(&kept[0], b"another system").unwrap();
    std::fs::remove_file(&s1_args).unwrap();
    let server = Served::start(
        &home,
        &path,
        true,
        &[
            ("FAKE_PODMAN_ARGS", args_log.as_os_str()),
            ("S1_ARGS", s1_args.as_os_str()),
        ],
    );
    let (status, started) = server.run("datasets/ds", "system1", OPS);
    assert_eq!(status, 202, "{started}");
    let done = server.over(started["job"].as_i64().unwrap());
    assert_eq!(done["state"], "failed", "{done}");
    assert!(done.to_string().contains("not the digest"), "{done}");
    assert!(!s1_args.exists(), "the model's code ran");
    drop(server);
    drop(files);
    drop(bin);
    drop(work);
    drop(dir);
}

/// The stacks and values of the opinions of a job's runs (`of` true) or
/// of every other job's, in stack order.
fn opinions_by_job(home: &Home, job: i64, of: bool) -> Vec<(i64, String, String)> {
    let mut store = home.store();
    let op = if of { "=" } else { "<>" };
    let sql = format!(
        "SELECT o.stack_id, o.axis, COALESCE(o.value, '') FROM {} o JOIN {} r ON r.id = o.run_id \
         WHERE r.job_id {op} {job} ORDER BY o.stack_id, o.axis",
        store.qualified("opinion"),
        store.qualified("pipeline_run")
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.int(0).unwrap(),
                r.text(1).unwrap().to_string(),
                r.text(2).unwrap().to_string(),
            )
        })
        .collect()
}

/// The most containers that ran at once, by the start and end each wrote.
fn most_at_once(log: &Path) -> usize {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut events: Vec<(f64, i32)> = text
        .lines()
        .filter_map(|l| {
            let (t, what) = l.split_once(' ')?;
            Some((t.parse().ok()?, if what == "start" { 1 } else { -1 }))
        })
        .collect();
    // an end before a start at the same instant
    events.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (mut now, mut most) = (0i32, 0i32);
    for (_, d) in events {
        now += d;
        most = most.max(now);
    }
    most as usize
}

/// Wave 7b §18: System 1's parts run several at once within the lane's
/// budget. Six parts of one stack, each a container that asks 2 cores of a
/// lane of 4: two run at once, never more; the runs are made in part
/// order; and the opinions are those of the same parts run one after
/// another (`--parts-at-once 1`).
#[test]
fn system_ones_parts_run_several_at_once_within_the_lanes_budget() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    if std::thread::available_parallelism().map_or(1, |n| n.get()) < 4 {
        eprintln!("the lane of this test asks 4 cores; this machine has fewer");
        return;
    }
    let bounded = system1_stand_in()
        .replace(
            "  import json, os, sys\n",
            "  import json, os, sys, time\n  open(os.environ[\"S1_TIMES\"], \"a\").write(\"%f start\\n\" % time.time()); time.sleep(1.5)\n",
        )
        .replace(
            "open(os.path.join(out, \"results.json\"), \"w\"))\n",
            "open(os.path.join(out, \"results.json\"), \"w\"))\n  open(os.environ[\"S1_TIMES\"], \"a\").write(\"%f end\\n\" % time.time())\n",
        )
        .replace("needs: {gpu: none}", "needs: {gpu: none, cores: 2, memory-gb: 1}");
    assert!(bounded.contains("S1_TIMES") && bounded.contains("cores: 2"));
    let Lab {
        home,
        dir,
        work,
        files,
        bin,
        path,
        args_log,
        s1,
        ..
    } = lab_with(None, &bounded);
    home.ok(&["pipeline", "lane", "--cores", "4", "--memory-gb", "100"]);
    let times = bin.path().join("times.log");
    let packs = packs();
    for step in [
        &["pseudonymize", "@ds", "--name", "at-once"][..],
        &["digest", "@ds", "--name", "at-once"],
        &["fingerprint"],
        &["classify", "--pack-dir", &packs],
        &["score", "@ds", "--name", "at-once", "--pack-dir", &packs],
    ] {
        let mut args = vec!["jobs", "enqueue", "--"];
        args.extend_from_slice(step);
        home.ok(&args);
    }
    let env: [(&str, &OsStr); 5] = [
        ("PATH", path.as_os_str()),
        ("NILS_PACK_DIR", OsStr::new(&packs)),
        ("FAKE_PODMAN_ARGS", args_log.as_os_str()),
        ("S1_TIMES", times.as_os_str()),
        ("NILS_TEST_PART_STACKS", OsStr::new("1")),
    ];
    let (good, out, err) = home.run_with(&["jobs", "work", "--once"], None, &env);
    assert!(good, "{err}\n{out}");
    let score = job_doc(&home, jobs_of(&home, "score")[0]);
    let job = score["result"]["queued"][0]["job"].as_i64().unwrap();
    let run = job_doc(&home, job);
    assert_eq!(run["state"], "done", "{run}");
    let parts = runs_of(&home, job);
    assert_eq!(parts.len(), 6, "{parts:?}");
    assert!(parts.iter().all(|(_, n)| *n == 1), "{parts:?}");
    assert_eq!(opinions_of(&home, s1), 6);
    let at_once = most_at_once(&times);
    assert_eq!(at_once, 2, "{}", std::fs::read_to_string(&times).unwrap());
    // the runs are made in part order: their handles' stacks ascend
    let handle_stack = |run: i64| -> i64 {
        let mut store = home.store();
        let sql = format!(
            "SELECT MIN(m.key) FROM {} m JOIN {} r ON r.handle_id = m.handle_id WHERE r.id = {run}",
            store.qualified("handle_member"),
            store.qualified("pipeline_run")
        );
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    };
    let firsts: Vec<i64> = parts.iter().map(|(r, _)| handle_stack(*r)).collect();
    let mut sorted = firsts.clone();
    sorted.sort();
    assert_eq!(firsts, sorted, "{parts:?}");
    let at_once_opinions = opinions_by_job(&home, job, true);
    assert_eq!(at_once_opinions.len(), 6);

    // the same parts one after another give the same opinions
    let handle = score["result"]["queued"][0]["command"]
        .as_array()
        .unwrap()
        .windows(2)
        .find(|w| w[0] == "--handle")
        .and_then(|w| w[1].as_str().map(str::to_string))
        .expect("the score's run names its handle");
    std::fs::remove_file(&times).unwrap();
    let (good, out, err) = home.run_with(
        &[
            "run",
            "system1-score@1",
            "--model",
            &s1.to_string(),
            "--handle",
            &handle,
            "--chunk",
            "1",
            "--parts-at-once",
            "1",
        ],
        None,
        &env,
    );
    assert!(good, "{err}\n{out}");
    assert_eq!(most_at_once(&times), 1);
    assert_eq!(opinions_by_job(&home, job, false), at_once_opinions);

    // at the keyboard the parts that end together print whole blocks: each
    // run's words in one piece, never a line among another's
    std::fs::remove_file(&times).unwrap();
    let (good, out, err) = home.run_with(
        &[
            "run",
            "system1-score@1",
            "--model",
            &s1.to_string(),
            "--handle",
            &handle,
            "--chunk",
            "1",
            "--parts-at-once",
            "2",
        ],
        None,
        &env,
    );
    assert!(good, "{err}\n{out}");
    assert_eq!(most_at_once(&times), 2);
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in out.lines() {
        if line.starts_with("run ") {
            blocks.push(Vec::new());
        }
        blocks
            .last_mut()
            .expect("the output begins with a run's line")
            .push(line);
    }
    assert_eq!(blocks.len(), 6, "{out}");
    let shape = |b: &Vec<&str>| -> Vec<String> {
        b.iter()
            .map(|l| l.split_whitespace().next().unwrap_or("").to_string())
            .collect()
    };
    let first = shape(&blocks[0]);
    assert!(first.len() >= 6, "{out}");
    for b in &blocks {
        assert_eq!(shape(b), first, "a run's words were broken up:\n{out}");
    }
    drop(files);
    drop(bin);
    drop(work);
    drop(dir);
}
