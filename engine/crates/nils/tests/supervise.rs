// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 5 section 10.4, bar 5: the supervisor updates a part from a signed
//! artifact, refuses an unsigned one with a named reason, and refuses a
//! tampered one with a named reason. The part under test is a file whose
//! content is its version, the restart command is `true`, and the channel
//! is a directory reached by file:// URLs.
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Out {
    let out = nils().args(args).output().expect("nils runs");
    Out {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

fn target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// A key pair, a part directory holding VERSION = `version`, its artifact
/// packed and signed into the channel directory, and latest.json.
struct Channel {
    dir: TempDir,
    keys: PathBuf,
}

impl Channel {
    fn new() -> Channel {
        let dir = TempDir::new("supervise-channel");
        let keys = dir.path().join("keys");
        let o = run(&["supervise", "keygen", "--out", keys.to_str().unwrap()]);
        assert!(o.ok, "{}", o.stderr);
        Channel { dir, keys }
    }

    fn trust(&self) -> PathBuf {
        self.keys.join("supervise.pub")
    }

    /// Publish one version of `part`; answers the manifest path.
    fn publish(&self, part: &str, version: &str, contracts: &[&str], sign: bool) -> PathBuf {
        let src = self
            .dir
            .path()
            .join("src")
            .join(format!("{part}-{version}"));
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::write(src.join("VERSION"), format!("{version}\n")).unwrap();
        std::fs::write(
            src.join("lib").join("notes.txt"),
            format!("{part} {version}"),
        )
        .unwrap();
        let out = self.dir.path().join("channel").join(part);
        let mut args = vec![
            "supervise",
            "pack",
            "--part",
            part,
            "--version",
            version,
            "--dir",
            src.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ];
        for c in contracts {
            args.push("--contract");
            args.push(c);
        }
        let o = run(&args);
        assert!(o.ok, "{}", o.stderr);
        let manifest = PathBuf::from(o.stdout.trim());
        if sign {
            let o = run(&[
                "supervise",
                "sign",
                "--key",
                self.keys.join("supervise.key").to_str().unwrap(),
                manifest.to_str().unwrap(),
            ]);
            assert!(o.ok, "{}", o.stderr);
        }
        let stem = manifest
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".json")
            .to_string();
        std::fs::write(
            out.join("latest.json"),
            serde_json::json!({
                "version": version,
                "manifest_url": format!("{stem}.json"),
                "artifact_url": format!("{stem}.tar.gz"),
                "signature_url": format!("{stem}.sig"),
            })
            .to_string(),
        )
        .unwrap();
        manifest
    }

    fn channel_url(&self) -> String {
        format!("file://{}", self.dir.path().join("channel").display())
    }

    /// A supervise.toml over one part installed at `install`.
    fn config(&self, part: &str, install: &Path, restart: &str) -> PathBuf {
        let path = self.dir.path().join("supervise.toml");
        std::fs::write(
            &path,
            format!(
                "trust = \"{}\"\nlog = \"{}\"\nsettle_seconds = 2\n[tokens]\n\"a-supervisor-token-of-length\" = \"admin@test\"\n[[parts]]\nname = \"{part}\"\ninstall = \"{}\"\nrestart = \"{restart}\"\nchannel = \"{}\"\nversion_file = \"VERSION\"\n",
                self.trust().display(),
                self.dir.path().join("supervise.log").display(),
                install.display(),
                self.channel_url()
            ),
        )
        .unwrap();
        path
    }
}

#[test]
fn keygen_sign_and_verify_round_trip() {
    let c = Channel::new();
    let manifest = c.publish("engine", "1.0.1", &["openapi=3", "pack=4"], true);
    let o = run(&[
        "supervise",
        "verify",
        "--trust",
        c.trust().to_str().unwrap(),
        manifest.to_str().unwrap(),
    ]);
    assert!(o.ok, "{}", o.stderr);
    let doc: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(doc["verified"], true);
    assert_eq!(doc["manifest"]["part"], "engine");
    assert_eq!(doc["manifest"]["version"], "1.0.1");
    assert_eq!(doc["manifest"]["target"], target());
    assert_eq!(doc["manifest"]["contracts"]["pack"], "4");
    assert_eq!(
        doc["manifest"]["files"],
        serde_json::json!(["VERSION", "lib/notes.txt"])
    );
    // the private key is the signer's alone
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(c.keys.join("supervise.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn an_unsigned_artifact_is_refused_with_the_named_reason() {
    let c = Channel::new();
    let manifest = c.publish("engine", "1.0.1", &[], false);
    let o = run(&[
        "supervise",
        "verify",
        "--trust",
        c.trust().to_str().unwrap(),
        manifest.to_str().unwrap(),
    ]);
    assert!(!o.ok);
    assert!(o.stderr.contains("refused (no signature)"), "{}", o.stderr);
    // a signature by a key the deployment does not trust is bad, not absent
    let other = TempDir::new("supervise-other");
    let o = run(&[
        "supervise",
        "keygen",
        "--out",
        other.path().to_str().unwrap(),
    ]);
    assert!(o.ok);
    let o = run(&[
        "supervise",
        "sign",
        "--key",
        other.path().join("supervise.key").to_str().unwrap(),
        manifest.to_str().unwrap(),
    ]);
    assert!(o.ok, "{}", o.stderr);
    let o = run(&[
        "supervise",
        "verify",
        "--trust",
        c.trust().to_str().unwrap(),
        manifest.to_str().unwrap(),
    ]);
    assert!(!o.ok);
    assert!(o.stderr.contains("refused (bad signature)"), "{}", o.stderr);
}

#[test]
fn a_tampered_tarball_fails_the_digest_and_a_wrong_target_or_contract_is_named() {
    let c = Channel::new();
    let manifest = c.publish("engine", "1.0.1", &["pack=5"], true);
    let tarball = manifest.with_file_name(
        manifest
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(".json", ".tar.gz"),
    );
    let mut bytes = std::fs::read(&tarball).unwrap();
    bytes.push(0);
    std::fs::write(&tarball, bytes).unwrap();
    let o = run(&[
        "supervise",
        "verify",
        "--trust",
        c.trust().to_str().unwrap(),
        manifest.to_str().unwrap(),
    ]);
    assert!(!o.ok);
    assert!(
        o.stderr.contains("refused (digest mismatch)"),
        "{}",
        o.stderr
    );

    let manifest = c.publish("engine", "1.0.2", &["pack=5"], true);
    let o = run(&[
        "supervise",
        "verify",
        "--trust",
        c.trust().to_str().unwrap(),
        "--target",
        "plan9-mips",
        manifest.to_str().unwrap(),
    ]);
    assert!(!o.ok);
    assert!(
        o.stderr.contains("refused (target mismatch)"),
        "{}",
        o.stderr
    );

    // what is installed speaks pack v4; the artifact speaks v5: refused unless allowed
    let installed = c.dir.path().join("installed.json");
    std::fs::write(&installed, serde_json::json!({ "part": "engine", "version": "1.0.0", "target": target(), "contracts": { "pack": "4" }, "sha256": "", "built_at": "", "files": [] }).to_string()).unwrap();
    let o = run(&[
        "supervise",
        "verify",
        "--trust",
        c.trust().to_str().unwrap(),
        "--installed",
        installed.to_str().unwrap(),
        manifest.to_str().unwrap(),
    ]);
    assert!(!o.ok);
    assert!(
        o.stderr.contains("refused (contract mismatch)")
            && o.stderr.contains("pack is 4 installed"),
        "{}",
        o.stderr
    );
    let o = run(&[
        "supervise",
        "verify",
        "--trust",
        c.trust().to_str().unwrap(),
        "--installed",
        installed.to_str().unwrap(),
        "--allow-contract-change",
        manifest.to_str().unwrap(),
    ]);
    assert!(o.ok, "{}", o.stderr);
}

#[test]
fn the_update_by_hand_applies_a_signed_artifact_and_refuses_an_unsigned_one() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");

    c.publish("engine", "1.0.1", &["openapi=3"], true);
    let o = run(&[
        "supervise",
        "update",
        "--config",
        config.to_str().unwrap(),
        "engine",
    ]);
    assert!(o.ok, "{}", o.stderr);
    let row: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(row["ok"], true);
    assert_eq!(row["from"], serde_json::Value::Null);
    assert_eq!(row["to"], "1.0.1");
    assert_eq!(
        std::fs::read_to_string(install.join("VERSION"))
            .unwrap()
            .trim(),
        "1.0.1"
    );
    assert_eq!(
        std::fs::read_to_string(install.join("lib/notes.txt")).unwrap(),
        "engine 1.0.1"
    );
    assert!(
        install.join(".previous/VERSION").exists(),
        "the previous file is kept"
    );
    let installed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(install.join("installed.json")).unwrap())
            .unwrap();
    assert_eq!(installed["version"], "1.0.1");

    // an unsigned 1.0.2 on the channel is refused by name, nothing moves, and the log says so
    c.publish("engine", "1.0.2", &["openapi=3"], false);
    let o = run(&[
        "supervise",
        "update",
        "--config",
        config.to_str().unwrap(),
        "engine",
    ]);
    assert!(!o.ok);
    assert!(o.stderr.contains("no signature"), "{}", o.stderr);
    assert_eq!(
        std::fs::read_to_string(install.join("VERSION"))
            .unwrap()
            .trim(),
        "1.0.1"
    );
    let log = std::fs::read_to_string(c.dir.path().join("supervise.log")).unwrap();
    let rows: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["ok"], true);
    assert_eq!(rows[1]["ok"], false);
    assert!(rows[1]["why"].as_str().unwrap().contains("no signature"));

    // a version the channel does not name is refused before anything is fetched
    let o = run(&[
        "supervise",
        "update",
        "--config",
        config.to_str().unwrap(),
        "engine",
        "--version",
        "9.9.9",
    ]);
    assert!(!o.ok);
    assert!(o.stderr.contains("the channel names 1.0.2"), "{}", o.stderr);
}

#[test]
fn a_part_that_does_not_come_back_is_rolled_back() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    // the restart rewrites the version file to what was there: the part never answers with the new version
    let restart = format!("printf '1.0.0\\n' > {}", install.join("VERSION").display());
    let config = c.config("engine", &install, &restart);
    c.publish("engine", "1.0.1", &[], true);
    let o = run(&[
        "supervise",
        "update",
        "--config",
        config.to_str().unwrap(),
        "engine",
    ]);
    assert!(!o.ok);
    assert!(
        o.stderr.contains("did not answer with 1.0.1") && o.stderr.contains("rolled back"),
        "{}",
        o.stderr
    );
    assert_eq!(
        std::fs::read_to_string(install.join("VERSION"))
            .unwrap()
            .trim(),
        "1.0.0"
    );
    assert!(!install.join("installed.json").exists());
}

struct Service {
    child: Child,
    url: String,
}

impl Service {
    fn start(config: &Path) -> Service {
        Service::start_with(config, &[])
    }

    fn start_with(config: &Path, env: &[(&str, &str)]) -> Service {
        let mut child = nils()
            .envs(env.iter().copied())
            .args([
                "supervise",
                "run",
                "--config",
                config.to_str().unwrap(),
                "--bind",
                "127.0.0.1:0",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the service starts");
        let mut line = String::new();
        let mut stdout = child.stdout.take().unwrap();
        let mut buf = [0u8; 1];
        while stdout.read(&mut buf).unwrap_or(0) == 1 {
            if buf[0] == b'\n' {
                break;
            }
            line.push(buf[0] as char);
        }
        let addr = line
            .split_whitespace()
            .nth(2)
            .expect("the bound address")
            .to_string();
        Service {
            child,
            url: format!("http://{addr}"),
        }
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        token: Option<&str>,
    ) -> (u16, serde_json::Value) {
        let mut stream = TcpStream::connect(self.url.trim_start_matches("http://")).unwrap();
        let body = body.unwrap_or("");
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        if !body.is_empty() {
            head.push_str("Content-Type: application/json\r\n");
        }
        if let Some(t) = token {
            head.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (
            status,
            serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
        )
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_door_reports_what_is_installed_and_updates_a_part_under_a_token() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");
    c.publish("engine", "1.0.1", &["openapi=3"], true);
    let s = Service::start(&config);
    let token = Some("a-supervisor-token-of-length");

    let (status, body) = s.call("GET", "/api/supervise/capabilities", None, None);
    assert_eq!(status, 401, "{body}");

    let (status, caps) = s.call("GET", "/api/supervise/capabilities", None, token);
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps["parts"][0]["name"], "engine");
    assert_eq!(caps["parts"][0]["version"], serde_json::Value::Null);
    assert_eq!(caps["parts"][0]["health"]["version"], "1.0.0");
    assert_eq!(caps["parts"][0]["newer"]["version"], "1.0.1");
    assert_eq!(caps["supervisor"]["target"], target());

    let (status, done) = s.call(
        "POST",
        "/api/supervise/update",
        Some(r#"{"part":"engine"}"#),
        token,
    );
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["applied"]["to"], "1.0.1");
    assert_eq!(done["by"], "admin@test");
    assert_eq!(
        std::fs::read_to_string(install.join("VERSION"))
            .unwrap()
            .trim(),
        "1.0.1"
    );

    let (status, caps) = s.call("GET", "/api/supervise/capabilities", None, token);
    assert_eq!(status, 200);
    assert_eq!(caps["parts"][0]["version"], "1.0.1");
    assert_eq!(caps["parts"][0]["newer"], serde_json::Value::Null);

    let (status, refused) = s.call(
        "POST",
        "/api/supervise/update",
        Some(r#"{"part":"desk"}"#),
        token,
    );
    assert_eq!(status, 404, "{refused}");

    let (status, log) = s.call("GET", "/api/supervise/log", None, token);
    assert_eq!(status, 200);
    assert_eq!(log["rows"].as_array().unwrap().len(), 1);
    assert_eq!(log["rows"][0]["digest"], caps["parts"][0]["digest"]);
}

/// The install door (the desk's Settings read it): an install nils setup
/// recorded is reported as recorded; a restart runs apart from the door and
/// ends with its reason; and a folder is looked inside before it is added,
/// by what its files are.
#[test]
fn the_door_reports_the_install_restarts_apart_and_looks_inside_a_folder() {
    use nils_dicom::synth::{self, MetaFields};
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");
    let home = TempDir::new("supervise-install");
    let settings = home.path().join("config");
    std::fs::create_dir_all(settings.join("nils")).unwrap();
    std::fs::write(
        settings.join("nils").join("setup.toml"),
        format!(
            "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"none\"\nreach = \"loopback\"\nbackend = \"sqlite\"\nat = \"2026-09-13T00:00:00Z\"\n\n[ports]\nengine = 8437\ndesk = 7200\nkvasir = 7100\nassistant = 7300\n\n[parts.engine]\nversion = \"1.0.0-alpha.14\"\npath = \"/usr/local/bin/nils\"\n\n[parts.desk]\nversion = \"1.0.0-alpha.14\"\npath = \"/usr/local/bin/nils-desk\"\n",
            home.path().join("nils").display()
        ),
    )
    .unwrap();
    let s = Service::start_with(
        &config,
        &[
            ("XDG_CONFIG_HOME", settings.to_str().unwrap()),
            // a channel that refuses at once, so the release check says so and nothing waits
            ("NILS_RELEASES", "http://127.0.0.1:9/releases"),
        ],
    );
    let token = Some("a-supervisor-token-of-length");

    let (status, _) = s.call("GET", "/api/supervise/install", None, None);
    assert_eq!(status, 401);
    let (status, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["runtime"], "machine", "{doc}");
    assert_eq!(doc["parts"]["engine"]["version"], "1.0.0-alpha.14", "{doc}");
    assert_eq!(
        doc["services"],
        serde_json::json!([]),
        "no services, so none to look at"
    );
    let engine = doc["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["part"] == "engine")
        .unwrap();
    assert_eq!(engine["address"], "127.0.0.1:8437", "{doc}");
    assert_eq!(engine["reach"], "this machine only", "{doc}");
    assert_eq!(doc["release"]["newer"], serde_json::Value::Null, "{doc}");
    assert_eq!(doc["release"]["command"], "nils update --all", "{doc}");

    let (status, run) = s.call(
        "POST",
        "/api/supervise/restart",
        Some(r#"{"part":"engine"}"#),
        token,
    );
    assert_eq!(status, 202, "{run}");
    let id = run["id"].as_str().unwrap().to_string();
    let mut ended = serde_json::Value::Null;
    for _ in 0..100 {
        let (status, doc) = s.call("GET", &format!("/api/supervise/runs/{id}"), None, token);
        assert_eq!(status, 200, "{doc}");
        if doc["state"] != "running" {
            ended = doc;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(ended["state"], "failed", "{ended}");
    assert!(
        ended["tail"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap_or("").contains("runs no services")),
        "{ended}"
    );
    let (status, refused) = s.call(
        "POST",
        "/api/supervise/restart",
        Some(r#"{"part":"everything"}"#),
        token,
    );
    assert_eq!(status, 400, "{refused}");

    let incoming = home.path().join("incoming");
    for (i, sop) in ["1.2.3.1.1.1", "1.2.3.1.1.2"].iter().enumerate() {
        let mr = synth::minimal_mr("1.2.3", "1.2.3.1", sop);
        std::fs::create_dir_all(incoming.join("mri-3t").join("a")).unwrap();
        std::fs::write(
            incoming.join("mri-3t").join("a").join(format!("IM_{i}")),
            synth::part10(&MetaFields::mr(sop), &mr, true),
        )
        .unwrap();
    }
    std::fs::create_dir_all(incoming.join("notes")).unwrap();
    std::fs::write(incoming.join("notes").join("readme.txt"), "not dicom").unwrap();
    std::fs::write(incoming.join("list.csv"), "a,b").unwrap();
    let body = serde_json::json!({ "path": incoming.display().to_string() }).to_string();
    let (status, seen) = s.call("POST", "/api/supervise/look", Some(&body), token);
    assert_eq!(status, 200, "{seen}");
    assert_eq!(seen["folders"][0]["name"], "mri-3t", "{seen}");
    assert_eq!(seen["folders"][0]["files"], 2, "{seen}");
    assert_eq!(seen["folders"][0]["dicom"], 2, "{seen}");
    assert_eq!(seen["folders"][0]["modalities"]["MR"], 2, "{seen}");
    assert_eq!(seen["folders"][1]["name"], "notes", "{seen}");
    assert_eq!(seen["folders"][1]["dicom"], 0, "{seen}");
    assert_eq!(seen["here"]["files"], 1, "{seen}");
    let missing = serde_json::json!({ "path": home.path().join("nowhere").display().to_string() })
        .to_string();
    let (status, none) = s.call("POST", "/api/supervise/look", Some(&missing), token);
    assert_eq!(status, 200, "{none}");
    assert_eq!(none["exists"], false, "{none}");
    let (status, _) = s.call(
        "POST",
        "/api/supervise/look",
        Some(r#"{"path":"relative"}"#),
        token,
    );
    assert_eq!(status, 400);
}
