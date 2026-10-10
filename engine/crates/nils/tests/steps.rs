// SPDX-License-Identifier: AGPL-3.0-only
//! A dataset's and a cohort's body-part and post-contrast steps run from
//! their doors (2026-10-10, record 56 section 2): `POST
//! /api/datasets/{name}/steps/{step}/run` and `POST
//! /api/cohorts/{name}/steps/{step}/run` freeze the scans the step counts
//! into a handle and queue the step's pipeline over them, a `pipeline` job
//! for the dataset or the cohort that the pipeline lane runs. The step on
//! the summary and on the cohort's document goes queued, running, then done
//! or failed, and says why a run of it would be refused now: no pipeline or
//! no model for it, no container runtime, a run of it already queued, and
//! post-contrast always, since no model answers it yet.
//!
//! No container runtime is needed and no real image is run: a stand-in for
//! podman on the server's search path runs the stand-in pipeline's command
//! on the host, under the name and with the model inputs of the certified
//! body-part model's pipeline, and proposes a body part and a body region
//! for every stack. On SQLite always, and on Postgres where a test DSN is
//! set.

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

/// A stand-in for the certified body-part model's pipeline: its name, its
/// three model inputs in its order and its two axes, and a command that
/// writes an answer per stack and proposes `brain` and `head` for each, by
/// the head and the coarse mode the run's model manifest names.
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
  import json, os, sys
  m = json.load(open(sys.argv[1])); out = sys.argv[2]
  man = json.load(open(sys.argv[3] + "/manifest.json"))
  ids = {{x["input"]: x["model_id"] for x in man["models"]}}
  assert list(ids) == ["encoder", "head", "coarse"], ids
  units, proposals = [], []
  for s in m["stacks"]:
      u = s["unit"]; d = os.path.join(out, u); os.makedirs(d, exist_ok=True)
      open(os.path.join(d, "answer.json"), "w").write(json.dumps({{"stack": s["stack_id"]}}))
      units.append({{"unit_id": u, "status": "succeeded", "derivatives": [u + "/answer.json"]}})
      proposals.append({{"stack_id": s["stack_id"], "axis": "body_part", "value": "brain", "probabilities": {{"brain": 0.9, "spine": 0.1}}, "model_id": ids["head"]}})
      proposals.append({{"stack_id": s["stack_id"], "axis": "body_region", "value": "head", "probabilities": {{"head": 0.9, "spine": 0.1}}, "model_id": ids["coarse"]}})
  json.dump({{"schema_version": "1", "units": units, "proposals": proposals}}, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation] [Inputs]
x-nils:
  analysis-level: stack
  input: {{layout: stacks}}
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

fn round(pg: Option<(String, String)>) {
    let home = Home {
        dir: TempDir::new("steps-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a steps test key\n"));
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
    // two subjects, three scans, a dataset that feeds a cohort
    let dir = TempDir::new("steps-ds");
    for (p, patient) in ["S-ONE", "S-TWO"].iter().enumerate() {
        let study = format!("1.2.826.0.1.3680043.8.497.{}.1", p + 1);
        for (n, description) in ["t1 mprage", "t2 flair"].iter().enumerate() {
            if p == 1 && n == 1 {
                continue;
            }
            let series = format!("{study}.{}", n + 1);
            for instance in 1..=2u32 {
                let sop = format!("{series}.{instance}");
                let mut e = synth::minimal_mr(&study, &series, &sop);
                e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
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
        "--cohort",
        "fed",
    ]);
    // a dataset nothing has read
    let empty = TempDir::new("steps-empty");
    std::fs::create_dir_all(empty.path().join("derivatives/dcm-anon")).unwrap();
    home.ok(&[
        "place",
        "add",
        "empty",
        empty.path().to_str().unwrap(),
        "--role",
        "source",
        "--patient-id",
        "id-type:patient-id",
        "--subjects",
        "generated",
    ]);
    // the working place a run's outputs go to
    let work = TempDir::new("steps-work");
    home.ok(&[
        "place",
        "add",
        "work",
        work.path().to_str().unwrap(),
        "--role",
        "working",
    ]);
    let bin = TempDir::new("steps-bin");
    let path = fake_path(&bin);
    let args_log = bin.path().join("args.log");
    let logged: [(&str, &OsStr); 1] = [("FAKE_PODMAN_ARGS", args_log.as_os_str())];

    // the dataset read through the worker
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

    // nothing runs either step yet, and each says why
    let s = server.get("/api/datasets/ds/summary", READS);
    assert_eq!(s["scans"], 3, "{s}");
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "off", "{s}");
    assert_eq!(bp["refusal"]["reason"], "no_pipeline", "{s}");
    let pc = step(&s, "post_contrast");
    assert_eq!(pc["state"], "off", "{s}");
    assert_eq!(pc["refusal"]["reason"], "no_model", "{s}");
    assert_eq!(
        pc["refusal"]["error"], "no post-contrast model is installed",
        "{s}"
    );
    // the door says the same
    let (status, doc) = server.run("datasets/ds", "post_contrast", OPS);
    assert_eq!(status, 409, "{doc}");
    assert_eq!(doc["error"], "no post-contrast model is installed", "{doc}");
    assert_eq!(doc["reason"], "no_model", "{doc}");
    assert_eq!(doc["step"], "post_contrast", "{doc}");
    assert_eq!(doc["disclosure"], "safe", "{doc}");
    let (status, doc) = server.run("cohorts/fed", "post_contrast", OPS);
    assert_eq!(status, 409, "{doc}");
    assert_eq!(doc["reason"], "no_model", "{doc}");
    let (status, doc) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 409, "{doc}");
    assert_eq!(doc["reason"], "no_pipeline", "{doc}");
    assert_eq!(doc["error"], "no body-part pipeline is installed", "{doc}");
    // a step no door runs, a dataset or a cohort that is not there
    let (status, doc) = server.run("datasets/ds", "sorted", OPS);
    assert_eq!(status, 404, "{doc}");
    assert_eq!(doc["reason"], "no_step", "{doc}");
    let (status, _) = server.run("datasets/nowhere", "body_part", OPS);
    assert_eq!(status, 404);
    let (status, _) = server.run("cohorts/nobody", "body_part", OPS);
    assert_eq!(status, 404);
    // a run reads pixels: Pipelines work at detail quasi
    let (status, doc) = server.run("datasets/ds", "body_part", PLAIN);
    assert_eq!(status, 403, "{doc}");
    let (status, _) = server.run("datasets/ds", "body_part", READS);
    assert_eq!(status, 403);
    // the doors are listed, with their grant and detail
    let caps = server.get("/api/capabilities", READS);
    for door in [
        "POST /api/datasets/{name}/steps/{step}/run",
        "POST /api/cohorts/{name}/steps/{step}/run",
    ] {
        assert!(
            caps["doors"].as_array().unwrap().iter().any(|d| d == door),
            "{door}: {caps}"
        );
        let row = caps["policy"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["door"] == door)
            .unwrap_or_else(|| panic!("no policy row for {door}"));
        assert_eq!(row["grant"], "pipelines:work", "{row}");
        assert_eq!(row["detail"], "quasi", "{row}");
        assert_eq!(row["cost"], "job", "{row}");
    }
    drop(server);

    // the body-part pipeline in the catalog, and no model for it yet
    let descriptor = work.file("bodypart-infer-fusion.yml", stand_in().as_bytes());
    let added = home.json(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
    std::fs::remove_file(&descriptor).unwrap();
    let pipeline = added["label"].as_str().unwrap().to_string();
    assert_eq!(pipeline, "bodypart-infer-fusion@1", "{added}");
    let server = Served::start(&home, &path, false, &logged);
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["refusal"]["reason"], "no_model", "{s}");
    assert_eq!(
        bp["refusal"]["error"], "no body-part model is installed",
        "{s}"
    );
    drop(server);

    // the certified model's three parts, the head and the coarse mode
    // admitted: the coarse mode whose card names the head is taken over a
    // newer one that does not, and one of another encoder never
    let files = TempDir::new("steps-models");
    let (encoder, encoder_digest) = register(
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
    let (unnamed_coarse, _) = register(
        &home,
        &files,
        json!({"name": "bp-coarse-unnamed", "version": "t1", "kind": "head", "task": "axis:body_region",
               "encoder": {"digest": encoder_digest}, "threshold": 0.5}),
        b"a coarse mode that names no head",
    );
    let (_, other_digest) = register(
        &home,
        &files,
        json!({"name": "other-enc", "version": "t1", "kind": "encoder", "task": "features:other"}),
        b"another encoder",
    );
    let (other_coarse, _) = register(
        &home,
        &files,
        json!({"name": "other-coarse", "version": "t1", "kind": "head", "task": "axis:body_region",
               "encoder": {"digest": other_digest}, "threshold": 0.5}),
        b"a coarse mode of another encoder",
    );
    let check = files.file(
        "check.json",
        json!({"suite": "steps", "passed": true, "checks": [{"name": "ece", "passed": true}]})
            .to_string()
            .as_bytes(),
    );
    for m in [head, coarse, unnamed_coarse, other_coarse] {
        home.ok(&[
            "model",
            "admit",
            &m.to_string(),
            "--check",
            check.to_str().unwrap(),
        ]);
    }

    // a server with no worker: the run waits in the queue
    let server = Served::start(&home, &path, false, &logged);
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "waiting", "{s}");
    assert_eq!(bp["served"], true, "{s}");
    assert!(bp["refusal"].is_null(), "{s}");
    // a dataset with no scans: served, and nothing to run it over
    let e = server.get("/api/datasets/empty/summary", READS);
    assert_eq!(
        step(&e, "body_part")["refusal"]["reason"],
        "no_scans",
        "{e}"
    );
    let (status, doc) = server.run("datasets/empty", "body_part", OPS);
    assert_eq!(status, 409, "{doc}");
    assert_eq!(doc["reason"], "no_scans", "{doc}");

    let (status, started) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 202, "{started}");
    let job = started["job"].as_i64().unwrap();
    assert_eq!(started["state"], "queued", "{started}");
    assert_eq!(started["step"], "body_part", "{started}");
    assert_eq!(started["for"], "dataset:ds", "{started}");
    assert_eq!(started["scans"], 3, "{started}");
    // the pipeline, the frozen scans, and the parts in the descriptor's order
    let words: Vec<String> = started["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect();
    let handle = started["handle"].as_i64().unwrap().to_string();
    assert_eq!(
        words[..4],
        ["run", pipeline.as_str(), "--handle", handle.as_str()],
        "{started}"
    );
    let models: Vec<String> = words
        .windows(2)
        .filter(|w| w[0] == "--model")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(
        models,
        [encoder, head, coarse].map(|m| m.to_string()),
        "the coarse mode that names the head, never one of another encoder: {started}"
    );
    // the handle holds the dataset's three scans
    {
        let mut store = home.store();
        let member = store.qualified("handle_member");
        let held = store
            .query(
                &format!("SELECT COUNT(*) FROM {member} WHERE handle_id = {handle}"),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(held, 3);
    }
    // the job: a pipeline run, queued for the dataset under its asker
    let queued = server.get(&format!("/api/jobs/{job}"), OPS);
    assert_eq!(queued["kind"], "pipeline", "{queued}");
    assert_eq!(queued["state"], "queued", "{queued}");
    assert_eq!(queued["args"]["step"], "body_part", "{queued}");
    assert_eq!(queued["args"]["for"], "dataset:ds", "{queued}");
    assert_eq!(queued["args"]["principal"], "ops@lab", "{queued}");
    // the step waits its turn, and a second run of it is refused meanwhile
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "queued", "{s}");
    assert_eq!(bp["job"], job, "{s}");
    assert_eq!(bp["jobs"], json!([job]), "{s}");
    assert!(bp["run"].is_null(), "{s}");
    assert_eq!(bp["refusal"]["reason"], "running", "{s}");
    let (status, doc) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 409, "{doc}");
    assert_eq!(doc["reason"], "running", "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap()
            .contains(&format!("job {job}")),
        "{doc}"
    );
    // it is in the dataset's log, and in no other's
    let log = server.get("/api/jobs?dataset=ds", OPS);
    assert!(
        log["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["id"] == job),
        "{log}"
    );
    let none = server.get("/api/jobs?dataset=empty&all=1", OPS);
    assert_eq!(none["count"], 0, "{none}");
    // dropped from the queue, the step waits again
    let (status, dropped) = server.call("POST", &format!("/api/jobs/{job}/cancel"), None, OPS);
    assert_eq!(status, 200, "{dropped}");
    let s = server.get("/api/datasets/ds/summary", READS);
    assert_eq!(step(&s, "body_part")["state"], "waiting", "{s}");
    assert!(step(&s, "body_part")["refusal"].is_null(), "{s}");
    let (status, again) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 202, "{again}");
    let job = again["job"].as_i64().unwrap();
    drop(server);

    // the pipeline lane runs it over the stand-in podman
    let server = Served::start(&home, &path, true, &logged);
    let ran = server.over(job);
    assert_eq!(ran["state"], "done", "{ran}");
    assert_eq!(ran["kind"], "pipeline", "{ran}");
    assert_eq!(ran["args"]["step"], "body_part", "{ran}");
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "done", "{s}");
    assert_eq!(bp["job"], job, "{s}");
    assert!(bp["run"].as_i64().is_some(), "{s}");
    assert_eq!(
        bp["answered"], 3,
        "every scan answered at the threshold: {s}"
    );
    assert!(bp["jobs"].as_array().unwrap().contains(&json!(job)), "{s}");
    assert!(bp["progress"].is_null(), "{s}");
    assert!(bp["finished_at"].is_string(), "{s}");
    assert!(bp["refusal"].is_null(), "it may run again: {s}");
    let runs = std::fs::read_to_string(&args_log).unwrap();
    assert_eq!(runs.lines().count(), 1, "{runs}");
    assert!(runs.contains("\"--network\", \"none\""), "{runs}");
    let log = server.get("/api/jobs?dataset=ds&all=1", OPS);
    assert!(
        log["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["id"] == job),
        "{log}"
    );

    // a cohort's step: its members' scans, run the same way
    let fed = server.get("/api/cohorts/fed", READS);
    let bp = step(&fed, "body_part");
    assert_eq!(bp["state"], "done", "a run over its scans ran: {fed}");
    assert!(bp["refusal"].is_null(), "{fed}");
    assert_eq!(
        step(&fed, "post_contrast")["refusal"]["reason"],
        "no_model",
        "{fed}"
    );
    let (status, started) = server.run("cohorts/fed", "body_part", OPS);
    assert_eq!(status, 202, "{started}");
    assert_eq!(started["for"], "cohort:fed", "{started}");
    assert_eq!(started["scans"], 3, "{started}");
    let cohort_job = started["job"].as_i64().unwrap();
    let ran = server.over(cohort_job);
    assert_eq!(ran["state"], "done", "{ran}");
    let fed = server.get("/api/cohorts/fed", READS);
    let bp = step(&fed, "body_part");
    assert_eq!(bp["state"], "done", "{fed}");
    assert_eq!(bp["job"], cohort_job, "{fed}");
    assert!(
        bp["jobs"].as_array().unwrap().contains(&json!(cohort_job)),
        "{fed}"
    );
    drop(server);

    // a run whose container fails: the step says failed, and may run again
    let mut failing = logged.to_vec();
    failing.push(("FAKE_PODMAN_EXIT", OsStr::new("125")));
    let server = Served::start(&home, &path, true, &failing);
    let (status, started) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 202, "{started}");
    let failed_job = started["job"].as_i64().unwrap();
    let ran = server.over(failed_job);
    assert_eq!(ran["state"], "failed", "{ran}");
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "failed", "{s}");
    assert_eq!(bp["job"], failed_job, "{s}");
    assert!(bp["refusal"].is_null(), "{s}");
    assert_eq!(bp["answered"], 3, "what ran before stays: {s}");
    drop(server);

    // record 55 E2: a cascade served, one model naming its parts: the
    // step gives each input its part, the cascade's own coarse mode, and
    // the cascade itself, in the descriptor's order
    let cascade_descriptor = work.file(
        "bodypart-infer-fusion-2.yml",
        stand_in()
            .replace(
                "    - {id: coarse, type: model, optional: true}\n",
                "    - {id: coarse, type: model, optional: true}\n    - {id: student, type: model, optional: true}\n    - {id: deferral, type: model, optional: true}\n    - {id: cascade, type: model, optional: true}\n",
            )
            .replace(
                r#"assert list(ids) == ["encoder", "head", "coarse"], ids"#,
                r#"assert list(ids)[:3] == ["encoder", "head", "coarse"], ids"#,
            )
            .as_bytes(),
    );
    let added = home.json(&[
        "pipeline",
        "add",
        cascade_descriptor.to_str().unwrap(),
        "--json",
    ]);
    std::fs::remove_file(&cascade_descriptor).unwrap();
    let pipeline2 = added["label"].as_str().unwrap().to_string();
    assert_eq!(pipeline2, "bodypart-infer-fusion@2", "{added}");
    let (student, student_digest) = register(
        &home,
        &files,
        json!({"name": "bp-student", "version": "t1", "kind": "encoder", "task": "features:bodypart_slice"}),
        b"a student's weights",
    );
    let (deferral, deferral_digest) = register(
        &home,
        &files,
        json!({"name": "bp-deferral", "version": "t1", "kind": "head", "task": "cascade:body_part",
               "encoder": {"digest": student_digest}}),
        b"a deferral model",
    );
    let (cascade, cascade_digest) = register(
        &home,
        &files,
        json!({"name": "bp-cascade", "version": "t1", "kind": "head", "task": "axis:body_part",
               "encoders": [{"digest": encoder_digest}, {"digest": student_digest}], "threshold": 0.5,
               "parts": {"encoder": encoder_digest, "head": head_digest, "student": student_digest,
                         "deferral": deferral_digest}}),
        b"a cascade",
    );
    let (cascade_coarse, _) = register(
        &home,
        &files,
        json!({"name": "bp-cascade-coarse", "version": "t1", "kind": "head", "task": "axis:body_region",
               "encoders": [{"digest": encoder_digest}, {"digest": student_digest}], "threshold": 0.5,
               "params": {"head": {"digest": cascade_digest}}}),
        b"the cascade's coarse mode",
    );
    for m in [cascade, cascade_coarse] {
        home.ok(&[
            "model",
            "admit",
            &m.to_string(),
            "--check",
            check.to_str().unwrap(),
        ]);
    }
    let server = Served::start(&home, &path, false, &logged);
    let (status, started) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 202, "{started}");
    let words: Vec<String> = started["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect();
    assert_eq!(words[1], pipeline2, "{started}");
    let models: Vec<String> = words
        .windows(2)
        .filter(|w| w[0] == "--model")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(
        models,
        [encoder, head, cascade_coarse, student, deferral, cascade].map(|m| m.to_string()),
        "a cascade's parts, its own coarse mode and itself: {started}"
    );
    let cascade_job = started["job"].as_i64().unwrap();
    let (status, dropped) = server.call(
        "POST",
        &format!("/api/jobs/{cascade_job}/cancel"),
        None,
        OPS,
    );
    assert_eq!(status, 200, "{dropped}");
    // the plain head keeps its run on the first descriptor's inputs: a
    // cascade's inputs are optional and left out without one
    home.ok(&[
        "model",
        "retire",
        &cascade.to_string(),
        "--why",
        "back to the certified path",
    ]);
    let (status, started) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 202, "{started}");
    let words: Vec<String> = started["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect();
    let models: Vec<String> = words
        .windows(2)
        .filter(|w| w[0] == "--model")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(
        models,
        [encoder, head, coarse].map(|m| m.to_string()),
        "without a cascade, the head's own parts: {started}"
    );
    let plain_job = started["job"].as_i64().unwrap();
    let (status, dropped) =
        server.call("POST", &format!("/api/jobs/{plain_job}/cancel"), None, OPS);
    assert_eq!(status, 200, "{dropped}");
    drop(server);

    // a machine with no container runtime: the door still answers, and says why
    let nothing = TempDir::new("steps-no-path");
    let server = Served::start(&home, nothing.path().as_os_str(), false, &logged);
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["refusal"]["reason"], "no_runtime", "{s}");
    assert!(
        bp["refusal"]["error"]
            .as_str()
            .unwrap()
            .contains("no container runtime"),
        "{s}"
    );
    let (status, doc) = server.run("datasets/ds", "body_part", OPS);
    assert_eq!(status, 409, "{doc}");
    assert_eq!(doc["reason"], "no_runtime", "{doc}");
    let (status, doc) = server.run("cohorts/fed", "post_contrast", OPS);
    assert_eq!(status, 409, "{doc}");
    assert_eq!(doc["reason"], "no_model", "{doc}");

    // a door's job that a worker took and that has written no run yet runs;
    // one that ended before its run began is the step's newest attempt
    let taken = {
        let mut store = home.store();
        let job = store.qualified("job");
        let now = nils_registry::time::now_iso();
        store
            .execute(
                &format!(
                    "INSERT INTO {job} (kind, name, args, state, started_at, heartbeat_at) VALUES \
                     ('pipeline', 'dataset:ds', '{{\"step\": \"body_part\", \"for\": \"dataset:ds\"}}', \
                     'running', '{now}', '{now}')"
                ),
                &[],
            )
            .unwrap();
        store
            .query(&format!("SELECT MAX(id) FROM {job}"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap()
    };
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "running", "{s}");
    assert_eq!(bp["job"], taken, "{s}");
    assert!(bp["run"].is_null(), "{s}");
    assert_eq!(bp["refusal"]["reason"], "running", "{s}");
    {
        let mut store = home.store();
        let job = store.qualified("job");
        let now = nils_registry::time::now_iso();
        store
            .execute(
                &format!(
                    "UPDATE {job} SET state = 'failed', finished_at = '{now}' WHERE id = {taken}"
                ),
                &[],
            )
            .unwrap();
    }
    let s = server.get("/api/datasets/ds/summary", READS);
    let bp = step(&s, "body_part");
    assert_eq!(bp["state"], "failed", "{s}");
    assert_eq!(bp["job"], taken, "{s}");
    assert!(bp["finished_at"].is_string(), "{s}");
    assert_eq!(bp["jobs"][0], taken, "{s}");
    drop(server);
    drop(files);
    drop(bin);
    drop(work);
    drop(empty);
    drop(dir);
}

#[test]
fn a_step_runs_from_its_door_through_queued_and_running_to_done_or_failed() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    round(None);
}

#[test]
fn a_step_runs_from_its_door_on_postgres_too() {
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
    let schema = "nils_steps";
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
