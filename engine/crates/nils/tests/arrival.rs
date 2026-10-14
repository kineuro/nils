// SPDX-License-Identifier: AGPL-3.0-only
//! Scoring at arrival and the slice cache (record 55 E5; spec section
//! 10.4; gate test T26): a dataset brought in gets its image answers
//! without a manual run, and a re-score after a model update reads the
//! slice cache instead of the files.
//!
//! A bring-in ends with `score`, which queues the body-part step's run over
//! the batch's new stacks with the chosen model, since its pipeline
//! declares `x-nils.on-arrival`; the run is given the working place's
//! `slice-cache/` and a `stacks.json` that names each file's SOP Instance
//! UID, and the sha256 of each file a run reported reading whole is kept.
//! After a new head is chosen, the step's run from its door finds every
//! file's sha256 in `stacks.json`, and the stand-in image reads its inputs
//! from the cache: the files it reads are counted in a log, and none is
//! read. A bring-in of nothing new queues no score, and a caller who may
//! not run a pipeline gets the score skipped, with the reason.
//!
//! No container runtime is needed and no real image is run: a stand-in for
//! podman runs the stand-in pipeline's command on the host. On SQLite
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

/// The body-part pipeline's stand-in, which scores at arrival and keeps
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

fn round(pg: Option<(String, String)>) {
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
    // the body-part pipeline, which scores at arrival, and its model
    let descriptor = work.file("bodypart-infer-fusion.yml", stand_in().as_bytes());
    let added = home.json(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
    std::fs::remove_file(&descriptor).unwrap();
    assert_eq!(added["label"], "bodypart-infer-fusion@1", "{added}");
    let files = TempDir::new("arrival-models");
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
    for m in [head, coarse] {
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
    let queued_runs = result["queued"].as_array().unwrap();
    assert_eq!(queued_runs.len(), 1, "{score}");
    assert_eq!(queued_runs[0]["step"], "body_part", "{score}");
    assert_eq!(queued_runs[0]["stacks"], 6, "{score}");
    // post-contrast is not installed: skipped, and why
    let skipped = result["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "{score}");
    assert_eq!(skipped[0]["step"], "post_contrast", "{score}");
    assert_eq!(skipped[0]["reason"], "no_pipeline", "{score}");
    // the run with the chosen model, for the dataset, under the chain's
    // principal, and it ran
    let run = server.over(queued_runs[0]["job"].as_i64().unwrap());
    assert_eq!(run["state"], "done", "{run}");
    assert_eq!(run["args"]["step"], "body_part", "{run}");
    assert_eq!(run["args"]["for"], "dataset:ds", "{run}");
    assert_eq!(run["args"]["arrival"]["batch"], "first", "{run}");
    let argv = run["args"]["queued"].to_string();
    assert!(argv.contains(&format!("\"--model\",\"{head}\"")), "{run}");
    // every scan has its image answer, and every file was read once
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "done", "{s}");
    assert_eq!(bp["answered"], 6, "{s}");
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
    let s = server.get("/api/datasets/ds/summary", READS);
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
    assert_eq!(jobs_of(&home, "pipeline").len(), 2);

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
    assert_eq!(jobs_of(&home, "pipeline").len(), 2, "no run was queued");
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
