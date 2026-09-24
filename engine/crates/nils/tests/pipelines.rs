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
    tree_at(|patient, n, slice| format!("{patient}/{n}/{slice}"))
}

/// [`tree`] with each file where `at` puts it.
fn tree_at(at: impl Fn(&str, &str, u32) -> String) -> TempDir {
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
                    &at(patient, n, slice),
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
        Lab::with_tree(name, tree())
    }

    fn with_tree(name: &str, src: TempDir) -> Lab {
        let lab = Lab {
            home: TempDir::new(&format!("{name}-home")),
            work: TempDir::new(&format!("{name}-work")),
            bin: TempDir::new(&format!("{name}-bin")),
            _src: src,
            path: OsString::new(),
        };
        let fake = lab.bin.file("podman", FAKE_PODMAN.as_bytes());
        // no GPU in the lab, whatever the host has: a machine with one would
        // otherwise pass it to an optional need
        let no_gpu = lab.bin.file("nvidia-smi", b"#!/bin/sh\nexit 1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [&fake, &no_gpu] {
                std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
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
        if !good {
            // the containers' own words, where a run got that far
            let logs: Vec<String> = std::fs::read_dir(self.work.path().join("runs"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| std::fs::read_to_string(e.path().join("log.txt")).ok())
                .collect();
            panic!("nils {}: {err}\n{}", args.join(" "), logs.join("\n"));
        }
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

/// This process's own uid and gid, which podman's `--user` names.
fn this_account() -> (u32, u32) {
    let id = |flag: &str| -> u32 {
        let out = Command::new("id").arg(flag).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    };
    (id("-u"), id("-g"))
}

/// Make a folder setgid to a group this account belongs to that is not its
/// own, as a shared working place is, so what is made in it takes that
/// group; answers the group, or none where the account has no other.
fn shared_group(dir: &Path) -> Option<u32> {
    let own = this_account().1;
    let out = Command::new("id").arg("-G").output().ok()?;
    let other: u32 = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|g| g.parse().ok())
        .find(|g| *g != own)?;
    let done = Command::new("chgrp")
        .arg(other.to_string())
        .arg(dir)
        .status()
        .ok()?
        .success()
        && Command::new("chmod")
            .arg("g+s")
            .arg(dir)
            .status()
            .ok()?
            .success();
    done.then_some(other)
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
    // a working place shared by a group, setgid: its folders take the
    // group, and the container must still run as this account's own
    let shared = shared_group(lab.work.path());
    if shared.is_none() {
        eprintln!("this account has one group; the setgid working place is not tried");
    }
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
    let proposed = &first["summary"]["proposals"];
    assert_eq!(
        (
            &proposed["given"],
            &proposed["declared"],
            &proposed["undeclared"],
            &proposed["taken"]
        ),
        (&json!(2), &json!(1), &json!(1), &json!(0)),
        "{proposed}"
    );
    // a proposal the contract does not admit (no probabilities, no model)
    // is refused whole, and says why; it is never a fact
    assert!(
        proposed["refused"]
            .as_str()
            .unwrap()
            .contains("probabilities"),
        "{proposed}"
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
    // wave 43's proof: an image's own USER beat keep-id; --user names the
    // engine's own account, which keep-id maps to itself, and never the
    // group of a shared (setgid) working place, which it does not map
    let (uid, gid) = this_account();
    assert!(pair(words, "--user", &format!("{uid}:{gid}")), "{words:?}");
    if let Some(g) = shared {
        use std::os::unix::fs::MetadataExt;
        let out = lab
            .work
            .path()
            .join(format!("derivatives/stack-echo/{run1}"));
        assert_eq!(
            std::fs::metadata(&out).unwrap().gid(),
            g,
            "the place is shared"
        );
    }
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
    // record 43 review: one bind per folder that holds the selection's
    // files, never the whole source place
    assert_eq!(
        manifest["sources"],
        json!([
            {"id": 0, "mount": "/source/0"}, {"id": 1, "mount": "/source/1"},
            {"id": 2, "mount": "/source/2"}, {"id": 3, "mount": "/source/3"},
        ])
    );
    assert_eq!(first["summary"]["scope"]["sources"], "folders", "{first}");
    let src = lab._src.path().display().to_string();
    assert!(
        !words.iter().any(|w| w == &format!("{src}:/source/0:ro")),
        "the source root is not bound: {words:?}"
    );
    assert!(
        words
            .iter()
            .any(|w| w == &format!("{src}/P1/1:/source/0:ro")),
        "{words:?}"
    );
    let stacks = manifest["stacks"].as_array().unwrap();
    assert_eq!(stacks.len(), 4);
    assert!(
        stacks
            .iter()
            .all(|s| s["files"].as_array().unwrap().len() == 12),
        "{manifest}"
    );
    // record 43: each stack's orientation, its axes as held now and its
    // slice count, for an image that seeds and picks slices
    for s in stacks {
        assert_eq!(s["slices"], 12, "{s}");
        assert!(s["orientation"].is_string(), "{s}");
        for key in ["body_part", "technique"] {
            assert!(s.get(key).is_some(), "{key}: {s}");
        }
    }
    assert!(
        stacks.iter().any(|s| s["technique"] == "MPRAGE"),
        "the classifier's technique: {manifest}"
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
    // record 43: the container exited 0 and a unit failed, so the run is
    // partial, not done, and the failed unit is a review item
    assert_eq!(failed["status"], "partial", "{failed}");
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
    assert_eq!(partial["status"], "partial", "{partial}");
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
    // wave 43's proof: a container that fails as a whole is one review
    // item of the run, not one per unit
    let raised = crashed["summary"]["review_items"].as_array().unwrap();
    assert_eq!(raised.len(), 1, "{crashed}");
    let listed = lab.json(&["review", "list", "--kind", "pipeline:qc", "--json"]);
    let of_run: Vec<&Value> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["ref"]["run_id"] == crashed["id"])
        .collect();
    assert_eq!(of_run.len(), 1, "{listed}");
    assert_eq!(of_run[0]["ref"]["unit"], "run", "{listed}");
    assert_eq!(of_run[0]["evidence"]["metrics"]["units"], 4, "{listed}");
    assert!(
        of_run[0]["evidence"]["error"]
            .as_str()
            .unwrap()
            .contains("exited 3"),
        "{listed}"
    );

    // wave 43's proof: podman's own failure (125, an image it cannot find)
    // names the image store it looked in
    let failed125 = lab
        .command(&lab.path)
        .args([
            "run",
            "stack-echo",
            "--select",
            "selection:every@1",
            "--json",
        ])
        .env("FAKE_PODMAN_EXIT", "125")
        .output()
        .unwrap();
    assert!(!failed125.status.success());
    let err = String::from_utf8_lossy(&failed125.stderr);
    assert!(err.contains("the container exited 125"), "{err}");
    assert!(
        err.contains("image store /fake/containers/storage"),
        "{err}"
    );
    assert!(err.contains("HOME is"), "{err}");

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
    assert_eq!(jobs, 7);
    let audited: i64 = store
        .query(
            "SELECT COUNT(*) FROM audit WHERE action = 'pipeline.run'",
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    assert_eq!(audited, 7);
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
    // record 43: the release of the run's input says so, and the history
    // leaves it out unless asked for it
    let history = lab.json(&["release", "--history", "--json"]);
    assert_eq!(history["count"], 0, "{history}");
    let all = lab.json(&["release", "--history", "--runs", "--json"]);
    let rows = all["releases"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{all}");
    assert_eq!(rows[0]["purpose"], "run_input", "{all}");
    assert_eq!(rows[0]["id"], v["input_release_id"], "{all}");
    let text = lab.ok(&["release", "--history"], None);
    assert!(!text.contains("pipeline-run-"), "{text}");

    // record 49 A1: units apart, each session in a container of its own
    // that sees its own subject's session alone and is named its subject
    let apart = BIDS_COPY
        .replace("name: bids-copy", "name: bids-apart")
        .replace(
            "  src, out = sys.argv[1], sys.argv[2]\n",
            "  src, out = sys.argv[1], sys.argv[2]\n  assert len(glob.glob(src + \"/sub-*/ses-*\")) == 1 and len(sys.argv) == 4, sys.argv\n",
        )
        .replace("  input: {layout: bids}\n", "  input: {layout: bids}\n  units: apart\n");
    assert!(
        apart.contains("units: apart") && apart.contains("assert len"),
        "{apart}"
    );
    lab.add_descriptor("bids-apart", &apart);
    let before = lab.podman_runs().len();
    let v = lab.json(&[
        "run",
        "bids-apart",
        "--select",
        "selection:every@1",
        "--json",
    ]);
    assert_eq!(v["status"], "done", "{v}");
    assert_eq!(v["summary"]["units"]["succeeded"], 2, "{v}");
    assert_eq!(v["summary"]["derivatives"], 2, "{v}");
    let runs = lab.podman_runs();
    assert_eq!(runs.len() - before, 2, "a container per session");
    let id = v["id"].as_i64().unwrap();
    for u in v["units_run"].as_array().unwrap() {
        let unit = u["unit"].as_str().unwrap();
        let input = lab
            .work
            .path()
            .join(format!("runs/{id}/units/{unit}/input"));
        let subjects: Vec<String> = std::fs::read_dir(&input)
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().unwrap().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(subjects.len(), 1, "{unit}: {subjects:?}");
        assert!(unit.starts_with(&subjects[0]), "{unit}: {subjects:?}");
        assert!(input.join("dataset_description.json").is_file());
    }
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
/// Record 49 R4's review: a reader of runs and of review items at plain.
const PLAIN_REVIEW: &str = "a-plain-review-token-of-lens";

impl Server {
    fn start(lab: &Lab) -> Server {
        Server::start_with(lab, &[])
    }

    fn start_with(lab: &Lab, extra: &[&str]) -> Server {
        let tokens = [
            format!("{OPERATOR}=ops@lab:operator"),
            format!("{PLAIN}=pat@lab:pipelines:work,pipelines:see"),
            format!("{READER}=lou@lab:reader"),
            format!("{PLAIN_REVIEW}=rae@lab:pipelines:see,review:see"),
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
            .args(extra)
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
    // record 49 A4: beside it, the starters the engine seeded at its start
    let echo = cat["pipelines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "stack-echo")
        .unwrap_or_else(|| panic!("{cat}"));
    assert_eq!(echo["label"], "stack-echo@1", "{cat}");
    assert_eq!(echo["starter"], false, "{echo}");
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

/// A reader of the probe's files as a derivative input, beside a label
/// set, in the stacks layout: it counts what it was given.
const UID_READER: &str = r#"name: uid-reader
schema-version: "0.5"
tool-version: "1.37.0"
container-image:
  type: docker
  image: "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e"
command-line: >-
  sh -c 'n=$(find [Inputs]/prev -name uid.txt | wc -l);
  l=no; if [ -f [Inputs]/labels/labels.tsv ]; then l=yes; fi;
  echo "$n $l" > [OutputLocation]/stack-seen.txt;
  for u in $(sed -n "s/.*\"unit\": *\"\(stack-[0-9]*\)\".*/\1/p" [Manifest]); do
  mkdir -p [OutputLocation]/$u; echo read > [OutputLocation]/$u/read.txt; done'
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  inputs:
    - {id: prev, type: "derivative:output"}
    - {id: labels, type: label_set}
  outputs:
    - id: read
      kind: output
      path-template: "stack-{stack}/read.txt"
"#;

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
  mkdir -p [OutputLocation]/$u; id -u > [OutputLocation]/$u/uid.txt; id -g > [OutputLocation]/$u/gid.txt;
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

/// An image that names a `USER` of its own, as the body-part image does:
/// busybox run as nobody unless the runner says otherwise.
const NOBODY_IMAGE: &str = "FROM docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e\nUSER 65534:65534\n";

/// An embedder on [`NOBODY_IMAGE`], as `bodypart-embed` is one: it takes
/// the embeddings the cache holds as an optional input, skips a stack it
/// has one for, and writes a one-value embedding for each other stack, with
/// its encoder's card. `kept.txt` says how many it was given.
fn emb_nobody(image: &str) -> String {
    let enc = format!("sha256:{}", "d".repeat(64));
    format!(
        r#"name: emb-nobody
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "{image}"
command-line: >-
  sh -c 'e={enc}; o=[OutputLocation]; mkdir -p $o/emb; us=""; k=0;
  for s in $(sed -n "s/.*\"unit\": *\"stack-\([0-9]*\)\".*/\1/p" [Manifest]); do
  if [ -n "$(find [Inputs]/embeddings -name $s.emb 2>/dev/null)" ]; then k=$((k+1)); st=skipped; d="";
  else h="{{\"format\":\"nils-embedding\",\"dtype\":\"<f4\",\"stack_id\":$s,\"encoder\":\"$e\",\"preprocess_version\":\"v1\",\"rows\":1,\"dim\":1,\"slices\":[ 0 ]}}";
  n=${{#h}}; p=$(( (12+n+63)/64*64-12-n ));
  {{ printf NILSEMB1; printf "\\$(printf %03o $((n%256)))\\$(printf %03o $((n/256)))\\000\\000"; printf %s "$h"; head -c $p /dev/zero; head -c 4 /dev/zero; }} > $o/emb/$s.emb;
  st=succeeded; d="\"emb/$s.emb\""; fi;
  us="$us${{us:+,}}{{\"unit_id\":\"stack-$s\",\"status\":\"$st\",\"derivatives\":[ $d ]}}"; done;
  echo $k > $o/kept.txt;
  echo "{{\"schema_version\":\"1\",\"units\":[ $us ],\"models\":[ {{\"name\":\"nobody-encoder\",\"version\":\"1\",\"kind\":\"encoder\",\"digest\":\"$e\",\"task\":\"encoder\"}} ]}}" > $o/results.json'
x-nils:
  analysis-level: stack
  input: {{layout: stacks}}
  inputs:
    - {{id: embeddings, type: "derivative:embedding", optional: true}}
  outputs:
    - {{id: emb, kind: embedding, path-template: "emb/{{stack}}.emb", media-type: application/vnd.nils.embedding, encoders: ["{enc}"]}}
"#
    )
}

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
    // a working place shared by a group, setgid, as a site's often is
    let shared = shared_group(lab.work.path());
    // the stand-in is not on this search path
    let path = std::env::var_os("PATH").unwrap_or_default();
    // rootless podman maps the user it runs as by name, so the real
    // containers run as the user this test is, not the lab's anna
    let me = std::env::var("USER").unwrap_or_default();
    let ok = |args: &[&str]| -> Value {
        let done = lab
            .command(&path)
            .env("USER", &me)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let (good, out, err) = (
            done.status.success(),
            String::from_utf8_lossy(&done.stdout).to_string(),
            String::from_utf8_lossy(&done.stderr).to_string(),
        );
        if !good {
            // the container's own words, when it ran
            let logs: Vec<String> = std::fs::read_dir(lab.work.path().join("runs"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| std::fs::read_to_string(e.path().join("log.txt")).ok())
                .collect();
            panic!("nils {}: {err}\n{}", args.join(" "), logs.join("\n"));
        }
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
        // and in this account's own group, whatever group the place has
        if runtime == "podman" {
            let gid: u32 = std::fs::read_to_string(dir.join("gid.txt"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert_eq!(gid, this_account().1, "shared group {shared:?}");
        }
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(dir.join("uid.txt")).unwrap().uid(), me);
        assert!(
            !dir.join("net.txt").exists(),
            "the container reached the network"
        );
    }

    // the typed inputs: the probe's files as a derivative input, linked
    // into the run's own inputs, and a label set bound at /inputs/labels
    // inside the read-only /inputs, whose mountpoint the engine makes first
    let exp = lab.work.path().join("export");
    std::fs::create_dir_all(&exp).unwrap();
    ok(&[
        "place",
        "add",
        "exp",
        exp.to_str().unwrap(),
        "--role",
        "export",
    ]);
    let tsv = lab.work.path().join("v0.tsv");
    std::fs::write(
        &tsv,
        "SeriesInstanceUID\tbody_part\tdate\n1.2.826.0.1.3680043.8.498.71.1.1\tBrain\t2024-05-06\n",
    )
    .unwrap();
    let imported = ok(&[
        "labels",
        "import-v0",
        "--tsv",
        tsv.to_str().unwrap(),
        "--to",
        exp.join("v0").to_str().unwrap(),
        "--json",
    ]);
    let set = imported["label_set"]["id"].to_string();
    let file = lab.work.path().join("reader.yml");
    std::fs::write(&file, UID_READER).unwrap();
    ok(&["pipeline", "add", file.to_str().unwrap()]);
    let read = ok(&[
        "run",
        "uid-reader",
        "--select",
        "selection:every@1",
        "--labels",
        &set,
        "--json",
    ]);
    assert_eq!(read["status"], "done", "{read}");
    let out = lab.work.path().join(read["output"].as_str().unwrap());
    let seen = std::fs::read_to_string(out.join("stack-seen.txt"))
        .unwrap_or_default()
        .trim()
        .to_string();
    assert_eq!(
        seen, "4 yes",
        "four probe files and the label set's labels.tsv"
    );

    // wave 43's proof: an image with a USER of its own ran as that user
    // despite keep-id, its outputs belonged to a sub-uid, and the next run
    // could not hard-link the cached embeddings (EPERM). Built here from
    // busybox with USER 65534, run twice: the second run is given the
    // first's embeddings, and every file is this user's.
    if runtime == "podman" {
        let ctx = lab.work.path().join("nobody-image");
        std::fs::create_dir_all(&ctx).unwrap();
        std::fs::write(ctx.join("Containerfile"), NOBODY_IMAGE).unwrap();
        let built = Command::new("podman")
            .args(["build", "-q", "-t", "localhost/nils-test-nobody", "-f"])
            .arg(ctx.join("Containerfile"))
            .arg(&ctx)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let digest = Command::new("podman")
            .args([
                "image",
                "inspect",
                "--format",
                "{{.Digest}}",
                "localhost/nils-test-nobody",
            ])
            .output()
            .unwrap();
        let digest = String::from_utf8_lossy(&digest.stdout).trim().to_string();
        assert!(digest.starts_with("sha256:"), "{digest}");
        let file = lab.work.path().join("emb-nobody.yml");
        std::fs::write(
            &file,
            emb_nobody(&format!("localhost/nils-test-nobody@{digest}")),
        )
        .unwrap();
        ok(&["pipeline", "add", file.to_str().unwrap()]);
        let owner = |p: &Path| {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(p).unwrap().uid()
        };
        let first = ok(&[
            "run",
            "emb-nobody",
            "--select",
            "selection:every@1",
            "--json",
        ]);
        assert_eq!(first["status"], "done", "{first}");
        assert_eq!(first["summary"]["embeddings"]["registered"], 4, "{first}");
        let out = lab.work.path().join(first["output"].as_str().unwrap());
        for e in std::fs::read_dir(out.join("emb")).unwrap() {
            let e = e.unwrap().path();
            assert_eq!(owner(&e), me, "{} is this user's", e.display());
        }
        let second = ok(&[
            "run",
            "emb-nobody",
            "--select",
            "selection:every@1",
            "--json",
        ]);
        assert_eq!(second["status"], "done", "{second}");
        assert_eq!(second["summary"]["units"]["skipped"], 4, "{second}");
        let out = lab.work.path().join(second["output"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(out.join("kept.txt"))
                .unwrap()
                .trim(),
            "4",
            "the second run was given the first's four embeddings"
        );
        assert_eq!(owner(&out.join("kept.txt")), me);
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

/// The four stand-ins of the body-part loop's runner half (record 43): an
/// embedding per stack under one encoder its results carry the card of,
/// seeds and a selection, a head fitted as a run-level model output with
/// its card, and proposals by that head. Each checks what stacks.json
/// carries and writes only under the output folder.
const LOOP: &str = r#"
- name: bp-embed
  command: |
    python3 -c '
    import json, os, struct, sys
    m = json.load(open(sys.argv[1])); out = sys.argv[2]
    enc = "sha256:" + "e" * 64
    units = []
    for s in m["stacks"]:
        h = json.dumps({"format": "nils-embedding", "dtype": "<f4", "stack_id": s["stack_id"], "encoder": enc, "preprocess_version": "v1", "rows": 3, "dim": 4, "slices": [0, 1, 2]}).encode()
        start = (12 + len(h) + 63) // 64 * 64
        b = b"NILSEMB1" + struct.pack("<I", len(h)) + h
        b += bytes(start - len(b)) + struct.pack("<12f", *[float(i) for i in range(12)])
        rel = "emb/%d.emb" % s["stack_id"]
        os.makedirs(os.path.join(out, "emb"), exist_ok=True)
        open(os.path.join(out, rel), "wb").write(b)
        units.append({"unit_id": s["unit"], "status": "succeeded", "derivatives": [rel]})
    card = {"name": "stand-in-encoder", "version": "1", "kind": "encoder", "digest": enc, "task": "encoder"}
    json.dump({"schema_version": "1", "units": units, "models": [card]}, open(os.path.join(out, "results.json"), "w"))
    ' [Manifest] [OutputLocation]
  outputs:
    - {id: enc, kind: embedding, path-template: "emb/{stack}.emb", media-type: application/vnd.nils.embedding, encoders: ["sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"]}
- name: bp-seed
  command: |
    python3 -c '
    import json, os, sys
    m = json.load(open(sys.argv[1])); out = sys.argv[2]
    st = m["stacks"]
    assert all(s["slices"] == 12 and s["orientation"] for s in st), st
    units = [{"unit_id": s["unit"], "status": "succeeded"} for s in st]
    seeds = [{"stack_id": s["stack_id"], "axis": "body_part", "value": "brain", "margin": 0.3, "source": "zero_shot"} for s in st[:3]]
    seeds.append({"stack_id": 999999, "axis": "body_part", "value": "spine", "margin": 0.1})
    sel = {"stacks": [st[0]["stack_id"], st[1]["stack_id"], 999999]}
    json.dump({"schema_version": "1", "units": units, "seeds": seeds, "selection": sel}, open(os.path.join(out, "results.json"), "w"))
    ' [Manifest] [OutputLocation]
  outputs:
    - {id: note, kind: output, path-template: "notes/{stack}.txt"}
- name: bp-train
  params: |
    - {id: enc, name: Encoder, type: String, value-key: "[ENC]", default-value: "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"}
    - {id: lie, name: Lie, type: String, value-key: "[LIE]", default-value: "no"}
  command: |
    python3 -c '
    import hashlib, json, os, sys
    m = json.load(open(sys.argv[1])); out = sys.argv[2]; enc = sys.argv[3]; lie = sys.argv[4]; seen = sys.argv[5]
    embs = [f for r, _, fs in os.walk(seen) for f in fs if f.endswith(".emb")]
    assert len(embs) == len(m["stacks"]), embs
    os.makedirs(os.path.join(out, "head"), exist_ok=True)
    body = json.dumps({"format": "stand-in-head", "classes": ["brain", "spine"], "lie": lie}, sort_keys=True).encode()
    open(os.path.join(out, "head/head.json"), "wb").write(body)
    d = "sha256:" + (hashlib.sha256(body).hexdigest() if lie == "no" else "0" * 64)
    card = {"name": "bp-head", "version": "h" + lie, "kind": "head", "digest": d, "task": "axis:body_part", "encoders": [{"digest": enc}], "threshold": 0.8}
    json.dump(card, open(os.path.join(out, "head/card.json"), "w"))
    units = [{"unit_id": s["unit"], "status": "succeeded"} for s in m["stacks"]]
    json.dump({"schema_version": "1", "units": units, "models": [card]}, open(os.path.join(out, "results.json"), "w"))
    ' [Manifest] [OutputLocation] [ENC] [LIE] [Inputs]/embeddings
  inputs:
    - {id: labels, type: label_set, optional: true}
    - {id: embeddings, type: "derivative:embedding"}
  outputs:
    - {id: head, kind: model, level: run, path-template: "head/head.json", card: head/card.json, media-type: application/json}
- name: bp-infer
  params: |
    - {id: p, name: Confidence, type: Number, value-key: "[P]", default-value: 0.95}
    - {id: extra, name: A stack outside, type: Number, value-key: "[EXTRA]", default-value: 0}
  command: |
    python3 -c '
    import hashlib, json, os, sys
    m = json.load(open(sys.argv[1])); out = sys.argv[2]; head = sys.argv[3]; p = float(sys.argv[4]); extra = int(float(sys.argv[5]))
    d = "sha256:" + hashlib.sha256(open(head, "rb").read()).hexdigest()
    props, units = [], []
    for i, s in enumerate(m["stacks"]):
        v, w = ("brain", "spine") if s["files"][0]["source"] % 2 == 0 else ("spine", "brain")
        q = p
        props.append({"stack_id": s["stack_id"], "axis": "body_part", "value": v, "probabilities": {v: q, w: round(1 - q, 6)}, "model_digest": d})
        units.append({"unit_id": s["unit"], "status": "succeeded"})
    if extra:
        props.append({"stack_id": extra, "axis": "body_part", "value": "brain", "probabilities": {"brain": 0.99, "spine": 0.01}, "model_digest": d})
        props.append({"stack_id": m["stacks"][0]["stack_id"], "axis": "body_part", "value": "brain", "probabilities": {"brain": 0.99, "spine": 0.01}, "model_digest": "sha256:" + "e" * 64})
    json.dump({"schema_version": "1", "units": units, "proposals": props}, open(os.path.join(out, "results.json"), "w"))
    ' [Manifest] [OutputLocation] [Inputs]/head/head.json [P] [EXTRA]
  inputs:
    - {id: head, type: model}
  outputs:
    - {id: note, kind: output, path-template: "notes/{stack}.txt"}
  proposals: [{axis: body_part}]
"#;

/// One of [`LOOP`]'s stand-ins as a descriptor of the stacks layout.
fn stand_in(name: &str) -> String {
    let all: Value =
        serde_json::to_value(serde_saphyr::from_str::<serde_json::Value>(LOOP).unwrap()).unwrap();
    let e = all
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap();
    let indent = |text: &str, n: usize| -> String {
        text.lines()
            .map(|l| format!("{}{l}\n", " ".repeat(n)))
            .collect()
    };
    let yaml = |v: &Value| serde_json::to_string(v).unwrap();
    let mut doc = format!(
        "name: {name}\nschema-version: \"0.5\"\ntool-version: \"1\"\ncontainer-image:\n  type: docker\n  image: \"example.org/{name}@sha256:{}\"\n",
        "c".repeat(64)
    );
    if let Some(p) = e["params"].as_str() {
        doc.push_str("inputs:\n");
        doc.push_str(&indent(p, 2));
    }
    doc.push_str("command-line: |\n");
    doc.push_str(&indent(e["command"].as_str().unwrap(), 2));
    doc.push_str("x-nils:\n  analysis-level: stack\n  input: {layout: stacks}\n");
    for key in ["inputs", "outputs", "proposals"] {
        if !e[key].is_null() {
            doc.push_str(&format!("  {key}: {}\n", yaml(&e[key])));
        }
    }
    doc
}

/// Record 43's rulings at the runner, over the body-part loop's shape:
/// embeddings kept under the cache's key, seeds apart from proposals and
/// saved as a selection, a head registered as a model from a run-level
/// output (its card, the label set it was given, its encoders), mounted
/// for the run that reads it, proposals staged at the card's threshold,
/// which a run may raise and not lower, and a newer run superseding what
/// the older one left untaken.
#[test]
fn the_body_part_loop_embeds_seeds_trains_a_model_and_proposes_through_the_runner() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-loop");
    for name in ["bp-embed", "bp-seed", "bp-train", "bp-infer"] {
        lab.add_descriptor(name, &stand_in(name));
    }
    let run = |args: &[&str]| -> Value {
        let mut all = vec!["run"];
        all.extend_from_slice(args);
        all.extend_from_slice(&["--select", "selection:every@1", "--json"]);
        lab.json(&all)
    };

    // embed: each file under the key, the encoder registered from its card
    let first = run(&["bp-embed"]);
    assert_eq!(first["status"], "done", "{first}");
    assert_eq!(
        first["summary"]["embeddings"],
        json!({"registered": 4, "kept": 0}),
        "{first}"
    );
    let enc = lab.json(&[
        "model",
        "show",
        &format!("sha256:{}", "e".repeat(64)),
        "--json",
    ]);
    assert_eq!(enc["kind"], "encoder", "{enc}");
    let rows = lab.json(&[
        "derivative",
        "list",
        "--run",
        &first["id"].to_string(),
        "--json",
    ]);
    let rows_of_first = rows.as_array().unwrap().clone();
    for d in rows.as_array().unwrap() {
        assert_eq!(d["kind"], "embedding");
        assert_eq!(d["model_id"], enc["id"]);
        assert_eq!(d["preprocess_version"], "v1");
    }
    // a second run: the key holds every stack, nothing new is registered
    let again = run(&["bp-embed"]);
    assert_eq!(
        again["summary"]["embeddings"],
        json!({"registered": 0, "kept": 4}),
        "{again}"
    );

    // seeds: kept as the run's one derivative of kind seeds, never proposals
    let seeded = run(&["bp-seed"]);
    assert_eq!(seeded["status"], "done", "{seeded}");
    let seeds = &seeded["summary"]["seeds"];
    assert_eq!(
        (&seeds["seeds"], &seeds["outside"], &seeds["selection"]),
        (&json!(3), &json!(1), &json!(2)),
        "{seeded}"
    );
    assert_eq!(seeded["summary"]["proposals"]["given"], 0);
    let listed = lab.json(&[
        "derivative",
        "list",
        "--run",
        &seeded["id"].to_string(),
        "--json",
    ]);
    let row = &listed.as_array().unwrap()[0];
    assert_eq!(
        (&row["kind"], &row["scope"]),
        (&json!("seeds"), &json!("run"))
    );
    assert!(row["subject_id"].is_null(), "{row}");
    let saved = lab.json(&[
        "pipeline",
        "seeds",
        &seeded["id"].to_string(),
        "--save",
        "to-curate",
        "--json",
    ]);
    assert_eq!(saved["per_value"]["body_part=brain"], 3, "{saved}");
    assert_eq!(saved["saved"]["version"], 1, "{saved}");
    let campaign = lab.json(&[
        "campaign",
        "create",
        "curate-seeds",
        "--axis",
        "body_part",
        "--select",
        "selection:to-curate@1",
        "--json",
    ]);
    assert_eq!(
        campaign["items"].as_array().unwrap().len(),
        2,
        "the suggested stacks, and only those: {campaign}"
    );

    // train: the head a run-level output, registered as a model
    let labels = lab.work.path().join("export");
    std::fs::create_dir_all(&labels).unwrap();
    lab.ok(
        &[
            "place",
            "add",
            "exp",
            labels.to_str().unwrap(),
            "--role",
            "export",
        ],
        None,
    );
    let tsv = lab.work.path().join("v0.tsv");
    let mut text = String::from("SeriesInstanceUID\tbody_part\tdate\n");
    // a person's labels on the second subject's stacks, which are then not
    // asked again; the first subject's are the model's to propose
    for root in ["72"] {
        for n in ["1", "2"] {
            text.push_str(&format!(
                "1.2.826.0.1.3680043.8.498.{root}.1.{n}\t{}\t2024-05-06\n",
                if n == "1" { "Brain" } else { "Spine" }
            ));
        }
    }
    std::fs::write(&tsv, text).unwrap();
    let imported = lab.json(&[
        "labels",
        "import-v0",
        "--tsv",
        tsv.to_str().unwrap(),
        "--to",
        labels.join("v0").to_str().unwrap(),
        "--json",
    ]);
    let set = imported["label_set"]["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("{imported}"));
    let set_digest = format!(
        "sha256:{}",
        imported["label_set"]["digest"].as_str().unwrap()
    );
    let lied = run(&["bp-train", "--param", "lie=yes"]);
    assert_eq!(
        lied["status"], "partial",
        "a card that is not the artifact's: {lied}"
    );
    assert!(
        lied["summary"]["refused_files"][0]["why"]
            .as_str()
            .unwrap()
            .contains("the artifact is"),
        "{lied}"
    );
    let trained = run(&["bp-train", "--labels", &set.to_string()]);
    assert_eq!(trained["status"], "done", "{trained}");
    let made = &trained["summary"]["models"][0];
    assert_eq!(made["model"]["state"], "registered", "{trained}");
    assert_eq!(made["trained_on"], set_digest.as_str(), "{trained}");
    assert_eq!(made["encoders"], json!([enc["id"]]), "{trained}");
    let head = lab.json(&["model", "show", "bp-head@hno", "--json"]);
    assert_eq!(head["card"]["trained_on"]["label_set"], set_digest.as_str());
    assert_eq!(head["encoder_model_ids"], json!([enc["id"]]));
    assert_eq!(head["threshold"], 0.8);
    let rows = lab.json(&[
        "derivative",
        "list",
        "--run",
        &trained["id"].to_string(),
        "--json",
    ]);
    let artifact = &rows.as_array().unwrap()[0];
    assert_eq!(
        (&artifact["kind"], &artifact["scope"], &artifact["model_id"]),
        (&json!("model"), &json!("run"), &head["id"])
    );
    let check = lab.work.path().join("check.json");
    std::fs::write(
        &check,
        json!({"suite": "heldout", "passed": true, "checks": [{"name": "ece", "passed": true}]})
            .to_string(),
    )
    .unwrap();
    lab.ok(
        &[
            "model",
            "admit",
            "bp-head@hno",
            "--check",
            check.to_str().unwrap(),
        ],
        None,
    );

    // infer: the head mounted, its proposals staged at its card's threshold
    let (good, _, err) = lab.run(
        &[
            "run",
            "bp-infer",
            "--model",
            "bp-head@hno",
            "--threshold",
            "0.5",
            "--select",
            "selection:every@1",
        ],
        None,
    );
    assert!(!good && err.contains("not lower it"), "{err}");
    let one = run(&["bp-infer", "--model", "bp-head@hno"]);
    assert_eq!(one["status"], "done", "{one}");
    let ingested = &one["summary"]["proposals"]["ingested"];
    assert_eq!(ingested["staged_members"], 2, "{one}");
    assert_eq!(ingested["items"], 2, "{one}");
    assert_eq!(
        ingested["decided"], 2,
        "a person's label is not asked again: {one}"
    );
    let two = run(&["bp-infer", "--model", "bp-head@hno", "--param", "p=0.97"]);
    let ingested = &two["summary"]["proposals"]["ingested"];
    assert_eq!(
        (&ingested["superseded"], &ingested["withdrawn"]),
        (&json!(2), &json!(2)),
        "{two}"
    );
    let raised = run(&[
        "bp-infer",
        "--model",
        "bp-head@hno",
        "--param",
        "p=0.97",
        "--threshold",
        "0.99",
    ]);
    let ingested = &raised["summary"]["proposals"]["ingested"];
    assert_eq!(ingested["staged_members"], 0, "{raised}");
    // record 43 review: a run speaks only for its stacks and its models; a
    // proposal on a stack outside its selection, or by a model it was not
    // given, is dropped and counted
    let curated: Vec<i64> = saved["selection"]["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_i64)
        .collect();
    let every: Vec<i64> = rows_of_first
        .iter()
        .filter_map(|d| d["stack_id"].as_i64())
        .collect();
    let extra = every.iter().find(|s| !curated.contains(s)).unwrap();
    let outside = lab.json(&[
        "run",
        "bp-infer",
        "--model",
        "bp-head@hno",
        "--select",
        "selection:to-curate@1",
        "--param",
        &format!("extra={extra}"),
        "--json",
    ]);
    let ingested = &outside["summary"]["proposals"]["ingested"];
    assert_eq!(ingested["out_of_run"], 2, "{outside}");
    // the words of the train run: no bind reaches the derivatives tree
    let tree = format!("{}/derivatives:", lab.work.path().display());
    assert!(
        !lab.podman_runs()
            .iter()
            .flatten()
            .any(|w| w.starts_with(&tree)),
        "the derivatives tree is not bound"
    );
    let open = lab.json(&["review", "list", "--kind", "body_part:model", "--json"]);
    let items = open["items"].as_array().unwrap();
    // what is live is the newest run's on each stack: the raised run's on
    // the stack the last run did not propose again, and the last run's
    let newest = [&raised["id"], &outside["id"]];
    let live: Vec<&Value> = items
        .iter()
        .filter(|i| i["status"] == "open" || i["status"] == "staged")
        .collect();
    assert_eq!(live.len(), 2, "{open}");
    assert!(
        live.iter().all(|i| newest.contains(&&i["ref"]["run_id"])),
        "{open}"
    );
    assert!(
        items
            .iter()
            .filter(|i| !newest.contains(&&i["ref"]["run_id"]))
            .all(|i| i["status"] == "superseded"),
        "{open}"
    );
}

/// A pipeline that tries what a hostile container could: links planted in
/// its output folder, a unit claiming another unit's file, a run-level
/// file that is a link out, and an embedding that is not a number.
const HOSTILE: &str = r#"name: hostile
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/hostile@sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
inputs:
  - {id: mode, name: Mode, type: String, value-key: "[MODE]", default-value: plain}
  - {id: victim, name: Victim, type: String, value-key: "[VICTIM]", default-value: none}
command-line: |
  python3 -c '
  import json, os, struct, sys
  m = json.load(open(sys.argv[1])); out = sys.argv[2]; mode = sys.argv[3]; victim = sys.argv[4]
  st = m["stacks"]; units = []
  for s in st:
      u = s["unit"]; os.makedirs(os.path.join(out, u), exist_ok=True)
      open(os.path.join(out, u, "out.txt"), "w").write(u)
      units.append({"unit_id": u, "status": "succeeded", "derivatives": [u + "/out.txt"]})
  if mode == "steal":
      units[1]["derivatives"].append(units[0]["unit_id"] + "/out.txt")
  if mode == "alias":
      b = os.path.join(out, units[1]["unit_id"], "out.txt")
      os.remove(b)
      os.symlink(os.path.join("..", units[0]["unit_id"], "out.txt"), b)
  doc = {"schema_version": "1", "units": units, "seeds": [{"stack_id": st[0]["stack_id"], "axis": "body_part", "value": "brain"}]}
  if mode == "seeds-link":
      os.symlink(victim, os.path.join(out, "nils-seeds.json"))
  if mode == "run-link":
      os.makedirs(os.path.join(out, "extra"), exist_ok=True)
      os.symlink(victim, os.path.join(out, "extra", "leak.txt"))
  if mode in ("nan", "foreign"):
      enc = "sha256:" + "e" * 64
      for s in st:
          h = json.dumps({"format": "nils-embedding", "dtype": "<f4", "stack_id": s["stack_id"], "encoder": enc, "preprocess_version": "v1", "rows": 1, "dim": 2, "slices": [0]}).encode()
          start = (12 + len(h) + 63) // 64 * 64
          b = b"NILSEMB1" + struct.pack("<I", len(h)) + h
          b += bytes(start - len(b)) + struct.pack("<2f", float("nan") if mode == "nan" else 0.5, 1.0)
          rel = s["unit"] + "/x.emb"
          open(os.path.join(out, rel), "wb").write(b)
          next(u for u in units if u["unit_id"] == s["unit"])["derivatives"].append(rel)
      doc["models"] = [{"name": "e", "version": "1", "kind": "encoder", "digest": enc, "task": "encoder"}]
  if mode == "results-link":
      os.symlink(victim, os.path.join(out, "results.json"))
  else:
      json.dump(doc, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation] [MODE] [VICTIM]
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  outputs:
    - {id: out, kind: output, path-template: "stack-{stack}/out.txt", media-type: text/plain}
    - {id: emb, kind: embedding, path-template: "stack-{stack}/x.emb"}
    - {id: extra, kind: output, level: run, path-template: "extra/*.txt"}
"#;

/// The review of record 43: the engine never reads or writes through a
/// link a container planted in its output folder, a unit takes only its
/// own files, a run-level file that leads out is refused as a review item
/// and the run goes on, and an embedding is read whole before it is kept.
#[test]
fn a_hostile_output_folder_is_refused_file_by_file_and_never_followed() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-hostile");
    lab.add_descriptor("hostile", HOSTILE);
    let victim = lab.work.path().join("victim.txt");
    std::fs::write(&victim, "the host's own file").unwrap();
    let v = victim.to_str().unwrap();
    let run = |mode: &str, victim: &str| -> Value {
        lab.json(&[
            "run",
            "hostile",
            "--select",
            "selection:every@1",
            "--param",
            &format!("mode={mode}"),
            "--param",
            &format!("victim={victim}"),
            "--json",
        ])
    };

    // a planted nils-seeds.json is never written through
    let seeded = run("seeds-link", v);
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "the host's own file"
    );
    assert_eq!(seeded["summary"]["seeds"]["seeds"], 1, "{seeded}");

    // results.json that is a link out of the folder is not read
    let fake = lab.work.path().join("fake-results.json");
    std::fs::write(&fake, r#"{"units": []}"#).unwrap();
    let linked = run("results-link", fake.to_str().unwrap());
    assert!(
        linked["summary"]["results"]
            .as_str()
            .unwrap()
            .starts_with("unreadable"),
        "{linked}"
    );
    assert_eq!(linked["summary"]["units"]["failed"], 4, "{linked}");
    assert_eq!(linked["status"], "partial", "{linked}");

    // a unit claims only its own files
    let stolen = run("steal", "none");
    assert_eq!(stolen["status"], "partial", "{stolen}");
    assert_eq!(
        stolen["summary"]["derivatives"], 5,
        "four outputs and the seeds: {stolen}"
    );
    let why = &stolen["summary"]["refused_files"][0];
    assert!(
        why["why"]
            .as_str()
            .unwrap()
            .contains("not a file this unit"),
        "{why}"
    );

    // a unit's file that is a link to another unit's is one file, one row
    let alias = run("alias", "none");
    assert_eq!(alias["status"], "partial", "{alias}");
    assert_eq!(
        alias["summary"]["derivatives"], 4,
        "three outputs and the seeds: {alias}"
    );
    assert!(
        alias["summary"]["refused_files"][0]["why"]
            .as_str()
            .unwrap()
            .contains("the same file"),
        "{alias}"
    );

    // a run-level link out is refused as a review item, and the run goes on
    let leaked = run("run-link", v);
    assert_eq!(leaked["status"], "partial", "{leaked}");
    assert_eq!(leaked["summary"]["derivatives"], 5, "{leaked}");
    assert_eq!(
        leaked["summary"]["refused_files"][0]["output"], "extra",
        "{leaked}"
    );
    let items = lab.json(&["review", "list", "--kind", "pipeline:qc", "--json"]);
    assert!(
        items["items"].as_array().unwrap().iter().any(|i| {
            i["ref"]["run_id"] == leaked["id"]
                && i["ref"]["unit"] == "run"
                && i["evidence"]["status"] == "refused"
        }),
        "{items}"
    );

    // an embedding holding a value that is not a number is refused whole
    let nan = run("nan", "none");
    assert_eq!(nan["summary"]["embeddings"]["registered"], 0, "{nan}");
    assert_eq!(nan["status"], "partial", "{nan}");

    // an embedding by an encoder the run was neither given nor declares
    let foreign = run("foreign", "none");
    assert_eq!(
        foreign["summary"]["embeddings"]["registered"], 0,
        "{foreign}"
    );
    assert!(
        foreign["summary"]["refused_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["why"]
                .as_str()
                .unwrap_or("")
                .contains("neither given nor declares")),
        "{foreign}"
    );

    // no registered path is a link: each row names the file where it is
    let rows = lab.json(&["derivative", "list", "--json"]);
    for d in rows.as_array().unwrap() {
        let file = lab.work.path().join(d["path"].as_str().unwrap());
        assert!(
            !std::fs::symlink_metadata(&file)
                .unwrap()
                .file_type()
                .is_symlink(),
            "{d}"
        );
    }
}

/// The second review of record 43: a folder whose name holds a ':' is
/// bound through its parent rather than failing the run, and files lying
/// directly in the source root bind that root, which the run's scope says.
#[test]
fn odd_folder_names_and_files_at_the_root_are_bound_and_said() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let src = tree_at(|patient, n, slice| match (patient, n) {
        ("P1", "1") => format!("P1/t1:mprage/{slice}"),
        ("P1", _) => format!("flair-{slice}"),
        _ => format!("{patient}/{n}/{slice}"),
    });
    let lab = Lab::with_tree("pipelines-odd", src);
    lab.add_descriptor(
        "stack-echo",
        &stack_echo(&format!("example.org/stack-echo@sha256:{}", "a".repeat(64))),
    );
    let v = lab.json(&[
        "run",
        "stack-echo",
        "--select",
        "selection:every@1",
        "--json",
    ]);
    assert_eq!(v["status"], "done", "{v}");
    let scope = &v["summary"]["scope"];
    assert_eq!(
        (&scope["widened"], &scope["roots"]),
        (&json!(1), &json!(1)),
        "{v}"
    );
    let words = lab.podman_runs().pop().unwrap();
    assert!(!words.iter().any(|w| w.contains("t1:mprage")), "{words:?}");
}

// ------------------------------------------------------------ record 49

/// A stand-in for apptainer, as the lane runs it (record 49 A2): it answers
/// `--version`, keeps the words of every call, makes the SIF file or the
/// sandbox folder a `build` names, and runs a `run`'s command on the host
/// with every container path written as its host folder, once it has
/// checked the local image it was given is there. It does not clear the
/// environment as `--cleanenv` does; the words say that it was asked to.
const FAKE_APPTAINER: &str = r#"#!/usr/bin/env python3
import json, os, re, subprocess, sys
a = sys.argv[1:]
if not a or a[0] == "--version":
    print("apptainer version 1.3.4"); sys.exit(0)
with open(os.environ["FAKE_APPTAINER_ARGS"], "a") as log:
    log.write(json.dumps(a) + "\n")
if a[0] == "build":
    target, source = a[-2], a[-1]
    if "--sandbox" in a:
        os.makedirs(target); open(os.path.join(target, "source"), "w").write(source)
    else:
        open(target, "w").write("SIF " + source)
    sys.exit(0)
if a[0] != "run":
    sys.exit(0)
mounts, env, i = {}, {}, 1
while i < len(a):
    w = a[i]
    if w == "--bind":
        host, ctr = a[i + 1].split(":")[:2]; mounts[ctr] = host; i += 2
    elif w == "--env":
        k, v = a[i + 1].split("=", 1); env[k] = v; i += 2
    elif w in ("--network", "--cpus", "--memory"):
        i += 2
    elif w.startswith("--"):
        i += 1
    else:
        break
image = a[i]; i += 1
if not os.path.exists(image):
    print("no image at " + image, file=sys.stderr); sys.exit(255)
keys = sorted(mounts, key=len, reverse=True)
pattern = re.compile("(" + "|".join(re.escape(k) for k in keys) + r")(?=/|$|[^A-Za-z0-9_])")
argv = [pattern.sub(lambda m: mounts[m.group(1)], w) for w in a[i:]]
env = {k: pattern.sub(lambda m: mounts[m.group(1)], v) for k, v in env.items()}
sys.exit(subprocess.call(argv, env=dict(os.environ, **env)))
"#;

/// A stand-in for `nvidia-smi` (no card is touched on a laptop that has
/// one): it names a card, answers a card's free memory from a file the test
/// writes, and keeps each question it was asked.
const FAKE_SMI: &str = r#"#!/bin/sh
echo "$*" >> "$FAKE_GPU_ASKED"
case "$1" in
  --query-gpu=name) echo "Stand-in Card"; exit 0 ;;
  --query-gpu=memory.free) echo "$(cat "$FAKE_GPU_FREE") MiB"; exit 0 ;;
esac
exit 1
"#;

/// A pipeline of the stacks layout whose units run apart (record 49 A1):
/// each unit sees its own stack alone, notes when it starts and ends in the
/// file `LANE_TRACE` names, sleeps, and writes one file.
fn stack_slow(name: &str, needs: &str, extra: &str) -> String {
    format!(
        r#"name: {name}
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/{name}@sha256:{hex}"
inputs:
  - id: sleep
    name: Seconds a unit takes
    type: Number
    value-key: "[SLEEP]"
    default-value: 1
command-line: |
  python3 -c '
  import json, os, sys, time
  m = json.load(open(sys.argv[1])); out = sys.argv[2]
  assert len(m["stacks"]) == 1, "a unit that runs apart sees its own stack"
  man = json.load(open(sys.argv[4] + "/manifest.json"))
  assert len(man["units"]) == 1 and man["unit"] == m["stacks"][0]["unit"]
  def mark(w):
      if not os.environ.get("LANE_TRACE"):
          return
      with open(os.environ["LANE_TRACE"], "a") as f:
          f.write("%s %.6f %s %s %s\n" % (w, time.time(), os.environ["NILS_UNIT"], os.environ.get("CUDA_VISIBLE_DEVICES", "-"), os.environ["NILS_CORES"]))
  mark("start")
  time.sleep(float(sys.argv[3]))
  s = m["stacks"][0]; u = s["unit"]; d = os.path.join(out, u); os.makedirs(d, exist_ok=True)
  open(os.path.join(d, "out.txt"), "w").write("stack %d\n" % s["stack_id"])
  json.dump({{"schema_version": "1", "units": [{{"unit_id": u, "status": "succeeded", "derivatives": [u + "/out.txt"]}}]}}, open(os.path.join(out, "results.json"), "w"))
  mark("end")
  ' [Manifest] [OutputLocation] [SLEEP] [Inputs]
x-nils:
  analysis-level: stack
  input: {{layout: stacks}}
  units: apart
  outputs:
    - id: slow
      kind: output
      path-template: "stack-{{stack}}/out.txt"
      media-type: text/plain
  needs: {needs}
{extra}"#,
        hex = "c".repeat(64),
    )
}

/// The trace a lane's units left: each start and end, in time order.
fn trace(path: &Path) -> Vec<(String, f64, String, String)> {
    let mut events: Vec<(String, f64, String, String)> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            Some((
                w[0].to_string(),
                w[1].parse().ok()?,
                w[2].to_string(),
                w[3].to_string(),
            ))
        })
        .collect();
    events.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    events
}

/// The most units that ran at once, by the trace.
fn most_at_once(path: &Path) -> usize {
    let (mut now, mut most) = (0usize, 0usize);
    for (w, ..) in trace(path) {
        if w == "start" {
            now += 1;
            most = most.max(now);
        } else {
            now -= 1;
        }
    }
    most
}

impl Lab {
    fn traced(&self, trace: &Path, args: &[&str]) -> (bool, String, String) {
        let child = self
            .command(&self.path)
            .args(args)
            .env("LANE_TRACE", trace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn count(&self, sql: &str) -> i64 {
        self.store().query(sql, &[]).unwrap()[0].int(0).unwrap()
    }
}

/// Record 49 A1: a run whose units run apart runs them side by side, each
/// in a container of its own that sees its own stack alone, as many at once
/// as the lane's cores and memory allow and never more; a unit that could
/// never fit is refused before anything runs.
#[test]
fn units_apart_fill_the_lane_s_budget_and_never_pass_it() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-lane-budget");
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 2, memory-gb: 1}", ""));
    let t = lab.work.path().join("trace-cores");
    // four cores: two units of two cores at once
    lab.ok(
        &["pipeline", "lane", "--cores", "4", "--memory-gb", "100"],
        None,
    );
    let (ok, out, err) = lab.traced(
        &t,
        &[
            "run",
            "slow",
            "--select",
            "selection:every@1",
            "--param",
            "sleep=1.2",
            "--json",
        ],
    );
    assert!(ok, "{err}");
    let run: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(run["status"], "done", "{run}");
    assert_eq!(run["units"], "apart");
    assert_eq!(run["summary"]["units"]["succeeded"], 4, "{run}");
    assert_eq!(run["derivatives"].as_array().unwrap().len(), 4, "{run}");
    assert_eq!(run["summary"]["lane"]["cores"], 4, "{run}");
    assert_eq!(run["unit_states"]["over"], 4, "{run}");
    assert_eq!(trace(&t).len(), 8);
    assert_eq!(most_at_once(&t), 2, "{:?}", trace(&t));
    // each unit had a container of its own, told its unit and its cores
    let runs = lab.podman_runs();
    assert_eq!(runs.len(), 4);
    for words in &runs {
        assert!(
            words.iter().any(|w| w.starts_with("NILS_UNIT=stack-")),
            "{words:?}"
        );
        assert!(words.iter().any(|w| w == "NILS_CORES=2"), "{words:?}");
    }

    // memory binds as cores do: three units of 1 GB in a lane of 3 GB,
    // each of one core, so the cores never bind first on a machine of 4
    // (the lane's cores are never more than the machine offers)
    lab.add_descriptor(
        "slow-one",
        &stack_slow("slow-one", "{cores: 1, memory-gb: 1}", ""),
    );
    let t = lab.work.path().join("trace-memory");
    lab.ok(
        &["pipeline", "lane", "--cores", "64", "--memory-gb", "3"],
        None,
    );
    let (ok, _, err) = lab.traced(
        &t,
        &[
            "run",
            "slow-one",
            "--select",
            "selection:every@1",
            "--param",
            "sleep=1.2",
        ],
    );
    assert!(ok, "{err}");
    assert_eq!(most_at_once(&t), 3, "{:?}", trace(&t));

    // a unit that could never fit is refused, and nothing runs
    lab.ok(&["pipeline", "lane", "--cores", "1"], None);
    let (ok, _, err) = lab.run(&["run", "slow", "--select", "selection:every@1"], None);
    assert!(!ok);
    assert!(
        err.contains("asks 2 cores") && err.contains("nils pipeline lane"),
        "{err}"
    );
    assert_eq!(lab.podman_runs().len(), 8);
    let lane: Value = serde_json::from_str(&lab.ok(&["pipeline", "lane", "--json"], None)).unwrap();
    assert_eq!(lane["cores"], 1, "{lane}");
    assert_eq!(lane["set"]["memory_gb"], 3, "{lane}");
}

/// Record 49 A1: a run whose engine is killed with units in flight is taken
/// up where it stopped. The units it finished are kept with their
/// derivatives and never run again; those in flight run again from a clean
/// folder. The pipeline lane's worker finds the run by itself and queues it
/// under what its job recorded; a run finished cannot be resumed.
#[test]
fn a_killed_run_is_taken_up_where_it_stopped() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-lane-resume");
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 1, memory-gb: 1}", ""));
    lab.ok(
        &["pipeline", "lane", "--cores", "2", "--memory-gb", "100"],
        None,
    );
    let t = lab.work.path().join("trace");
    let mut child = {
        use std::os::unix::process::CommandExt;
        lab.command(&lab.path)
            .args([
                "run",
                "slow",
                "--select",
                "selection:every@1",
                "--param",
                "sleep=2",
            ])
            .env("LANE_TRACE", &t)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // a group of its own, so the kill takes its containers with it
            .process_group(0)
            .spawn()
            .unwrap()
    };
    // the first two units end, the next two start; then the engine dies
    let started = std::time::Instant::now();
    loop {
        let over = lab.count("SELECT COUNT(*) FROM pipeline_unit WHERE state = 'over'");
        let marked = trace(&t).iter().filter(|e| e.0 == "start").count();
        if over >= 2 && marked >= 3 {
            break;
        }
        assert!(
            started.elapsed().as_secs() < 60,
            "the run never got half way"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let pgid = child.id();
    Command::new("kill")
        .args(["-KILL", &format!("-{pgid}")])
        .status()
        .unwrap();
    let _ = child.wait();
    let over_before: Vec<String> = lab
        .store()
        .query(
            "SELECT unit FROM pipeline_unit WHERE state = 'over' ORDER BY unit",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
    assert!(
        over_before.len() >= 2 && over_before.len() < 4,
        "{over_before:?}"
    );
    assert_eq!(
        lab.count("SELECT COUNT(*) FROM pipeline_run WHERE status = 'running'"),
        1,
        "a killed engine leaves its run saying running"
    );
    let derivatives_before = lab.count("SELECT COUNT(*) FROM derivative");
    assert_eq!(derivatives_before, over_before.len() as i64);

    // the pipeline lane's worker takes it up again by itself
    let mut worker = lab.command(&lab.path);
    worker
        .args(["jobs", "work", "--lane", "pipelines", "--once"])
        .env("LANE_TRACE", &t);
    let out = worker.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run: Value =
        serde_json::from_str(&lab.ok(&["pipeline", "runs", "1", "--json"], None)).unwrap();
    assert_eq!(run["status"], "done", "{run}");
    assert_eq!(run["resumes"], 1, "{run}");
    assert_eq!(run["summary"]["resumed"], true, "{run}");
    assert_eq!(run["summary"]["units"]["succeeded"], 4, "{run}");
    assert_eq!(lab.count("SELECT COUNT(*) FROM derivative"), 4);
    assert_eq!(
        lab.count("SELECT COUNT(DISTINCT path) FROM derivative"),
        4,
        "no unit registered twice"
    );
    // a unit over before the kill never started again
    let events = trace(&t);
    for unit in &over_before {
        let starts = events
            .iter()
            .filter(|e| e.0 == "start" && &e.2 == unit)
            .count();
        assert_eq!(starts, 1, "{unit} ran again: {events:?}");
    }
    // every unit ended once at least, and the ones in flight twice started
    let starts = events.iter().filter(|e| e.0 == "start").count();
    assert!(starts > 4 && starts <= 6, "{events:?}");
    let ends = events.iter().filter(|e| e.0 == "end").count();
    assert_eq!(ends, 4, "a killed unit never ended: {events:?}");
    let attempts: i64 = lab.count("SELECT SUM(attempts) FROM pipeline_unit");
    assert_eq!(attempts as usize, starts, "{events:?}");
    // the job that took it up ran as the one the killed run recorded
    let jobs: Value =
        serde_json::from_str(&lab.ok(&["jobs", "list", "--all", "--json"], None)).unwrap();
    let resumed = jobs
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["args"]["resume"] == 1)
        .unwrap_or_else(|| panic!("{jobs}"));
    assert_eq!(resumed["state"], "done", "{resumed}");
    assert_eq!(resumed["kind"], "pipeline", "{resumed}");
    // a run that is done has nothing left to run
    let (ok, _, err) = lab.run(&["run", "--resume", "1"], None);
    assert!(!ok && err.contains("nothing is left to run"), "{err}");
}

/// Record 49 A1, the proof: a long run and a digest go on together. The
/// pipeline lane runs the run while the main lane runs a digest queued after
/// it, which ends first.
#[test]
fn a_long_run_and_a_digest_go_on_together() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-lane-digest");
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 1, memory-gb: 1}", ""));
    // one unit at a time: four units of three seconds each
    lab.ok(&["pipeline", "lane", "--cores", "1"], None);
    let src = format!("src={}", lab._src.path().display());
    let server = Server::start_with(&lab, &["--ingest-root", &src]);
    let (status, doc) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["run", "slow", "--select", "selection:every@1", "--param", "sleep=3"]})),
        OPERATOR,
    );
    assert_eq!(status, 202, "{doc}");
    let run_job = doc["job"].as_i64().unwrap();
    let job = |id: i64| {
        server
            .call("GET", &format!("/api/jobs/{id}"), None, OPERATOR)
            .1
    };
    let started = std::time::Instant::now();
    while job(run_job)["progress"]["running"].as_u64() != Some(1) {
        assert!(started.elapsed().as_secs() < 60, "{}", job(run_job));
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let (status, doc) = server.call(
        "POST",
        "/api/jobs",
        Some(json!({"command": ["digest", "@src"]})),
        OPERATOR,
    );
    assert_eq!(status, 202, "{doc}");
    let digest_job = doc["job"].as_i64().unwrap();
    let mut digest = Value::Null;
    for _ in 0..600 {
        digest = job(digest_job);
        if matches!(
            digest["state"].as_str(),
            Some("done" | "failed" | "cancelled")
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(digest["state"], "done", "{digest}");
    // the run is still going when the digest queued after it is done
    let during = job(run_job);
    assert_eq!(during["state"], "running", "{during}");
    assert!(during["progress"]["over"].as_u64().unwrap() < 4, "{during}");
    let mut state = Value::Null;
    for _ in 0..900 {
        state = job(run_job);
        if matches!(
            state["state"].as_str(),
            Some("done" | "failed" | "cancelled")
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(state["state"], "done", "{state}");
    // the two lanes' workers, each a row of its own kind
    let (_, all) = server.call("GET", "/api/jobs?all=1", None, OPERATOR);
    let text = all.to_string();
    assert!(text.contains("\"pipeline-worker\""), "{all}");
}

/// Record 49 A2: under apptainer the image is built once from its pinned
/// digest and kept by it; a GPU unit waits while the card's free memory is
/// below its need and each holds a lease on the named card, so two units
/// that would not fit together never run at once. No card is used: a
/// stand-in nvidia-smi answers.
#[test]
fn apptainer_runs_a_built_image_and_a_gpu_unit_waits_for_its_lease() {
    if !have("python3") {
        eprintln!("python3 is not installed; the stand-ins need it, so this test is skipped");
        return;
    }
    let lab = Lab::new("pipelines-lane-gpu");
    let fake = lab.bin.file("apptainer", FAKE_APPTAINER.as_bytes());
    let smi = lab.bin.file("nvidia-smi", FAKE_SMI.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for f in [&fake, &smi] {
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let free = lab.work.path().join("gpu-free");
    std::fs::write(&free, "2000").unwrap();
    let asked = lab.work.path().join("gpu-asked");
    let apptainer_args = lab.bin.path().join("apptainer.log");
    let t = lab.work.path().join("trace");
    lab.add_descriptor(
        "gpu-slow",
        &stack_slow(
            "gpu-slow",
            "{gpu: required, gpu-memory-gb: 4, cores: 1, memory-gb: 1}",
            "",
        ),
    );
    lab.ok(&["pipeline", "runtime", "--set", "apptainer"], None);
    lab.ok(
        &[
            "pipeline",
            "lane",
            "--cores",
            "8",
            "--memory-gb",
            "100",
            "--gpu-card",
            "1",
        ],
        None,
    );
    let child = lab
        .command(&lab.path)
        .args([
            "run",
            "gpu-slow",
            "--select",
            "selection:every@1",
            "--param",
            "sleep=0.5",
            "--json",
        ])
        .env("LANE_TRACE", &t)
        .env("FAKE_APPTAINER_ARGS", &apptainer_args)
        .env("FAKE_GPU_FREE", &free)
        .env("FAKE_GPU_ASKED", &asked)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // 2000 MiB free and 4096 needed: the units wait, and say why
    let started = std::time::Instant::now();
    loop {
        let progress: String = lab
            .store()
            .query("SELECT progress FROM job WHERE kind = 'pipeline'", &[])
            .unwrap()
            .first()
            .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
            .unwrap_or_default();
        if progress.contains("card 1: 2000 MiB free") {
            break;
        }
        assert!(started.elapsed().as_secs() < 60, "{progress}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(trace(&t).is_empty(), "a unit started without its lease");
    // room for one unit's need, not two: one lease at a time
    std::fs::write(&free, "6000").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(run["status"], "done", "{run}");
    assert_eq!(run["runtime"], "apptainer");
    assert_eq!(run["device"], "cuda:Stand-in Card");
    assert_eq!(most_at_once(&t), 1, "{:?}", trace(&t));
    assert!(
        trace(&t).iter().all(|e| e.3 == "1"),
        "the leased card alone: {:?}",
        trace(&t)
    );
    for u in run["units_run"].as_array().unwrap() {
        assert_eq!(u["gpu_card"], 1, "{u}");
    }
    let questions = std::fs::read_to_string(&asked).unwrap();
    assert!(
        questions.contains("--query-gpu=memory.free --format=csv,noheader -i 1"),
        "{questions}"
    );
    // the image: built once from the pinned digest, run from where it is kept
    let calls: Vec<Vec<String>> = std::fs::read_to_string(&apptainer_args)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let builds: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "build").collect();
    assert_eq!(builds.len(), 1, "{calls:?}");
    let sif = lab
        .work
        .path()
        .join("images")
        .join(format!("{}.sif", "c".repeat(64)));
    assert!(sif.is_file());
    assert_eq!(
        builds[0].last().unwrap(),
        &format!("docker://example.org/gpu-slow@sha256:{}", "c".repeat(64))
    );
    let execs: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "run").collect();
    assert_eq!(execs.len(), 4);
    let work = lab.work.path().canonicalize().unwrap();
    for e in &execs {
        assert_eq!(
            &e[..7],
            [
                "run",
                "--containall",
                "--cleanenv",
                "--no-home",
                "--net",
                "--network",
                "none"
            ]
        );
        assert!(e.iter().any(|w| w == "--nv"), "{e:?}");
        assert!(pair(e, "--env", "CUDA_VISIBLE_DEVICES=1"), "{e:?}");
        assert!(e.iter().any(|w| w == sif.to_str().unwrap()), "{e:?}");
        // bound: the unit's own input, inputs and output, and the folders
        // of its stack's files, nothing wider
        let binds: Vec<&String> = e
            .windows(2)
            .filter(|w| w[0] == "--bind")
            .map(|w| &w[1])
            .collect();
        for b in &binds {
            let host = Path::new(b.split(':').next().unwrap());
            let host = host.canonicalize().unwrap_or(host.to_path_buf());
            let unit_side = host.starts_with(work.join("runs/1/units"))
                || host
                    .to_string_lossy()
                    .starts_with(&format!("{}/derivatives/gpu-slow/1/stack-", work.display()));
            let source = host.starts_with(lab._src.path().canonicalize().unwrap());
            assert!(unit_side || source, "{b} is wider than the unit");
        }
        assert!(binds.iter().any(|b| b.ends_with(":/input:ro")));
    }
    // a second run finds the image kept and builds nothing
    std::fs::write(&free, "99999").unwrap();
    let mut again = lab.command(&lab.path);
    again
        .args([
            "run",
            "gpu-slow",
            "--select",
            "selection:every@1",
            "--param",
            "sleep=0",
        ])
        .env("LANE_TRACE", &t)
        .env("FAKE_APPTAINER_ARGS", &apptainer_args)
        .env("FAKE_GPU_FREE", &free)
        .env("FAKE_GPU_ASKED", &asked);
    assert!(again.output().unwrap().status.success());
    let builds = std::fs::read_to_string(&apptainer_args)
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("[\"build\""))
        .count();
    assert_eq!(builds, 1);
    // with no card named, a unit that needs one is refused before it runs
    lab.ok(&["pipeline", "lane", "--gpu-card", "none"], None);
    let mut refused = lab.command(&lab.path);
    refused
        .args(["run", "gpu-slow", "--select", "selection:every@1"])
        .env("FAKE_APPTAINER_ARGS", &apptainer_args)
        .env("FAKE_GPU_ASKED", &asked);
    let out = refused.output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("uses no card"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Record 49 R3: a secret the site sets is mounted read-only into the
/// containers of the pipeline that declares it, and only those; it is never
/// in an output, a log, the run's record or its results, even when the
/// pipeline prints it, writes it into a file and reports it. A grep of every
/// file under the working place, the registry and what the engine printed
/// finds none of it.
#[test]
fn a_secret_is_mounted_for_its_run_alone_and_found_nowhere_after() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-lane-secret");
    const TOKEN: &str = "NILS-PLANTED-SECRET-7f3a9c1e2b";
    const SECOND: &str = "Xk2vQ9pLm4sT8wZ1";
    let secret_dir = TempDir::new("pipelines-secret-home");
    let licence = secret_dir.file("license.txt", format!("{TOKEN}\n{SECOND}\n").as_bytes());
    let leak = format!(
        r#"name: leak
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/leak@sha256:{hex}"
command-line: |
  python3 -c '
  import json, os, sys
  m = json.load(open(sys.argv[1])); out = sys.argv[2]
  lic = open(os.environ["FS_LICENSE"]).read()
  print("the licence reads: " + lic)
  print("a line of it: " + lic.splitlines()[1], file=sys.stderr)
  s = m["stacks"][0]; u = s["unit"]; d = os.path.join(out, u); os.makedirs(d, exist_ok=True)
  open(os.path.join(d, "out.txt"), "w").write("stack %d\n" % s["stack_id"])
  open(os.path.join(d, "copy.txt"), "w").write(lic)
  json.dump({{"schema_version": "1", "units": [{{"unit_id": u, "status": "succeeded", "derivatives": [u + "/out.txt", u + "/copy.txt"], "metrics": {{"licence": lic.strip()}}}}]}}, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation]
x-nils:
  analysis-level: stack
  input: {{layout: stacks}}
  units: apart
  secrets:
    - id: freesurfer_license
      env: FS_LICENSE
  outputs:
    - id: out
      kind: output
      path-template: "stack-{{stack}}/*.txt"
      media-type: text/plain
"#,
        hex = "d".repeat(64)
    );
    lab.add_descriptor("leak", &leak);
    // not set: the run is refused before anything runs, and says the cure
    let (ok, _, err) = lab.run(&["run", "leak", "--select", "selection:every@1"], None);
    assert!(!ok);
    assert!(
        err.contains("nils pipeline secret set freesurfer_license"),
        "{err}"
    );
    lab.ok(
        &[
            "pipeline",
            "secret",
            "set",
            "freesurfer_license",
            "--file",
            licence.to_str().unwrap(),
        ],
        None,
    );
    let listed: Value =
        serde_json::from_str(&lab.ok(&["pipeline", "secret", "list", "--json"], None)).unwrap();
    assert_eq!(
        listed,
        json!([{"id": "freesurfer_license", "readable": true}])
    );
    let (ok, out, err) = lab.run(
        &["run", "leak", "--select", "selection:every@1", "--json"],
        None,
    );
    assert!(ok, "{err}");
    let run: Value = serde_json::from_str(&out).unwrap();
    // each copy was removed and refused; the clean file of each unit stands
    assert_eq!(run["status"], "partial", "{run}");
    assert_eq!(run["derivatives"].as_array().unwrap().len(), 4, "{run}");
    assert_eq!(run["summary"]["secrets"], json!(["freesurfer_license"]));
    let refused = run["summary"]["refused_files"].as_array().unwrap();
    assert_eq!(
        refused
            .iter()
            .filter(|r| r["why"]
                .as_str()
                .unwrap_or("")
                .contains("held a secret input"))
            .count(),
        4,
        "{run}"
    );
    // mounted read-only for this pipeline's containers
    let mount = format!("{}:/secrets/freesurfer_license:ro", licence.display());
    let runs = lab.podman_runs();
    assert_eq!(runs.len(), 4);
    assert!(runs.iter().all(|w| pair(w, "--volume", &mount)), "{runs:?}");
    assert!(
        runs.iter()
            .all(|w| pair(w, "--env", "FS_LICENSE=/secrets/freesurfer_license"))
    );
    // and never for a pipeline that does not declare it
    lab.add_descriptor(
        "stack-echo",
        &stack_echo(&format!("example.org/stack-echo@sha256:{}", "a".repeat(64))),
    );
    lab.ok(
        &["run", "stack-echo", "--select", "selection:every@1"],
        None,
    );
    let runs = lab.podman_runs();
    assert!(
        !runs[4].iter().any(|w| w.contains("secrets")),
        "{:?}",
        runs[4]
    );
    // the logs say where it was, never what
    let log =
        std::fs::read_to_string(lab.work.path().join("runs/1/units/stack-1/log.txt")).unwrap();
    assert!(log.contains("[secret freesurfer_license]"), "{log}");

    // nothing anywhere holds a byte of it: every file the working place and
    // the registry hold, and what the engine printed
    let mut files = Vec::new();
    let mut stack = vec![lab.work.path().to_path_buf(), lab.home.path().to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let kind = e.file_type().unwrap();
            if kind.is_dir() {
                stack.push(e.path());
            } else if kind.is_file() {
                files.push(e.path());
            }
        }
    }
    assert!(files.len() > 20, "{}", files.len());
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        for needle in [TOKEN, SECOND] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{} holds the secret",
                f.display()
            );
        }
    }
    for text in [&out, &err] {
        assert!(!text.contains(TOKEN) && !text.contains(SECOND));
    }
    // the rows say so too, read through the store
    let mut store = lab.store();
    for (table, cols) in [
        ("pipeline_run", "summary || COALESCE(error, '') || params"),
        ("pipeline_unit", "COALESCE(outcome, '')"),
        ("review_item", "*"),
        (
            "job",
            "COALESCE(args, '') || COALESCE(result, '') || COALESCE(progress, '') || COALESCE(error, '')",
        ),
        ("audit", "*"),
    ] {
        let sql = if cols == "*" {
            format!("SELECT * FROM {table}")
        } else {
            format!("SELECT {cols} FROM {table}")
        };
        let rows = store.query(&sql, &[]).unwrap();
        for r in rows {
            for cell in &r.0 {
                if let nils_registry::store::Cell::Text(t) = cell {
                    assert!(!t.contains(TOKEN) && !t.contains(SECOND), "{table}: {t}");
                }
            }
        }
    }
}

/// Record 49 R3, after review: a run cancelled while its units hold the
/// secret in their logs and outputs leaves none of it behind; the units in
/// flight are swept as the ones that ended are.
#[test]
fn a_cancelled_run_leaves_no_secret_behind() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-secret-cancel");
    const TOKEN: &str = "NILS-PLANTED-SECRET-c4nc3l-9d2e";
    let secret_dir = TempDir::new("pipelines-secret-cancel-home");
    let licence = secret_dir.file("license.txt", format!("{TOKEN}\n").as_bytes());
    let leak = format!(
        r#"name: leak-slow
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/leak-slow@sha256:{hex}"
command-line: |
  python3 -c '
  import json, os, sys, time
  m = json.load(open(sys.argv[1])); out = sys.argv[2]
  lic = open(os.environ["FS_LICENSE"]).read()
  print("the licence reads: " + lic, flush=True)
  s = m["stacks"][0]; u = s["unit"]; d = os.path.join(out, u); os.makedirs(d, exist_ok=True)
  open(os.path.join(d, "copy.txt"), "w").write(lic)
  with open(os.environ["LANE_TRACE"], "a") as f:
      f.write("start %.6f %s - 1\n" % (time.time(), u))
  time.sleep(60)
  ' [Manifest] [OutputLocation]
x-nils:
  analysis-level: stack
  input: {{layout: stacks}}
  units: apart
  secrets:
    - id: freesurfer_license
      env: FS_LICENSE
  outputs:
    - id: out
      kind: output
      path-template: "stack-{{stack}}/*.txt"
      media-type: text/plain
  needs: {{cores: 1, memory-gb: 1}}
"#,
        hex = "e".repeat(64)
    );
    lab.add_descriptor("leak-slow", &leak);
    lab.ok(
        &[
            "pipeline",
            "secret",
            "set",
            "freesurfer_license",
            "--file",
            licence.to_str().unwrap(),
        ],
        None,
    );
    lab.ok(&["pipeline", "lane", "--cores", "2"], None);
    let t = lab.work.path().join("trace");
    let child = lab
        .command(&lab.path)
        .args(["run", "leak-slow", "--select", "selection:every@1"])
        .env("LANE_TRACE", &t)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while trace(&t).len() < 2 {
        assert!(started.elapsed().as_secs() < 60, "{:?}", trace(&t));
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let job = lab.count("SELECT job_id FROM pipeline_run WHERE id = 1");
    lab.ok(&["jobs", "cancel", &job.to_string()], None);
    let _ = child.wait_with_output().unwrap();
    let mut stack = vec![lab.work.path().to_path_buf()];
    let mut seen = 0;
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let kind = e.file_type().unwrap();
            if kind.is_dir() {
                stack.push(e.path());
            } else if kind.is_file() && e.file_name() != "trace" {
                seen += 1;
                let bytes = std::fs::read(e.path()).unwrap();
                assert!(
                    !bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()),
                    "{} holds the secret",
                    e.path().display()
                );
            }
        }
    }
    assert!(seen > 3, "{seen}");
}

/// Record 49 A1: a person's cancel stops the units in flight and closes the
/// run as cancelled, with the units it finished kept; `nils run --resume`
/// goes on from there.
#[test]
fn a_cancelled_run_keeps_its_units_and_goes_on() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-lane-cancel");
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 1, memory-gb: 1}", ""));
    lab.ok(&["pipeline", "lane", "--cores", "1"], None);
    let t = lab.work.path().join("trace");
    let child = lab
        .command(&lab.path)
        .args([
            "run",
            "slow",
            "--select",
            "selection:every@1",
            "--param",
            "sleep=1",
        ])
        .env("LANE_TRACE", &t)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while lab.count("SELECT COUNT(*) FROM pipeline_unit WHERE state = 'over'") < 1 {
        assert!(started.elapsed().as_secs() < 60);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let job = lab.count("SELECT job_id FROM pipeline_run WHERE id = 1");
    lab.ok(&["jobs", "cancel", &job.to_string()], None);
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("nils run --resume 1"), "{err}");
    let run: Value =
        serde_json::from_str(&lab.ok(&["pipeline", "runs", "1", "--json"], None)).unwrap();
    assert_eq!(run["status"], "cancelled", "{run}");
    let over = run["unit_states"]["over"].as_u64().unwrap();
    assert!((1..4).contains(&over), "{run}");
    assert!(
        run["unit_states"].get("running").is_none(),
        "none left in flight: {run}"
    );
    // taken up again by hand: the rest run, and nothing twice
    let (ok, out, err) = lab.traced(&t, &["run", "--resume", "1", "--json"]);
    assert!(ok, "{err}");
    let run: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(run["status"], "done", "{run}");
    assert_eq!(run["summary"]["units"]["succeeded"], 4, "{run}");
    assert_eq!(lab.count("SELECT COUNT(DISTINCT path) FROM derivative"), 4);
    assert_eq!(lab.count("SELECT COUNT(*) FROM derivative"), 4);
    let ends = trace(&t).iter().filter(|e| e.0 == "end").count();
    assert_eq!(ends, 4, "{:?}", trace(&t));
}

/// Record 49 A3: a pipeline of the stacks layout that writes one table a
/// stack, a volume and a site, and reports an SNR among its metrics, one
/// stack's (`low`) under the declared check.
const VOLUMES: &str = r#"name: volumes
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/volumes@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
inputs:
  - id: low
    name: The stack whose SNR is low
    type: Number
    value-key: "[LOW]"
    default-value: 0
    integer: true
  - id: scale
    name: What each volume is multiplied by
    type: Number
    value-key: "[SCALE]"
    default-value: 1000
command-line: |
  python3 -c '
  import json, os, sys
  m = json.load(open(sys.argv[1])); out = sys.argv[2]; low = int(sys.argv[3]); scale = float(sys.argv[4])
  units = []
  for s in m["stacks"]:
      u = s["unit"]; d = os.path.join(out, u); os.makedirs(d, exist_ok=True)
      open(os.path.join(d, "volumes.csv"), "w").write("Brain Volume,Site\n%s,lab\n" % (scale * s["stack_id"]))
      snr = 3.5 if s["stack_id"] == low else 20
      units.append({"unit_id": u, "status": "succeeded", "derivatives": [u + "/volumes.csv"], "metrics": {"snr": snr}})
  json.dump({"schema_version": "1", "units": units}, open(os.path.join(out, "results.json"), "w"))
  ' [Manifest] [OutputLocation] [LOW] [SCALE]
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  outputs:
    - id: volumes
      kind: table
      path-template: "stack-{stack}/volumes.csv"
      columns:
        - {name: brain_volume, unit: mm3}
        - {name: site, type: text}
  qc: ["snr >= 8", "brain_volume <= 100000000"]
  needs: {unit-minutes: 1}
"#;

/// The rows of an exported handle, each a map from its header, the columns
/// the document asked for under their paths.
fn exported(lab: &Lab, handle: &Value) -> Vec<std::collections::BTreeMap<String, String>> {
    let csv = lab.work.path().join(format!("handle-{handle}.csv"));
    lab.ok(
        &[
            "ask",
            "handles",
            "export",
            "--handle",
            &handle.to_string(),
            "--out",
            csv.to_str().unwrap(),
        ],
        None,
    );
    let text = std::fs::read_to_string(&csv).unwrap();
    let mut lines = text.lines();
    let header: Vec<String> = lines
        .next()
        .unwrap_or_default()
        .split(',')
        .map(|c| c.trim_matches('"').to_string())
        .collect();
    lines
        .map(|l| {
            header
                .iter()
                .cloned()
                .zip(l.split(',').map(|c| c.trim_matches('"').to_string()))
                .collect()
        })
        .collect()
}

/// A row's value under a column whose header is, or ends with, `name`.
fn cell<'a>(row: &'a std::collections::BTreeMap<String, String>, name: &str) -> &'a str {
    row.iter()
        .find(|(h, _)| *h == name || h.ends_with(&format!(".{name}")) || h.ends_with(name))
        .map(|(_, v)| v.as_str())
        .unwrap_or_else(|| panic!("no column {name} in {row:?}"))
}

fn ask_file(lab: &Lab, name: &str, doc: &Value) -> String {
    let f = lab.work.path().join(format!("{name}.json"));
    std::fs::write(&f, doc.to_string()).unwrap();
    f.to_str().unwrap().to_string()
}

/// Record 49 A3's proof, the first half: a run's table is answered in the
/// ask, each value traced to its run, per scan at detail quasi and as a
/// total below it; a planted breach of a declared check raises its
/// `pipeline:qc` item, which names the metric and the value.
#[test]
fn a_run_s_table_answers_in_the_ask_and_a_planted_breach_raises_its_item() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-numbers");
    lab.add_descriptor("volumes", VOLUMES);
    let stacks: Vec<i64> = lab
        .store()
        .query("SELECT id FROM stack ORDER BY id", &[])
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect();
    assert_eq!(stacks.len(), 4);
    let low = stacks[0];
    let v = lab.json(&[
        "run",
        "volumes",
        "--select",
        "selection:every@1",
        "--param",
        &format!("low={low}"),
        "--json",
    ]);
    // a breach is not a failure: the run is done, the unit's item raised
    assert_eq!(v["status"], "done", "{v}");
    let numbers = &v["summary"]["numbers"];
    assert_eq!(numbers["tables"]["files"], 4, "{v}");
    assert_eq!(numbers["tables"]["rows"], 4, "{v}");
    // a volume and a site a stack, and the SNR a check read from results
    assert_eq!(numbers["measures"], 12, "{v}");
    assert_eq!(numbers["checks"]["declared"], 2, "{v}");
    assert_eq!(numbers["checks"]["breaches"], 1, "{v}");
    assert_eq!(numbers["checks"]["unchecked"], 0, "{v}");
    let run = v["id"].as_i64().unwrap();
    let rows = lab.json(&["derivative", "list", "--run", &run.to_string(), "--json"]);
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .all(|d| d["kind"] == "table"),
        "{rows}"
    );
    let mut store = lab.store();
    let items = store
        .query(
            "SELECT evidence FROM review_item WHERE kind = 'pipeline:qc' AND status = 'open'",
            &[],
        )
        .unwrap();
    assert_eq!(items.len(), 1, "one planted breach, one item");
    let evidence: Value = serde_json::from_str(items[0].text(0).unwrap()).unwrap();
    assert_eq!(evidence["status"], "breach", "{evidence}");
    assert_eq!(
        evidence["error"], "snr is 3.5, and the check is snr >= 8",
        "{evidence}"
    );
    assert_eq!(
        evidence["metrics"]["breaches"][0]["value"], 3.5,
        "{evidence}"
    );
    drop(store);

    // record 49 R4, after the assistant's review: below detail quasi every
    // door that shows the run says its checks as counts by check, a count
    // of fewer than 5 scans withheld, and no unit, value or tool's words
    let server = Server::start(&lab);
    let unit = format!("stack-{low}");
    let (status, full) = server.call("GET", &format!("/api/pipeline-runs/{run}"), None, OPERATOR);
    assert_eq!(status, 200, "{full}");
    assert_eq!(
        full["summary"]["breaches"][0]["unit"],
        unit.as_str(),
        "{full}"
    );
    assert_eq!(full["summary"]["breaches"][0]["breaches"][0]["value"], 3.5);
    let quiet = |doc: &Value| {
        let text = doc.to_string();
        assert!(!text.contains(&unit), "{unit} below quasi: {text}");
        assert!(!text.contains("3.5"), "a value below quasi: {text}");
        assert!(
            !text.contains("the check is"),
            "a breach's words below quasi: {text}"
        );
    };
    for token in [PLAIN, PLAIN_REVIEW] {
        let (status, doc) = server.call("GET", &format!("/api/pipeline-runs/{run}"), None, token);
        assert_eq!(status, 200, "{doc}");
        quiet(&doc);
        let s = &doc["summary"];
        assert_eq!(s["detail"], "totals", "{doc}");
        assert_eq!(s["breaches"], json!([]), "{doc}");
        assert_eq!(
            s["breaches_by_check"],
            json!([{"check": "snr >= 8", "metric": "snr", "units": null, "withheld": true}]),
            "one breach stands for one scan: {doc}"
        );
        assert!(s["numbers"]["checks"]["breaches"].is_null(), "{doc}");
        assert_eq!(s["numbers"]["checks"]["declared"], 2, "{doc}");
        assert_eq!(s["units"]["total"], 4, "{doc}");
        assert_eq!(doc["units_run"], json!([]), "{doc}");
        let (status, list) = server.call("GET", "/api/pipeline-runs", None, token);
        assert_eq!(status, 200, "{list}");
        quiet(&list);
        if let Some(job) = doc["job_id"].as_i64() {
            let (status, j) = server.call("GET", &format!("/api/jobs/{job}"), None, token);
            assert_eq!(status, 200, "{j}");
            quiet(&j);
            let (status, all) = server.call("GET", "/api/jobs?all=1", None, token);
            assert_eq!(status, 200, "{all}");
            quiet(&all);
        }
    }
    // the review list says the run's items one a check, a count of fewer
    // than 5 withheld, never one a unit, and an item is not read by its id
    let (status, items) = server.call("GET", "/api/review?kind=pipeline:qc", None, PLAIN_REVIEW);
    assert_eq!(status, 200, "{items}");
    quiet(&items);
    assert_eq!(items["count"], 1, "{items}");
    let group = &items["items"][0];
    assert_eq!(group["grouped"], true, "{group}");
    assert_eq!(group["evidence"]["status"], "breach", "{group}");
    assert_eq!(group["evidence"]["check"], "snr >= 8", "{group}");
    assert!(
        group["units"].is_null() && group["withheld"] == true,
        "{group}"
    );
    assert_eq!(
        group["ref"],
        json!({"run_id": run, "pipeline": "volumes@1"}),
        "{group}"
    );
    assert!(group.get("id").is_none(), "{group}");
    let (_, full_items) = server.call("GET", "/api/review?kind=pipeline:qc", None, OPERATOR);
    let id = full_items["items"][0]["id"].as_i64().unwrap();
    let (status, one) = server.call("GET", &format!("/api/review/{id}"), None, PLAIN_REVIEW);
    assert_eq!(status, 404, "{one}");
    quiet(&one);
    let (status, sum) = server.call("GET", "/api/review/summary", None, PLAIN_REVIEW);
    assert_eq!(status, 200, "{sum}");
    assert!(
        sum["by_kind"]["pipeline:qc"].is_null(),
        "one open item: {sum}"
    );
    let (_, sum) = server.call("GET", "/api/review/summary", None, OPERATOR);
    assert_eq!(sum["by_kind"]["pipeline:qc"], 1, "{sum}");
    let (_, doc) = server.call(
        "GET",
        &format!("/api/pipeline-runs/{run}"),
        None,
        PLAIN_REVIEW,
    );
    assert!(doc["summary"]["review_items"].is_null(), "{doc}");
    let (status, why) = server.call("GET", &format!("/api/explain/{low}"), None, PLAIN_REVIEW);
    assert!(status == 200 || status == 404, "{why}");
    quiet(&why);
    // at detail quasi the review item still names the unit and the value
    let (_, items) = server.call("GET", "/api/review?kind=pipeline:qc", None, OPERATOR);
    assert_eq!(items["items"][0]["ref"]["unit"], unit.as_str(), "{items}");
    assert_eq!(
        items["items"][0]["evidence"]["metrics"]["breaches"][0]["value"],
        3.5
    );
    // the pre-flight door reads its pipeline's name decoded, as a query
    // is: `volumes@1` sent as `volumes%401`
    for path in [
        "/api/pipelines/volumes%401/preflight",
        "/api/pipelines/volumes@1/preflight",
    ] {
        let (status, pre) = server.call(
            "POST",
            path,
            Some(json!({"select": "selection:every@1"})),
            PLAIN,
        );
        assert_eq!(status, 200, "{path}: {pre}");
        assert_eq!(pre["units"]["total"], 4, "{path}: {pre}");
    }
    drop(server);

    // the ask reads the run's numbers as fields of the stack, with the run
    let packs = packs();
    let p = packs.to_str().unwrap();
    let per_scan = json!({
        "ast_version": 1,
        "sets": {"s": {"grain": "stack"}},
        "out": {"set": "s", "level": "record", "columns": [
            ["field", {}, "id"],
            ["field", {}, "measure.volumes.brain_volume"],
            ["field", {}, "measure.volumes.site"],
            ["field", {}, "measure.volumes.snr"],
            ["field", {}, "measure.volumes.run"],
        ], "order": [[["field", {}, "id"], "asc"]]},
    });
    let file = ask_file(&lab, "per-scan", &per_scan);
    let answer = lab.json(&["ask", "run", "--file", &file, "--pack-dir", p, "--json"]);
    let rows = exported(&lab, &answer["handle"]);
    assert_eq!(rows.len(), 4, "{rows:?}");
    for r in &rows {
        let id: f64 = cell(r, "id").parse().unwrap();
        let volume: f64 = cell(r, "measure.volumes.brain_volume").parse().unwrap();
        assert_eq!(volume, 1000.0 * id, "{r:?}");
        assert_eq!(cell(r, "measure.volumes.site"), "lab", "{r:?}");
        let snr: f64 = cell(r, "measure.volumes.snr").parse().unwrap();
        assert_eq!(snr, if id as i64 == low { 3.5 } else { 20.0 }, "{r:?}");
        assert_eq!(
            cell(r, "measure.volumes.run"),
            run.to_string(),
            "each value traced to its run: {r:?}"
        );
    }

    // R4: below detail quasi a scan's value is refused, and its total is
    // answered over a group
    let (good, _, err) = lab.run_on(
        &lab.path,
        &["ask", "run", "--file", &file, "--pack-dir", p, "--json"],
        None,
    );
    assert!(good, "at the keyboard every class is held");
    let mut plain = lab.command(&lab.path);
    let out = plain
        .env("NILS_JOB_DETAIL", "plain")
        .args(["ask", "run", "--file", &file, "--pack-dir", p, "--json"])
        .output()
        .unwrap();
    let err_text = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err_text}");
    assert!(
        err_text.contains("read per row only at detail quasi"),
        "{err_text} {err}"
    );
    let totals = json!({
        "ast_version": 1,
        "sets": {
            "s": {"grain": "stack"},
            "g": {"grain": "group", "group": {"of": "s", "by": [["field", {}, "n_instances"]]},
                  "bind": {
                      "total": ["sum", {"set": "s"}, ["field", {}, "measure.volumes.brain_volume"]],
                      "mean": ["avg", {"set": "s"}, ["field", {}, "measure.volumes.brain_volume"]],
                      "scans": ["count", {"set": "s"}],
                  }},
        },
        "out": {"set": "g", "level": "aggregate", "columns": [
            ["field", {}, "n_instances"], ["field", {}, "total"], ["field", {}, "mean"], ["field", {}, "scans"],
        ]},
    });
    let file = ask_file(&lab, "totals", &totals);
    let mut plain = lab.command(&lab.path);
    let out = plain
        .env("NILS_JOB_DETAIL", "plain")
        .args(["ask", "run", "--file", &file, "--pack-dir", p, "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = exported(&lab, &answer["handle"]);
    assert_eq!(rows.len(), 1, "{rows:?}");
    // Nima's ruling after review: below detail quasi a group's totals of a
    // measure show only for 5 scans or more (D27's k, for measures alone);
    // this group holds 4, so its totals and its count are withheld
    for c in ["total", "mean", "scans", "_rows"] {
        if let Some(v) = rows[0].get(c) {
            assert_eq!(v, "", "{c} is withheld for a group of 4: {rows:?}");
        }
    }
    // at detail quasi the same group's totals are answered
    let answer = lab.json(&["ask", "run", "--file", &file, "--pack-dir", p, "--json"]);
    let rows = exported(&lab, &answer["handle"]);
    let sum: f64 = stacks.iter().map(|s| 1000.0 * *s as f64).sum();
    assert_eq!(
        cell(&rows[0], "total").parse::<f64>().unwrap(),
        sum,
        "{rows:?}"
    );
    assert_eq!(cell(&rows[0], "scans"), "4", "{rows:?}");
    // a filter on a measure counts toward the same rule: a count of the
    // scans it keeps is withheld below 5, and none is still none
    for (bound, shown) in [(0.0, false), (1.0e12, true)] {
        let filtered = json!({
            "ast_version": 1,
            "sets": {"s": {"grain": "stack", "where": [
                [">", {}, ["field", {}, "measure.volumes.brain_volume"], bound]]}},
            "out": {"set": "s", "level": "count"},
        });
        let file = ask_file(&lab, &format!("filtered-{bound}"), &filtered);
        let out = lab
            .command(&lab.path)
            .env("NILS_JOB_DETAIL", "plain")
            .args(["ask", "run", "--file", &file, "--pack-dir", p, "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
        let rows = exported(&lab, &answer["handle"]);
        let want = if shown { "0" } else { "" };
        assert_eq!(cell(&rows[0], "rows"), want, "{rows:?}");
        assert_eq!(cell(&rows[0], "subjects"), want, "{rows:?}");
    }

    // a plain list of the scans a measure filter keeps is refused below
    // detail quasi: the list itself says each one's measure against the
    // bound (Nima's ruling after review)
    let listed = json!({
        "ast_version": 1,
        "sets": {"s": {"grain": "stack", "where": [
            [">", {}, ["field", {}, "measure.volumes.brain_volume"], 0.0]]}},
        "out": {"set": "s", "level": "record", "columns": [["field", {}, "id"]]},
    });
    let file = ask_file(&lab, "listed", &listed);
    let out = lab
        .command(&lab.path)
        .env("NILS_JOB_DETAIL", "plain")
        .args(["ask", "run", "--file", &file, "--pack-dir", p, "--json"])
        .output()
        .unwrap();
    let err_text = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "a list over a measure filter is refused"
    );
    assert!(err_text.contains("filtered on a measure"), "{err_text}");
    // at detail quasi the same list is answered
    let answer = lab.json(&["ask", "run", "--file", &file, "--pack-dir", p, "--json"]);
    assert_eq!(exported(&lab, &answer["handle"]).len(), 4);

    // a newer run's value is the one the ask reads, and names that run
    let again = lab.json(&[
        "run",
        "volumes",
        "--select",
        "selection:every@1",
        "--param",
        "scale=2000",
        "--json",
    ]);
    assert_eq!(
        again["summary"]["numbers"]["checks"]["breaches"], 0,
        "{again}"
    );
    let file = ask_file(&lab, "per-scan-again", &per_scan);
    let answer = lab.json(&["ask", "run", "--file", &file, "--pack-dir", p, "--json"]);
    for r in exported(&lab, &answer["handle"]) {
        let id: f64 = cell(&r, "id").parse().unwrap();
        let volume: f64 = cell(&r, "measure.volumes.brain_volume").parse().unwrap();
        assert_eq!(volume, 2000.0 * id, "{r:?}");
        assert_eq!(
            cell(&r, "measure.volumes.run"),
            again["id"].to_string(),
            "{r:?}"
        );
    }

    // the newest run of the pipeline's current version is read, even where
    // an older version ran later, and names that run
    lab.add_descriptor(
        "volumes",
        &VOLUMES.replace("tool-version: \"1\"", "tool-version: \"2\""),
    );
    let current = lab.json(&[
        "run",
        "volumes",
        "--select",
        "selection:every@1",
        "--param",
        "scale=3000",
        "--json",
    ]);
    assert_eq!(current["pipeline"], "volumes@2", "{current}");
    let older = lab.json(&[
        "run",
        "volumes@1",
        "--select",
        "selection:every@1",
        "--param",
        "scale=5000",
        "--json",
    ]);
    assert_eq!(older["pipeline"], "volumes@1", "{older}");
    let file = ask_file(&lab, "per-scan-current", &per_scan);
    let answer = lab.json(&["ask", "run", "--file", &file, "--pack-dir", p, "--json"]);
    for r in exported(&lab, &answer["handle"]) {
        let id: f64 = cell(&r, "id").parse().unwrap();
        let volume: f64 = cell(&r, "measure.volumes.brain_volume").parse().unwrap();
        assert_eq!(volume, 3000.0 * id, "{r:?}");
        assert_eq!(
            cell(&r, "measure.volumes.run"),
            current["id"].to_string(),
            "{r:?}"
        );
    }
}

/// Record 49 A3: a pipeline of the bids layout that needs a T1w and a
/// FLAIR a session, and makes an output only where it has both.
const NEEDS_FLAIR: &str = r#"name: needs-flair
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/needs-flair@sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
command-line: |
  python3 -c '
  import glob, os, shutil, sys
  src, out = sys.argv[1], sys.argv[2]
  for t in sorted(glob.glob(src + "/sub-*/ses-*/anat/*_T1w.nii.gz")):
      a = os.path.dirname(t)
      if not glob.glob(a + "/*_FLAIR.nii.gz"):
          continue
      rel = os.path.relpath(t, src)
      d = os.path.join(out, os.path.dirname(rel)); os.makedirs(d, exist_ok=True)
      shutil.copy(t, os.path.join(d, os.path.basename(rel).replace("_T1w", "_desc-both_T1w")))
      open(os.path.join(d, "stats.json"), "w").write("{\"Bytes\": %d}" % os.path.getsize(t))
  ' [InputDataset] [OutputLocation]
x-nils:
  analysis-level: session
  input: {layout: bids, roles: [t1w, flair]}
  outputs:
    - id: both
      kind: output
      path-template: "sub-{subject}/ses-{session}/anat/*_desc-both_T1w.nii.gz"
    - id: stats
      kind: table
      path-template: "sub-{subject}/ses-{session}/anat/stats.json"
      columns: [{name: bytes, type: integer}]
  needs: {cores: 2, memory-gb: 3, unit-minutes: 4}
"#;

/// Record 49 A3's proof, the second half: the pre-flight of a selection
/// whose one session lacks its FLAIR counts the units the run then has and
/// names the one it then fails, at the command line and at the door, and
/// estimates from the descriptor and then from the run.
#[test]
fn the_preflight_counts_what_the_run_then_does() {
    if !have("python3") || !have("dcm2niix") {
        eprintln!(
            "python3 or dcm2niix is not installed; the bids layout needs a converter, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("pipelines-preflight");
    lab.add_descriptor("needs-flair", NEEDS_FLAIR);
    // one session's FLAIR left out of the selection
    let flair = lab
        .store()
        .query(
            "SELECT ps.stack_id FROM pick_stack ps JOIN pick p ON p.id = ps.pick_id \
             WHERE p.role = 'flair' AND p.withdrawn_at IS NULL ORDER BY ps.stack_id LIMIT 1",
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    let doc = lab.work.path().join("most.json");
    std::fs::write(
        &doc,
        json!({"ast_version": 1, "sets": {"most": {"grain": "stack",
            "where": [["<>", {}, ["field", {}, "id"], flair]]}},
            "out": {"set": "most", "level": "record"}})
        .to_string(),
    )
    .unwrap();
    let packs = packs();
    lab.ok(
        &[
            "ask",
            "selections",
            "save",
            "--name",
            "most",
            "--file",
            doc.to_str().unwrap(),
            "--pack-dir",
            packs.to_str().unwrap(),
        ],
        None,
    );
    let pre = lab.json(&[
        "run",
        "needs-flair",
        "--select",
        "selection:most@1",
        "--preflight",
        "--json",
    ]);
    assert_eq!(pre["stacks"], 3, "{pre}");
    assert_eq!(pre["units"]["total"], 2, "{pre}");
    assert_eq!(pre["units"]["missing"], 1, "{pre}");
    let why = pre["missing"][0]["why"][0].as_str().unwrap();
    assert!(why.contains("no flair is picked"), "{pre}");
    assert_eq!(pre["estimate"]["source"], "descriptor", "{pre}");
    assert_eq!(
        pre["estimate"]["seconds"], 480.0,
        "two units of four minutes, one at a time: {pre}"
    );
    assert_eq!(pre["gpu"]["need"], "none");
    assert_eq!(pre["gpu"]["device"], "cpu");
    assert_eq!(pre["needs"]["cores"], 2.0, "{pre}");
    assert_eq!(pre["budget"]["fits"], true, "{pre}");
    assert_eq!(pre["ready"], true, "{pre}");
    // nothing ran
    assert!(lab.podman_runs().is_empty());
    // the words a person reads
    let text = lab.ok(
        &[
            "run",
            "needs-flair",
            "--select",
            "selection:most@1",
            "--preflight",
        ],
        None,
    );
    assert!(text.contains("2 (1 ready, 1 missing an input)"), "{text}");
    // a parameter it does not have is a blocker, not a run
    let bad = lab.json(&[
        "run",
        "needs-flair",
        "--select",
        "selection:most@1",
        "--preflight",
        "--param",
        "nope=1",
        "--json",
    ]);
    assert_eq!(bad["ready"], false, "{bad}");

    // the door answers the same, and at detail plain names no session
    let server = Server::start(&lab);
    let (status, door) = server.call(
        "POST",
        "/api/pipelines/needs-flair/preflight",
        Some(json!({"select": "selection:most@1"})),
        OPERATOR,
    );
    assert_eq!(status, 200, "{door}");
    assert_eq!(door["units"], pre["units"], "{door}");
    assert!(door["missing"][0]["session_day"].is_string(), "{door}");
    let (status, flat) = server.call(
        "POST",
        "/api/pipelines/needs-flair/preflight",
        Some(json!({"handle": pre["handle"]})),
        PLAIN,
    );
    assert_eq!(status, 200, "{flat}");
    assert_eq!(flat["units"], pre["units"]);
    assert!(flat["missing"][0].get("session_day").is_none(), "{flat}");
    assert!(flat["missing"][0]["why"].is_array(), "{flat}");
    let (status, _) = server.call(
        "POST",
        "/api/pipelines/needs-flair/preflight",
        Some(json!({"handle": pre["handle"]})),
        READER,
    );
    assert_eq!(status, 403, "a reader holds no pipelines:see");
    let (status, _) = server.call(
        "POST",
        "/api/pipelines/no-such/preflight",
        Some(json!({"handle": pre["handle"]})),
        OPERATOR,
    );
    assert_eq!(status, 404);
    drop(server);

    // the run then has those units, and fails the one named
    let v = lab.json(&[
        "run",
        "needs-flair",
        "--select",
        "selection:most@1",
        "--json",
    ]);
    assert_eq!(v["summary"]["units"]["total"], pre["units"]["total"], "{v}");
    assert_eq!(
        v["summary"]["units"]["failed"], pre["units"]["missing"],
        "{v}"
    );
    assert_eq!(
        v["summary"]["units"]["succeeded"], pre["units"]["ready"],
        "{v}"
    );
    // a session's table is the session's measure in the ask
    assert_eq!(v["summary"]["numbers"]["measures"], 1, "{v}");
    let sessions = json!({
        "ast_version": 1,
        "sets": {"s": {"grain": "session"}},
        "out": {"set": "s", "level": "record", "columns": [
            ["field", {}, "id"], ["field", {}, "measure.needs-flair.bytes"],
            ["field", {}, "measure.needs-flair.run"],
        ]},
    });
    let file = ask_file(&lab, "sessions", &sessions);
    let answer = lab.json(&[
        "ask",
        "run",
        "--file",
        &file,
        "--pack-dir",
        packs.to_str().unwrap(),
        "--json",
    ]);
    let rows = exported(&lab, &answer["handle"]);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let measured: Vec<&std::collections::BTreeMap<String, String>> = rows
        .iter()
        .filter(|r| !cell(r, "measure.needs-flair.bytes").is_empty())
        .collect();
    assert_eq!(measured.len(), 1, "the session with both: {rows:?}");
    assert!(
        cell(measured[0], "measure.needs-flair.bytes")
            .parse::<f64>()
            .unwrap()
            > 0.0
    );
    assert_eq!(
        cell(measured[0], "measure.needs-flair.run"),
        v["id"].to_string()
    );
    // and the next pre-flight estimates from that run
    let after = lab.json(&[
        "run",
        "needs-flair",
        "--select",
        "selection:most@1",
        "--preflight",
        "--json",
    ]);
    assert_eq!(after["estimate"]["source"], "runs", "{after}");
    assert_eq!(after["estimate"]["runs"], 1, "{after}");
}

/// Record 49 A4's proof: an engine started on a fresh registry seeds the
/// starter catalog, each version marked as a starter and each image pinned;
/// a second start adds nothing, a person's own version is never gone over,
/// and the setting turns the seeding off.
#[test]
fn the_starter_catalog_is_seeded_at_the_engine_s_start() {
    if !have("python3") {
        eprintln!("python3 is not installed; the lab needs it, so this test is skipped");
        return;
    }
    let lab = Lab::new("pipelines-starter");
    let before = lab.json(&["pipeline", "starter", "--json"]);
    assert_eq!(before["seeding"], "on");
    assert!(
        before["starters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state"] == "absent"),
        "{before}"
    );
    let server = Server::start(&lab);
    let (status, cat) = server.call("GET", "/api/pipelines", None, OPERATOR);
    assert_eq!(status, 200, "{cat}");
    let names: Vec<&str> = cat["pipelines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["starter"] == true)
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "freesurfer-recon-all",
            "mriqc",
            "n4-bias-correction",
            "samseg-lesions",
            "synthseg",
            "synthstrip"
        ],
        "{cat}"
    );
    for p in cat["pipelines"].as_array().unwrap() {
        assert_eq!(p["origin"], "starter", "{p}");
        assert!(p["image"].as_str().unwrap().contains("@sha256:"), "{p}");
    }
    drop(server);
    // a second start adds nothing
    let server = Server::start(&lab);
    let (_, again) = server.call("GET", "/api/pipelines", None, OPERATOR);
    assert_eq!(again["pipelines"].as_array().unwrap().len(), 6, "{again}");
    drop(server);
    let listed = lab.json(&["pipeline", "starter", "--json"]);
    for s in listed["starters"].as_array().unwrap() {
        assert!(
            s["state"]
                .as_str()
                .unwrap()
                .starts_with("in the catalog as"),
            "{s}"
        );
    }
    // a person's own version of a starter is left as it is
    let mine = std::fs::read_to_string(repo().join("pipelines/synthstrip/nils.job.yml"))
        .unwrap()
        .replace("default-value: 2\n", "default-value: 3\n");
    let v = lab.add_descriptor("synthstrip-mine", &mine);
    assert_eq!(v["label"], "synthstrip@2", "{v}");
    assert_eq!(v["starter"], false, "{v}");
    let listed = lab.json(&["pipeline", "starter", "--seed", "--json"]);
    assert!(listed["added"].as_array().unwrap().is_empty(), "{listed}");
    let state = listed["starters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "synthstrip")
        .unwrap()["state"]
        .clone();
    assert_eq!(state, "in the catalog as synthstrip@1", "{listed}");

    // the setting turns it off, on a registry that has none yet
    let fresh = Lab::new("pipelines-starter-off");
    fresh.ok(&["pipeline", "starter", "--off"], None);
    let server = Server::start(&fresh);
    let (_, none) = server.call("GET", "/api/pipelines", None, OPERATOR);
    assert!(none["pipelines"].as_array().unwrap().is_empty(), "{none}");
    drop(server);
    let text = fresh.ok(&["pipeline", "starter"], None);
    assert!(
        text.contains("seeding at the engine's start: off"),
        "{text}"
    );
}
