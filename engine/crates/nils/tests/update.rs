// SPDX-License-Identifier: AGPL-3.0-only
//! `nils update`: the other half of the one line installer. The release is a
//! directory reached by file:// URLs, laid out the way a GitHub release is
//! (`latest/download/VERSION` and `download/v<version>/<file>` beside a
//! `SHA256SUMS`), so what the test drives is what a person drives.
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// The name this platform's binary has in a release.
fn file() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    let name = format!("nils-{}-{arch}", std::env::consts::OS);
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// A release directory: `latest/download/VERSION` and one version's files
/// with their sums. The binary is a script whose text stands for the build.
struct Releases {
    dir: TempDir,
}

impl Releases {
    fn new() -> Releases {
        Releases {
            dir: TempDir::new("nils-releases"),
        }
    }

    fn url(&self) -> String {
        format!("file://{}", self.dir.path().display())
    }

    /// Publish one version; `packs` puts a packs.tar.gz beside the binary.
    fn publish(&self, version: &str, packs: bool) {
        let into = self.dir.path().join("download").join(format!("v{version}"));
        std::fs::create_dir_all(&into).unwrap();
        let binary = format!("#!/bin/sh\necho nils {version}\n");
        std::fs::write(into.join(file()), &binary).unwrap();
        let mut sums = format!("{}  {}\n", sha256_hex(binary.as_bytes()), file());
        if packs {
            let tar = self.pack_tarball(version);
            sums.push_str(&format!("{}  packs.tar.gz\n", sha256_hex(&tar)));
            std::fs::write(into.join("packs.tar.gz"), &tar).unwrap();
        }
        std::fs::write(into.join("SHA256SUMS"), sums).unwrap();
        let latest = self.dir.path().join("latest").join("download");
        std::fs::create_dir_all(&latest).unwrap();
        std::fs::write(latest.join("VERSION"), format!("{version}\n")).unwrap();
    }

    /// A tarball holding `packs/demo/pack.yml`, the shape the release makes.
    fn pack_tarball(&self, version: &str) -> Vec<u8> {
        let src = self.dir.path().join("src").join(version);
        let pack = src.join("packs").join("demo");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("pack.yml"), format!("pack: demo {version}\n")).unwrap();
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        tar.append_dir_all("packs", src.join("packs")).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }

    /// Overwrite one file of a published version, leaving its sum behind.
    fn tamper(&self, version: &str, name: &str) {
        let path = self
            .dir
            .path()
            .join("download")
            .join(format!("v{version}"))
            .join(name);
        std::fs::write(path, "not what the sums say").unwrap();
    }
}

fn installed(dir: &Path) -> PathBuf {
    dir.join(if cfg!(windows) { "nils.exe" } else { "nils" })
}

#[test]
fn check_says_what_it_would_install_and_installs_nothing() {
    let releases = Releases::new();
    releases.publish("99.0.0", false);
    let to = TempDir::new("nils-update-check");
    let o = run(&[
        "update",
        "--check",
        "--channel",
        &releases.url(),
        "--to",
        to.path().to_str().unwrap(),
    ]);
    assert!(o.ok, "{}", o.stderr);
    assert!(o.stdout.contains("99.0.0"), "{}", o.stdout);
    assert!(o.stdout.contains("would install"), "{}", o.stdout);
    assert!(
        !installed(to.path()).exists(),
        "--check wrote {}",
        installed(to.path()).display()
    );
}

#[test]
fn a_newer_release_is_installed_where_it_is_told_to_go() {
    let releases = Releases::new();
    releases.publish("99.0.0", false);
    let to = TempDir::new("nils-update-to");
    let o = run(&[
        "update",
        "--channel",
        &releases.url(),
        "--to",
        to.path().to_str().unwrap(),
    ]);
    assert!(o.ok, "{}", o.stderr);
    let at = installed(to.path());
    assert!(at.is_file(), "{}", o.stdout);
    let text = std::fs::read_to_string(&at).unwrap();
    assert!(text.contains("echo nils 99.0.0"), "{text}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&at).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o111,
            0o111,
            "the binary is not executable: {mode:o}"
        );
    }
}

#[test]
fn a_file_that_does_not_match_the_release_sums_is_refused_and_nothing_is_written() {
    let releases = Releases::new();
    releases.publish("99.0.0", false);
    releases.tamper("99.0.0", &file());
    let to = TempDir::new("nils-update-tampered");
    let o = run(&[
        "update",
        "--channel",
        &releases.url(),
        "--to",
        to.path().to_str().unwrap(),
    ]);
    assert!(!o.ok, "a tampered file was installed: {}", o.stdout);
    assert!(o.stderr.contains("checksum"), "{}", o.stderr);
    assert!(!installed(to.path()).exists(), "it wrote the binary anyway");
}

#[test]
fn the_packs_of_a_release_replace_the_ones_in_use() {
    let releases = Releases::new();
    releases.publish("99.0.0", true);
    let home = TempDir::new("nils-update-packs");
    // a registry home with a pack of its own, which is the directory in use
    let old = home.path().join("packs").join("demo");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("pack.yml"), "pack: demo 1.0.0\n").unwrap();
    let to = TempDir::new("nils-update-packs-to");
    let o = run(&[
        "--registry",
        home.path().to_str().unwrap(),
        "update",
        "--channel",
        &releases.url(),
        "--to",
        to.path().to_str().unwrap(),
    ]);
    assert!(o.ok, "{}", o.stderr);
    let fresh = std::fs::read_to_string(home.path().join("packs/demo/pack.yml")).unwrap();
    assert_eq!(fresh.trim(), "pack: demo 99.0.0", "{}", o.stdout);
    assert!(o.stdout.contains("packs"), "{}", o.stdout);
}

#[test]
fn a_release_no_newer_than_this_binary_is_left_alone() {
    let releases = Releases::new();
    releases.publish("0.0.1", false);
    let o = run(&["update", "--channel", &releases.url()]);
    assert!(o.ok, "{}", o.stderr);
    assert!(o.stdout.contains("newest release"), "{}", o.stdout);
}

#[test]
fn a_registry_with_no_packs_looks_where_an_installer_leaves_them() {
    let home = TempDir::new("nils-packs-search");
    let data = TempDir::new("nils-packs-data");
    let pack = data.path().join("nils").join("packs").join("demo");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("pack.yml"), "pack: demo\n").unwrap();
    let out = nils()
        .args([
            "--registry",
            home.path().to_str().unwrap(),
            "pack",
            "list",
            "--json",
        ])
        .env("XDG_DATA_HOME", data.path())
        .env_remove("NILS_PACK_DIR")
        .output()
        .expect("nils runs");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("demo"),
        "the data directory's packs were not found: {text}"
    );
}

#[test]
fn an_empty_directory_is_not_a_pack_directory() {
    let home = TempDir::new("nils-packs-empty");
    // a home with an empty `packs`, and nothing anywhere else to find
    std::fs::create_dir_all(home.path().join("packs")).unwrap();
    let nowhere = TempDir::new("nils-packs-nowhere");
    let out = nils()
        .args(["--registry", home.path().to_str().unwrap(), "pack", "list"])
        .env("XDG_DATA_HOME", nowhere.path())
        .env_remove("NILS_PACK_DIR")
        .output()
        .expect("nils runs");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "an empty directory was taken as one");
    assert!(stderr.contains("no pack directory"), "{stderr}");
}
