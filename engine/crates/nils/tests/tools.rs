// SPDX-License-Identifier: AGPL-3.0-only

//! The tools the engine runs beside itself (2026-10-12): dcm2niix, whether
//! it is here, and an admin's install of the pinned build into the
//! registry's tools folder, from a mirror, offline, as the tests run it;
//! and (gate T31, step 15.7) from an archive given whole, at its door and
//! with `--from`, for an engine that cannot reach the releases.

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
        self.send(method, path, token, &[])
    }

    /// A request carrying `body` as it stands, as a browser sends a file.
    fn send(&self, method: &str, path: &str, token: &str, body: &[u8]) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let kind = if body.is_empty() {
            String::new()
        } else {
            "Content-Type: application/zip\r\n".to_string()
        };
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{kind}Content-Length: {}\r\nAuthorization: Bearer {token}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
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

/// What an audit row says it did, its details read whether they are kept
/// as text or as a document.
fn details(row: &Value) -> Value {
    match &row["details"] {
        Value::String(s) => serde_json::from_str(s).unwrap(),
        v => v.clone(),
    }
}

/// The releases nobody answers at, as an engine that cannot reach them
/// meets them.
const UNREACHABLE: (&str, &str) = ("NILS_DCM2NIIX_RELEASES", "http://127.0.0.1:9");

/// Gate T31, step 15.7, at the command line: the status names the file to
/// fetch (its name, the release's own address and its sha256), and `nils
/// tools install dcm2niix --from FILE` installs it with nothing downloaded.
/// The archive is held to the pinned sha256 before anything is unpacked: a
/// wrong one is refused with the file it takes named and nothing written,
/// as are a file that is not there and one too large to be the archive;
/// the right one is installed as the download is, and audited as given.
#[test]
fn dcm2niix_installs_from_a_file_given_whole_at_the_command_line() {
    let Some(archive) = archive_here() else {
        eprintln!("rordenlab builds no dcm2niix for this machine; skipped");
        return;
    };
    let lab = Lab::new("tools-from");
    let files = TempDir::new("dcm2niix-given");
    let zip = zip_of(&[("dcm2niix", STAND_IN)]);
    let given = files.file(archive, &zip);
    let given = given.to_str().unwrap();
    let digest = sha256(&zip);

    let listed = lab.json(&["tools", "list", "--json"], &[UNREACHABLE]);
    let file = &listed["tools"][0]["install"]["file"];
    assert_eq!(
        file["door"], "POST /api/tools/dcm2niix/install-file",
        "{listed}"
    );
    assert_eq!(
        file["command"], "nils tools install dcm2niix --from FILE",
        "{listed}"
    );
    assert_eq!(file["archive"], archive, "{listed}");
    assert_eq!(
        file["url"],
        format!("https://github.com/rordenlab/dcm2niix/releases/download/{VERSION}/{archive}"),
        "{listed}"
    );
    assert_eq!(
        file["sha256"], listed["tools"][0]["install"]["sha256"],
        "{listed}"
    );
    assert_eq!(file["sha256"].as_str().unwrap().len(), 64, "{listed}");
    assert_eq!(file["max_bytes"], 4 << 20, "{listed}");
    let (good, out, err) = lab.run(&["tools", "list"], &[UNREACHABLE], None);
    assert!(good, "{err}");
    assert!(out.contains(&format!("fetch {archive} (")), "{out}");
    assert!(
        out.contains("nils tools install dcm2niix --from FILE"),
        "{out}"
    );

    // the stand-in is not the pinned build: refused before anything is
    // unpacked, with the file it takes named
    let (good, _, err) = lab.run(
        &["tools", "install", "dcm2niix", "--from", given],
        &[UNREACHABLE],
        None,
    );
    assert!(!good);
    assert!(
        err.contains(&format!("{given} is not {archive} of dcm2niix {VERSION}")),
        "{err}"
    );
    assert!(err.contains("sha256"), "{err}");
    assert!(err.contains("nothing was unpacked or written"), "{err}");
    assert!(lab.tools_files().is_empty(), "{:?}", lab.tools_files());

    // a file that is not there, and one too large to be the archive
    let pinned = [UNREACHABLE, ("NILS_DCM2NIIX_TEST_SHA256", digest.as_str())];
    let nowhere = files.path().join("nowhere").join(archive);
    let (good, _, err) = lab.run(
        &[
            "tools",
            "install",
            "dcm2niix",
            "--from",
            nowhere.to_str().unwrap(),
        ],
        &pinned,
        None,
    );
    assert!(!good);
    assert!(err.contains(nowhere.to_str().unwrap()), "{err}");
    let big = files.file("big.zip", &vec![0u8; (4 << 20) + 1]);
    let (good, _, err) = lab.run(
        &[
            "tools",
            "install",
            "dcm2niix",
            "--from",
            big.to_str().unwrap(),
        ],
        &pinned,
        None,
    );
    assert!(!good);
    assert!(err.contains("more than 4194304 bytes"), "{err}");
    assert!(lab.tools_files().is_empty(), "{:?}", lab.tools_files());

    // held to its own digest, as a debug build lets a test hold it: the
    // install the download makes, from the file, with the releases out of
    // reach
    let done = lab.json(
        &["tools", "install", "dcm2niix", "--from", given, "--json"],
        &pinned,
    );
    assert_eq!(done["installed"], true, "{done}");
    assert_eq!(done["source"], given, "{done}");
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
    assert_eq!(lab.tools_files(), vec![lab.installed()]);
    let audit = lab.json(&["audit", "list", "--action", "tool.", "--json"], &[]);
    let rows = audit.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{audit}");
    assert_eq!(rows[0]["action"], "tool.install", "{audit}");
    assert_eq!(details(&rows[0])["given"], true, "{audit}");
    assert_eq!(details(&rows[0])["source"], given, "{audit}");

    // found now: a second is left alone
    let again = lab.json(
        &["tools", "install", "dcm2niix", "--from", given, "--json"],
        &pinned,
    );
    assert_eq!(again["installed"], false, "{again}");
    assert!(
        again["why"].as_str().unwrap().contains("here already"),
        "{again}"
    );
}

/// Gate T31, step 15.7, at the door: the capabilities list the install
/// from a file with its policy row (install:work) and name the file to
/// fetch. The install that cannot reach the releases says so and names the
/// file and its door. An operator is refused before the body is read; a
/// wrong archive and one too large are refused with nothing written; a
/// tool the engine does not install is not found; and the pinned one is
/// installed as the download is, audited as given, and left alone after.
#[test]
fn the_install_from_a_file_is_an_admin_s_door() {
    let Some(archive) = archive_here() else {
        eprintln!("rordenlab builds no dcm2niix for this machine; skipped");
        return;
    };
    let lab = Lab::new("tools-file-door");
    let zip = zip_of(&[("dcm2niix", STAND_IN)]);
    let digest = sha256(&zip);
    let server = Server::start(
        &lab,
        8,
        &[UNREACHABLE, ("NILS_DCM2NIIX_TEST_SHA256", digest.as_str())],
    );
    let (operator, admin) = ("an-operator-token-of-len", "an-admin-token-of-length");
    let door = "/api/tools/dcm2niix/install-file";

    let (status, caps) = server.request("GET", "/api/capabilities", admin);
    assert_eq!(status, 200, "{caps}");
    assert!(
        caps["doors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == "POST /api/tools/dcm2niix/install-file"),
        "{}",
        caps["doors"]
    );
    assert!(
        caps["policy"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["door"] == "POST /api/tools/dcm2niix/install-file"
                && r["grant"] == "install:work"
                && r["writes"] == true),
        "{}",
        caps["policy"]
    );
    let file = &caps["tools"]["dcm2niix"]["install"]["file"];
    assert_eq!(file["archive"], archive, "{file}");
    assert_eq!(
        file["door"], "POST /api/tools/dcm2niix/install-file",
        "{file}"
    );

    // the releases out of reach: the install says so, and names the file
    // to fetch and the door that takes it
    let (status, doc) = server.request("POST", "/api/tools/dcm2niix/install", admin);
    assert_eq!(status, 502, "{doc}");
    assert_eq!(doc["offline"], true, "{doc}");
    assert_eq!(doc["file"], *file, "{doc}");
    let error = doc["error"].as_str().unwrap();
    assert!(error.contains("Install from a file"), "{error}");
    assert!(error.contains("--from FILE"), "{error}");
    assert!(lab.tools_files().is_empty(), "{:?}", lab.tools_files());

    // an operator: refused before the body is read, nothing written
    let (status, doc) = server.send("POST", door, operator, &zip);
    assert_eq!(status, 403, "{doc}");
    assert!(
        doc["error"].as_str().unwrap().contains("install:work"),
        "{doc}"
    );
    assert!(lab.tools_files().is_empty(), "{:?}", lab.tools_files());

    // an archive that is not the pinned build: refused before anything is
    // unpacked, with the file it takes named
    let wrong = zip_of(&[("dcm2niix", b"#!/bin/sh\nexit 0\n")]);
    let (status, doc) = server.send("POST", door, admin, &wrong);
    assert_eq!(status, 422, "{doc}");
    let error = doc["error"].as_str().unwrap();
    assert!(
        error.contains(&format!(
            "the archive uploaded is not {archive} of dcm2niix {VERSION}"
        )),
        "{error}"
    );
    assert!(error.contains(&sha256(&wrong)), "{error}");
    assert!(error.contains("nothing was unpacked or written"), "{error}");
    assert_eq!(doc["offline"], false, "{doc}");
    assert_eq!(doc["file"], *file, "{doc}");
    assert!(lab.tools_files().is_empty(), "{:?}", lab.tools_files());

    // larger than any archive of the pinned build
    let (status, doc) = server.send("POST", door, admin, &vec![0u8; (4 << 20) + 1]);
    assert_eq!(status, 413, "{doc}");
    assert!(lab.tools_files().is_empty(), "{:?}", lab.tools_files());

    // a tool the engine does not install
    let (status, doc) = server.send("POST", "/api/tools/fsl/install-file", admin, &zip);
    assert_eq!(status, 404, "{doc}");

    // the pinned one: installed as the download is
    let (status, doc) = server.send("POST", door, admin, &zip);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["installed"], true, "{doc}");
    assert_eq!(doc["source"], "the archive uploaded", "{doc}");
    assert_eq!(doc["tool"]["found_by"], "tools", "{doc}");
    assert_eq!(
        doc["tool"]["path"],
        lab.installed().display().to_string(),
        "{doc}"
    );
    assert_eq!(lab.tools_files(), vec![lab.installed()]);

    // found now: a second is left alone
    let (status, doc) = server.send("POST", door, admin, &zip);
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["installed"], false, "{doc}");
    server.finish();

    let audit = lab.json(&["audit", "list", "--action", "tool.", "--json"], &[]);
    let rows = audit.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{audit}");
    assert_eq!(rows[0]["principal"], "adm@lab", "{audit}");
    assert_eq!(details(&rows[0])["given"], true, "{audit}");
    assert_eq!(
        details(&rows[0])["source"],
        "the archive uploaded",
        "{audit}"
    );
}
