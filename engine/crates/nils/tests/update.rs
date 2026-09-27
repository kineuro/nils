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
    // no setup is recorded where these look, whatever the machine has
    let config = TempDir::new("nils-update-no-record");
    let out = nils()
        .args(args)
        .env("XDG_CONFIG_HOME", config.path())
        .env_remove("NILS_DESK_RELEASES")
        .env_remove("NILS_RELEASES")
        .output()
        .expect("nils runs");
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

/// This engine's own version, which an engine part at the newest release has.
const ENGINE: &str = env!("CARGO_PKG_VERSION");

/// The desk's name for this platform's binary in a release.
fn desk_file() -> String {
    file().replacen("nils-", "nils-desk-", 1)
}

/// A desk channel beside the engine's: its own `VERSION` and binaries, as the
/// desk's own repository publishes them, and a `contracts.json` where given.
fn publish_desk(dir: &Path, version: &str, contracts: Option<&str>) {
    let into = dir.join("download").join(format!("v{version}"));
    std::fs::create_dir_all(&into).unwrap();
    let binary = format!("#!/bin/sh\necho nils-desk {version}\n");
    std::fs::write(into.join(desk_file()), &binary).unwrap();
    let mut sums = format!("{}  {}\n", sha256_hex(binary.as_bytes()), desk_file());
    if let Some(doc) = contracts {
        std::fs::write(into.join("contracts.json"), doc).unwrap();
        sums.push_str(&format!("{}  contracts.json\n", sha256_hex(doc.as_bytes())));
    }
    std::fs::write(into.join("SHA256SUMS"), sums).unwrap();
    let latest = dir.join("latest").join("download");
    std::fs::create_dir_all(&latest).unwrap();
    std::fs::write(latest.join("VERSION"), format!("{version}\n")).unwrap();
}

/// A recorded setup with an engine and a desk binary, in a config directory
/// of its own, and nothing that runs.
struct Install {
    config: TempDir,
    base: TempDir,
}

impl Install {
    fn new(engine: &str, desk: &str) -> Install {
        let config = TempDir::new("nils-update-parts-config");
        let base = TempDir::new("nils-update-parts-base");
        let desk_path = base.path().join("bin").join("nils-desk");
        std::fs::create_dir_all(desk_path.parent().unwrap()).unwrap();
        std::fs::write(&desk_path, format!("#!/bin/sh\necho nils-desk {desk}\n")).unwrap();
        let record = format!(
            "dir = \"{dir}\"\nmode = \"local\"\nservice = \"none\"\n\n\
             [parts.engine]\nversion = \"{engine}\"\npath = \"{dir}/bin/nils\"\nkind = \"binary\"\n\n\
             [parts.desk]\nversion = \"{desk}\"\npath = \"{desk_path}\"\nkind = \"binary\"\n",
            dir = base.path().display(),
            desk_path = desk_path.display(),
        );
        let at = config.path().join("nils");
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join("setup.toml"), record).unwrap();
        Install { config, base }
    }

    fn run(&self, args: &[&str], engine: &Releases, desk: &Path) -> Out {
        self.run_with(args, engine, desk, &[])
    }

    fn run_with(&self, args: &[&str], engine: &Releases, desk: &Path, env: &[(&str, &str)]) -> Out {
        let out = nils()
            .args(args)
            .env_remove("NILS_SETUP_DESK_VERSION")
            .envs(env.iter().copied())
            .env("HOME", self.base.path())
            .env("XDG_CONFIG_HOME", self.config.path())
            .env("XDG_DATA_HOME", self.base.path().join("data"))
            .env("NILS_RELEASES", engine.url())
            .env("NILS_DESK_RELEASES", format!("file://{}", desk.display()))
            .env_remove("NILS_UPDATE_HANDED_OVER")
            .output()
            .expect("nils runs");
        Out {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    fn desk(&self) -> String {
        std::fs::read_to_string(self.base.path().join("bin").join("nils-desk")).unwrap()
    }

    fn record(&self) -> String {
        std::fs::read_to_string(self.config.path().join("nils").join("setup.toml")).unwrap()
    }
}

#[test]
fn a_desk_released_alone_is_offered_while_the_engine_is_the_newest() {
    let engine = Releases::new();
    engine.publish(ENGINE, false);
    let desk = TempDir::new("nils-desk-releases");
    publish_desk(desk.path(), "99.0.0", None);
    let install = Install::new(ENGINE, ENGINE);
    let o = install.run(&["update", "--check"], &engine, desk.path());
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert!(
        o.stdout
            .contains(&format!("nils {ENGINE} is the newest release")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains(&format!("engine {ENGINE}: the newest release")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!("desk {ENGINE}: 99.0.0 is out")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("nils update --all would take desk 99.0.0"),
        "{}",
        o.stdout
    );
    assert_eq!(
        install.desk(),
        format!("#!/bin/sh\necho nils-desk {ENGINE}\n"),
        "--check changed the desk"
    );

    // and the desk alone is taken, the engine left where it is
    let o = install.run(&["update", "--part", "desk"], &engine, desk.path());
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert_eq!(
        install.desk(),
        "#!/bin/sh\necho nils-desk 99.0.0\n",
        "{}",
        o.stdout
    );
    assert!(
        install.record().contains("version = \"99.0.0\""),
        "{}",
        install.record()
    );
    assert!(
        install
            .record()
            .contains(&format!("version = \"{ENGINE}\"")),
        "{}",
        install.record()
    );
    let o = install.run(&["update", "--check"], &engine, desk.path());
    assert!(
        o.stdout.contains("every part is at its newest release"),
        "{}",
        o.stdout
    );
}

#[test]
fn every_part_at_its_newest_offers_nothing() {
    let engine = Releases::new();
    engine.publish(ENGINE, false);
    let desk = TempDir::new("nils-desk-releases-current");
    publish_desk(desk.path(), "1.0.0-alpha.51", None);
    let install = Install::new(ENGINE, "1.0.0-alpha.51");
    let o = install.run(&["update", "--check"], &engine, desk.path());
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert!(
        o.stdout.contains("desk 1.0.0-alpha.51: the newest release"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("every part is at its newest release"),
        "{}",
        o.stdout
    );
    // --all with nothing behind changes nothing
    let o = install.run(&["update", "--all"], &engine, desk.path());
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert!(
        o.stdout.contains("desk: 1.0.0-alpha.51 is the newest"),
        "{}",
        o.stdout
    );
}

#[test]
fn a_development_channel_is_measured_by_its_builds() {
    // a wave's build is ahead of the release it came from, and behind the
    // channel's next build of the same wave
    let dev = format!("{ENGINE}.dev.2");
    let engine = Releases::new();
    engine.publish(&dev, false);
    let desk = TempDir::new("nils-desk-releases-dev");
    publish_desk(desk.path(), &format!("{ENGINE}.dev.3"), None);
    let install = Install::new(&dev, &dev);
    let o = install.run(&["update", "--check"], &engine, desk.path());
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert!(
        o.stdout
            .contains(&format!("engine {dev}: the newest release")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains(&format!("nils update --all would take desk {ENGINE}.dev.3")),
        "{}",
        o.stdout
    );

    publish_desk(desk.path(), ENGINE, None);
    let o = install.run(&["update", "--check"], &engine, desk.path());
    assert!(
        o.stdout.contains("every part is at its newest release"),
        "{}",
        o.stdout
    );
}

#[test]
fn a_desk_that_needs_a_newer_engine_contract_is_not_installed() {
    let engine = Releases::new();
    engine.publish(ENGINE, false);
    let desk = TempDir::new("nils-desk-releases-floor");
    publish_desk(
        desk.path(),
        "99.0.0",
        Some(r#"{"openapi": "999", "openapi_floor": "999", "suite": "3", "suite_floor": "2"}"#),
    );
    let install = Install::new(ENGINE, ENGINE);
    let o = install.run(&["update", "--check"], &engine, desk.path());
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert!(o.stdout.contains("99.0.0 is out and waits"), "{}", o.stdout);
    assert!(o.stdout.contains("HTTP contract 999"), "{}", o.stdout);
    assert!(
        o.stdout.contains("every part is at its newest release"),
        "{}",
        o.stdout
    );

    let o = install.run(&["update", "--part", "desk"], &engine, desk.path());
    assert!(
        o.stdout.contains("HTTP contract 999"),
        "{}\n{}",
        o.stdout,
        o.stderr
    );
    assert_eq!(
        install.desk(),
        format!("#!/bin/sh\necho nils-desk {ENGINE}\n"),
        "the desk was replaced"
    );

    // a floor this engine speaks is taken
    publish_desk(
        desk.path(),
        "99.0.1",
        Some(r#"{"openapi": "7", "openapi_floor": "1", "suite": "3", "suite_floor": "1"}"#),
    );
    let o = install.run(&["update", "--part", "desk"], &engine, desk.path());
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert_eq!(
        install.desk(),
        "#!/bin/sh\necho nils-desk 99.0.1\n",
        "{}",
        o.stdout
    );
}

#[test]
fn a_lab_pins_the_desk_at_a_version_of_its_own() {
    // the desk's number is its own: a pin below the engine's is taken as named
    let engine = Releases::new();
    engine.publish(ENGINE, false);
    let desk = TempDir::new("nils-desk-releases-pin");
    publish_desk(desk.path(), "0.4.2", None);
    let install = Install::new(ENGINE, ENGINE);
    let pin = [("NILS_SETUP_DESK_VERSION", "v0.4.2")];
    let o = install.run_with(&["update", "--check"], &engine, desk.path(), &pin);
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert!(
        o.stdout.contains("desk") && o.stdout.contains("follows v0.4.2"),
        "{}",
        o.stdout
    );
    let o = install.run_with(&["update", "--part", "desk"], &engine, desk.path(), &pin);
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);
    assert_eq!(
        install.desk(),
        "#!/bin/sh\necho nils-desk 0.4.2\n",
        "{}",
        o.stdout
    );
    assert!(
        install.record().contains("version = \"0.4.2\""),
        "{}",
        install.record()
    );
}

#[test]
fn a_part_without_releases_of_its_own_is_refused_by_name() {
    let o = run(&["update", "--part", "postgres", "--check"]);
    assert!(!o.ok, "{}", o.stdout);
    assert!(
        o.stderr.contains("engine, desk, assistant, kvasir"),
        "{}",
        o.stderr
    );
}
