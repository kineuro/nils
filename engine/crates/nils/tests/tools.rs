// SPDX-License-Identifier: AGPL-3.0-only

//! The tools the engine runs beside itself (2026-10-12): dcm2niix, whether
//! it is here, and an admin's install of the pinned build into the
//! registry's tools folder, from a mirror, offline, as the tests run it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use nils_dicom::synth::TempDir;
use serde_json::{Value, json};

const VERSION: &str = "v1.0.20260724";

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

/// The archive this machine takes, where rordenlab publishes one.
fn archive_here() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("dcm2niix_lnx.zip"),
        ("macos", _) => Some("dcm2niix_mac.zip"),
        _ => None,
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// A zip archive of the files given, each deflated, shaped as rordenlab's.
fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for (name, body) in files {
        let mut deflated =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        deflated.write_all(body).unwrap();
        let packed = deflated.finish().unwrap();
        let mut crc = flate2::Crc::new();
        crc.update(body);
        let offset = out.len() as u32;
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend(sig.to_le_bytes());
            if central {
                h.extend(20u16.to_le_bytes());
            }
            h.extend(20u16.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            h.extend(8u16.to_le_bytes());
            h.extend(0u32.to_le_bytes());
            h.extend(crc.sum().to_le_bytes());
            h.extend((packed.len() as u32).to_le_bytes());
            h.extend((body.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central {
                h.extend([0u8; 6]);
                h.extend(0o100_755u32.wrapping_shl(16).to_le_bytes());
                h.extend(offset.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend(&packed);
        directory.extend(header(0x0201_4b50, true));
    }
    let start = out.len() as u32;
    out.extend(&directory);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((directory.len() as u32).to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// A stand-in dcm2niix that answers its version as the real one does.
const STAND_IN: &[u8] =
    b"#!/bin/sh\necho \"Chris Rorden's dcm2niiX version v1.0.20260724 (stand-in)\"\n";

/// A mirror of the releases holding the stand-in under this machine's
/// archive name, and its sha256.
fn mirror() -> Option<(TempDir, String)> {
    let archive = archive_here()?;
    let dir = TempDir::new("dcm2niix-mirror");
    let zip = zip_of(&[("dcm2niix", STAND_IN)]);
    dir.file(&format!("{VERSION}/{archive}"), &zip);
    Some((dir, sha256(&zip)))
}

/// A registry of its own, and a search path that holds no dcm2niix.
struct Lab {
    home: TempDir,
    empty: TempDir,
}

impl Lab {
    fn new(name: &str) -> Lab {
        let lab = Lab {
            home: TempDir::new(&format!("{name}-home")),
            empty: TempDir::new(&format!("{name}-path")),
        };
        let (good, _, err) = lab.run(&["key", "add", "k"], &[], Some("a tools test key\n"));
        assert!(good, "{err}");
        let (good, _, err) = lab.run(&["init", "--key", "k"], &[], None);
        assert!(good, "{err}");
        lab
    }

    fn command(&self, env: &[(&str, &str)]) -> Command {
        let mut c = nils();
        c.arg("--registry")
            .arg(self.home.path())
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env("PATH", self.empty.path())
            .env_remove("NILS_DCM2NIIX_RELEASES")
            .env_remove("NILS_DCM2NIIX_TEST_SHA256");
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

    fn installed(&self) -> PathBuf {
        self.home
            .path()
            .join("tools")
            .join("dcm2niix")
            .join(VERSION)
            .join("dcm2niix")
    }

    /// Every file under the registry's tools folder.
    fn tools_files(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.home.path().join("tools"), &mut out);
        out
    }
}

/// From a mirror: an archive that is not the pinned build is refused with
/// nothing written; the pinned one is installed into the registry's tools
/// folder, found there and asked its version, and audited; an install found
/// already is left alone; and a mirror that cannot be reached says so with
/// the way by hand.
#[test]
fn dcm2niix_installs_from_a_mirror_and_is_left_alone_after() {
    let Some((mirror, digest)) = mirror() else {
        eprintln!("rordenlab builds no dcm2niix for this machine; skipped");
        return;
    };
    let lab = Lab::new("tools-cli");
    let base = format!("file://{}", mirror.path().display());
    let from_mirror = [("NILS_DCM2NIIX_RELEASES", base.as_str())];

    let before = lab.json(&["tools", "list", "--json"], &from_mirror);
    let t = &before["tools"][0];
    assert_eq!(t["name"], "dcm2niix", "{before}");
    assert_eq!(t["present"], false, "{before}");
    assert_eq!(t["pinned"], VERSION, "{before}");
    assert_eq!(t["install"]["available"], true, "{before}");
    assert_eq!(t["install"]["grant"], "install:work", "{before}");
    assert_eq!(
        t["install"]["door"], "POST /api/tools/dcm2niix/install",
        "{before}"
    );
    assert!(
        t["install"]["source"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{base}/{VERSION}/")),
        "{before}"
    );

    // the stand-in is not the pinned build: refused, nothing written
    let (good, _, err) = lab.run(&["tools", "install", "dcm2niix"], &from_mirror, None);
    assert!(!good);
    assert!(err.contains("sha256"), "{err}");
    assert!(err.contains("nothing was written"), "{err}");
    assert!(lab.tools_files().is_empty(), "{:?}", lab.tools_files());

    // held to its own digest, as a debug build lets a test hold it
    let pinned = [
        ("NILS_DCM2NIIX_RELEASES", base.as_str()),
        ("NILS_DCM2NIIX_TEST_SHA256", digest.as_str()),
    ];
    let done = lab.json(&["tools", "install", "dcm2niix", "--json"], &pinned);
    assert_eq!(done["installed"], true, "{done}");
    assert_eq!(done["tool"]["present"], true, "{done}");
    assert_eq!(done["tool"]["found_by"], "tools", "{done}");
    assert_eq!(
        done["tool"]["path"],
        lab.installed().display().to_string(),
        "{done}"
    );
    assert!(
        done["tool"]["version"].as_str().unwrap().contains(VERSION),
        "{done}"
    );
    assert_eq!(done["tool"]["current"], true, "{done}");
    assert_eq!(lab.tools_files(), vec![lab.installed()]);
    let audit = lab.json(&["audit", "list", "--action", "tool.", "--json"], &[]);
    assert_eq!(
        audit.to_string().matches("tool.install").count(),
        1,
        "{audit}"
    );

    // found now, so a second install leaves it as it is
    let written = std::fs::metadata(lab.installed())
        .unwrap()
        .modified()
        .unwrap();
    let again = lab.json(&["tools", "install", "dcm2niix", "--json"], &pinned);
    assert_eq!(again["installed"], false, "{again}");
    assert!(
        again["why"].as_str().unwrap().contains("here already"),
        "{again}"
    );
    assert_eq!(
        std::fs::metadata(lab.installed())
            .unwrap()
            .modified()
            .unwrap(),
        written
    );
    let audit = lab.json(&["audit", "list", "--action", "tool.", "--json"], &[]);
    assert_eq!(
        audit.to_string().matches("tool.install").count(),
        1,
        "{audit}"
    );

    // a mirror nobody answers at: the way by hand, nothing written
    let offline = Lab::new("tools-offline");
    let (good, _, err) = offline.run(
        &["tools", "install", "dcm2niix"],
        &[("NILS_DCM2NIIX_RELEASES", "http://127.0.0.1:9")],
        None,
    );
    assert!(!good);
    assert!(err.contains("could not be reached"), "{err}");
    assert!(err.contains("by hand"), "{err}");
    assert!(err.contains("make it executable"), "{err}");
    assert!(offline.tools_files().is_empty());
}

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

impl Server {
    fn start(lab: &Lab, requests: usize, env: &[(&str, &str)]) -> Server {
        let mut cmd = lab.command(env);
        cmd.args([
            "serve",
            "--bind",
            "127.0.0.1:0",
            "--requests",
            &requests.to_string(),
            "--auth",
            "token",
            "--token",
            "a-reader-token-of-length=reader@lab:reader",
            "--token",
            "an-operator-token-of-len=ops@lab:operator",
            "--token",
            "an-admin-token-of-length=adm@lab:admin",
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

    fn request(&self, method: &str, path: &str, token: &str) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\nAuthorization: Bearer {token}\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).unwrap();
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
        (
            status,
            serde_json::from_str(body).unwrap_or(Value::String(body.to_string())),
        )
    }

    fn finish(mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while self.child.try_wait().unwrap().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "nils serve still waits: the request count is wrong"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// The door: read under install:see or pipelines:see, an install an
/// admin's alone (install:work), refused to an operator with nothing
/// written, and the capabilities saying what is here.
#[test]
fn the_install_door_is_an_admin_s() {
    let Some((mirror, digest)) = mirror() else {
        eprintln!("rordenlab builds no dcm2niix for this machine; skipped");
        return;
    };
    let lab = Lab::new("tools-door");
    let base = format!("file://{}", mirror.path().display());
    let server = Server::start(
        &lab,
        7,
        &[
            ("NILS_DCM2NIIX_RELEASES", base.as_str()),
            ("NILS_DCM2NIIX_TEST_SHA256", digest.as_str()),
        ],
    );
    let (reader, operator, admin) = (
        "a-reader-token-of-length",
        "an-operator-token-of-len",
        "an-admin-token-of-length",
    );
    let (status, doc) = server.request("GET", "/api/tools", reader);
    assert_eq!(status, 403, "{doc}");
    let (status, doc) = server.request("GET", "/api/tools", operator);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["tools"][0]["present"], false, "{doc}");
    let (status, doc) = server.request("POST", "/api/tools/dcm2niix/install", operator);
    assert_eq!(status, 403, "{doc}");
    assert!(lab.tools_files().is_empty());
    let (status, doc) = server.request("POST", "/api/tools/dcm2niix/install", admin);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["installed"], true, "{doc}");
    assert_eq!(doc["tool"]["found_by"], "tools", "{doc}");
    let (status, doc) = server.request("POST", "/api/tools/fsl/install", admin);
    assert_eq!(status, 404, "{doc}");
    let (status, doc) = server.request("GET", "/api/capabilities", admin);
    assert_eq!(status, 200, "{doc}");
    let t = &doc["tools"]["dcm2niix"];
    assert_eq!(t["present"], true, "{t}");
    assert_eq!(t["path"], lab.installed().display().to_string(), "{t}");
    assert!(
        doc["policy"].as_array().unwrap().iter().any(|r| r["door"]
            == "POST /api/tools/dcm2niix/install"
            && r["grant"] == "install:work"),
        "{}",
        doc["policy"]
    );
    let (status, doc) = server.request("GET", "/api/tools", admin);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["tools"][0]["version"], json!(t["version"]), "{doc}");
    server.finish();
}
