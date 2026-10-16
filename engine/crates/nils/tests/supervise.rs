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
        self.config_with(part, install, restart, "")
    }

    /// The same, with `top` more lines of the file's own keys, as `look_again_seconds = 0`.
    fn config_with(&self, part: &str, install: &Path, restart: &str, top: &str) -> PathBuf {
        let path = self.dir.path().join("supervise.toml");
        std::fs::write(
            &path,
            format!(
                "trust = \"{}\"\nlog = \"{}\"\nsettle_seconds = 2\n{top}[tokens]\n\"a-supervisor-token-of-length\" = \"admin@test\"\n[[parts]]\nname = \"{part}\"\ninstall = \"{}\"\nrestart = \"{restart}\"\nchannel = \"{}\"\nversion_file = \"VERSION\"\n",
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

/// The install door says each part beside its own newest release: a desk
/// released alone is offered while the engine is at its newest, by name, and
/// the update of one part is asked for by its name.
#[test]
fn the_install_door_offers_a_desk_released_alone() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");
    let home = TempDir::new("supervise-parts");
    let settings = home.path().join("config");
    std::fs::create_dir_all(settings.join("nils")).unwrap();
    std::fs::write(
        settings.join("nils").join("setup.toml"),
        format!(
            "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"none\"\n\n[parts.engine]\nversion = \"1.0.0-alpha.14\"\npath = \"/usr/local/bin/nils\"\n\n[parts.desk]\nversion = \"1.0.0-alpha.14\"\npath = \"/usr/local/bin/nils-desk\"\n",
            home.path().join("nils").display()
        ),
    )
    .unwrap();
    let channel = |name: &str, version: &str| {
        let dir = home.path().join(name);
        std::fs::create_dir_all(dir.join("latest").join("download")).unwrap();
        std::fs::write(dir.join("latest").join("download").join("VERSION"), version).unwrap();
        format!("file://{}", dir.display())
    };
    let engine = channel("engine-releases", "1.0.0-alpha.14\n");
    let desk = channel("desk-releases", "1.0.0-alpha.15\n");
    let s = Service::start_with(
        &config,
        &[
            ("XDG_CONFIG_HOME", settings.to_str().unwrap()),
            ("NILS_RELEASES", engine.as_str()),
            ("NILS_DESK_RELEASES", desk.as_str()),
        ],
    );
    let token = Some("a-supervisor-token-of-length");
    let (status, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(status, 200, "{doc}");
    let release = &doc["release"];
    assert_eq!(release["installed"], "1.0.0-alpha.14", "{doc}");
    assert_eq!(release["newest"], "1.0.0-alpha.14", "{doc}");
    assert_eq!(release["behind"], serde_json::json!(["desk"]), "{doc}");
    assert_eq!(release["newer"], "desk 1.0.0-alpha.15", "{doc}");
    let parts = release["parts"].as_array().expect("each part");
    let of = |name: &str| parts.iter().find(|p| p["part"] == name).cloned().unwrap();
    assert_eq!(of("engine")["newer"], serde_json::Value::Null, "{doc}");
    assert_eq!(of("desk")["installed"], "1.0.0-alpha.14", "{doc}");
    assert_eq!(of("desk")["newest"], "1.0.0-alpha.15", "{doc}");
    assert_eq!(of("desk")["newer"], "1.0.0-alpha.15", "{doc}");
    assert_eq!(of("desk")["command"], "nils update --part desk", "{doc}");

    let (status, refused) = s.call(
        "POST",
        "/api/supervise/update-all",
        Some(r#"{"part":"postgres"}"#),
        token,
    );
    assert_eq!(status, 400, "{refused}");
}

/// An engine install recorded at one version and a channel of files naming
/// another as its newest release, each changed between two calls as an update
/// and a publication change them.
struct Releases {
    home: TempDir,
    settings: PathBuf,
}

impl Releases {
    fn new(installed: &str, newest: &str) -> Releases {
        let home = TempDir::new("supervise-releases");
        let settings = home.path().join("config");
        std::fs::create_dir_all(settings.join("nils")).unwrap();
        std::fs::create_dir_all(home.path().join("releases/latest/download")).unwrap();
        let releases = Releases { home, settings };
        releases.update_to(installed);
        releases.publish(newest);
        releases
    }

    /// The setup record says the engine is at `version`, as an update leaves it.
    fn update_to(&self, version: &str) {
        std::fs::write(
            self.settings.join("nils").join("setup.toml"),
            format!(
                "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"none\"\n\n[parts.engine]\nversion = \"{version}\"\npath = \"{}\"\n",
                self.home.path().join("nils").display(),
                self.home.path().join("engine").join("nils").display()
            ),
        )
        .unwrap();
    }

    /// The channel names `version` as its newest release.
    fn publish(&self, version: &str) {
        std::fs::write(
            self.home.path().join("releases/latest/download/VERSION"),
            format!("{version}\n"),
        )
        .unwrap();
    }

    fn serve(&self, config: &Path) -> Service {
        let url = format!("file://{}", self.home.path().join("releases").display());
        Service::start_with(
            config,
            &[
                ("XDG_CONFIG_HOME", self.settings.to_str().unwrap()),
                ("NILS_RELEASES", url.as_str()),
            ],
        )
    }
}

/// The engine's own row of an install door's answer.
fn engine_row(doc: &serde_json::Value) -> serde_json::Value {
    doc["release"]["parts"]
        .as_array()
        .expect("each part")
        .iter()
        .find(|p| p["part"] == "engine")
        .cloned()
        .expect("the engine's row")
}

/// 2026-10-16: the desk's Parts page named alpha.84 as the newest release a
/// minute after alpha.85 was published, because the door keeps what it looked
/// at for a quarter of an hour and said nothing of how old that was. It says
/// when it looked, and looks again when asked.
#[test]
fn the_install_door_says_when_it_looked_and_looks_again_when_asked() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    // a minute need not pass between two looks of this test's
    let config = c.config_with("engine", &install, "true", "look_again_seconds = 0\n");
    let releases = Releases::new("1.0.0-alpha.83", "1.0.0-alpha.84");
    let s = releases.serve(&config);
    let token = Some("a-supervisor-token-of-length");

    let (status, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["release"]["installed"], "1.0.0-alpha.83", "{doc}");
    assert_eq!(doc["release"]["newer"], "1.0.0-alpha.84", "{doc}");
    let at = doc["release"]["looked_at"].clone();

    // alpha.85 is published; a plain read keeps what was looked at, as the page did on 2026-10-16
    releases.publish("1.0.0-alpha.85");
    let (_, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(doc["release"]["newer"], "1.0.0-alpha.84", "{doc}");

    // asked again, it looks again
    let (status, doc) = s.call("GET", "/api/supervise/install?fresh=1", None, token);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["release"]["newest"], "1.0.0-alpha.85", "{doc}");
    assert_eq!(doc["release"]["newer"], "1.0.0-alpha.85", "{doc}");
    assert_eq!(engine_row(&doc)["newer"], "1.0.0-alpha.85", "{doc}");

    // and says when it looked: to the second, in UTC, and no earlier than the first look
    let again = doc["release"]["looked_at"]
        .as_str()
        .unwrap_or_else(|| panic!("the door says when it looked: {doc}"))
        .to_string();
    let first = at
        .as_str()
        .unwrap_or_else(|| panic!("the first look says when too: {at}"));
    assert!(
        again.len() == 20 && again.ends_with('Z') && again.as_str() >= first,
        "{first} {again}"
    );

    // a plain read after it keeps that look, until it is asked again
    releases.publish("1.0.0-alpha.86");
    let (_, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(doc["release"]["newer"], "1.0.0-alpha.85", "{doc}");
    assert_eq!(doc["release"]["looked_at"], again.as_str(), "{doc}");
}

/// What the door looked at may be older than what is installed, as it is the
/// moment an update lands: the record is read on every call and an offer is
/// only a release newer than it, so nothing older is ever offered. And a
/// person's asking again is let through no more than once a minute, which is
/// what GitHub's sixty requests an hour for an address allow.
#[test]
fn the_install_door_never_offers_an_older_release_and_looks_again_no_more_than_once_a_minute() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");
    let releases = Releases::new("1.0.0-alpha.83", "1.0.0-alpha.84");
    let s = releases.serve(&config);
    let token = Some("a-supervisor-token-of-length");

    let (_, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(doc["release"]["newer"], "1.0.0-alpha.84", "{doc}");
    let at = doc["release"]["looked_at"].clone();
    assert!(at.is_string(), "the door says when it looked: {doc}");

    // the update lands (alpha.85, past what the door looked at): the look is
    // older than what is installed, and offers nothing
    releases.update_to("1.0.0-alpha.85");
    let (_, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(doc["release"]["installed"], "1.0.0-alpha.85", "{doc}");
    assert_eq!(doc["release"]["newest"], "1.0.0-alpha.84", "{doc}");
    assert_eq!(doc["release"]["newer"], serde_json::Value::Null, "{doc}");
    assert_eq!(doc["release"]["behind"], serde_json::json!([]), "{doc}");
    assert_eq!(engine_row(&doc)["newer"], serde_json::Value::Null, "{doc}");

    // asked again within the minute, the door does not look: the look it
    // kept is the one it answers with, and says so
    releases.publish("1.0.0-alpha.86");
    let (status, doc) = s.call("GET", "/api/supervise/install?fresh=1", None, token);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["release"]["newest"], "1.0.0-alpha.84", "{doc}");
    assert_eq!(doc["release"]["newer"], serde_json::Value::Null, "{doc}");
    assert_eq!(doc["release"]["looked_at"], at, "{doc}");
}

/// The signed channel's door (the first of the supervisor's) offers a release
/// newer than the installed one, never one that only differs: a channel that
/// still names an older release after a newer one is installed offers nothing.
#[test]
fn the_channel_door_offers_no_release_older_than_the_installed_one() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");
    c.publish("engine", "1.0.1", &["openapi=3"], true);
    let s = Service::start(&config);
    let token = Some("a-supervisor-token-of-length");
    let (status, done) = s.call(
        "POST",
        "/api/supervise/update",
        Some(r#"{"part":"engine"}"#),
        token,
    );
    assert_eq!(status, 200, "{done}");

    // the channel's latest goes back to 1.0.0: it differs, and is older
    c.publish("engine", "1.0.0", &["openapi=3"], true);
    let (_, caps) = s.call("GET", "/api/supervise/capabilities", None, token);
    assert_eq!(caps["parts"][0]["version"], "1.0.1", "{caps}");
    assert_eq!(caps["parts"][0]["newer"], serde_json::Value::Null, "{caps}");

    // a later one is offered
    c.publish("engine", "1.0.2", &["openapi=3"], true);
    let (_, caps) = s.call("GET", "/api/supervise/capabilities", None, token);
    assert_eq!(caps["parts"][0]["newer"]["version"], "1.0.2", "{caps}");
}

/// The install door says the rule packs beside the ones the engine's release
/// carries, read where the engine reads them, and offers the update that
/// takes them when only they are behind.
#[test]
fn the_install_door_says_packs_behind_the_engines_release() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");
    let home = TempDir::new("supervise-packs");
    let packs = home.path().join("engine").join("packs");
    std::fs::create_dir_all(packs.join("mri")).unwrap();
    std::fs::write(packs.join("mri/pack.yml"), "pack: mri\nversion: 0.7.0\n").unwrap();
    let settings = home.path().join("config");
    std::fs::create_dir_all(settings.join("nils")).unwrap();
    std::fs::write(
        settings.join("nils").join("setup.toml"),
        format!(
            "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"none\"\n\n[parts.engine]\nversion = \"1.0.0-alpha.14\"\npath = \"{}\"\n\n[site]\npack_dir = \"{}\"\n",
            home.path().join("nils").display(),
            home.path().join("engine").join("nils").display(),
            packs.display()
        ),
    )
    .unwrap();
    // the engine's channel, with the packs its release carries
    let releases = home.path().join("engine-releases");
    std::fs::create_dir_all(releases.join("latest").join("download")).unwrap();
    std::fs::write(
        releases.join("latest").join("download").join("VERSION"),
        "1.0.0-alpha.14\n",
    )
    .unwrap();
    let src = home.path().join("src");
    std::fs::create_dir_all(src.join("packs").join("mri")).unwrap();
    std::fs::write(
        src.join("packs/mri/pack.yml"),
        "pack: mri\nversion: 0.8.0\n",
    )
    .unwrap();
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    tar.append_dir_all("packs", src.join("packs")).unwrap();
    let tar = tar.into_inner().unwrap().finish().unwrap();
    let at = releases.join("download").join("v1.0.0-alpha.14");
    std::fs::create_dir_all(&at).unwrap();
    std::fs::write(at.join("packs.tar.gz"), &tar).unwrap();
    let sum = hex::encode(ring::digest::digest(&ring::digest::SHA256, &tar).as_ref());
    std::fs::write(at.join("SHA256SUMS"), format!("{sum}  packs.tar.gz\n")).unwrap();
    let engine = format!("file://{}", releases.display());
    let s = Service::start_with(
        &config,
        &[
            ("XDG_CONFIG_HOME", settings.to_str().unwrap()),
            ("NILS_RELEASES", engine.as_str()),
        ],
    );
    let token = Some("a-supervisor-token-of-length");
    let (status, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(status, 200, "{doc}");
    let release = &doc["release"];
    let packs_doc = &release["packs"];
    assert_eq!(packs_doc["dir"], packs.display().to_string(), "{doc}");
    assert_eq!(packs_doc["release"], "1.0.0-alpha.14", "{doc}");
    assert_eq!(packs_doc["installed"][0]["version"], "0.7.0", "{doc}");
    assert_eq!(packs_doc["bundled"][0]["version"], "0.8.0", "{doc}");
    assert_eq!(packs_doc["stale"], serde_json::json!(["mri"]), "{doc}");
    assert_eq!(packs_doc["behind"], true, "{doc}");
    assert_eq!(release["behind"], serde_json::json!(["packs"]), "{doc}");
    assert_eq!(release["newer"], "packs of 1.0.0-alpha.14", "{doc}");

    // what is on disk is read on every call: packs brought up to date are
    // no longer offered
    std::fs::write(packs.join("mri/pack.yml"), "pack: mri\nversion: 0.8.0\n").unwrap();
    let (_, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(doc["release"]["packs"]["behind"], false, "{doc}");
    assert_eq!(doc["release"]["behind"], serde_json::json!([]), "{doc}");
    assert_eq!(doc["release"]["newer"], serde_json::Value::Null, "{doc}");
}

/// A tarball holding `packs/<name>/pack.yml` for each pack given, the shape
/// both an engine release's `packs.tar.gz` and a pack's own release carry.
fn packs_tarball(packs: &[(&str, &str)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (name, manifest) in packs {
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header,
            format!("packs/{name}/pack.yml"),
            manifest.as_bytes(),
        )
        .unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// Record 55 B5 (T9): the install door says a rules release beside the
/// engine's, as a part of its own in the shape of every other part's, so a
/// desk offers it; and the update the desk asks for with it takes the rules
/// and nothing else, here and in no place outside the test's own.
#[test]
fn the_install_door_offers_a_rules_release_and_its_update_takes_only_the_rules() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");
    let home = TempDir::new("supervise-rules");
    let mri = |version: &str| format!("pack: mri\nversion: {version}\ncontract: 8\nmodality: MR\n");
    let packs = home.path().join("engine").join("packs");
    std::fs::create_dir_all(packs.join("mri")).unwrap();
    std::fs::write(packs.join("mri/pack.yml"), mri("0.8.0")).unwrap();
    let settings = home.path().join("config");
    std::fs::create_dir_all(settings.join("nils")).unwrap();
    let record = settings.join("nils").join("setup.toml");
    std::fs::write(
        &record,
        format!(
            "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"none\"\n\n[parts.engine]\nversion = \"1.0.0-alpha.14\"\npath = \"{}\"\n\n[site]\npack_dir = \"{}\"\n",
            home.path().join("nils").display(),
            home.path().join("engine").join("nils").display(),
            packs.display()
        ),
    )
    .unwrap();
    // the engine's channel: its release carries mri 0.8.0, and the MRI
    // pack's own release 0.9.0 is out beside it
    let releases = home.path().join("engine-releases");
    std::fs::create_dir_all(releases.join("latest").join("download")).unwrap();
    std::fs::write(
        releases.join("latest").join("download").join("VERSION"),
        "1.0.0-alpha.14\n",
    )
    .unwrap();
    let at = releases.join("download").join("v1.0.0-alpha.14");
    std::fs::create_dir_all(&at).unwrap();
    let tar = packs_tarball(&[("mri", &mri("0.8.0"))]);
    std::fs::write(at.join("packs.tar.gz"), &tar).unwrap();
    std::fs::write(
        at.join("SHA256SUMS"),
        format!("{}  packs.tar.gz\n", sha256_hex(&tar)),
    )
    .unwrap();
    let own = releases.join("download").join("pack-mri-v0.9.0");
    std::fs::create_dir_all(&own).unwrap();
    let tar = packs_tarball(&[("mri", &mri("0.9.0"))]);
    std::fs::write(own.join("pack-mri.tar.gz"), &tar).unwrap();
    std::fs::write(
        own.join("SHA256SUMS"),
        format!("{}  pack-mri.tar.gz\n", sha256_hex(&tar)),
    )
    .unwrap();
    std::fs::write(
        releases
            .join("latest")
            .join("download")
            .join("pack-mri.VERSION"),
        "0.9.0\n",
    )
    .unwrap();
    let engine = format!("file://{}", releases.display());
    let s = Service::start_with(
        &config,
        &[
            ("XDG_CONFIG_HOME", settings.to_str().unwrap()),
            ("HOME", home.path().to_str().unwrap()),
            ("NILS_RELEASES", engine.as_str()),
        ],
    );
    let token = Some("a-supervisor-token-of-length");
    let (status, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(status, 200, "{doc}");
    let release = &doc["release"];
    assert_eq!(release["packs"]["behind"], false, "{doc}");
    assert_eq!(release["behind"], serde_json::json!(["rules"]), "{doc}");
    assert_eq!(release["newer"], "rules mri 0.9.0", "{doc}");
    let row = release["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["part"] == "rules")
        .cloned()
        .expect("the rules are a part");
    assert_eq!(row["pack"], "mri", "{row}");
    assert_eq!(row["installed"], "0.8.0", "{row}");
    assert_eq!(row["newer"], "0.9.0", "{row}");
    assert_eq!(row["command"], "nils update --part rules", "{row}");
    assert_eq!(release["rules"][0], row, "{doc}");

    // the desk's update control for it: a run of nils update --part rules
    let (status, run) = s.call(
        "POST",
        "/api/supervise/update-all",
        Some(r#"{"part":"rules"}"#),
        token,
    );
    assert_eq!(status, 202, "{run}");
    let id = run["id"].as_str().unwrap().to_string();
    // followed until it ends, done, failed or ended: any other answer is no
    // end, and is kept to be shown should the run never end
    let mut last = serde_json::Value::Null;
    let mut ended = None;
    for _ in 0..300 {
        let (status, doc) = s.call("GET", &format!("/api/supervise/runs/{id}"), None, token);
        if status == 200 && matches!(doc["state"].as_str(), Some("done" | "failed" | "ended")) {
            ended = Some(doc);
            break;
        }
        last = doc;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let ended = ended.unwrap_or_else(|| panic!("the run did not end in 30 s: {last}"));
    assert_eq!(ended["state"], "done", "{ended}");
    assert_eq!(
        std::fs::read_to_string(packs.join("mri/pack.yml")).unwrap(),
        mri("0.9.0"),
        "{ended}"
    );
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(
        text.contains("[rules.mri]") && text.contains("version = \"0.9.0\""),
        "{text}"
    );
    assert!(
        text.contains("version = \"1.0.0-alpha.14\""),
        "the engine stays: {text}"
    );
    // every path the run named is the test's own
    for line in ended["tail"].as_array().unwrap() {
        for word in line.as_str().unwrap_or("").split_whitespace() {
            let word = word.trim_matches(|c: char| ",;:()".contains(c));
            if word.starts_with('/') {
                assert!(
                    std::path::Path::new(word).starts_with(home.path()),
                    "{word} is outside the test's own directory: {ended}"
                );
            }
        }
    }

    // and the door, which reads the disk on every call, offers it no more
    let (_, doc) = s.call("GET", "/api/supervise/install", None, token);
    assert_eq!(doc["release"]["behind"], serde_json::json!([]), "{doc}");
    assert_eq!(doc["release"]["newer"], serde_json::Value::Null, "{doc}");
}

/// A run the door starts is read back by its id at once, and on every read
/// after it while its recorder rewrites it as the run starts and as it ends:
/// the desk asks for the run right after the 202 and then polls it. CI once
/// found the first read answered 404, "no run by that id" (2026-10-14).
#[test]
fn a_run_the_door_starts_is_read_back_by_its_id_at_once_and_on_every_read() {
    let c = Channel::new();
    let install = c.dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("VERSION"), "1.0.0\n").unwrap();
    let config = c.config("engine", &install, "true");
    // no setup is recorded here, so each run (`nils update --part rules`)
    // refuses at once: many runs, each read from its first moment to its end
    let home = TempDir::new("supervise-runs-read");
    let settings = home.path().join("config");
    std::fs::create_dir_all(&settings).unwrap();
    let s = Service::start_with(
        &config,
        &[
            ("XDG_CONFIG_HOME", settings.to_str().unwrap()),
            ("HOME", home.path().to_str().unwrap()),
        ],
    );
    let token = Some("a-supervisor-token-of-length");
    let mut reads = 0;
    for round in 0..20 {
        // the run before has ended, though its recorder may still be exiting
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let run = loop {
            let (status, run) = s.call(
                "POST",
                "/api/supervise/update-all",
                Some(r#"{"part":"rules"}"#),
                token,
            );
            if status == 202 {
                break run;
            }
            assert_eq!(status, 409, "round {round}: {run}");
            assert!(std::time::Instant::now() < deadline, "round {round}: {run}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let id = run["id"].as_str().unwrap().to_string();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let ended = loop {
            let (status, doc) = s.call("GET", &format!("/api/supervise/runs/{id}"), None, token);
            reads += 1;
            assert_eq!(status, 200, "round {round}, read {reads}: {doc}");
            assert_eq!(doc["id"], id.as_str(), "round {round}: {doc}");
            match doc["state"].as_str() {
                Some("running") => {
                    assert!(std::time::Instant::now() < deadline, "round {round}: {doc}");
                }
                Some("done" | "failed" | "ended") => break doc,
                _ => panic!("round {round}: a state no run has: {doc}"),
            }
        };
        assert_eq!(
            ended["state"], "failed",
            "with no setup recorded the update refuses: {ended}"
        );
        assert!(
            ended["tail"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l.as_str().unwrap_or("").contains("no setup is recorded")),
            "{ended}"
        );
    }
    assert!(reads >= 20, "{reads}");
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
    // every card this machine has, and beside them the one with the most
    // memory as the card; a machine with none has an empty list and no card
    let cards = doc["machine"]["cards"].as_array().expect("a list of cards");
    assert!(
        cards
            .iter()
            .all(|c| c["name"].is_string() && c["memory_gb"].is_number()),
        "{doc}"
    );
    match cards
        .iter()
        .filter_map(|c| c["memory_gb"].as_f64())
        .reduce(f64::max)
    {
        Some(most) => assert_eq!(
            doc["machine"]["card"]["memory_gb"].as_f64(),
            Some(most),
            "{doc}"
        ),
        None => assert!(doc["machine"]["card"].is_null(), "{doc}"),
    }

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

    // the folders inside, for a path browser, and where a browser starts
    let (status, listed) = s.call("POST", "/api/supervise/folders", Some(&body), token);
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed["folders"][0]["name"], "mri-3t", "{listed}");
    assert_eq!(listed["folders"][1]["name"], "notes", "{listed}");
    assert_eq!(listed["files"], 1, "{listed}");
    assert_eq!(listed["readable"], true, "{listed}");
    assert_eq!(listed["timed_out"], false, "{listed}");
    let (status, start) = s.call("POST", "/api/supervise/folders", Some("{}"), token);
    assert_eq!(status, 200, "{start}");
    assert_eq!(start["path"], serde_json::Value::Null, "{start}");
    assert_eq!(start["roots"][0]["path"], "/", "{start}");
    assert_eq!(start["roots"][0]["kind"], "root", "{start}");
    let (status, _) = s.call(
        "POST",
        "/api/supervise/folders",
        Some(r#"{"path":"relative"}"#),
        token,
    );
    assert_eq!(status, 400);
}
