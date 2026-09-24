// SPDX-License-Identifier: AGPL-3.0-only

//! Record 43 S1 to S3, pipelines make derivatives, through the binary.
//!
//! No container runtime is needed: a stand-in for podman on the search
//! path answers the runner's questions and runs a pipeline's command on the
//! host with every container path written as its host folder, and it keeps
//! the words it was given, so the flags every run carries are checked. The
//! whole flow runs on it: the catalog and its refusals, the selection frozen
//! and pinned, both input layouts (the bids one where dcm2niix is
//! installed), results.json and its absence, derivatives with the engine's
//! own digests, `pipeline:qc` review items, the same digests on a re-run,
//! the queue through the door at detail quasi, and the two transports of
//! the derivative door. A machine with no runtime reports the capability
//! off, with no error.
//!
//! One test runs real containers: the N4 descriptor of this repository on
//! a synthetic release, and a probe of the uid a container's process has on
//! the host. It runs where rootless podman is installed (the CI's runners),
//! or with `NILS_TEST_PIPELINE_RUNTIME=docker` on a machine an operator lets
//! run docker, and says so and passes elsewhere.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn packs() -> PathBuf {
    repo().join("packs")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// A stand-in for podman: it answers `--version` and `info` as a rootless
/// podman does, keeps the words of a `run`, and runs the command after the
/// image on the host, every container path in it written as the host
/// folder mounted there, in one pass.
const FAKE_PODMAN: &str = r#"#!/usr/bin/env python3
import json, os, re, subprocess, sys
a = sys.argv[1:]
if not a or a[0] == "--version":
    print("podman version 9.9.9-test"); sys.exit(0)
if a[0] == "info":
    print("true"); sys.exit(0)
if a[0] != "run":
    sys.exit(0)
with open(os.environ["FAKE_PODMAN_ARGS"], "a") as log:
    log.write(json.dumps(a) + "\n")
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
sys.exit(subprocess.call(argv, env=dict(os.environ, **env)))
"#;

/// A test pipeline of the stacks layout: for each stack it writes one file
/// and says so in results.json, as `mode` asks.
fn stack_echo(image: &str) -> String {
    format!(
        r#"# SPDX-License-Identifier: AGPL-3.0-only
name: stack-echo
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "{image}"
inputs:
  - id: mode
    name: What to do
    type: String
    value-key: "[MODE]"
    default-value: all
    value-choices: [all, fail-last, partial, silent, crash]
command-line: |
  python3 -c '
  import json, os, sys
  m = json.load(open(sys.argv[1])); out = sys.argv[2]; mode = sys.argv[4]
  man = json.load(open(sys.argv[3] + "/manifest.json"))
  assert man["contract"] == "job/v1" and len(man["units"]) == len(m["stacks"])
  units = []
  st = m["stacks"]
  for i, s in enumerate(st):
      u = s["unit"]; d = os.path.join(out, u); os.makedirs(d, exist_ok=True)
      if mode == "fail-last" and i == len(st) - 1:
          units.append({{"unit_id": u, "status": "failed", "error": "refused on purpose", "metrics": {{"files": len(s["files"])}}}})
          continue
      open(os.path.join(d, "out.txt"), "w").write("stack %d holds %d files\n" % (s["stack_id"], len(s["files"])))
      units.append({{"unit_id": u, "status": "succeeded", "derivatives": [u + "/out.txt"]}})
  if mode == "crash":
      sys.exit(3)
  if mode == "partial":
      units = units[:1]
  if mode != "silent":
      proposals = [{{"axis": "body_part", "value": "brain", "stack_id": st[0]["stack_id"]}}, {{"axis": "elsewhere", "value": "x"}}]
      json.dump({{"schema_version": "1", "units": units, "proposals": proposals}}, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation] [Inputs] [MODE]
x-nils:
  analysis-level: stack
  input: {{layout: stacks}}
  outputs:
    - id: echo
      kind: output
      path-template: "stack-{{stack}}/out.txt"
      media-type: text/plain
  needs: {{gpu: optional}}
  proposals: [{{axis: body_part}}]
"#
    )
}

/// A test pipeline of the bids layout: it copies each session's T1w where
/// N4 would write its output, as N4's descriptor finds it, and writes no
/// results.json.
const BIDS_COPY: &str = r#"name: bids-copy
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/bids-copy@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
command-line: |
  python3 -c '
  import glob, os, shutil, sys
  src, out = sys.argv[1], sys.argv[2]
  for f in sorted(glob.glob(src + "/sub-*/ses-*/anat/*_T1w.nii.gz")):
      rel = os.path.relpath(f, src)
      d = os.path.join(out, os.path.dirname(rel)); os.makedirs(d, exist_ok=True)
      shutil.copy(f, os.path.join(d, os.path.basename(rel).replace("_T1w", "_desc-n4_T1w")))
  ' [InputDataset] [OutputLocation] [ParticipantLabels]
x-nils:
  analysis-level: session
  input: {layout: bids}
  outputs:
    - id: n4_image
      kind: output
      path-template: "sub-{subject}/ses-{session}/anat/*_desc-n4_T1w.nii.gz"
"#;

/// Two people, one session each, a 3D T1 and a FLAIR with real pixels, so
/// a converter reads them and a pick chooses among them.
fn tree() -> TempDir {
    let dir = TempDir::new("pipelines-src");
    let people = [
        ("P1", "20220115", "1.2.826.0.1.3680043.8.498.71"),
        ("P2", "20230310", "1.2.826.0.1.3680043.8.498.72"),
    ];
    for (patient, day, root) in people {
        for (n, description, protocol) in [
            ("1", "t1_mprage_sag", "MPRAGE"),
            ("2", "t2_flair_sag", "FLAIR"),
        ] {
            let study = format!("{root}.1");
            let series = format!("{root}.1.{n}");
            for slice in 1..=12u32 {
                let sop = format!("{series}.{slice}");
                let mut e = synth::minimal_mr(&study, &series, &sop);
                let pixels: Vec<u8> = (0..32u32 * 32)
                    .flat_map(|i| {
                        let (x, y) = (i % 32, i / 32);
                        let v: u16 = 200
                            + (x * 7 + y * 3 + slice * 11) as u16
                            + if (8..24).contains(&x) && (8..24).contains(&y) {
                                600
                            } else {
                                0
                            };
                        v.to_le_bytes()
                    })
                    .collect();
                e.extend([
                    synth::text(tags::PATIENT_ID, VR::LO, patient),
                    synth::text(tags::STUDY_DATE, VR::DA, day),
                    synth::text(tags::SERIES_TIME, VR::TM, &format!("0{n}1415")),
                    synth::text(tags::SERIES_DESCRIPTION, VR::LO, description),
                    synth::text(tags::PROTOCOL_NAME, VR::LO, protocol),
                    synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
                    synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                    synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                    synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                    synth::us(tags::ROWS, 32),
                    synth::us(tags::COLUMNS, 32),
                    synth::us(tags::BITS_ALLOCATED, 16),
                    synth::us(tags::BITS_STORED, 12),
                    synth::us(tags::HIGH_BIT, 11),
                    synth::us(tags::PIXEL_REPRESENTATION, 0),
                    synth::us(tags::SAMPLES_PER_PIXEL, 1),
                    synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
                    synth::text(tags::PIXEL_SPACING, VR::DS, "1.0\\1.0"),
                    synth::text(tags::SLICE_THICKNESS, VR::DS, "1.0"),
                    synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "0\\1\\0\\0\\0\\-1"),
                    synth::text(
                        tags::IMAGE_POSITION_PATIENT,
                        VR::DS,
                        &format!("{slice}\\0\\0"),
                    ),
                    synth::text(tags::INSTANCE_NUMBER, VR::IS, &slice.to_string()),
                    synth::bytes(tags::PIXEL_DATA, VR::OW, pixels),
                ]);
                dir.file(
                    &format!("{patient}/{n}/{slice}"),
                    &synth::part10(&MetaFields::mr(&sop), &e, true),
                );
            }
        }
    }
    dir
}

/// A registry of those four stacks, fingerprinted, classified and picked,
/// a working place, the selection of every stack, and a search path whose
/// first folder holds the stand-in podman.
struct Lab {
    home: TempDir,
    work: TempDir,
    bin: TempDir,
    _src: TempDir,
    path: OsString,
}

impl Lab {
    fn new(name: &str) -> Lab {
        let lab = Lab {
            home: TempDir::new(&format!("{name}-home")),
            work: TempDir::new(&format!("{name}-work")),
            bin: TempDir::new(&format!("{name}-bin")),
            _src: tree(),
            path: OsString::new(),
        };
        let fake = lab.bin.file("podman", FAKE_PODMAN.as_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut path = lab.bin.path().as_os_str().to_owned();
        if let Some(p) = std::env::var_os("PATH") {
            path.push(":");
            path.push(p);
        }
        let lab = Lab { path, ..lab };
        lab.ok(&["key", "add", "k"], Some("a pipelines test key\n"));
        lab.ok(&["init", "--key", "k"], None);
        let src = lab._src.path().to_str().unwrap().to_string();
        lab.ok(&["digest", "--name", "a", "--no-private", &src], None);
        lab.ok(&["fingerprint"], None);
        let packs = packs();
        let packs = packs.to_str().unwrap();
        lab.ok(&["classify", "--pack-dir", packs], None);
        lab.ok(&["pick", "run", "--pack-dir", packs], None);
        let work = lab.work.path().to_str().unwrap().to_string();
        lab.ok(
            &[
                "place", "add", "scratch", &work, "--role", "working", "--fast",
            ],
            None,
        );
        let doc = lab.work.path().join("every.json");
        std::fs::write(
            &doc,
            json!({"ast_version": 1, "sets": {"every": {"grain": "stack"}}, "out": {"set": "every", "level": "record"}}).to_string(),
        )
        .unwrap();
        lab.ok(
            &[
                "ask",
                "selections",
                "save",
                "--name",
                "every",
                "--file",
                doc.to_str().unwrap(),
                "--pack-dir",
                packs,
            ],
            None,
        );
        std::fs::remove_file(&doc).unwrap();
        lab
    }

    fn command(&self, path: &OsString) -> Command {
        let mut c = nils();
        c.arg("--registry")
            .arg(self.home.path())
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env("PATH", path)
            .env("NILS_PACK_DIR", packs())
            .env("FAKE_PODMAN_ARGS", self.bin.path().join("args.log"))
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL");
        c
    }

    fn run_on(
        &self,
        path: &OsString,
        args: &[&str],
        stdin: Option<&str>,
    ) -> (bool, String, String) {
        let mut child = self
            .command(path)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
        drop(input);
        let out = child.wait_with_output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        self.run_on(&self.path, args, stdin)
    }

    fn ok(&self, args: &[&str], stdin: Option<&str>) -> String {
        let (good, out, err) = self.run(args, stdin);
        assert!(good, "nils {}: {err}", args.join(" "));
        out
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.ok(args, None);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    /// The words the stand-in podman was given, one run after another.
    fn podman_runs(&self) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(self.bin.path().join("args.log")).unwrap_or_default();
        text.lines()
            .filter(|r| !r.trim().is_empty())
            .map(|r| serde_json::from_str(r).unwrap())
            .collect()
    }

    fn store(&self) -> nils_registry::Store {
        nils_registry::Store::open_sqlite(&self.home.path().join("registry.db")).unwrap()
    }

    fn add_descriptor(&self, name: &str, text: &str) -> Value {
        let file = self.work.path().join(format!("{name}.yml"));
        std::fs::write(&file, text).unwrap();
        let v = self.json(&["pipeline", "add", file.to_str().unwrap(), "--json"]);
        std::fs::remove_file(&file).unwrap();
        v
    }
}

fn pair(words: &[String], a: &str, b: &str) -> bool {
    words.windows(2).any(|w| w[0] == a && w[1] == b)
}

#[test]
fn the_catalog_refuses_an_unpinned_image_and_takes_v0_s_n4_re_pinned() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-catalog");
    // an image named by a tag is refused, and so is v0's config id
    let file = lab.work.path().join("tagged.yml");
    for image in [
        "ghcr.io/nima-ch/bids-prep-wrapper:v1".to_string(),
        format!("antsx/ants@sha256:{}", "9ab4cff8"),
    ] {
        std::fs::write(&file, stack_echo(&image)).unwrap();
        let (good, _, err) = lab.run(&["pipeline", "add", file.to_str().unwrap()], None);
        assert!(!good);
        assert!(
            err.contains("not pinned by its registry manifest digest"),
            "{err}"
        );
    }
    // the repository's N4 descriptor, v0's re-pinned, checks
    let n4 = repo().join("pipelines/n4-bias-correction/nils.job.yml");
    let v = lab.json(&["pipeline", "add", n4.to_str().unwrap(), "--json"]);
    assert_eq!(v["label"], "n4-bias-correction@1", "{v}");
    assert_eq!(v["added"], true);
    assert_eq!(v["layout"], "bids");
    assert_eq!(v["level"], "session");
    assert!(
        v["image"]
            .as_str()
            .unwrap()
            .starts_with("docker.io/antsx/ants@sha256:"),
        "{v}"
    );
    assert!(
        v["descriptor_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    // the same descriptor again is the version it is
    let again = lab.json(&["pipeline", "add", n4.to_str().unwrap(), "--json"]);
    assert_eq!(again["id"], v["id"]);
    assert_eq!(again["added"], false);
    let listed = lab.json(&["pipeline", "list", "--json"]);
    assert_eq!(listed["pipelines"].as_array().unwrap().len(), 1);
    assert_eq!(listed["capability"]["enabled"], true, "{listed}");
    assert_eq!(listed["capability"]["runtime"]["name"], "podman");
    let shown = lab.ok(&["pipeline", "show", "n4-bias-correction"], None);
    assert!(shown.contains("shrink_factor"), "{shown}");
    // an audit row for the add, none for the second
    let mut store = lab.store();
    let n = store
        .query(
            "SELECT COUNT(*) FROM audit WHERE action = 'pipeline.add'",
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn a_machine_with_no_runtime_reports_pipelines_off_and_no_error() {
    let lab_home = TempDir::new("pipelines-none-home");
    let empty = TempDir::new("pipelines-none-path");
    let path = empty.path().as_os_str().to_owned();
    let run = |args: &[&str], stdin: Option<&str>| {
        let mut c = nils();
        let mut child = c
            .arg("--registry")
            .arg(lab_home.path())
            .args(args)
            .env("PATH", &path)
            .env_remove("NILS_DSN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(t) = stdin {
            input.write_all(t.as_bytes()).unwrap();
        }
        drop(input);
        let out = child.wait_with_output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };
    assert!(run(&["key", "add", "k"], Some("a none test key\n")).0);
    assert!(run(&["init", "--key", "k"], None).0);
    let work = TempDir::new("pipelines-none-work");
    assert!(
        run(
            &[
                "place",
                "add",
                "w",
                work.path().to_str().unwrap(),
                "--role",
                "working"
            ],
            None
        )
        .0
    );
    let (good, out, err) = run(&["pipeline", "list", "--json"], None);
    assert!(good, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["capability"]["enabled"], false, "{v}");
    assert!(
        v["capability"]["reason"]
            .as_str()
            .unwrap()
            .contains("no container runtime here"),
        "{v}"
    );
    assert_eq!(v["capability"]["runtime"], Value::Null);
    // docker is never taken unless an operator chooses it
    let (good, out, _) = run(&["pipeline", "runtime", "--json"], None);
    assert!(good);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["choice"], "auto");
    assert!(
        v["looked"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["runtime"] != "docker"),
        "{v}"
    );
    let (good, _, err) = run(&["pipeline", "runtime", "--set", "lxc"], None);
    assert!(!good && err.contains("--set is one of"), "{err}");
    // a run says why and stops before anything is written
    let n4 = repo().join("pipelines/n4-bias-correction/nils.job.yml");
    assert!(run(&["pipeline", "add", n4.to_str().unwrap()], None).0);
    let (good, _, err) = run(&["run", "n4-bias-correction", "--handle", "1"], None);
    assert!(!good);
    assert!(err.contains("pipelines are off"), "{err}");
    assert!(
        std::fs::read_dir(work.path()).unwrap().next().is_none(),
        "nothing written"
    );
}

#[test]
fn a_stacks_run_registers_its_outputs_raises_its_failures_and_repeats_its_digests() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-stacks");
    lab.add_descriptor(
        "stack-echo",
        &stack_echo(&format!("example.org/stack-echo@sha256:{}", "a".repeat(64))),
    );

    // a run over every stack, with results.json
    let first = lab.json(&[
        "run",
        "stack-echo",
        "--select",
        "selection:every@1",
        "--json",
    ]);
    assert_eq!(first["status"], "done", "{first}");
    assert_eq!(first["pipeline"], "stack-echo@1");
    assert_eq!(first["runtime"], "podman");
    assert_eq!(first["runtime_version"], "9.9.9-test");
    assert_eq!(
        first["device"], "cpu",
        "no GPU here, and the need is optional"
    );
    assert_eq!(
        first["params"],
        json!({"mode": "all"}),
        "every parameter, the default filled"
    );
    assert_eq!(first["summary"]["units"]["succeeded"], 4, "{first}");
    assert_eq!(first["summary"]["derivatives"], 4);
    assert_eq!(
        first["summary"]["proposals"],
        json!({"given": 2, "declared": 1, "undeclared": 1, "taken": 0})
    );
    assert_eq!(first["derivatives"].as_array().unwrap().len(), 4);
    let run1 = first["id"].as_i64().unwrap();
    let handle = first["handle_id"].as_i64().unwrap();

    // the flags every run carries, as podman was given them
    let runs = lab.podman_runs();
    assert_eq!(runs.len(), 1);
    let words = &runs[0];
    assert!(pair(words, "--network", "none"), "{words:?}");
    assert!(pair(words, "--userns", "keep-id"), "{words:?}");
    assert!(pair(words, "--cap-drop", "all"), "{words:?}");
    let input = format!("{}/runs/{run1}/input:/input:ro", lab.work.path().display());
    assert!(pair(words, "--volume", &input), "{words:?}");
    let output = format!(
        "{}/derivatives/stack-echo/{run1}:/output",
        lab.work.path().display()
    );
    assert!(pair(words, "--volume", &output), "{words:?}");
    assert!(
        words.iter().any(|w| w.ends_with(":/source/0:ro")),
        "{words:?}"
    );
    assert!(!words.iter().any(|w| w.contains("nvidia")), "{words:?}");

    // the manifest names each stack's files under the source mounted
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(
            lab.work
                .path()
                .join(format!("runs/{run1}/input/stacks.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["contract"], "job/v1");
    assert_eq!(
        manifest["sources"],
        json!([{"id": 0, "mount": "/source/0"}])
    );
    let stacks = manifest["stacks"].as_array().unwrap();
    assert_eq!(stacks.len(), 4);
    assert!(
        stacks
            .iter()
            .all(|s| s["files"].as_array().unwrap().len() == 12),
        "{manifest}"
    );

    // each output a derivative of its stack, hashed by the engine, naming the run
    let listed = lab.json(&["derivative", "list", "--run", &run1.to_string(), "--json"]);
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), 4);
    for d in rows {
        assert_eq!(d["kind"], "output");
        assert_eq!(d["scope"], "stack");
        assert_eq!(d["run_id"], run1);
        assert_eq!(d["media_type"], "text/plain");
        let file = lab.work.path().join(d["path"].as_str().unwrap());
        assert_eq!(d["sha256"], sha256(&std::fs::read(&file).unwrap()));
        assert!(
            d["path"]
                .as_str()
                .unwrap()
                .starts_with(&format!("derivatives/stack-echo/{run1}/stack-"))
        );
    }

    // the run pins its handle
    let mut store = lab.store();
    let pins = nils_ask::handle::pinned_by(&mut store, handle).unwrap();
    assert!(pins.contains(&format!("pipeline run {run1}")), "{pins:?}");

    // the same run again: the same digests, in a folder of its own
    let second = lab.json(&[
        "run",
        "stack-echo",
        "--select",
        "selection:every@1",
        "--json",
    ]);
    assert_eq!(second["status"], "done");
    assert_eq!(second["results_digest"], first["results_digest"]);
    let digests = |run: &Value| -> Vec<String> {
        let rows = lab.json(&[
            "derivative",
            "list",
            "--run",
            &run["id"].to_string(),
            "--json",
        ]);
        let mut d: Vec<String> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["sha256"].as_str().unwrap().to_string())
            .collect();
        d.sort();
        d
    };
    assert_eq!(digests(&first), digests(&second));

    // a unit that failed, and units the results did not name, are review items
    let failed = lab.json(&[
        "run",
        "stack-echo",
        "--select",
        "selection:every@1",
        "--param",
        "mode=fail-last",
        "--json",
    ]);
    assert_eq!(failed["status"], "done", "the container exited 0: {failed}");
    assert_eq!(failed["summary"]["units"]["failed"], 1);
    assert_eq!(failed["summary"]["derivatives"], 3);
    let items = failed["summary"]["review_items"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(items.len(), 1);
    let partial = lab.json(&[
        "run",
        "stack-echo",
        "--select",
        "selection:every@1",
        "--param",
        "mode=partial",
        "--json",
    ]);
    assert_eq!(partial["summary"]["units"]["unreported"], 3, "{partial}");
    let listed = lab.json(&["review", "list", "--kind", "pipeline:qc", "--json"]);
    let open: Vec<&Value> = listed["items"].as_array().unwrap().iter().collect();
    assert_eq!(open.len(), 4, "{listed}");
    let one = open.iter().find(|i| i["id"] == items[0]).unwrap();
    assert_eq!(one["scope"], "run");
    assert_eq!(one["ref"]["run_id"], failed["id"]);
    assert_eq!(one["evidence"]["error"], "refused on purpose");
    assert_eq!(one["evidence"]["metrics"]["files"], 12);
    // the kind is one the review-item contract admits
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(repo().join("contracts/review-item/v4/review-item.schema.json"))
            .unwrap(),
    )
    .unwrap();
    let pattern =
        regex::Regex::new(schema["properties"]["kind"]["pattern"].as_str().unwrap()).unwrap();
    assert!(pattern.is_match(one["kind"].as_str().unwrap()), "{one}");

    // without results.json a unit's files are the ones its template finds
    let silent = lab.json(&[
        "run",
        "stack-echo",
        "--select",
        "selection:every@1",
        "--param",
        "mode=silent",
        "--json",
    ]);
    assert_eq!(silent["summary"]["units"]["succeeded"], 4, "{silent}");
    assert_eq!(
        silent["summary"]["results"],
        "none: found by the declared templates"
    );

    // a container that fails registers nothing and says where its log is
    let (good, out, err) = lab.run(
        &[
            "run",
            "stack-echo",
            "--select",
            "selection:every@1",
            "--param",
            "mode=crash",
            "--json",
        ],
        None,
    );
    assert!(!good, "{out}");
    assert!(err.contains("the container exited 3"), "{err}");
    let crashed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(crashed["status"], "failed");
    assert_eq!(crashed["exit_code"], 3);
    assert_eq!(crashed["summary"]["derivatives"], 0);
    assert_eq!(crashed["summary"]["units"]["failed"], 4);

    // a parameter out of its choices, and one not declared, are refused
    let (good, _, err) = lab.run(
        &[
            "run",
            "stack-echo",
            "--select",
            "selection:every@1",
            "--param",
            "mode=loud",
        ],
        None,
    );
    assert!(!good && err.contains("mode is one of"), "{err}");
    let (good, _, err) = lab.run(
        &[
            "run",
            "stack-echo",
            "--select",
            "selection:every@1",
            "--param",
            "speed=1",
        ],
        None,
    );
    assert!(!good && err.contains("no parameter speed"), "{err}");

    // every run is a job of kind pipeline, and an audit row
    let jobs: i64 = store
        .query("SELECT COUNT(*) FROM job WHERE kind = 'pipeline'", &[])
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(jobs, 6);
    let audited: i64 = store
        .query(
            "SELECT COUNT(*) FROM audit WHERE action = 'pipeline.run'",
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(audited, 6);
    let shown = lab.ok(&["pipeline", "runs", &run1.to_string()], None);
    assert!(shown.contains("4 succeeded, 0 failed"), "{shown}");
}

#[test]
fn a_bids_run_meets_one_t1w_per_session_and_registers_one_output_per_session() {
    if !have("python3") || !have("dcm2niix") {
        eprintln!(
            "python3 or dcm2niix is not installed; the bids layout needs a converter, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-bids");
    lab.add_descriptor("bids-copy", BIDS_COPY);
    let v = lab.json(&[
        "run",
        "bids-copy",
        "--select",
        "selection:every@1",
        "--json",
    ]);
    assert_eq!(v["status"], "done", "{v}");
    assert_eq!(
        v["summary"]["units"]["total"], 2,
        "one unit per session: {v}"
    );
    assert_eq!(v["summary"]["units"]["succeeded"], 2, "{v}");
    assert_eq!(
        v["summary"]["derivatives"], 2,
        "one T1w per session, the picks applied: {v}"
    );
    assert!(v["input_release_id"].is_i64(), "{v}");
    let run = v["id"].as_i64().unwrap();
    // the participants were named on the command line
    let words = &lab.podman_runs()[0];
    let image_at = words.iter().position(|w| w.contains("@sha256:")).unwrap();
    // python3 -c <code> /input /output, then one label per participant
    assert_eq!(
        &words[image_at + 4..image_at + 6],
        ["/input", "/output"],
        "{words:?}"
    );
    assert_eq!(words[image_at + 6..].len(), 2, "{words:?}");
    let rows = lab.json(&["derivative", "list", "--run", &run.to_string(), "--json"]);
    for d in rows.as_array().unwrap() {
        assert_eq!(d["scope"], "session", "{d}");
        assert!(d["session_day"].is_string(), "{d}");
        assert!(
            d["path"].as_str().unwrap().ends_with("_desc-n4_T1w.nii.gz"),
            "{d}"
        );
    }
    let days: std::collections::BTreeSet<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["session_day"].as_str().unwrap())
        .collect();
    assert_eq!(
        days.into_iter().collect::<Vec<_>>(),
        ["2022-01-15", "2023-03-10"]
    );
    // the release a run's input was is the run's, in its own folder
    let (good, _, err) = lab.run(
        &[
            "release",
            "--out",
            lab.work.path().join("elsewhere").to_str().unwrap(),
            "--into-run",
            &run.to_string(),
            "--layout",
            "bids",
        ],
        None,
    );
    assert!(!good, "a finished run's folder is not a release target");
    assert!(err.contains("--into-run"), "{err}");
}

/// A server with the worker beside the doors, on the lab's search path.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const OPERATOR: &str = "an-operator-token-of-length";
const PLAIN: &str = "a-plain-pipelines-token-long";
const READER: &str = "a-reader-token-of-its-length";

impl Server {
    fn start(lab: &Lab) -> Server {
        let tokens = [
            format!("{OPERATOR}=ops@lab:operator"),
            format!("{PLAIN}=pat@lab:pipelines:work,pipelines:see"),
            format!("{READER}=lou@lab:reader"),
        ]
        .join(",");
        let mut child = lab
            .command(&lab.path)
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "1",
                "--worker",
                "--auth",
                "token",
            ])
            .args(["--pack-dir", packs().to_str().unwrap()])
            .env("NILS_TOKENS", tokens)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            let _ = child.kill();
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        // the worker prints a line a job; nobody need read them
        std::thread::spawn(move || for _ in lines {});
        Server { child, port }
    }

    fn raw(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&response[..split]).to_string();
        let status: u16 = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, response[split + 4..].to_vec())
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>, token: &str) -> (u16, Value) {
        let (status, bytes) = self.raw(method, path, body, token);
        let text = String::from_utf8_lossy(&bytes).to_string();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
}

#[test]
fn a_run_queued_at_the_door_is_served_by_bytes_and_by_a_shared_path() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-door");
    lab.add_descriptor(
        "stack-echo",
        &stack_echo(&format!("example.org/stack-echo@sha256:{}", "a".repeat(64))),
    );
    let server = Server::start(&lab);

    let (status, caps) = server.call("GET", "/api/capabilities", None, OPERATOR);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["pipelines"]["enabled"], true, "{caps}");
    assert_eq!(caps["pipelines"]["grants"]["run_detail"], "quasi");
    for door in ["GET /api/pipelines", "GET /api/pipeline-runs/{id}"] {
        assert!(
            caps["doors"].as_array().unwrap().iter().any(|d| d == door),
            "{door}"
        );
    }
    let (status, cat) = server.call("GET", "/api/pipelines", None, OPERATOR);
    assert_eq!(status, 200);
    assert_eq!(cat["pipelines"][0]["label"], "stack-echo@1", "{cat}");
    let (status, _) = server.call("GET", "/api/pipelines", None, READER);
    assert_eq!(status, 403, "a reader holds no pipelines:see");

    // a run reads pixels: pipelines:work at plain detail is refused
    let command = json!({"command": ["run", "stack-echo", "--select", "selection:every@1"]});
    let (status, doc) = server.call("POST", "/api/jobs", Some(command.clone()), PLAIN);
    assert_eq!(status, 403, "{doc}");
    // a path a caller composes never reaches the runner
    let (status, doc) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["run", "stack-echo", "--select", "selection:every@1", "--pack-dir", "/tmp"]})),
        OPERATOR,
    );
    assert_eq!(status, 400, "{doc}");
    let (status, doc) = server.call("POST", "/api/jobs", Some(command), OPERATOR);
    assert_eq!(status, 202, "{doc}");
    let job = doc["job"].as_i64().unwrap();
    let mut state = Value::Null;
    for _ in 0..600 {
        let (_, j) = server.call("GET", &format!("/api/jobs/{job}"), None, OPERATOR);
        state = j;
        if matches!(
            state["state"].as_str(),
            Some("done" | "failed" | "cancelled")
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(state["state"], "done", "{state}");
    assert_eq!(state["kind"], "pipeline", "{state}");
    let (status, runs) = server.call("GET", "/api/pipeline-runs", None, OPERATOR);
    assert_eq!(status, 200);
    let run = &runs["runs"][0];
    assert_eq!(run["status"], "done", "{runs}");
    assert_eq!(run["job_id"], job);
    assert_eq!(run["principal"], "ops@lab");
    let derivative = run["derivatives"][0].as_i64().unwrap();

    // by the door: the bytes, with their digest
    let (status, bytes) = server.raw(
        "GET",
        &format!("/api/derivatives/{derivative}/content"),
        None,
        OPERATOR,
    );
    assert_eq!(status, 200);
    let (_, d) = server.call(
        "GET",
        &format!("/api/derivatives/{derivative}"),
        None,
        OPERATOR,
    );
    assert_eq!(d["sha256"], sha256(&bytes), "{d}");
    assert_eq!(d["transports"], json!(["door"]));
    // no share declared: the door says so
    let (status, doc) = server.call(
        "GET",
        &format!("/api/derivatives/{derivative}/content?transport=share"),
        None,
        OPERATOR,
    );
    assert_eq!(status, 409, "{doc}");
    assert!(
        doc["error"]
            .as_str()
            .unwrap()
            .contains("declares no share path"),
        "{doc}"
    );

    // the place declares where its clients reach it, and the door answers
    // with a path there; this client shares the volume at the same path
    let places = lab.ok(&["place", "list", "--json"], None);
    let places: Value = serde_json::from_str(&places).unwrap();
    let id = places
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["role"] == "working")
        .unwrap()["id"]
        .to_string();
    let work = lab.work.path().canonicalize().unwrap();
    lab.ok(
        &["place", "set", &id, "--share", work.to_str().unwrap()],
        None,
    );
    let (_, d) = server.call(
        "GET",
        &format!("/api/derivatives/{derivative}"),
        None,
        OPERATOR,
    );
    assert_eq!(d["transports"], json!(["door", "share"]), "{d}");
    let (status, shared) = server.call(
        "GET",
        &format!("/api/derivatives/{derivative}/content?transport=share"),
        None,
        OPERATOR,
    );
    assert_eq!(status, 200, "{shared}");
    assert_eq!(shared["transport"], "share");
    let path = shared["path"].as_str().unwrap();
    assert_eq!(
        sha256(&std::fs::read(path).unwrap()),
        shared["sha256"].as_str().unwrap()
    );
    assert_eq!(shared["sha256"], d["sha256"], "the two transports agree");
    let (status, _) = server.call(
        "GET",
        &format!("/api/derivatives/{derivative}/content?transport=pigeon"),
        None,
        OPERATOR,
    );
    assert_eq!(status, 400);
    // the path is quasi, as the bytes are
    let (status, _) = server.call(
        "GET",
        &format!("/api/derivatives/{derivative}/content?transport=share"),
        None,
        PLAIN,
    );
    assert_eq!(status, 403);
    drop(server);
    let mut store = lab.store();
    let reads = store
        .query(
            "SELECT COUNT(*) FROM audit WHERE action = 'derivative.read'",
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(reads, 2, "the bytes and the path, each audited");
}

/// Which real runtime this machine offers a test, if any.
fn real_runtime() -> Option<&'static str> {
    if std::env::var("NILS_TEST_PIPELINE_RUNTIME").as_deref() == Ok("docker") && have("docker") {
        return Some("docker");
    }
    if have("podman") {
        let rootless = Command::new("podman")
            .args(["info", "--format", "{{.Host.Security.Rootless}}"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true");
        if rootless == Some(true) {
            return Some("podman");
        }
    }
    None
}

/// A probe of who a container's process is on the host, in the stacks
/// layout, on a small public image pinned by its index digest.
const UID_PROBE: &str = r#"name: uid-probe
schema-version: "0.5"
tool-version: "1.37.0"
container-image:
  type: docker
  image: "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e"
command-line: >-
  sh -c 'for u in $(sed -n "s/.*\"unit\": *\"\(stack-[0-9]*\)\".*/\1/p" [Manifest]); do
  mkdir -p [OutputLocation]/$u; id -u > [OutputLocation]/$u/uid.txt;
  if wget -q -T 2 -O /dev/null http://example.com 2>/dev/null; then echo net > [OutputLocation]/$u/net.txt; fi;
  done'
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  outputs:
    - id: uid
      kind: output
      path-template: "stack-{stack}/uid.txt"
"#;

#[test]
fn real_containers_run_n4_rootless_with_no_network() {
    let Some(runtime) = real_runtime() else {
        eprintln!(
            "no rootless podman here, and NILS_TEST_PIPELINE_RUNTIME is not docker; the real containers are not run"
        );
        return;
    };
    if !have("dcm2niix") {
        eprintln!(
            "dcm2niix is not installed; the bids input of N4 cannot be released, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-real");
    // the stand-in is not on this search path
    let path = std::env::var_os("PATH").unwrap_or_default();
    let ok = |args: &[&str]| -> Value {
        let (good, out, err) = lab.run_on(&path, args, None);
        assert!(good, "nils {}: {err}", args.join(" "));
        serde_json::from_str(&out).unwrap_or(Value::Null)
    };
    if runtime == "docker" {
        ok(&["pipeline", "runtime", "--set", "docker", "--json"]);
    }
    let cap = ok(&["pipeline", "runtime", "--json"]);
    assert_eq!(cap["runtime"]["name"], runtime, "{cap}");

    // who the container's process is on the host, and that it has no network
    let file = lab.work.path().join("probe.yml");
    std::fs::write(&file, UID_PROBE).unwrap();
    ok(&["pipeline", "add", file.to_str().unwrap()]);
    let probe = ok(&[
        "run",
        "uid-probe",
        "--select",
        "selection:every@1",
        "--json",
    ]);
    assert_eq!(probe["status"], "done", "{probe}");
    assert_eq!(probe["summary"]["units"]["succeeded"], 4, "{probe}");
    let me = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(lab.work.path()).unwrap().uid()
    };
    let out = lab.work.path().join(probe["output"].as_str().unwrap());
    for entry in std::fs::read_dir(&out).unwrap() {
        let dir = entry.unwrap().path();
        let uid: u32 = std::fs::read_to_string(dir.join("uid.txt"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(uid, me, "the container's process is this user, not root");
        assert_ne!(uid, 0);
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(dir.join("uid.txt")).unwrap().uid(), me);
        assert!(
            !dir.join("net.txt").exists(),
            "the container reached the network"
        );
    }

    // N4 over the frozen selection: one derivative per session, and the
    // same digests again
    let n4 = repo().join("pipelines/n4-bias-correction/nils.job.yml");
    ok(&["pipeline", "add", n4.to_str().unwrap()]);
    let first = ok(&[
        "run",
        "n4-bias-correction",
        "--select",
        "selection:every@1",
        "--param",
        "shrink_factor=2",
        "--json",
    ]);
    assert_eq!(first["status"], "done", "{first}");
    assert_eq!(first["summary"]["units"]["succeeded"], 2, "{first}");
    assert_eq!(
        first["summary"]["derivatives"], 2,
        "one per session: {first}"
    );
    let second = ok(&[
        "run",
        "n4-bias-correction",
        "--select",
        "selection:every@1",
        "--param",
        "shrink_factor=2",
        "--json",
    ]);
    assert_eq!(
        second["results_digest"], first["results_digest"],
        "a re-run gives the same digests"
    );
}
