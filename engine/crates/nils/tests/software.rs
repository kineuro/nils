// SPDX-License-Identifier: AGPL-3.0-only

//! The software place (record 57 P5; Wave 7b, section 6), through the
//! binary and its doors: a tool installed by digest and verified (probe,
//! fingerprint, test run), usable by hand from `bin/` and named by its
//! module, NILS running the same image (G7); a pull whose digest differs
//! refused with nothing recorded; an existing install registered as the
//! site's, never removed, its bytes untouched (section 6.3); a tool's terms
//! accepted as an event, its licence file placed and never recorded, and a
//! run refused before it starts until then (G9); every missing dependency
//! reported per item with its fix, on a fixture per item (G10); and the
//! converter as one package among the others.
//!
//! No container runtime is needed: a stand-in for docker on the search
//! path answers `--version`, `pull`, `image inspect`, `image rm` and `run`
//! as docker does, keeps the words it was given, and reports the digest a
//! test asks it to. The host's real runtimes are never on the path.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;
use serde_json::{Value, json};

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn sha256(bytes: &[u8]) -> String {
    format!(
        "sha256:{}",
        hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
    )
}

/// A stand-in for docker: it answers as docker does, keeps the words it is
/// given in `FAKE_DOCKER_ARGS`, reports `FAKE_DOCKER_DIGEST` as the digest
/// of what it pulled (else the one asked), and runs a command by printing
/// what a tool would.
const FAKE_DOCKER: &str = r#"#!/usr/bin/env python3
import json, os, sys
a = sys.argv[1:]
log = os.environ.get("FAKE_DOCKER_ARGS")
if log:
    with open(log, "a") as f:
        f.write(json.dumps(a) + "\n")
if not a or a[0] == "--version":
    print("Docker version 27.0.0-test, build abcdef0"); sys.exit(0)
if a[0] == "pull":
    if os.environ.get("FAKE_DOCKER_PULL_FAILS"):
        print("Error response from daemon: manifest unknown", file=sys.stderr); sys.exit(1)
    print("Digest: " + a[1].split("@", 1)[1]); sys.exit(0)
if a[0] == "image" and a[1] == "inspect":
    ref = a[-1]
    repo, digest = ref.rsplit("@", 1)
    digest = os.environ.get("FAKE_DOCKER_DIGEST", digest)
    print(json.dumps([repo + "@" + digest]) + "\t4261550\tsha256:" + "e" * 64); sys.exit(0)
if a[0] == "image" and a[1] == "rm":
    sys.exit(0)
if a[0] == "run":
    i = 0
    while i < len(a) and "@sha256:" not in a[i]:
        i += 1
    cmd = a[i + 1:]
    if cmd[1:2] == ["--version"]:
        print("stand-in tool 1.2.3"); sys.exit(0)
    print("the stand-in ran " + " ".join(cmd)); sys.exit(0)
sys.exit(0)
"#;

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const IMAGE_ECHO: &str =
    "example.org/echo-tool@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const IMAGE_LICENSED: &str = "example.org/licensed-tool@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A descriptor of the stacks layout whose tool is `name` at `version`,
/// with `needs` and `more` under x-nils as given.
fn descriptor(name: &str, version: &str, image: &str, needs: &str, more: &str) -> String {
    format!(
        r#"name: {name}
schema-version: "0.5"
tool-version: "{version}"
container-image:
  type: docker
  image: "{image}"
command-line: "{name} [Manifest] [OutputLocation] [Inputs]"
x-nils:
  analysis-level: stack
  input: {{layout: stacks}}
  outputs:
    - id: echo
      kind: output
      path-template: "stack-{{stack}}/out.txt"
      media-type: text/plain
  needs: {{{needs}}}
{more}"#
    )
}

/// A descriptor of the bids layout, which needs the converter.
fn bids_descriptor(name: &str, image: &str) -> String {
    format!(
        r#"name: {name}
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "{image}"
command-line: "{name} [InputDataset] [OutputLocation] [ParticipantLabels]"
x-nils:
  analysis-level: session
  input: {{layout: bids}}
  outputs:
    - id: n4_image
      kind: output
      path-template: "sub-{{subject}}/ses-{{session}}/anat/*_desc-n4_T1w.nii.gz"
"#
    )
}

const PACK_ECHO: &str = r#"nils_pack: 1
pipeline: {name: echo-tool, descriptor_sha256: "sha256:00"}
source:
  image: "example.org/echo-tool@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  probe: {command: ["echo-tool", "--version"], expect: "stand-in tool 1.2.3"}
licence: {spdx: MIT, redistributable: true, accept: false}
tests: {command: ["echo-tool", "run", "/input"], expect: "the stand-in ran"}
"#;

const PACK_LICENSED: &str = r#"nils_pack: 1
pipeline: {name: licensed-tool, descriptor_sha256: "sha256:00"}
source:
  image: "example.org/licensed-tool@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  probe: {command: ["licensed-tool", "--version"], expect: "stand-in tool"}
licence:
  spdx: LicenseRef-Site
  redistributable: false
  site_file: {env: TOOL_LICENSE, mount: /license.txt, name: license.txt}
  terms: {address: "https://example.org/terms", version: "2026"}
  accept: true
  env: {TOOL_ALLOW_DEEP: "1"}
"#;

const PACK_SITE: &str = r#"nils_pack: 1
pipeline: {name: site-tool, descriptor_sha256: "sha256:00"}
source:
  image: "example.org/site-tool@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
  key_files: [bin, models]
licence: {spdx: Apache-2.0, redistributable: true, accept: false}
"#;

/// A registry of its own with a working place, the stand-in docker as its
/// runtime and nothing else on its search path but python3, env and sh.
struct Lab {
    home: TempDir,
    work: TempDir,
    bin: TempDir,
    path: OsString,
    args_log: PathBuf,
}

fn executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

impl Lab {
    fn new(name: &str) -> Lab {
        Lab::with_runtime(name, true)
    }

    /// Over the host's own docker, with no stand-in but for nvidia-smi, so
    /// no card is touched: for the one live test.
    fn with_real_docker(name: &str) -> Lab {
        let bin = TempDir::new(&format!("{name}-bin"));
        executable(&bin.file("nvidia-smi", b"#!/bin/sh\nexit 1\n"));
        let mut path = bin.path().as_os_str().to_owned();
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        Lab::finish(name, bin, path)
    }

    fn with_runtime(name: &str, docker: bool) -> Lab {
        let bin = TempDir::new(&format!("{name}-bin"));
        if docker {
            executable(&bin.file("docker", FAKE_DOCKER.as_bytes()));
        }
        executable(&bin.file("nvidia-smi", b"#!/bin/sh\nexit 1\n"));
        // the interpreters the stand-ins need, and nothing else of the host
        let tools = bin.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        for program in ["python3", "env", "sh", "readlink", "dirname", "id"] {
            let found = std::env::var_os("PATH")
                .and_then(|p| {
                    std::env::split_paths(&p)
                        .map(|d| d.join(program))
                        .find(|f| f.is_file())
                })
                .unwrap_or_else(|| panic!("no {program} on the host"));
            #[cfg(unix)]
            std::os::unix::fs::symlink(found, tools.join(program)).unwrap();
        }
        let mut path = bin.path().as_os_str().to_owned();
        path.push(":");
        path.push(tools.as_os_str());
        Lab::finish(name, bin, path)
    }

    fn finish(name: &str, bin: TempDir, path: OsString) -> Lab {
        let lab = Lab {
            home: TempDir::new(&format!("{name}-home")),
            work: TempDir::new(&format!("{name}-work")),
            args_log: bin.path().join("docker-args.jsonl"),
            bin,
            path,
        };
        let (good, _, err) = lab.run(&["key", "add", "k"], &[], Some("a software test key\n"));
        assert!(good, "{err}");
        let (good, _, err) = lab.run(&["init", "--key", "k"], &[], None);
        assert!(good, "{err}");
        let work = lab.work.path().display().to_string();
        let (good, _, err) = lab.run(
            &["place", "add", "work", &work, "--role", "working", "--fast"],
            &[],
            None,
        );
        assert!(good, "{err}");
        let (good, _, err) = lab.run(&["pipeline", "runtime", "--set", "docker"], &[], None);
        assert!(good, "{err}");
        lab
    }

    fn command(&self, env: &[(&str, &str)]) -> Command {
        let mut c = nils();
        c.arg("--registry")
            .arg(self.home.path())
            .env("USER", "adm")
            .env("HOSTNAME", "ward-3")
            .env("PATH", &self.path)
            .env("FAKE_DOCKER_ARGS", &self.args_log)
            .env_remove("FAKE_DOCKER_DIGEST")
            .env_remove("FAKE_DOCKER_PULL_FAILS");
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }

    fn run(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> (bool, String, String) {
        let mut child = self
            .command(env)
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

    fn json(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let (good, out, err) = self.run(args, env, None);
        assert!(good, "nils {}: {err}", args.join(" "));
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    fn add_pipeline(&self, text: &str) -> String {
        let file = self.bin.file(
            &format!("descriptors/{}.yml", &sha256(text.as_bytes())[7..23]),
            text.as_bytes(),
        );
        let v = self.json(&["pipeline", "add", file.to_str().unwrap(), "--json"], &[]);
        assert!(v["name"].is_string(), "{v}");
        format!("{}@{}", v["name"].as_str().unwrap(), v["version"])
    }

    fn pack_file(&self, name: &str, text: &str) -> PathBuf {
        self.bin
            .file(&format!("packs/{name}.yaml"), text.as_bytes())
    }

    /// The software place: `software/` under the working place.
    fn place(&self) -> PathBuf {
        self.work.path().join("software")
    }

    /// The words the stand-in docker was given, one call per line.
    fn docker_calls(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(&self.args_log)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn package<'a>(listed: &'a Value, spec: &str) -> &'a Value {
        listed["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["spec"] == spec)
            .unwrap_or_else(|| panic!("no {spec} in {listed}"))
    }

    fn need<'a>(package: &'a Value, check: &str) -> &'a Value {
        package["needs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["check"] == check)
            .unwrap_or_else(|| panic!("no {check} check in {package}"))
    }
}

/// `nils serve` over the lab, with tokens of the three roles.
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

const READER: &str = "a-reader-token-of-length=reader@lab:reader";
const ADMIN: &str = "an-admin-token-of-length=adm@lab:admin";

impl Server {
    fn start(lab: &Lab, requests: usize) -> Server {
        let mut cmd = lab.command(&[]);
        cmd.args([
            "serve",
            "--bind",
            "127.0.0.1:0",
            "--requests",
            &requests.to_string(),
            "--auth",
            "token",
            "--token",
            READER,
            "--token",
            ADMIN,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        let child = cmd.spawn().unwrap();
        let mut held = Server { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let first = BufReader::new(stdout).lines().next().unwrap().unwrap();
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        held
    }

    fn send(&self, method: &str, path: &str, token: &str, body: Option<&Value>) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let kind = if body.is_empty() {
            String::new()
        } else {
            "Content-Type: application/json\r\n".to_string()
        };
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{kind}Content-Length: {}\r\nAuthorization: Bearer {}\r\n\r\n",
            body.len(),
            token.split('=').next().unwrap()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let body = serde_json::from_str(body).unwrap_or(Value::Null);
        (status, body)
    }
}

/// G7: a tool installed by digest and verified (probe, fingerprint, test
/// run); usable by hand from `bin/`; the module names the right paths; NILS
/// runs the same image; installed again it is left alone; removed whole.
#[test]
fn a_tool_is_installed_by_digest_verified_and_run_by_hand_from_bin() {
    let lab = Lab::new("software-install");
    let label = lab.add_pipeline(&descriptor(
        "echo-tool",
        "2.0",
        IMAGE_ECHO,
        "gpu: optional",
        "",
    ));
    assert_eq!(label, "echo-tool@1");

    // before: the catalog's tool is a package, missing, with its needs checked
    let before = lab.json(&["software", "list", "--json"], &[]);
    assert_eq!(
        before["place"]["path"],
        lab.place().display().to_string(),
        "{before}"
    );
    assert!(before["place"]["declared"].is_null());
    let p = Lab::package(&before, "echo-tool@2.0");
    assert_eq!(p["status"], "missing", "{p}");
    assert_eq!(p["pipelines"], json!(["echo-tool@1"]), "{p}");
    assert_eq!(p["source"], IMAGE_ECHO, "{p}");
    assert_eq!(Lab::need(p, "runtime")["state"], "ok", "{p}");
    assert_eq!(Lab::need(p, "runtime")["said"], "docker 27.0.0-test", "{p}");
    // an optional GPU missing is a caveat, not a refusal (G10)
    assert_eq!(Lab::need(p, "gpu")["state"], "warn", "{p}");
    assert_eq!(Lab::need(p, "memory")["state"], "ok", "{p}");
    assert_eq!(Lab::need(p, "disk")["state"], "ok", "{p}");
    assert_eq!(
        p["install"]["command"], "nils software install echo-tool@2.0",
        "{p}"
    );

    // installed, by digest, with the pack
    let pack = lab.pack_file("echo", PACK_ECHO);
    let done = lab.json(
        &[
            "software",
            "install",
            "echo-tool@2.0",
            "--pack",
            pack.to_str().unwrap(),
            "--json",
        ],
        &[],
    );
    assert_eq!(done["installed"], true, "{done}");
    assert_eq!(done["verified"], true, "{done}");
    let p = &done["package"];
    assert_eq!(p["status"], "installed", "{p}");
    assert_eq!(p["how"], "installed", "{p}");
    assert_eq!(p["managed"], true, "{p}");
    assert_eq!(p["kind"], "image", "{p}");
    assert_eq!(p["image"], IMAGE_ECHO, "{p}");
    assert_eq!(p["digest_asked"], DIGEST_A, "{p}");
    assert_eq!(p["digest"], DIGEST_A, "{p}");
    assert_eq!(p["fingerprint"], DIGEST_A, "{p}");
    assert_eq!(p["runtime"], "docker", "{p}");
    assert!(p["by"].as_str().unwrap().starts_with("adm"), "{p}");
    assert_eq!(p["size_bytes"], 4_261_550, "{p}");
    assert!(p["verified_at"].is_string(), "{p}");
    assert_eq!(p["verification"]["probe"]["passed"], true, "{p}");
    assert!(
        p["verification"]["probe"]["said"]
            .as_str()
            .unwrap()
            .contains("stand-in tool 1.2.3"),
        "{p}"
    );
    assert_eq!(p["verification"]["fingerprint"]["passed"], true, "{p}");
    assert_eq!(p["verification"]["test"]["passed"], true, "{p}");
    assert_eq!(p["licence"]["spdx"], "MIT", "{p}");
    assert_eq!(p["licence"]["accept"], false, "{p}");
    assert_eq!(Lab::need(p, "licence")["state"], "ok", "{p}");
    assert_eq!(Lab::need(p, "image")["state"], "ok", "{p}");
    // NILS runs the same image: the pre-flight's image check names the install
    let software = &Lab::need(p, "image")["software"];
    assert_eq!(software["how"], "installed", "{p}");
    assert_eq!(software["image"], IMAGE_ECHO, "{p}");
    assert_eq!(software["fingerprint"], DIGEST_A, "{p}");
    assert_eq!(Lab::need(p, "image")["key"]["same_as_pinned"], true, "{p}");
    assert_eq!(Lab::need(p, "image")["key"]["key"], DIGEST_A, "{p}");

    // the pull asked the digest and the store was asked what came; no push
    let calls = lab.docker_calls();
    assert!(
        calls.iter().any(|c| c[0] == "pull" && c[1] == IMAGE_ECHO),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c[0] == "image" && c[1] == "inspect" && c.last().unwrap() == IMAGE_ECHO),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|c| c[0] != "push" && !c.contains(&"push".to_string())),
        "{calls:?}"
    );
    // the probe and the test run, with no network
    let runs: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "run").collect();
    assert_eq!(runs.len(), 2, "{calls:?}");
    for r in &runs {
        assert!(r.windows(2).any(|w| w == ["--network", "none"]), "{r:?}");
        assert!(r.contains(&IMAGE_ECHO.to_string()), "{r:?}");
    }

    // the layout of section 6.1
    let place = lab.place();
    let install = place.join("echo-tool").join("2.0");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(install.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["source"], IMAGE_ECHO, "{manifest}");
    assert_eq!(manifest["digest"], DIGEST_A, "{manifest}");
    assert_eq!(
        manifest["pack_sha256"],
        sha256(PACK_ECHO.as_bytes()),
        "{manifest}"
    );
    assert_eq!(manifest["how"], "installed", "{manifest}");
    assert!(install.join("nils-pack.yaml").is_file());
    let wrapper = install.join("bin").join("echo-tool");
    assert!(wrapper.is_file(), "{}", wrapper.display());
    let text = std::fs::read_to_string(&wrapper).unwrap();
    assert!(
        text.contains("readlink -f"),
        "the wrapper finds the place from its own location: {text}"
    );
    assert!(text.contains(IMAGE_ECHO), "{text}");
    assert!(
        place
            .join("echo-tool")
            .join("bin")
            .join("echo-tool-run")
            .is_file()
    );
    let link = place.join("bin").join("echo-tool");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        PathBuf::from("../echo-tool/2.0/bin/echo-tool")
    );
    let module = std::fs::read_to_string(place.join("modules/echo-tool/2.0.lua")).unwrap();
    assert!(module.contains("local here = myFileName()"), "{module}");
    assert!(
        module.contains("pathJoin(place, \"echo-tool\", \"2.0\")"),
        "{module}"
    );
    assert!(
        module.contains("prepend_path(\"PATH\", pathJoin(install, \"bin\"))"),
        "{module}"
    );
    assert!(module.contains(IMAGE_ECHO), "{module}");

    // usable by hand from bin/: the wrapper runs the same image through docker
    let said = Command::new(&link)
        .arg("--version")
        .env("PATH", &lab.path)
        .env_remove("FAKE_DOCKER_ARGS")
        .output()
        .unwrap();
    assert!(
        said.status.success(),
        "{}",
        String::from_utf8_lossy(&said.stderr)
    );
    assert!(
        String::from_utf8_lossy(&said.stdout).contains("stand-in tool 1.2.3"),
        "{}",
        String::from_utf8_lossy(&said.stdout)
    );
    let said = Command::new(place.join("bin").join("echo-tool-run"))
        .args(["echo-tool", "--version"])
        .env("PATH", &lab.path)
        .env_remove("FAKE_DOCKER_ARGS")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&said.stdout).contains("stand-in tool 1.2.3"),
        "{}",
        String::from_utf8_lossy(&said.stderr)
    );

    // listed as installed; verified again on asking; installed again, left alone
    let listed = lab.json(&["software", "list", "--json"], &[]);
    assert_eq!(
        Lab::package(&listed, "echo-tool@2.0")["status"],
        "installed"
    );
    let again = lab.json(&["software", "verify", "echo-tool@2.0", "--json"], &[]);
    assert_eq!(again["verified"], true, "{again}");
    assert!(
        again["verification"]["fingerprint"]["said"]
            .as_str()
            .unwrap()
            .contains("as recorded"),
        "{again}"
    );
    let twice = lab.json(&["software", "install", "echo-tool@2.0", "--json"], &[]);
    assert_eq!(twice["installed"], false, "{twice}");
    assert!(
        twice["why"].as_str().unwrap().contains("installed already"),
        "{twice}"
    );
    let (_, out, _) = lab.run(&["software", "list"], &[], None);
    assert!(out.contains("echo-tool@2.0"), "{out}");
    assert!(out.contains("installed"), "{out}");

    // audited, without a byte of anything but names and digests
    let audit = lab.json(&["audit", "list", "--action", "software.", "--json"], &[]);
    assert!(audit.to_string().contains("software.install"), "{audit}");
    assert!(audit.to_string().contains("software.verify"), "{audit}");

    // removed whole: the image out of the store, the manifest, wrappers and module gone
    let removed = lab.json(&["software", "remove", "echo-tool@2.0", "--json"], &[]);
    assert_eq!(removed["removed"], true, "{removed}");
    assert!(!install.exists());
    assert!(std::fs::symlink_metadata(&link).is_err());
    assert!(!place.join("modules/echo-tool/2.0.lua").exists());
    assert!(!place.join("echo-tool/bin/echo-tool-run").exists());
    let calls = lab.docker_calls();
    assert!(
        calls
            .iter()
            .any(|c| c[0] == "image" && c[1] == "rm" && c[2] == IMAGE_ECHO),
        "{calls:?}"
    );
    let listed = lab.json(&["software", "list", "--json"], &[]);
    assert_eq!(Lab::package(&listed, "echo-tool@2.0")["status"], "missing");
}

/// G7: the digest asked for and the digest got are equal, or the install
/// fails: what came is taken out of the store again and nothing is
/// recorded. A pull that fails says so with the runtime's words.
#[test]
fn a_pull_whose_digest_differs_is_refused_and_nothing_is_recorded() {
    let lab = Lab::new("software-digest");
    lab.add_pipeline(&descriptor("echo-tool", "2.0", IMAGE_ECHO, "gpu: none", ""));
    let (good, _, err) = lab.run(
        &["software", "install", "echo-tool@2.0"],
        &[("FAKE_DOCKER_DIGEST", DIGEST_B)],
        None,
    );
    assert!(!good);
    assert!(err.contains(&format!("asked for {DIGEST_A}")), "{err}");
    assert!(err.contains(&format!("got {DIGEST_B}")), "{err}");
    assert!(err.contains("not installed"), "{err}");
    assert!(!lab.place().join("echo-tool").exists());
    let calls = lab.docker_calls();
    assert!(
        calls.iter().any(|c| c[0] == "image" && c[1] == "rm"),
        "{calls:?}"
    );
    let listed = lab.json(&["software", "list", "--json"], &[]);
    assert_eq!(Lab::package(&listed, "echo-tool@2.0")["status"], "missing");

    let (good, _, err) = lab.run(
        &["software", "install", "echo-tool@2.0"],
        &[("FAKE_DOCKER_PULL_FAILS", "1")],
        None,
    );
    assert!(!good);
    assert!(err.contains("could not pull"), "{err}");
    assert!(err.contains("manifest unknown"), "{err}");
    assert!(!lab.place().join("echo-tool").exists());

    // a source not pinned by its digest is refused before anything is pulled
    let (good, _, err) = lab.run(
        &[
            "software",
            "install",
            "other@1",
            "--source",
            "example.org/other:latest",
        ],
        &[],
        None,
    );
    assert!(!good);
    assert!(err.contains("not pinned by its digest"), "{err}");
    assert!(
        lab.docker_calls()
            .iter()
            .all(|c| c[0] != "pull" || c[1] == IMAGE_ECHO)
    );
}

/// Section 6.3: an existing install registered as the site's, `managed:
/// false` in externals.yaml and its manifest, verified by the fingerprint
/// of its key files (a viewer's library added changes nothing), its bytes
/// untouched, and never removed by the engine (409).
#[test]
fn a_registered_install_stays_the_site_s_and_is_never_removed() {
    let lab = Lab::new("software-register");
    let site = TempDir::new("software-register-site");
    let home = site.path().join("site-tool-1.0");
    std::fs::create_dir_all(home.join("bin")).unwrap();
    std::fs::create_dir_all(home.join("models")).unwrap();
    std::fs::write(
        home.join("bin/site-tool"),
        b"#!/bin/sh\necho site tool 1.0\n",
    )
    .unwrap();
    std::fs::write(home.join("models/w.bin"), b"weights").unwrap();
    let checksums = |root: &Path| -> Vec<(String, String)> {
        let mut out = Vec::new();
        for rel in ["bin/site-tool", "models/w.bin"] {
            out.push((
                rel.to_string(),
                sha256(&std::fs::read(root.join(rel)).unwrap()),
            ));
        }
        out
    };
    let before = checksums(&home);
    let pack = lab.pack_file("site", PACK_SITE);
    let done = lab.json(
        &[
            "software",
            "register",
            "site-tool@1.0",
            "--kind",
            "sandbox",
            "--path",
            home.to_str().unwrap(),
            "--pack",
            pack.to_str().unwrap(),
            "--json",
        ],
        &[],
    );
    assert_eq!(done["registered"], true, "{done}");
    assert_eq!(done["verified"], true, "{done}");
    let p = &done["package"];
    assert_eq!(p["how"], "registered", "{p}");
    assert_eq!(p["managed"], false, "{p}");
    assert_eq!(p["status"], "registered", "{p}");
    assert_eq!(p["kind"], "sandbox", "{p}");
    assert_eq!(p["path"], home.display().to_string(), "{p}");
    // the pinned image it stands in for, from the pack
    assert_eq!(
        p["digest_asked"],
        format!("sha256:{}", "c".repeat(64)),
        "{p}"
    );
    assert!(p["digest"].is_null(), "{p}");
    let fingerprint = p["fingerprint"].as_str().unwrap().to_string();
    assert!(fingerprint.starts_with("sha256:"), "{p}");
    assert_eq!(p["verification"]["fingerprint"]["passed"], true, "{p}");
    assert_eq!(p["verification"]["probe"]["ran"], false, "{p}");
    assert_eq!(p["verification"]["test"]["ran"], false, "{p}");
    // its own key: no installed copy of the pinned image to compare with
    assert_eq!(
        checksums(&home),
        before,
        "the registered install's bytes are untouched"
    );

    let externals = std::fs::read_to_string(lab.place().join("externals.yaml")).unwrap();
    assert!(externals.contains("spec: \"site-tool@1.0\""), "{externals}");
    assert!(externals.contains("kind: sandbox"), "{externals}");
    assert!(externals.contains("managed: false"), "{externals}");
    assert!(externals.contains(home.to_str().unwrap()), "{externals}");
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(lab.place().join("site-tool/1.0/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["managed"], false, "{manifest}");
    assert_eq!(manifest["how"], "registered", "{manifest}");
    assert_eq!(manifest["image"], home.display().to_string(), "{manifest}");
    // the wrappers and the module are written in the place, never under the path
    assert!(lab.place().join("site-tool/1.0/bin").is_dir());
    let module = std::fs::read_to_string(lab.place().join("modules/site-tool/1.0.lua")).unwrap();
    assert!(module.contains(home.to_str().unwrap()), "{module}");
    assert!(!home.join("manifest.json").exists());
    assert!(!home.join("nils-pack.yaml").exists());

    // a viewer's library beside the key files changes nothing; a key file does
    std::fs::create_dir_all(home.join("lib")).unwrap();
    std::fs::write(home.join("lib/libviewer.so"), b"viewer").unwrap();
    let again = lab.json(&["software", "verify", "site-tool@1.0", "--json"], &[]);
    assert_eq!(again["verified"], true, "{again}");
    assert_eq!(again["package"]["fingerprint"], fingerprint, "{again}");
    std::fs::write(home.join("models/w.bin"), b"other weights").unwrap();
    let changed = lab.json(&["software", "verify", "site-tool@1.0", "--json"], &[]);
    assert_eq!(changed["verified"], false, "{changed}");
    assert_eq!(
        changed["verification"]["fingerprint"]["passed"], false,
        "{changed}"
    );
    assert!(
        changed["verification"]["fingerprint"]["said"]
            .as_str()
            .unwrap()
            .contains("the install changed"),
        "{changed}"
    );
    let listed = lab.json(&["software", "list", "--json"], &[]);
    assert_eq!(
        Lab::package(&listed, "site-tool@1.0")["status"],
        "failed verification"
    );
    std::fs::write(home.join("models/w.bin"), b"weights").unwrap();

    // never removed by the engine
    let (good, _, err) = lab.run(&["software", "remove", "site-tool@1.0"], &[], None);
    assert!(!good);
    assert!(err.contains("registered as the site's install"), "{err}");
    assert!(
        err.contains("never updates or removes what it did not install"),
        "{err}"
    );
    assert!(home.join("bin/site-tool").is_file());
    assert!(lab.place().join("site-tool/1.0/manifest.json").is_file());
    // nor installed over
    let (good, _, err) = lab.run(
        &[
            "software",
            "install",
            "site-tool@1.0",
            "--source",
            &format!("example.org/site-tool@sha256:{}", "c".repeat(64)),
        ],
        &[],
        None,
    );
    assert!(!good);
    assert!(err.contains("does not install over"), "{err}");

    // what is wrong with a registration is said
    let (good, _, err) = lab.run(
        &[
            "software",
            "register",
            "x@1",
            "--kind",
            "sif",
            "--path",
            home.to_str().unwrap(),
        ],
        &[],
        None,
    );
    assert!(!good);
    assert!(err.contains("not a file, which a SIF is"), "{err}");
    let (good, _, err) = lab.run(
        &[
            "software",
            "register",
            "x@1",
            "--kind",
            "home",
            "--path",
            home.to_str().unwrap(),
        ],
        &[],
        None,
    );
    assert!(!good);
    assert!(
        err.contains("--kind is one of sif, sandbox or image"),
        "{err}"
    );
}

/// G9: an acceptance is recorded (who, when, terms, file sha256), the
/// licence file is placed readable by the engine's account alone and its
/// content is never recorded, and a run of the tool is refused before it
/// starts (409, naming the terms' address) until an administrator accepts.
/// Through the doors, under their grants.
#[test]
fn terms_are_accepted_as_events_and_a_run_is_refused_until_then() {
    let lab = Lab::new("software-licence");
    lab.add_pipeline(&descriptor(
        "licensed-tool",
        "3.0",
        IMAGE_LICENSED,
        "gpu: none",
        "  secrets:\n    - {id: tool_license, mount: /license.txt, env: TOOL_LICENSE}\n",
    ));
    let server = Server::start(&lab, 14);

    // a reader holds neither install:see nor pipelines:see; an admin reads
    // the place, and a reader may not install
    let (status, _) = server.send("GET", "/api/software", READER, None);
    assert_eq!(status, 403);
    let (status, listed) = server.send("GET", "/api/software", ADMIN, None);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(
        Lab::package(&listed, "licensed-tool@3.0")["status"],
        "missing",
        "{listed}"
    );
    let (status, _) = server.send(
        "POST",
        "/api/software/install",
        READER,
        Some(&json!({"spec": "licensed-tool@3.0"})),
    );
    assert_eq!(status, 403);

    // installed at the door, with the pack: the terms need an acceptance
    let (status, done) = server.send(
        "POST",
        "/api/software/install",
        ADMIN,
        Some(&json!({"app": "licensed-tool", "version": "3.0", "pack": PACK_LICENSED})),
    );
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["installed"], true, "{done}");
    let p = &done["package"];
    assert_eq!(p["licence"]["accept"], true, "{p}");
    assert_eq!(
        p["licence"]["terms"]["address"], "https://example.org/terms",
        "{p}"
    );
    assert!(p["licence"]["accepted"].is_null(), "{p}");
    assert_eq!(p["licence"]["file"]["present"], false, "{p}");
    let licence = Lab::need(p, "licence");
    assert_eq!(licence["state"], "missing", "{p}");
    assert_eq!(licence["failure"]["cause"], "licence", "{p}");
    assert!(
        licence["said"]
            .as_str()
            .unwrap()
            .contains("https://example.org/terms"),
        "{p}"
    );
    assert!(
        licence["said"]
            .as_str()
            .unwrap()
            .contains("nils software accept licensed-tool@3.0"),
        "{p}"
    );
    assert_eq!(
        licence["fix"]["door"], "POST /api/software/licensed-tool/3.0/accept",
        "{p}"
    );

    // a run is refused before it starts, naming the terms' address
    let (status, refused) = server.send(
        "POST",
        "/api/jobs",
        ADMIN,
        Some(&json!({"command": ["run", "licensed-tool", "--select", "selection:every@1"]})),
    );
    assert_eq!(status, 409, "{refused}");
    let error = refused["error"].as_str().unwrap();
    assert!(error.contains("refused before it started"), "{error}");
    assert!(error.contains("https://example.org/terms"), "{error}");
    assert!(error.contains("(failure: licence)"), "{error}");
    let checks = refused["requirements"].as_array().unwrap();
    let failed = checks.iter().find(|c| c["check"] == "licence").unwrap();
    assert_eq!(failed["ok"], false, "{refused}");
    assert_eq!(failed["failure"]["cause"], "licence", "{refused}");
    assert_eq!(failed["failure"]["kind"], "dependency", "{refused}");

    // accepted, with the site file placed: the file's sha256 is recorded,
    // its content is not
    let licence_text = "licence text that never leaves the file: n4x7-q2zv-8kpm-w3rt\n";
    let (status, accepted) = server.send(
        "POST",
        "/api/software/licensed-tool/3.0/accept",
        ADMIN,
        Some(&json!({"file": licence_text, "note": "accepted for the lab"})),
    );
    assert_eq!(status, 200, "{accepted}");
    let a = &accepted["acceptance"];
    assert_eq!(a["by"], "adm@lab", "{accepted}");
    assert!(a["at"].is_string(), "{accepted}");
    assert_eq!(
        a["terms"]["address"], "https://example.org/terms",
        "{accepted}"
    );
    assert_eq!(a["terms"]["version"], "2026", "{accepted}");
    assert_eq!(
        a["file"]["sha256"],
        sha256(licence_text.as_bytes()),
        "{accepted}"
    );
    assert_eq!(a["file"]["name"], "license.txt", "{accepted}");
    assert_eq!(a["spdx"], "LicenseRef-Site", "{accepted}");
    assert!(!accepted.to_string().contains("n4x7-q2zv"), "{accepted}");
    let file = lab.place().join("licences/licensed-tool/license.txt");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), licence_text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    // the place says so, and the run is no longer refused for its terms
    let (status, listed) = server.send("GET", "/api/software", ADMIN, None);
    assert_eq!(status, 200);
    let p = Lab::package(&listed, "licensed-tool@3.0");
    assert_eq!(p["licence"]["accepted"]["by"], "adm@lab", "{p}");
    assert_eq!(
        p["licence"]["accepted"]["file"]["sha256"],
        sha256(licence_text.as_bytes()),
        "{p}"
    );
    assert_eq!(p["licence"]["file"]["present"], true, "{p}");
    assert_eq!(Lab::need(p, "licence")["state"], "ok", "{p}");
    assert!(
        Lab::need(p, "licence")["said"]
            .as_str()
            .unwrap()
            .contains("accepted by adm@lab"),
        "{p}"
    );
    let (status, after) = server.send(
        "POST",
        "/api/jobs",
        ADMIN,
        Some(&json!({"command": ["run", "licensed-tool", "--select", "selection:every@1"]})),
    );
    assert!(
        !after.to_string().contains("(failure: licence)"),
        "{status} {after}"
    );

    // the acceptance is audited, the licence's content nowhere in the registry
    let (status, audit) = server.send("GET", "/api/audit?action=software.accept", ADMIN, None);
    assert_eq!(status, 200, "{audit}");
    assert!(audit.to_string().contains("software.accept"), "{audit}");
    assert!(
        audit.to_string().contains(&sha256(licence_text.as_bytes())),
        "{audit}"
    );
    drop(server);
    let registry = std::fs::read(lab.home.path().join("registry.db")).unwrap();
    let needle = b"n4x7-q2zv-8kpm-w3rt";
    assert!(
        !registry.windows(needle.len()).any(|w| w == needle),
        "the licence's content is in the registry"
    );
    let (_, out, _) = lab.run(&["software", "list"], &[], None);
    assert!(out.contains("terms accepted by adm"), "{out}");
    assert!(!out.contains("n4x7"), "{out}");

    // the acceptance at the keyboard needs the terms where no pack names them
    let (good, _, err) = lab.run(&["software", "accept", "other@1"], &[], None);
    assert!(!good);
    assert!(err.contains("--terms URL"), "{err}");
    let own = lab.json(
        &[
            "software",
            "accept",
            "other@1",
            "--terms",
            "https://example.org/other",
            "--json",
        ],
        &[],
    );
    assert_eq!(
        own["acceptance"]["terms"]["address"], "https://example.org/other",
        "{own}"
    );
    assert!(own["file"].is_null(), "{own}");
}

/// G10: each missing item (runtime, GPU, memory, disk, converter, licence)
/// reported per item with its fix, at install and at the pre-flight, on a
/// fixture per item.
#[test]
fn each_missing_dependency_is_reported_per_item_with_its_fix() {
    let lab = Lab::new("software-needs");
    let big = "d".repeat(64);
    lab.add_pipeline(&descriptor(
        "needs-gpu",
        "1",
        &format!("example.org/needs-gpu@sha256:{big}"),
        "gpu: required",
        "",
    ));
    lab.add_pipeline(&descriptor(
        "needs-memory",
        "1",
        &format!("example.org/needs-memory@sha256:{big}"),
        "memory-gb: 100000",
        "",
    ));
    lab.add_pipeline(&descriptor(
        "needs-disk",
        "1",
        &format!("example.org/needs-disk@sha256:{big}"),
        "disk-gb: 1000000000",
        "",
    ));
    lab.add_pipeline(&bids_descriptor(
        "needs-converter",
        &format!("example.org/needs-converter@sha256:{big}"),
    ));
    lab.add_pipeline(&descriptor(
        "needs-licence",
        "1",
        &format!("example.org/needs-licence@sha256:{big}"),
        "gpu: none",
        "  secrets:\n    - {id: tool_license, mount: /license.txt, env: TOOL_LICENSE}\n",
    ));
    let listed = lab.json(&["software", "list", "--json"], &[]);
    let fix_of = |c: &Value| {
        c["failure"]["advice"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    let gpu = Lab::need(Lab::package(&listed, "needs-gpu@1"), "gpu");
    assert_eq!(gpu["state"], "missing", "{gpu}");
    assert_eq!(gpu["failure"]["cause"], "gpu_driver", "{gpu}");
    assert!(
        gpu["said"].as_str().unwrap().contains("needs a GPU"),
        "{gpu}"
    );
    assert!(!fix_of(gpu).is_empty(), "{gpu}");

    let memory = Lab::need(Lab::package(&listed, "needs-memory@1"), "memory");
    assert_eq!(memory["state"], "missing", "{memory}");
    assert_eq!(memory["failure"]["cause"], "memory", "{memory}");
    assert!(
        memory["said"].as_str().unwrap().contains("100000 GB"),
        "{memory}"
    );
    assert!(fix_of(memory).contains("more memory"), "{memory}");

    let disk = Lab::need(Lab::package(&listed, "needs-disk@1"), "disk");
    assert_eq!(disk["state"], "missing", "{disk}");
    assert_eq!(disk["failure"]["cause"], "disk", "{disk}");
    assert!(fix_of(disk).contains("free space"), "{disk}");

    let converter = Lab::need(Lab::package(&listed, "needs-converter@1"), "converter");
    assert_eq!(converter["state"], "missing", "{converter}");
    assert_eq!(converter["failure"]["cause"], "dcm2niix", "{converter}");
    assert_eq!(
        converter["failure"]["fix"]["command"], "nils software install dcm2niix",
        "{converter}"
    );
    assert!(
        fix_of(converter).contains("nils software install dcm2niix"),
        "{converter}"
    );

    let licence = Lab::need(Lab::package(&listed, "needs-licence@1"), "licence");
    assert_eq!(licence["state"], "missing", "{licence}");
    assert_eq!(licence["failure"]["cause"], "licence", "{licence}");
    assert!(
        licence["said"].as_str().unwrap().contains("tool_license"),
        "{licence}"
    );
    assert!(
        licence["said"]
            .as_str()
            .unwrap()
            .contains("nils software accept needs-licence@1 --file PATH"),
        "{licence}"
    );

    // the same items at the install, which a missing runtime refuses
    let (good, _, err) = lab.run(&["pipeline", "runtime", "--set", "off"], &[], None);
    assert!(good, "{err}");
    let listed = lab.json(&["software", "list", "--json"], &[]);
    let runtime = Lab::need(Lab::package(&listed, "needs-gpu@1"), "runtime");
    assert_eq!(runtime["state"], "missing", "{runtime}");
    assert_eq!(runtime["failure"]["cause"], "runtime", "{runtime}");
    assert!(
        runtime["said"]
            .as_str()
            .unwrap()
            .contains("pipelines are off"),
        "{runtime}"
    );
    assert!(!fix_of(runtime).is_empty(), "{runtime}");
    let (good, _, err) = lab.run(&["software", "install", "needs-gpu@1"], &[], None);
    assert!(!good);
    assert!(err.contains("pipelines are off"), "{err}");
    assert!(err.contains("runtime"), "{err}");
    assert!(!lab.place().join("needs-gpu").exists());
}

/// Section 6.7: the converter is one package among the others, installed
/// with `nils software install dcm2niix`; `nils tools install dcm2niix`
/// stays as its alias for one release.
#[test]
fn the_converter_is_one_package_among_the_others() {
    let lab = Lab::new("software-converter");
    let listed = lab.json(&["software", "list", "--json"], &[]);
    let p = Lab::package(&listed, "dcm2niix@v1.0.20260724");
    assert_eq!(p["kind"], "binary", "{p}");
    assert_eq!(p["status"], "missing", "{p}");
    assert_eq!(
        p["install"]["command"], "nils software install dcm2niix",
        "{p}"
    );
    assert_eq!(p["install"]["alias"], "nils tools install dcm2niix", "{p}");
    assert_eq!(p["tool"]["name"], "dcm2niix", "{p}");
    assert_eq!(p["tool"]["present"], false, "{p}");
    // the old door and command list the same tool
    let tools = lab.json(&["tools", "list", "--json"], &[]);
    assert_eq!(tools["tools"][0]["name"], "dcm2niix", "{tools}");
    assert_eq!(
        tools["tools"][0]["install"]["command"], "nils software install dcm2niix",
        "{tools}"
    );
    // another version of the converter is not this release's
    let (good, _, err) = lab.run(&["software", "install", "dcm2niix@v0.1"], &[], None);
    assert!(!good);
    assert!(err.contains("alone"), "{err}");
    let (good, _, err) = lab.run(&["software", "remove", "dcm2niix"], &[], None);
    assert!(!good);
    assert!(err.contains("no install of dcm2niix of its own"), "{err}");
}

/// Which real docker this machine offers a test, if any: the host's, where
/// an operator lets a test run it (`NILS_TEST_PIPELINE_RUNTIME=docker`, as
/// the pipeline tests take it).
fn real_docker() -> bool {
    std::env::var("NILS_TEST_PIPELINE_RUNTIME").as_deref() == Ok("docker")
        && std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("docker").is_file()))
}

/// G7 on a real runtime: a small public image pulled by its digest from its
/// own source, the digest the store reports equal to the one asked, the
/// probe and the test run in the real container, the wrapper run by hand
/// through the same image, and the image taken out of the store again on
/// remove. Runs where NILS_TEST_PIPELINE_RUNTIME is docker and docker is
/// here, and says so and passes elsewhere.
#[test]
fn a_real_image_is_pulled_by_digest_verified_and_removed() {
    if !real_docker() {
        eprintln!("NILS_TEST_PIPELINE_RUNTIME is not docker, or docker is not here; the live pull is not run");
        return;
    }
    const BUSYBOX: &str =
        "docker.io/library/busybox@sha256:66a6306db78bf2dbf3487f293aa8d6990d8e506fdffab9cc43fe422becf886e4";
    const BUSYBOX_DIGEST: &str =
        "sha256:66a6306db78bf2dbf3487f293aa8d6990d8e506fdffab9cc43fe422becf886e4";
    let pack = format!(
        r#"nils_pack: 1
pipeline: {{name: busybox-echo, descriptor_sha256: "sha256:00"}}
source:
  image: "{BUSYBOX}"
  probe: {{command: ["busybox", "--help"], expect: "BusyBox v1.37.0"}}
licence: {{spdx: GPL-2.0-only, redistributable: true, accept: false}}
tests: {{command: ["sh", "-c", "echo the test ran"], expect: "the test ran"}}
"#
    );
    let lab = Lab::with_real_docker("software-live");
    lab.add_pipeline(&descriptor("busybox-echo", "1.37.0", BUSYBOX, "gpu: none", ""));
    let pack = lab.pack_file("busybox", &pack);
    let done = lab.json(
        &["software", "install", "busybox-echo@1.37.0", "--pack", pack.to_str().unwrap(), "--json"],
        &[],
    );
    assert_eq!(done["installed"], true, "{done}");
    assert_eq!(done["verified"], true, "{done}");
    let p = &done["package"];
    assert_eq!(p["digest_asked"], BUSYBOX_DIGEST, "{p}");
    assert_eq!(p["digest"], BUSYBOX_DIGEST, "{p}");
    assert_eq!(p["fingerprint"], BUSYBOX_DIGEST, "{p}");
    assert!(p["size_bytes"].as_u64().unwrap() > 1_000_000, "{p}");
    assert_eq!(p["verification"]["probe"]["passed"], true, "{p}");
    assert!(p["verification"]["probe"]["said"].as_str().unwrap().contains("BusyBox v1.37.0"), "{p}");
    assert_eq!(p["verification"]["test"]["passed"], true, "{p}");
    // by hand, through the same image
    let said = Command::new(lab.place().join("bin").join("busybox"))
        .arg("--help")
        .env("PATH", &lab.path)
        .output()
        .unwrap();
    assert!(said.status.success(), "{}", String::from_utf8_lossy(&said.stderr));
    assert!(
        String::from_utf8_lossy(&said.stdout).contains("BusyBox v1.37.0"),
        "{}",
        String::from_utf8_lossy(&said.stdout)
    );
    let removed = lab.json(&["software", "remove", "busybox-echo@1.37.0", "--json"], &[]);
    assert_eq!(removed["removed"], true, "{removed}");
    assert!(!lab.place().join("busybox-echo").exists());
}
