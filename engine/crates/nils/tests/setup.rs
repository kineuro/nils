// SPDX-License-Identifier: AGPL-3.0-only
//! `nils setup`: the wizard a person meets after the one line installer.
//!
//! Nothing here reaches the network, starts a container or touches a
//! database. The release is a directory of files reached by `file://`, the
//! way `tests/update.rs` lays one out; podman and docker are asserted
//! through `--print`, which says the exact commands and unit files and
//! changes nothing; and every run is told there is no terminal, so it takes
//! the defaults instead of waiting for an answer.
use std::path::{Path, PathBuf};
use std::process::Command;

use nils_dicom::synth::TempDir;

/// A copy of the binary in a directory of its own, so what the wizard
/// installs beside itself lands in the temporary directory and not in the
/// build tree.
struct Installed {
    dir: TempDir,
}

impl Installed {
    /// The binary in a `bin` directory of its own, which is where the one
    /// line installer puts it: `~/.local/bin` for a person, `/usr/local/bin`
    /// for root. The prefix above it is where the packs go.
    fn new(name: &str) -> Installed {
        let dir = TempDir::new(name);
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        let to = dir
            .path()
            .join("bin")
            .join(if cfg!(windows) { "nils.exe" } else { "nils" });
        std::fs::copy(env!("CARGO_BIN_EXE_nils"), &to).expect("the binary copies");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Installed { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir
            .path()
            .join("bin")
            .join(if cfg!(windows) { "nils.exe" } else { "nils" })
    }
}

/// Run a command, retrying while the binary just copied is still held open
/// for writing by a sibling test's spawn. The window is a fork away and it
/// closes at once; a copy of an executable is otherwise the simplest way to
/// give the wizard a directory of its own to install beside itself.
fn output(command: &mut Command) -> std::process::Output {
    for _ in 0..40 {
        match command.output() {
            Ok(out) => return out,
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => panic!("nils runs: {e}"),
        }
    }
    panic!("the binary stayed busy");
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Out {
    fn says(&self, text: &str) {
        assert!(
            self.stdout.contains(text),
            "{text} is not in the output:\n{}\n{}",
            self.stdout,
            self.stderr
        );
    }
}

/// Run the wizard with no terminal, no colour, and a configuration
/// directory of its own, so nothing of the machine's own is read or written.
fn setup(binary: &Path, config: &Path, args: &[&str]) -> Out {
    let out = output(
        Command::new(binary)
            .arg("setup")
            .args(args)
            .env("NILS_NO_TTY", "1")
            .env("NO_COLOR", "1")
            .env("XDG_CONFIG_HOME", config)
            .env_remove("NILS_RELEASES")
            .env_remove("NILS_DESK_RELEASES"),
    );
    Out {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// This platform's target, spelled the way a release file is.
fn target() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{}-{arch}", std::env::consts::OS)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// A release directory holding one version of the engine and the desk.
struct Releases {
    dir: TempDir,
}

impl Releases {
    fn new(version: &str) -> Releases {
        let releases = Releases {
            dir: TempDir::new("nils-setup-releases"),
        };
        let into = releases
            .dir
            .path()
            .join("download")
            .join(format!("v{version}"));
        std::fs::create_dir_all(&into).unwrap();
        let mut sums = String::new();
        for part in ["nils", "nils-desk"] {
            let name = if cfg!(windows) {
                format!("{part}-{}.exe", target())
            } else {
                format!("{part}-{}", target())
            };
            let body = format!("#!/bin/sh\necho {part} {version}\n");
            std::fs::write(into.join(&name), &body).unwrap();
            sums.push_str(&format!("{}  {name}\n", sha256_hex(body.as_bytes())));
        }
        // The rule packs travel with the binaries, in one tarball holding a
        // `packs/` directory, which is what a machine install unpacks.
        let packs = tar_gz_of_one_pack();
        std::fs::write(into.join("packs.tar.gz"), &packs).unwrap();
        sums.push_str(&format!("{}  packs.tar.gz\n", sha256_hex(&packs)));
        std::fs::write(into.join("SHA256SUMS"), sums).unwrap();
        let latest = releases.dir.path().join("latest").join("download");
        std::fs::create_dir_all(&latest).unwrap();
        std::fs::write(latest.join("VERSION"), format!("{version}\n")).unwrap();
        releases
    }

    fn url(&self) -> String {
        format!("file://{}", self.dir.path().display())
    }
}

/// A `packs.tar.gz` of one pack, shaped the way the release's is: a `packs/`
/// directory holding one pack directory with a `pack.toml` in it.
fn tar_gz_of_one_pack() -> Vec<u8> {
    let toml = b"name = \"mri\"\nversion = \"0.0.1\"\ncontract = 4\n";
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut builder = tar::Builder::new(&mut gz);
        builder.mode(tar::HeaderMode::Deterministic);
        let mut header = tar::Header::new_gnu();
        header.set_size(toml.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "packs/mri/pack.toml", &toml[..])
            .unwrap();
        builder.finish().unwrap();
    }
    gz.finish().unwrap()
}

fn state_of(config: &Path) -> String {
    std::fs::read_to_string(config.join("nils").join("setup.toml"))
        .expect("the wizard wrote its state file")
}

#[test]
fn print_says_the_registry_the_desk_and_the_mode_and_writes_nothing() {
    let nils = Installed::new("nils-setup-print");
    let config = TempDir::new("nils-setup-print-config");
    let base = TempDir::new("nils-setup-print-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--dir",
            dir.to_str().unwrap(),
            "--parts",
            "desk",
            "--mode",
            "local",
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says(&dir.join("registry").display().to_string());
    o.says(
        &dir.join("desk")
            .join("nils-desk.toml")
            .display()
            .to_string(),
    );
    o.says("the desk keeps the people and their passwords (local)");
    o.says("nothing was changed");
    assert!(!dir.exists(), "--print made {}", dir.display());
    assert!(
        !config.path().join("nils").join("setup.toml").exists(),
        "--print wrote the state file"
    );
}

#[test]
fn a_run_with_no_terminal_takes_the_defaults_and_says_so() {
    let nils = Installed::new("nils-setup-piped");
    let config = TempDir::new("nils-setup-piped-config");
    let base = TempDir::new("nils-setup-piped-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--dir",
            dir.to_str().unwrap(),
            "--parts",
            "engine",
            "--no-service",
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says("no terminal here, so every default is taken");
    // The registry's passphrase cannot be asked for, so one is made and the
    // person is told where it is rather than left with a registry they
    // cannot open.
    o.says("no terminal to ask on");
    let secret = dir.join("key.passphrase");
    assert!(secret.is_file(), "{} was not written", secret.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&secret).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the passphrase is readable by others");
    }
    assert!(state_of(config.path()).contains("mode = \"off\""));
}

#[test]
fn yes_makes_a_registry_and_a_state_file_and_a_second_run_changes_nothing() {
    let nils = Installed::new("nils-setup-yes");
    let config = TempDir::new("nils-setup-yes-config");
    let base = TempDir::new("nils-setup-yes-base");
    let dir = base.path().join("nils");
    let args = [
        "--yes",
        "--parts",
        "engine",
        "--dir",
        dir.to_str().unwrap(),
        "--no-service",
    ];

    let first = setup(&nils.path(), config.path(), &args);
    assert!(first.ok, "{}", first.stderr);
    first.says("registry at");
    assert!(dir.join("registry").join("nils.toml").is_file());
    let state = state_of(config.path());
    assert!(
        state.contains(&format!("dir = \"{}\"", dir.display())),
        "{state}"
    );
    assert!(state.contains("runtime = \"machine\""), "{state}");
    assert!(state.contains("[parts.engine]"), "{state}");
    let key = std::fs::read(dir.join("registry").join("keys")).ok();

    let second = setup(&nils.path(), config.path(), &args);
    assert!(second.ok, "{}", second.stderr);
    second.says("already there");
    second.says("places: backups as backup, registry as registry");
    assert_eq!(
        std::fs::read(dir.join("registry").join("keys")).ok(),
        key,
        "the second run made a new key"
    );
    // The same places, not four more.
    let again = state_of(config.path());
    assert_eq!(
        again.lines().find(|l| l.starts_with("places =")),
        state.lines().find(|l| l.starts_with("places =")),
        "the second run declared different places"
    );
}

/// A rerun that names another directory of DICOM moves the source place
/// there, where the old path stayed and the engine was given the new one;
/// and a source place added with nils place add is mounted and handed to the
/// engine like the one setup asked for.
#[test]
fn a_rerun_follows_the_source_and_every_source_place_is_read() {
    let nils = Installed::new("nils-setup-sources");
    let config = TempDir::new("nils-setup-sources-config");
    let base = TempDir::new("nils-setup-sources-base");
    let dir = base.path().join("nils");
    let registry = dir.join("registry");
    let (first, moved, added) = (
        base.path().join("dicom-a"),
        base.path().join("dicom-b"),
        base.path().join("scanner-2"),
    );
    for d in [&first, &moved, &added] {
        std::fs::create_dir_all(d).unwrap();
    }
    let run = |source: &Path, more: &[&str]| {
        let mut args = vec![
            "--yes",
            "--parts",
            "engine",
            "--dir",
            dir.to_str().unwrap(),
            "--source",
            source.to_str().unwrap(),
        ];
        args.extend_from_slice(more);
        setup(&nils.path(), config.path(), &args)
    };

    let o = run(&first, &["--no-service"]);
    assert!(o.ok, "{}", o.stderr);
    let made = output(
        Command::new(nils.path())
            .arg("--registry")
            .arg(&registry)
            .args(["place", "add", "scanner2"])
            .arg(&added)
            .args(["--role", "source"]),
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );

    let o = run(&moved, &["--no-service"]);
    assert!(o.ok, "{}", o.stderr);
    o.says("the source place is now");
    let listed = output(
        Command::new(nils.path())
            .arg("--registry")
            .arg(&registry)
            .args(["place", "list", "--json"]),
    );
    let places: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let path_of = |name: &str| {
        places
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .and_then(|p| p["path"].as_str())
            .map(PathBuf::from)
    };
    assert_eq!(
        path_of("source"),
        Some(std::fs::canonicalize(&moved).unwrap()),
        "{places}"
    );
    let state = state_of(config.path());
    assert!(state.contains("scanner2"), "{state}");

    let o = run(&moved, &["--print", "--runtime", "podman", "--service"]);
    assert!(o.ok, "{}", o.stderr);
    o.says(&format!("-v {0}:{0}:ro", moved.display()));
    o.says(&format!("-v {0}:{0}:ro", added.display()));
    o.says(&format!("--ingest-root scanner2={}", added.display()));
}

/// A desk registered at a provider already is named with flags: the desk
/// gets its [oidc] table and the client's secret beside it, the record keeps
/// the provider, and a later run gives the engine the provider's trust.
#[test]
fn a_provider_named_with_flags_is_written_for_the_desk_and_the_engine() {
    let releases = Releases::new("99.0.0");
    let nils = Installed::new("nils-setup-provider");
    let config = TempDir::new("nils-setup-provider-config");
    let base = TempDir::new("nils-setup-provider-base");
    let dir = base.path().join("nils");
    let secret = base.path().join("secret");
    std::fs::write(&secret, "s3cret\n").unwrap();
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "desk",
            "--mode",
            "oidc",
            "--dir",
            dir.to_str().unwrap(),
            "--no-service",
            "--oidc-issuer",
            "https://auth.example.org/application/o/nils/",
            "--oidc-client-id",
            "abc123",
            "--oidc-client-secret-file",
            secret.to_str().unwrap(),
            "--oidc-jwks",
            "https://auth.example.org/application/o/nils/jwks/",
            "--channel",
            &releases.url(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    let desk = std::fs::read_to_string(dir.join("desk").join("nils-desk.toml")).unwrap();
    assert!(desk.contains("\n[oidc]\n"), "{desk}");
    assert!(desk.contains("client_id = \"abc123\""), "{desk}");
    let kept = dir.join("desk").join("client-secret");
    assert_eq!(std::fs::read_to_string(&kept).unwrap().trim(), "s3cret");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        !o.stdout.contains("nils-desk register --authentik"),
        "{}",
        o.stdout
    );
    let state = state_of(config.path());
    assert!(state.contains("abc123"), "{state}");

    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--runtime",
            "podman",
            "--parts",
            "desk",
            "--mode",
            "oidc",
            "--dir",
            dir.to_str().unwrap(),
            "--service",
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says("--oidc-trust issuer=https://auth.example.org/application/o/nils/,audience=abc123");
}

/// A machine install carries no rule packs in the binary, so it has to take
/// them from the release. Without them the engine starts saying `packs none`
/// and refuses to digest anything, which is the whole of what it is for.
/// They go where the engine looks by default, not only where the service
/// this wizard wrote would look.
#[test]
fn a_machine_install_puts_the_packs_where_the_engine_looks() {
    let releases = Releases::new("99.0.0");
    let nils = Installed::new("nils-setup-packs");
    let config = TempDir::new("nils-setup-packs-config");
    let base = TempDir::new("nils-setup-packs-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "engine",
            "--dir",
            dir.to_str().unwrap(),
            "--no-service",
            "--channel",
            &releases.url(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    // The binary sits in a bin directory, so the packs sit in the share
    // directory of the same prefix: one of the places the engine looks with
    // no flag at all.
    let prefix = nils
        .path()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let packs = prefix.join("share").join("nils").join("packs");
    assert!(
        packs.join("mri").join("pack.toml").is_file(),
        "no pack at {}:\n{}",
        packs.display(),
        o.stdout
    );
    o.says("packs at");
}

/// An install that would not work is not finished. A desk the release does
/// not have stops the install with the reason and a failure, where it once
/// ended on a card saying NILS was running with no desk to open.
#[test]
fn a_desk_that_cannot_be_installed_stops_the_install() {
    let releases = Releases::new("99.0.0");
    let desk = releases
        .dir
        .path()
        .join("download")
        .join("v99.0.0")
        .join(if cfg!(windows) {
            format!("nils-desk-{}.exe", target())
        } else {
            format!("nils-desk-{}", target())
        });
    std::fs::remove_file(&desk).unwrap();
    let nils = Installed::new("nils-setup-stops");
    let config = TempDir::new("nils-setup-stops-config");
    let base = TempDir::new("nils-setup-stops-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "desk",
            "--dir",
            dir.to_str().unwrap(),
            "--no-service",
            "--channel",
            &releases.url(),
        ],
    );
    assert!(!o.ok, "the install went on without its desk:\n{}", o.stdout);
    assert!(
        o.stderr.contains("the desk was not installed"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr.contains("nils uninstall removes it"),
        "{}",
        o.stderr
    );
}

#[test]
fn the_mode_writes_the_configuration_that_mode_asks_for() {
    let releases = Releases::new("99.0.0");
    for (mode, wanted, absent) in [
        ("off", "mode = \"off\"", "[local]"),
        ("local", "[local]", "# [oidc]"),
        ("oidc", "# [oidc]", "[local]"),
    ] {
        let nils = Installed::new(&format!("nils-setup-mode-{mode}"));
        let config = TempDir::new(&format!("nils-setup-mode-{mode}-config"));
        let base = TempDir::new(&format!("nils-setup-mode-{mode}-base"));
        let dir = base.path().join("nils");
        let o = setup(
            &nils.path(),
            config.path(),
            &[
                "--yes",
                "--parts",
                "desk",
                "--mode",
                mode,
                "--dir",
                dir.to_str().unwrap(),
                "--no-service",
                "--channel",
                &releases.url(),
            ],
        );
        assert!(o.ok, "{mode}: {}", o.stderr);
        let text = std::fs::read_to_string(dir.join("desk").join("nils-desk.toml"))
            .unwrap_or_else(|e| panic!("{mode}: no desk configuration: {e}"));
        assert!(
            text.contains(&format!("mode = \"{mode}\"")),
            "{mode}: {text}"
        );
        assert!(text.contains(wanted), "{mode}: {text}");
        assert!(!text.contains(absent), "{mode}: {text}");
        assert!(text.contains("[engine]"), "{mode}: {text}");
        // every install runs the supervisor, which the desk's settings read
        assert!(text.contains("[supervisor]"), "{mode}: {text}");
        assert!(
            dir.join("supervise").join("supervise.toml").is_file(),
            "{mode}: no supervisor was written"
        );
        // The desk that was asked for came from the release, checked.
        assert!(
            dir.join("desk").exists() && nils.path().with_file_name("nils-desk").is_file(),
            "{mode}: the desk was not installed: {}",
            o.stdout
        );
        if mode == "local" {
            o.says("nils-desk user add");
        }
        if mode == "oidc" {
            o.says("nils-desk register --authentik");
        }
    }
}

#[test]
fn podman_is_one_pod_that_publishes_only_the_desk() {
    let nils = Installed::new("nils-setup-podman");
    let config = TempDir::new("nils-setup-podman-config");
    let base = TempDir::new("nils-setup-podman-base");
    let dir = base.path().join("nils");
    let source = base.path().join("dicom");
    std::fs::create_dir_all(&source).unwrap();
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--runtime",
            "podman",
            "--parts",
            "desk",
            "--mode",
            "local",
            "--dir",
            dir.to_str().unwrap(),
            "--source",
            source.to_str().unwrap(),
            "--service",
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says("podman pod create --name nils -p 127.0.0.1:7200:7200");
    o.says("--pod nils --name nils-engine");
    o.says(&format!("-v {0}:{0}:U", dir.join("registry").display()));
    o.says(&format!("-v {0}:{0}:ro", source.display()));
    o.says("ghcr.io/kineuro/nils:v");
    o.says("ghcr.io/kineuro/nils-desk:v");
    // local mode: the engine trusts the desk to say who the person is
    o.says("--auth oidc --oidc-trust issuer=http://127.0.0.1:7200");
    // the quadlets, which is how podman comes back after a restart
    o.says("nils.pod");
    o.says("PublishPort=127.0.0.1:7200:7200");
    o.says("nils-engine.container");
    o.says("Pod=nils.pod");
    o.says("nils-desk.container");
    assert!(!dir.exists(), "--print made {}", dir.display());
}

#[test]
fn docker_is_a_network_and_a_compose_file_and_owns_no_mounts() {
    let nils = Installed::new("nils-setup-docker");
    let config = TempDir::new("nils-setup-docker-config");
    let base = TempDir::new("nils-setup-docker-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--runtime",
            "docker",
            "--parts",
            "desk",
            "--dir",
            dir.to_str().unwrap(),
            "--service",
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says("docker network create nils");
    o.says("--network nils --name nils-engine");
    // A container reaches the desk by its name on a docker network, not on
    // a loopback that is its own.
    let with_login = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--runtime",
            "docker",
            "--parts",
            "desk",
            "--mode",
            "local",
            "--dir",
            dir.to_str().unwrap(),
        ],
    );
    assert!(with_login.ok, "{}", with_login.stderr);
    with_login.says("jwks=http://nils-desk:7200/.well-known/jwks.json");
    o.says("-p 127.0.0.1:7200:7200");
    o.says("compose.yaml");
    o.says("image: ghcr.io/kineuro/nils:v");
    o.says("container_name: nils-desk");
    o.says("depends_on: [engine]");
    assert!(
        !o.stdout.contains(":U"),
        "docker was given podman's mount flag:\n{}",
        o.stdout
    );
}

/// An address given on the command line is what the desk answers at, is
/// written down, and is still there after an update, which is made from the
/// record alone.
#[test]
fn an_origin_given_is_recorded_and_survives_an_update() {
    let releases = Releases::new("99.0.0");
    let nils = Installed::new("nils-setup-origin");
    let config = TempDir::new("nils-setup-origin-config");
    let base = TempDir::new("nils-setup-origin-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "desk",
            "--mode",
            "local",
            "--origin",
            "https://nils.example.org",
            "--dir",
            dir.to_str().unwrap(),
            "--no-service",
            "--channel",
            &releases.url(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    let desk = dir.join("desk").join("nils-desk.toml");
    let text = std::fs::read_to_string(&desk).expect("the desk's configuration");
    assert!(
        text.contains("origin = \"https://nils.example.org\""),
        "{text}"
    );
    // the desk binds where only a proxy here reaches it, on the port it was
    // given, which moves where this machine holds the default already
    let port = text
        .lines()
        .find_map(|line| line.strip_prefix("bind = \"127.0.0.1:"))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("it did not bind the loopback: {text}"));
    assert!(
        text.contains(&format!("also_origins = [\"http://127.0.0.1:{port}\"")),
        "a browser on the machine still opens it: {text}"
    );
    let state = state_of(config.path());
    assert!(
        state.contains("origin = \"https://nils.example.org\""),
        "{state}"
    );

    // the update writes the desk's configuration again from the record
    let again = setup(
        &nils.path(),
        config.path(),
        &["--update", "--yes", "--channel", &releases.url()],
    );
    assert!(again.ok, "{}", again.stderr);
    again.says("https://nils.example.org");
    let after = std::fs::read_to_string(&desk).expect("the desk's configuration");
    assert!(
        after.contains("origin = \"https://nils.example.org\""),
        "the update stamped an address of its own over it:\n{after}"
    );
}

/// An address a browser could not open is refused before anything is written.
#[test]
fn an_origin_that_is_not_an_address_is_refused_and_nothing_is_written() {
    let nils = Installed::new("nils-setup-origin-bad");
    let config = TempDir::new("nils-setup-origin-bad-config");
    let base = TempDir::new("nils-setup-origin-bad-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "desk",
            "--origin",
            "nils.example.org",
            "--dir",
            dir.to_str().unwrap(),
            "--no-service",
        ],
    );
    assert!(!o.ok, "a bare host was taken as an address:\n{}", o.stdout);
    assert!(
        o.stderr
            .contains("has no scheme: write https://nils.example.org"),
        "{}",
        o.stderr
    );
    assert!(!dir.exists(), "{} was made", dir.display());
    assert!(
        !config.path().join("nils").join("setup.toml").exists(),
        "the record was written"
    );
}

/// With no session of this account there is nothing to keep the parts
/// running, and an install told to write services says so at the question,
/// before a file is written.
#[cfg(target_os = "linux")]
#[test]
fn an_install_that_cannot_keep_services_says_so_before_it_writes_anything() {
    let nils = Installed::new("nils-setup-nosession");
    let config = TempDir::new("nils-setup-nosession-config");
    let base = TempDir::new("nils-setup-nosession-base");
    let dir = base.path().join("nils");
    let out = output(
        Command::new(nils.path())
            .arg("setup")
            .args([
                "--yes",
                "--parts",
                "engine",
                "--service",
                "--dir",
                dir.to_str().unwrap(),
            ])
            .env("NILS_NO_TTY", "1")
            .env("NO_COLOR", "1")
            .env("XDG_CONFIG_HOME", config.path())
            // what a machine nobody is logged in to has: systemctl, and no
            // user manager for it to talk to
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("NILS_RELEASES")
            .env_remove("NILS_DESK_RELEASES"),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "it went on and installed:\n{stdout}");
    assert!(stderr.contains("no systemd session"), "{stderr}");
    assert!(stderr.contains("loginctl enable-linger"), "{stderr}");
    assert!(!dir.exists(), "{} was made", dir.display());
    assert!(
        !config.path().join("systemd").exists(),
        "units were written for a manager that would not take them"
    );
}

/// Whether this run is root, which decides which sentence a refusal of the
/// machine's own services carries.
#[cfg(target_os = "linux")]
fn as_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
        .unwrap_or(false)
}

/// `--print` says the units an install on this machine would write and the
/// calls it would make to hand them over, for the services of this account
/// and for the machine's own alike, and writes nothing either way.
#[cfg(target_os = "linux")]
#[test]
fn print_says_the_units_and_the_calls_of_an_install_on_this_machine() {
    let nils = Installed::new("nils-setup-print-units");
    let config = TempDir::new("nils-setup-print-units-config");
    let base = TempDir::new("nils-setup-print-units-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--service",
            "--parts",
            "desk",
            "--mode",
            "local",
            "--dir",
            dir.to_str().unwrap(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says("nils-engine.service");
    o.says("nils-desk.service");
    o.says("nils-supervise.service");
    o.says("ExecStart=");
    o.says("WantedBy=default.target");
    o.says("systemctl --user daemon-reload");
    o.says("systemctl --user restart nils-engine");
    assert!(!dir.exists(), "--print made {}", dir.display());
    // and takes every step on the registry as itself
    assert!(
        !o.stdout.contains("runuser"),
        "an install of this account's own acts as another:\n{}",
        o.stdout
    );
    // an install of this account's own restarts its own units and replaces
    // files it owns, so it is given no privilege at all
    assert!(
        !o.stdout.contains("nils-manage") && !o.stdout.contains("sudoers"),
        "an install of this account's own grew a privilege it does not need:\n{}",
        o.stdout
    );

    // the services of this machine, with an account of its own for the desk
    // and the capabilities the engine needs
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--system",
            "--account",
            "desk=nils-desk",
            "--account",
            "supervisor=nils-deploy",
            "--capabilities",
            "CAP_DAC_OVERRIDE,CAP_DAC_READ_SEARCH",
            "--parts",
            "desk",
            "--mode",
            "local",
            "--dir",
            dir.to_str().unwrap(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says("/etc/systemd/system");
    o.says("User=nils\n");
    o.says("User=nils-desk\n");
    o.says("AmbientCapabilities=CAP_DAC_OVERRIDE CAP_DAC_READ_SEARCH");
    o.says("WantedBy=multi-user.target");
    o.says("systemctl daemon-reload");
    o.says("systemctl enable nils-engine");
    // and the privilege it keeps, whole, so an operator can read exactly
    // what would go on the machine and put it there by hand
    o.says("User=nils-deploy\n");
    o.says("/usr/local/sbin/nils-manage");
    o.says("/etc/sudoers.d/nils-manage");
    o.says("nils-deploy ALL=(root) NOPASSWD:");
    o.says("/usr/local/sbin/nils-manage restart engine");
    o.says("/usr/local/sbin/nils-manage restart all");
    o.says("/usr/local/sbin/nils-manage reapply");
    o.says("/usr/local/sbin/nils-manage reapply all");
    o.says("/usr/local/sbin/nils-manage update");
    // and every step on the registry as the engine's account, through the
    // engine binary, the way its service reaches the registry
    let as_engine = format!(
        "runuser -u nils -- {} --registry {}",
        nils.path().display(),
        dir.join("registry").display()
    );
    o.says(&format!("{as_engine} key add nils\n"));
    o.says(&format!(
        "{as_engine} setup-registry init --backend sqlite\n"
    ));
    o.says(&format!("{as_engine} setup-registry declare\n"));
    o.says(&format!(
        "      registry {} --role registry --backup backups",
        dir.join("registry").display()
    ));
    assert!(!dir.exists(), "--print made {}", dir.display());
    assert!(
        !config.path().join("nils").join("setup.toml").exists(),
        "--print wrote the state file"
    );

    // the account that keeps the parts running is named outright and never
    // fallen back to, so an install that leaves it out is told so, and no
    // rule is written for an account nobody chose
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--system",
            "--parts",
            "desk",
            "--mode",
            "local",
            "--dir",
            dir.to_str().unwrap(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says("--account supervisor=nils-deploy");
    o.says("not the account any part runs as");
    assert!(
        !o.stdout.contains("NOPASSWD:"),
        "a rule was written for an account nobody named:\n{}",
        o.stdout
    );
    assert!(!dir.exists(), "--print made {}", dir.display());
}

/// The services of this machine are root's, and every account they run as
/// has to be there: what this run cannot have is said before a file is
/// written, with the fix in the same sentence.
#[cfg(target_os = "linux")]
#[test]
fn services_of_this_machine_are_refused_before_anything_is_written() {
    let nils = Installed::new("nils-setup-system-refused");
    let config = TempDir::new("nils-setup-system-refused-config");
    let base = TempDir::new("nils-setup-system-refused-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--system",
            "--account",
            "supervisor=nils-deploy",
            "--parts",
            "engine",
            "--dir",
            dir.to_str().unwrap(),
        ],
    );
    assert!(!o.ok, "it went on and installed:\n{}", o.stdout);
    if as_root() {
        assert!(
            o.stderr.contains("no account on this machine named")
                || o.stderr.contains("no accounts on this machine named")
                || o.stderr.contains("no systemd"),
            "{}",
            o.stderr
        );
    } else {
        assert!(
            o.stderr.contains("is root's to do") || o.stderr.contains("no systemd"),
            "{}",
            o.stderr
        );
    }
    assert!(!dir.exists(), "{} was made", dir.display());
    assert!(
        !config.path().join("nils").join("setup.toml").exists(),
        "the record was written"
    );

    // a capability is a thing a service of this account cannot carry at all
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "engine",
            "--no-service",
            "--capabilities",
            "CAP_DAC_OVERRIDE",
            "--dir",
            dir.to_str().unwrap(),
        ],
    );
    assert!(!o.ok, "{}", o.stdout);
    assert!(
        o.stderr.contains("--capabilities goes with --system"),
        "{}",
        o.stderr
    );
    assert!(!dir.exists(), "{} was made", dir.display());
}

/// An install whose services are the machine's own is updated as the install
/// it is: a run that cannot write those services says so and changes
/// nothing, rather than writing half an install of this account's.
#[cfg(target_os = "linux")]
#[test]
fn an_update_of_the_machines_services_says_what_it_cannot_do() {
    let nils = Installed::new("nils-setup-system-update");
    let config = TempDir::new("nils-setup-system-update-config");
    let base = TempDir::new("nils-setup-system-update-base");
    let dir = base.path().join("nils");
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    std::fs::create_dir_all(config.path().join("nils")).unwrap();
    let record = config.path().join("nils").join("setup.toml");
    let written = format!(
        "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\n\
         service = \"systemd system units\"\nreach = \"loopback\"\nbackend = \"sqlite\"\n\n\
         [parts.engine]\nversion = \"1.0.0\"\npath = \"/usr/local/bin/nils\"\nkind = \"binary\"\n\n\
         [system]\ncapabilities = [\"CAP_DAC_OVERRIDE\"]\n\n\
         [system.accounts]\nengine = \"nils-nobody-of-this-machine\"\n",
        dir.display()
    );
    std::fs::write(&record, &written).unwrap();

    let o = setup(&nils.path(), config.path(), &["--update", "--yes"]);
    assert!(!o.ok, "it went on and updated:\n{}", o.stdout);
    assert!(o.stderr.contains("nothing was changed"), "{}", o.stderr);
    if as_root() {
        assert!(
            o.stderr.contains("nils-nobody-of-this-machine"),
            "{}",
            o.stderr
        );
    } else {
        assert!(o.stderr.contains("is root's to do"), "{}", o.stderr);
    }
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        written,
        "the record was rewritten by a run that could not do the work"
    );
    assert!(
        !config.path().join("systemd").exists(),
        "units of this account were written for an install whose services are the machine's"
    );
}

#[test]
fn the_network_and_no_login_are_not_offered_together() {
    let nils = Installed::new("nils-setup-reach");
    let config = TempDir::new("nils-setup-reach-config");
    let base = TempDir::new("nils-setup-reach-base");
    let dir = base.path().join("nils");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--parts",
            "desk",
            "--mode",
            "off",
            "--reach",
            "network",
            "--dir",
            dir.to_str().unwrap(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    o.says("off mode has no login, so anyone on that network who finds the port gets the whole registry");
    // The default answer keeps the people in the desk instead, so the plan
    // that follows is a login, not an open registry on the network.
    o.says("(local)");
}

#[test]
fn what_this_machine_can_do_is_said_before_the_assistant_is_offered() {
    let nils = Installed::new("nils-setup-card");
    let config = TempDir::new("nils-setup-card-config");
    let base = TempDir::new("nils-setup-card-base");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--dir",
            base.path().join("nils").to_str().unwrap(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    let said = [
        "No local model worth serving.",
        "A 7B to 14B model fits.",
        "A 27B model at 4 bit fits",
    ];
    assert!(
        said.iter().any(|s| o.stdout.contains(s)),
        "the card was not read out:\n{}",
        o.stdout
    );
    if o.stdout.contains("No local model worth serving.") {
        o.says("no assistant at all");
        o.says("Kvasir marks that backend remote");
    }
}

/// With the assistant, the plan names the llama.cpp build that runs the
/// models Kvasir starts, and where it listens.
#[test]
fn the_plan_names_llama_cpp_beside_the_assistant() {
    let nils = Installed::new("nils-setup-llama");
    let config = TempDir::new("nils-setup-llama-config");
    let base = TempDir::new("nils-setup-llama-base");
    let o = setup(
        &nils.path(),
        config.path(),
        &[
            "--print",
            "--parts",
            "engine,desk,assistant",
            "--runtime",
            "machine",
            "--dir",
            base.path().join("nils").to_str().unwrap(),
        ],
    );
    assert!(o.ok, "{}", o.stderr);
    let built = matches!(std::env::consts::OS, "linux" | "macos")
        && matches!(std::env::consts::ARCH, "x86_64" | "aarch64");
    if !built {
        o.says("no build for this machine");
        return;
    }
    o.says("b10964, the");
    o.says("runs the models Kvasir starts, on 127.0.0.1:7110");
    if o.stdout.contains("nils-llama.service") {
        o.says("--no-models-autoload --models-max 1");
        // and the units a run stops while their files change, each just
        // before its own
        o.says("stopped while their files change");
        o.says("stop nils-llama  while the older llama.cpp builds are removed");
        o.says("stop kvasir  while Kvasir's source is taken and built");
        o.says("stop nils-assistant  while the assistant's source is taken and built");
        assert!(
            !o.stdout.contains("stop nils-supervise"),
            "the supervisor would be stopped:\n{}",
            o.stdout
        );
    }
}

/// An uninstall takes Kvasir's state with NILS, where the data is kept too:
/// the models it holds and their keys, its subscriptions, its seal key and
/// pepper, and the assistant's key. The registry and the assistant's history
/// stay.
#[test]
fn an_uninstall_takes_kvasirs_state_and_keeps_the_data() {
    let nils = Installed::new("nils-setup-uninstall-kvasir");
    let config = TempDir::new("nils-setup-uninstall-kvasir-config");
    let base = TempDir::new("nils-setup-uninstall-kvasir-base");
    let dir = base.path().join("nils");
    let kvasir = dir.join("kvasir");
    std::fs::create_dir_all(kvasir.join("state")).unwrap();
    for file in [
        "kvasir.json",
        "kvasir.sqlite",
        "kvasir.seal",
        "kvasir.pepper",
        "assistant.key",
        "state/held",
    ] {
        std::fs::write(kvasir.join(file), "x").unwrap();
    }
    let assistant = dir.join("assistant");
    std::fs::create_dir_all(assistant.join("dist")).unwrap();
    std::fs::write(assistant.join("assistant.sqlite"), "x").unwrap();
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    std::fs::write(
        dir.join("registry").join("nils.toml"),
        "backend = \"sqlite\"\n",
    )
    .unwrap();
    let record = config.path().join("nils");
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(
        record.join("setup.toml"),
        format!(
            "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"none\"\n\n\
             [parts.kvasir]\nversion = \"from source\"\npath = \"{}\"\nkind = \"node\"\n\n\
             [parts.assistant]\nversion = \"from source\"\npath = \"{}\"\nkind = \"node\"\n",
            dir.display(),
            kvasir.display(),
            assistant.display()
        ),
    )
    .unwrap();

    let out = output(
        Command::new(nils.path())
            .args(["uninstall", "--keep-data", "--yes"])
            .env("NILS_NO_TTY", "1")
            .env("NO_COLOR", "1")
            .env("XDG_CONFIG_HOME", config.path()),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("the models it holds"), "{stdout}");
    assert!(!kvasir.exists(), "Kvasir's state stayed:\n{stdout}");
    assert!(!assistant.join("dist").exists(), "{stdout}");
    assert!(
        assistant.join("assistant.sqlite").is_file(),
        "the assistant's history is data"
    );
    assert!(dir.join("registry").join("nils.toml").is_file());
    assert!(!record.join("setup.toml").exists());
}

/// A purge removes the directory a setup record names, whatever is in it. An
/// install that stopped at the registry leaves a directory holding the
/// registry's key and neither a registry nor a desk, and refusing that left
/// the key on the disk after a purge that said it was done.
#[test]
fn a_purge_removes_the_directory_an_install_that_stopped_partway_left() {
    let nils = Installed::new("nils-setup-purge-half-made");
    let config = TempDir::new("nils-setup-purge-half-made-config");
    let base = TempDir::new("nils-setup-purge-half-made-base");
    let dir = base.path().join("nils");
    std::fs::create_dir_all(dir.join("registry").join("keys")).unwrap();
    std::fs::write(dir.join("registry").join("keys").join("nils"), "a key").unwrap();
    std::fs::create_dir_all(dir.join("desk")).unwrap();
    let record = config.path().join("nils");
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(
        record.join("setup.toml"),
        format!(
            "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"none\"\n\
             unfinished = true\n",
            dir.display()
        ),
    )
    .unwrap();

    let out = output(
        Command::new(nils.path())
            .args(["uninstall", "--purge", "--yes"])
            .env("NILS_NO_TTY", "1")
            .env("NO_COLOR", "1")
            .env("XDG_CONFIG_HOME", config.path()),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.exists(), "the registry's key stayed:\n{stdout}");
    assert!(!record.join("setup.toml").exists(), "{stdout}");
    assert!(
        stdout.contains("NILS and everything it made are gone from this machine"),
        "{stdout}"
    );
}

/// A purge whose schemas were not dropped prints the command with the
/// password masked, and keeps the file the connection string is in beside the
/// directory that goes, so the sentence can be acted on without the password
/// ever reaching a terminal, a scrollback or a log.
#[test]
fn a_purge_that_could_not_drop_a_schema_prints_no_password_and_keeps_the_string() {
    let nils = Installed::new("nils-setup-purge-password");
    let config = TempDir::new("nils-setup-purge-password-config");
    let base = TempDir::new("nils-setup-purge-password-base");
    let dir = base.path().join("nils");
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    // the record names a schema the registry does not answer at, so the drop
    // is refused with no database asked at all
    std::fs::write(
        dir.join("registry").join("nils.toml"),
        "backend = \"postgres\"\ndsn = \"postgres://nils:s3cret@127.0.0.1/nils\"\n\
         schema = \"theirs\"\n",
    )
    .unwrap();
    let record = config.path().join("nils");
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(
        record.join("setup.toml"),
        format!(
            "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"none\"\n\
             backend = \"postgres:ours\"\nregistry_made = \"ours\"\n",
            dir.display()
        ),
    )
    .unwrap();

    let out = output(
        Command::new(nils.path())
            .args(["uninstall", "--purge", "--yes"])
            .env("NILS_NO_TTY", "1")
            .env("NO_COLOR", "1")
            .env("XDG_CONFIG_HOME", config.path()),
    );
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{said}");
    assert!(
        !said.contains("s3cret"),
        "the password was printed:\n{said}"
    );
    assert!(
        said.contains("postgres://nils:***@127.0.0.1/nils"),
        "the command names the database with the password masked:\n{said}"
    );
    let kept = base.path().join("nils.registry.toml");
    assert!(
        kept.is_file(),
        "the connection string was not kept:\n{said}"
    );
    assert!(
        std::fs::read_to_string(&kept)
            .unwrap()
            .contains("postgres://nils:s3cret@127.0.0.1/nils")
    );
    assert!(
        said.contains(&format!(
            "the connection string it needs is in {}",
            kept.display()
        )),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "the connection string, kept in {}",
            kept.display()
        )),
        "the last line names it too:\n{said}"
    );
    assert!(!dir.exists(), "the directory still goes:\n{said}");
}

/// With no record left, a purge removes nothing it cannot say is this
/// install's, and says which files it found, where they are, and what to do
/// with them, so nobody is left with a key and no sentence.
#[test]
fn with_no_record_a_purge_names_the_files_it_found_and_removes_none_of_them() {
    let nils = Installed::new("nils-setup-purge-no-record");
    let config = TempDir::new("nils-setup-purge-no-record-config");
    let home = TempDir::new("nils-setup-purge-no-record-home");
    let dir = home.path().join("nils");
    let key = dir.join("registry").join("keys").join("nils");
    std::fs::create_dir_all(dir.join("registry").join("keys")).unwrap();
    std::fs::write(&key, "a key").unwrap();
    std::fs::create_dir_all(dir.join("working")).unwrap();
    std::fs::write(dir.join("working").join("one.dcm"), "x").unwrap();

    let out = output(
        Command::new(nils.path())
            .args(["uninstall", "--purge", "--yes"])
            .env("NILS_NO_TTY", "1")
            .env("NO_COLOR", "1")
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", config.path()),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("no setup is recorded"), "{stdout}");
    assert!(
        stdout.contains(&key.display().to_string()),
        "the key is named where it is: {stdout}"
    );
    assert!(
        stdout.contains(&dir.join("working").join("one.dcm").display().to_string()),
        "{stdout}"
    );
    assert!(
        stdout.contains("the registry's key is among them"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("rm -rf {}", dir.display())),
        "what to do with them: {stdout}"
    );
    assert!(key.is_file(), "nothing here was removed:\n{stdout}");
}

/// An uninstall takes away the privilege the install was given: the rule
/// naming what may be run as root, and the program it named, the rule first.
#[test]
fn an_uninstall_takes_away_the_privilege_it_was_given() {
    let nils = Installed::new("nils-setup-uninstall-privilege");
    let config = TempDir::new("nils-setup-uninstall-privilege-config");
    let base = TempDir::new("nils-setup-uninstall-privilege-base");
    let dir = base.path().join("nils");
    std::fs::create_dir_all(dir.join("registry")).unwrap();
    std::fs::write(
        dir.join("registry").join("nils.toml"),
        "backend = \"sqlite\"\n",
    )
    .unwrap();
    // the two files of the arrangement, where this test may write them
    let helper = base.path().join("nils-manage");
    let rule = base.path().join("sudoers.d-nils-manage");
    std::fs::write(&helper, "#!/bin/sh\nexit 2\n").unwrap();
    std::fs::write(
        &rule,
        "nils-deploy ALL=(root) NOPASSWD: /usr/local/sbin/nils-manage restart engine\n",
    )
    .unwrap();
    let record = config.path().join("nils");
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(
        record.join("setup.toml"),
        format!(
            "dir = \"{}\"\nmode = \"off\"\nruntime = \"machine\"\nservice = \"systemd system units\"\n\n\
             [parts.engine]\nversion = \"1.0.0\"\npath = \"{}\"\nkind = \"binary\"\n\n\
             [helper]\naccount = \"nils-deploy\"\npath = \"{}\"\nrule = \"{}\"\n",
            dir.display(),
            base.path().join("bin").join("nils").display(),
            helper.display(),
            rule.display()
        ),
    )
    .unwrap();

    let out = output(
        Command::new(nils.path())
            .args(["uninstall", "--keep-data", "--yes"])
            .env("NILS_NO_TTY", "1")
            .env("NO_COLOR", "1")
            .env("XDG_CONFIG_HOME", config.path()),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("privilege"), "{stdout}");
    assert!(!rule.exists(), "the rule stayed:\n{stdout}");
    assert!(!helper.exists(), "the program stayed:\n{stdout}");
    // and the data is the data
    assert!(dir.join("registry").join("nils.toml").is_file());
    assert!(!record.join("setup.toml").exists());
}

/// A command run with something on its input, retrying while the binary just
/// copied is still held open for writing, as [`output`] does.
fn with_input(command: &mut Command, input: &str) -> std::process::Output {
    use std::io::Write as _;
    use std::process::Stdio;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for _ in 0..40 {
        match command.spawn() {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    // a command that stops before reading says why on stderr
                    let _ = stdin.write_all(input.as_bytes());
                }
                return child.wait_with_output().expect("nils ran");
            }
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => panic!("nils runs: {e}"),
        }
    }
    panic!("the binary stayed busy");
}

/// What setup runs as the engine's account under runuser is the engine's own
/// command, run here as this account: the places declared as setup declares
/// them, with no dataset declared on a source and a place already named moved
/// rather than refused, and the source places read back as the registry
/// stands, or where it stands said, or a registry that does not answer failed
/// with its reason.
#[test]
fn the_steps_taken_as_the_engines_account_declare_and_read_back_the_places_as_setup_does() {
    let nils = Installed::new("nils-setup-registry-steps");
    let base = TempDir::new("nils-setup-registry-steps-base");
    let registry = base.path().join("registry");
    let nils_at = |registry: &Path| {
        let mut command = Command::new(nils.path());
        command.arg("--registry").arg(registry);
        command
    };
    let json = |out: &std::process::Output| -> serde_json::Value {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("one JSON document")
    };

    // the registry, made by the two steps setup takes, the passphrase on the input
    let added = with_input(
        nils_at(&registry).args(["key", "add", "nils"]),
        "a fixture passphrase\n",
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let made = with_input(
        nils_at(&registry).args(["setup-registry", "init", "--backend", "sqlite"]),
        "",
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let settings = |db: &Path| -> Vec<(String, String)> {
        let db = rusqlite::Connection::open(db).unwrap();
        ["pseudonym_scheme", "display_length", "pseudonym_key"]
            .iter()
            .map(|key| {
                let value: String = db
                    .query_row(
                        "SELECT value FROM registry_meta WHERE key = ?1",
                        [key],
                        |row| row.get(0),
                    )
                    .unwrap();
                ((*key).to_string(), value)
            })
            .collect()
    };
    let made_as_setup = [
        ("pseudonym_scheme".to_string(), "blake2b-32".to_string()),
        ("display_length".to_string(), "12".to_string()),
        ("pseudonym_key".to_string(), "nils".to_string()),
    ];
    assert_eq!(
        settings(&registry.join("registry.db")),
        made_as_setup,
        "the registry setup makes, with the settings of its own process"
    );
    let sources = json(&output(
        nils_at(&registry).args(["setup-registry", "sources"]),
    ));
    assert_eq!(sources["registry"], true, "{sources}");
    assert_eq!(sources["sources"], serde_json::json!([]), "{sources}");

    // declared as setup declares them
    let (first, moved, added) = (
        base.path().join("dicom-a"),
        base.path().join("dicom-b"),
        base.path().join("scanner-2"),
    );
    for d in [first.join("dcm-raw"), moved.clone(), added.clone()] {
        std::fs::create_dir_all(d).unwrap();
    }
    let specs = |source: &Path| {
        serde_json::json!([
            {"name": "backups", "role": "backup", "path": base.path().join("backups"),
             "backup": null, "snapshots": false, "protected": false, "fast": false},
            {"name": "registry", "role": "registry", "path": registry,
             "backup": "backups", "snapshots": false, "protected": false, "fast": false},
            {"name": "source", "role": "source", "path": source,
             "backup": null, "snapshots": false, "protected": false, "fast": false},
        ])
        .to_string()
    };
    let declared = with_input(
        nils_at(&registry).args(["setup-registry", "declare"]),
        &specs(&first),
    );
    assert!(
        declared.status.success(),
        "{}",
        String::from_utf8_lossy(&declared.stderr)
    );
    let answered = String::from_utf8_lossy(&declared.stdout).to_string();
    let last: serde_json::Value =
        serde_json::from_str(answered.lines().last().unwrap_or_default()).unwrap();
    let names: Vec<&str> = last["places"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(names, ["backups", "registry", "source"], "{answered}");
    assert!(
        base.path().join("backups").is_dir(),
        "a place's missing directory is made by the account declaring it"
    );
    assert!(
        first.join("dcm-raw").is_dir() && !first.join("dcm-anon").exists(),
        "setup declares no dataset on a source, so nothing in its folder is renamed"
    );

    // a source added by hand, and a rerun that names another folder
    let placed = output(
        nils_at(&registry)
            .args(["place", "add", "scanner2"])
            .arg(&added)
            .args(["--role", "source"]),
    );
    assert!(
        placed.status.success(),
        "{}",
        String::from_utf8_lossy(&placed.stderr)
    );
    let again = with_input(
        nils_at(&registry).args(["setup-registry", "declare"]),
        &specs(&moved),
    );
    let answered = String::from_utf8_lossy(&again.stdout).to_string();
    assert!(again.status.success(), "{answered}");
    let moved_to = std::fs::canonicalize(&moved).unwrap();
    assert!(
        answered.contains(&serde_json::json!({ "said": format!("the source place is now {}", moved_to.display()) }).to_string()),
        "a place already named is moved, and that is said: {answered}"
    );
    assert!(answered.contains("scanner2"), "{answered}");

    let sources = json(&output(
        nils_at(&registry).args(["setup-registry", "sources"]),
    ));
    let listed: Vec<(String, String)> = sources["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_string(),
                p["path"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        listed.contains(&("source".to_string(), moved_to.display().to_string())),
        "{sources}"
    );
    assert!(
        listed.iter().any(|(name, _)| name == "scanner2"),
        "a source added at the desk is read back: {sources}"
    );

    // a registry at another schema is left as it is, and where it stands said
    let db = rusqlite::Connection::open(registry.join("registry.db")).unwrap();
    let version: String = db
        .query_row(
            "SELECT value FROM registry_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    db.execute(
        "UPDATE registry_meta SET value = '1' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    let sources = json(&output(
        nils_at(&registry).args(["setup-registry", "sources"]),
    ));
    assert_eq!(sources["registry"], true, "{sources}");
    assert_eq!(sources["ahead"], false, "{sources}");
    assert!(
        sources["schema"]
            .as_str()
            .is_some_and(|s| s.contains("schema version 1, behind")),
        "{sources}"
    );
    let still: String = db
        .query_row(
            "SELECT value FROM registry_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still, "1", "reading it did not migrate it");
    assert_ne!(version, "1");

    // no registry holds no places; one that does not answer fails, with why
    let none = json(&output(
        nils_at(&base.path().join("nothing")).args(["setup-registry", "sources"]),
    ));
    assert_eq!(none, serde_json::json!({ "registry": false }));
    let broken = base.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("nils.toml"), "backend = 3\n").unwrap();
    let refused = output(nils_at(&broken).args(["setup-registry", "sources"]));
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).starts_with("nils: nils.toml"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let unanswered = with_input(
        Command::new(nils.path()).args(["setup-registry", "connect", "--schema", "nils"]),
        "postgres://nils@127.0.0.1:1/nils\n",
    );
    assert!(!unanswered.status.success());
    assert!(
        String::from_utf8_lossy(&unanswered.stderr).starts_with("nils: "),
        "{}",
        String::from_utf8_lossy(&unanswered.stderr)
    );
}

/// On Postgres the step that makes a registry is given the connection string
/// on its input alone, and writes it into `nils.toml` as it was given, with
/// the settings setup makes every registry with. Runs where a test DSN is set.
#[test]
fn the_step_that_makes_a_registry_on_postgres_reads_the_connection_string_from_its_input() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres test is skipped");
        return;
    };
    let schema = "nils_setup_step_input";
    let drop = || {
        nils_registry::Store::connect_postgres(&dsn, schema)
            .expect("connect")
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    let nils = Installed::new("nils-setup-registry-step-postgres");
    let base = TempDir::new("nils-setup-registry-step-postgres");
    let registry = base.path().join("registry");
    let nils_at = || {
        let mut command = Command::new(nils.path());
        command.arg("--registry").arg(&registry);
        command
    };
    let added = with_input(
        nils_at().args(["key", "add", "nils"]),
        "a fixture passphrase\n",
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let words = [
        "setup-registry",
        "init",
        "--backend",
        "postgres",
        "--schema",
        schema,
    ];
    assert!(
        words.iter().all(|word| !word.contains(dsn.as_str())),
        "a command line is every account's to read"
    );
    let made = with_input(nils_at().args(words), &format!("{dsn}\n"));
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(registry.join("nils.toml")).unwrap()).unwrap();
    assert_eq!(config["backend"].as_str(), Some("postgres"));
    assert_eq!(
        config["dsn"].as_str(),
        Some(dsn.as_str()),
        "the connection string as it was given, without the line end after it"
    );
    assert_eq!(config["schema"].as_str(), Some(schema));
    let mut store = nils_registry::Store::connect_postgres(&dsn, schema).expect("connect");
    let meta = store
        .query(
            &format!(
                "SELECT key, value FROM {} \
                 WHERE key IN ('pseudonym_scheme', 'display_length', 'pseudonym_key') ORDER BY key",
                store.qualified("registry_meta")
            ),
            &[],
        )
        .expect("the registry's settings");
    let settings: Vec<(String, String)> = meta
        .iter()
        .map(|row| {
            (
                row.text(0).unwrap_or_default().to_string(),
                row.text(1).unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        settings,
        [
            ("display_length".to_string(), "12".to_string()),
            ("pseudonym_key".to_string(), "nils".to_string()),
            ("pseudonym_scheme".to_string(), "blake2b-32".to_string()),
        ]
    );
    drop();
}

/// The step a purge drops the registry's schemas through takes the schema it
/// is given and the linkage store beside it, and leaves every other schema in
/// the database alone. Runs where a test DSN is set.
#[test]
fn the_step_that_drops_a_registrys_schemas_takes_the_one_it_is_given_and_no_other() {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres test is skipped");
        return;
    };
    let mine = "nils_setup_drop_mine";
    let theirs = "nils_setup_drop_theirs";
    let standing = |schema: &str| -> Vec<String> {
        let mut store = nils_registry::Store::connect_postgres(&dsn, "public").expect("connect");
        store
            .query(
                &format!(
                    "SELECT schema_name FROM information_schema.schemata \
                     WHERE schema_name IN ('{schema}', '{schema}_linkage') ORDER BY schema_name"
                ),
                &[],
            )
            .expect("the schemas there")
            .iter()
            .map(|row| row.text(0).unwrap_or_default().to_string())
            .collect()
    };
    let clear = || {
        let mut store = nils_registry::Store::connect_postgres(&dsn, "public").expect("connect");
        for schema in [mine, theirs] {
            store
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {schema} CASCADE; \
                     DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
                ))
                .expect("drop");
        }
    };
    clear();
    let nils = Installed::new("nils-setup-drop-postgres");
    let base = TempDir::new("nils-setup-drop-postgres");
    // two registries in one database: the one this install made, and one the
    // site already had
    for (schema, name) in [(mine, "mine"), (theirs, "theirs")] {
        let registry = base.path().join(name);
        let at = || {
            let mut command = Command::new(nils.path());
            command.arg("--registry").arg(&registry);
            command
        };
        let added = with_input(at().args(["key", "add", "nils"]), "a fixture passphrase\n");
        assert!(
            added.status.success(),
            "{}",
            String::from_utf8_lossy(&added.stderr)
        );
        let made = with_input(
            at().args([
                "setup-registry",
                "init",
                "--backend",
                "postgres",
                "--schema",
                schema,
            ]),
            &format!("{dsn}\n"),
        );
        assert!(
            made.status.success(),
            "{}",
            String::from_utf8_lossy(&made.stderr)
        );
    }
    assert_eq!(
        standing(mine).len(),
        2,
        "the registry and its linkage store"
    );
    assert_eq!(standing(theirs).len(), 2);

    let dropped = with_input(
        Command::new(nils.path()).args(["setup-registry", "drop", "--schema", mine]),
        &format!("{dsn}\n"),
    );
    assert!(
        dropped.status.success(),
        "{}",
        String::from_utf8_lossy(&dropped.stderr)
    );
    assert!(
        String::from_utf8_lossy(&dropped.stdout).contains(&format!("{mine}_linkage")),
        "it says what it dropped: {}",
        String::from_utf8_lossy(&dropped.stdout)
    );
    assert!(
        standing(mine).is_empty(),
        "both schemas of this install are gone"
    );
    assert_eq!(
        standing(theirs).len(),
        2,
        "a schema the record does not name is left as it is"
    );
    // dropping again is not a failure, so the rest of a purge goes on
    let again = with_input(
        Command::new(nils.path()).args(["setup-registry", "drop", "--schema", mine]),
        &format!("{dsn}\n"),
    );
    assert!(again.status.success());
    clear();
}

#[test]
fn update_all_without_a_setup_says_where_it_looked() {
    let nils = Installed::new("nils-setup-update-all");
    let config = TempDir::new("nils-setup-update-all-config");
    let out = output(
        Command::new(nils.path())
            .args(["update", "--all"])
            .env("XDG_CONFIG_HOME", config.path())
            .env_remove("NILS_RELEASES"),
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "it claimed to update nothing");
    assert!(stderr.contains("no setup is recorded"), "{stderr}");
    assert!(stderr.contains("setup.toml"), "{stderr}");
}

#[test]
fn a_setup_that_is_there_is_named_and_update_needs_one() {
    let nils = Installed::new("nils-setup-again");
    let config = TempDir::new("nils-setup-again-config");
    let base = TempDir::new("nils-setup-again-base");
    let dir = base.path().join("nils");

    // Nothing recorded yet, so there is nothing to update.
    let empty = setup(&nils.path(), config.path(), &["--update"]);
    assert!(!empty.ok, "it updated a setup that is not there");
    assert!(
        empty.stderr.contains("--update needs a setup"),
        "{}",
        empty.stderr
    );

    let made = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "engine",
            "--dir",
            dir.to_str().unwrap(),
            "--no-service",
        ],
    );
    assert!(made.ok, "{}", made.stderr);

    // Now it opens with what is installed, where, in what mode and how it runs.
    let again = setup(&nils.path(), config.path(), &["--print"]);
    assert!(again.ok, "{}", again.stderr);
    // The version this binary carries, not a literal, so that raising it
    // for a release is one line in one file.
    again.says(&format!(
        "engine {} in {}, off mode, on the machine",
        env!("CARGO_PKG_VERSION"),
        dir.display()
    ));
    again.says("started by hand");

    // --update takes the base directory from the state without being told.
    let update = setup(&nils.path(), config.path(), &["--update", "--print"]);
    assert!(update.ok, "{}", update.stderr);
    update.says("Updating");
    update.says(&dir.display().to_string());
    update.says("nothing was changed");
}

/// `nils update --all` takes the newest `nils` first and starts it to update
/// the parts, so they move to the versions the newest release pins. The
/// release's engine here is a script that writes down how it was started.
#[cfg(unix)]
#[test]
fn update_all_takes_the_newest_nils_first_and_hands_it_the_parts() {
    let nils = Installed::new("nils-update-hands-over");
    let config = TempDir::new("nils-update-hands-over-config");
    let base = TempDir::new("nils-update-hands-over-base");
    let dir = base.path().join("nils");
    let made = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "engine",
            "--dir",
            dir.to_str().unwrap(),
            "--no-service",
        ],
    );
    assert!(made.ok, "{}", made.stderr);

    let releases = Releases::new("99.0.0");
    let started = base.path().join("started");
    let name = format!("nils-{}", target());
    let into = releases.dir.path().join("download").join("v99.0.0");
    let body = format!(
        "#!/bin/sh\nprintf '%s|%s\\n' \"$NILS_UPDATE_HANDED_OVER\" \"$*\" > '{}'\n",
        started.display()
    );
    std::fs::write(into.join(&name), &body).unwrap();
    let sums: String = std::fs::read_to_string(into.join("SHA256SUMS"))
        .unwrap()
        .lines()
        .map(|line| {
            if line.ends_with(&format!("  {name}")) {
                format!("{}  {name}\n", sha256_hex(body.as_bytes()))
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    std::fs::write(into.join("SHA256SUMS"), sums).unwrap();

    let to = base.path().join("bin");
    let registry = dir.join("registry");
    let out = output(
        Command::new(nils.path())
            .args(["--registry", registry.to_str().unwrap()])
            .args(["update", "--all", "--channel", &releases.url()])
            .args(["--to", to.to_str().unwrap()])
            .env("XDG_CONFIG_HOME", config.path())
            .env("XDG_DATA_HOME", base.path().join("data"))
            .env_remove("NILS_UPDATE_HANDED_OVER"),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("nils 99.0.0 at"), "{stdout}");
    let how = std::fs::read_to_string(&started)
        .unwrap_or_else(|e| panic!("the new binary was not started ({e}):\n{stdout}\n{stderr}"));
    assert!(
        how.starts_with(&format!("{}|", env!("CARGO_PKG_VERSION"))),
        "it was not told what it replaced: {how}"
    );
    assert!(
        how.trim_end().ends_with(&format!(
            "update --all --channel {} --to {}",
            releases.url(),
            to.display()
        )),
        "it was not started with the update's own arguments: {how}"
    );
}

/// A `nils` an update started carries the update on and does not install
/// itself again; with no setup recorded there are no parts, and it says so.
#[test]
fn a_nils_an_update_started_updates_the_parts_and_not_itself() {
    let nils = Installed::new("nils-update-carried-on");
    let config = TempDir::new("nils-update-carried-on-config");
    let base = TempDir::new("nils-update-carried-on-base");
    let releases = Releases::new("99.0.0");
    let to = base.path().join("bin");
    let out = output(
        Command::new(nils.path())
            .args(["update", "--all", "--channel", &releases.url()])
            .args(["--to", to.to_str().unwrap()])
            .env("XDG_CONFIG_HOME", config.path())
            .env("XDG_DATA_HOME", base.path().join("data"))
            .env("NILS_UPDATE_HANDED_OVER", "1.0.0-alpha.1"),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "{stdout}");
    assert!(
        stdout.contains("carries on the update from 1.0.0-alpha.1"),
        "{stdout}\n{stderr}"
    );
    assert!(stderr.contains("no setup is recorded"), "{stderr}");
    assert!(
        !to.join("nils").exists(),
        "it installed itself again:\n{stdout}"
    );
}

/// A site names the places it has, with the roles and the guarantees they
/// really have and the directories the engine reads among them, and gets
/// exactly those. The archives, the rule packs and the request handlers are
/// the site's to set, and they reach the unit. A second run declares nothing
/// twice, and an update, which is made from the record alone, reverts none
/// of it.
#[test]
fn a_site_declares_the_places_it_has_and_an_update_reverts_none_of_them() {
    // a release older than this binary, so that the packs come from it and
    // the wizard leaves alone the binary it is run from, which the runs
    // after the first one are run from too
    let releases = Releases::new("1.0.0-alpha.1");
    let nils = Installed::new("nils-setup-site");
    let config = TempDir::new("nils-setup-site-config");
    let base = TempDir::new("nils-setup-site-base");
    let dir = base.path().join("nils");
    let at = |name: &str| base.path().join(name).display().to_string();
    let (archives, dicom, results, shared, packs) = (
        at("archives"),
        at("dicom"),
        at("results"),
        at("shared"),
        at("packs"),
    );
    let registry = dir.join("registry").display().to_string();
    // five places on directories of the site's own, of five roles, two of
    // them read by the engine although neither is a dataset
    let declared = [
        format!("archives={archives},role=backup,snapshots,protected"),
        format!("registry={registry},role=registry,backup=archives,protected,fast"),
        format!("source={dicom},role=source,snapshots,protected"),
        format!("results={results},role=working,fast,read"),
        format!("shared={shared},role=share,protected,read"),
    ];
    let (url, dir_s) = (releases.url(), dir.display().to_string());
    let run = |more: &[&str]| {
        let mut args = vec![
            "--yes",
            "--parts",
            "engine",
            "--dir",
            &dir_s,
            "--channel",
            &url,
            "--pack-dir",
            &packs,
            "--workers",
            "12",
        ];
        for place in &declared {
            args.push("--place");
            args.push(place);
        }
        args.extend_from_slice(more);
        setup(&nils.path(), config.path(), &args)
    };
    let o = run(&["--no-service"]);
    assert!(o.ok, "{}\n{}", o.stdout, o.stderr);

    // asked of this build's own binary, since the one the wizard installed
    // beside itself is the release's
    let listed = || {
        let out = output(
            Command::new(env!("CARGO_BIN_EXE_nils"))
                .arg("--registry")
                .arg(&registry)
                .args(["place", "list", "--json"]),
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let doc: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("the places did not list: {e}\n{text}"));
        doc.as_array().cloned().unwrap_or_default()
    };
    let places = listed();
    assert_eq!(places.len(), 5, "{places:#?}");
    let one = |places: &[serde_json::Value], name: &str| {
        places
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("no {name} place in {places:#?}"))
            .clone()
    };
    let works = one(&places, "results");
    assert_eq!(works["role"], "working", "{works:#?}");
    assert_eq!(works["guarantees"]["fast"], true, "{works:#?}");
    assert_eq!(works["guarantees"]["protected"], false, "{works:#?}");
    let held = one(&places, "registry");
    assert_eq!(held["guarantees"]["backup"], "archives", "{held:#?}");
    assert_eq!(held["guarantees"]["protected"], true, "{held:#?}");
    assert_eq!(one(&places, "shared")["role"], "share");
    assert_eq!(one(&places, "archives")["role"], "backup");
    assert!(
        Path::new(&packs).join("mri").join("pack.toml").is_file(),
        "the packs went where the site said:\n{}",
        o.stdout
    );
    let state = state_of(config.path());
    assert!(state.contains("[[site.places]]"), "{state}");
    assert!(state.contains("workers = 12"), "{state}");
    assert!(state.contains("role = \"share\""), "{state}");

    // a second run declares the same places, and declares nothing twice
    let again = run(&["--no-service"]);
    assert!(again.ok, "{}\n{}", again.stdout, again.stderr);
    let after = listed();
    assert_eq!(after.len(), 5, "a rerun declared them again: {after:#?}");
    assert_eq!(one(&after, "results")["role"], "working");
    assert_eq!(one(&after, "results")["id"], works["id"]);

    // an update is made from the record alone, and reverts nothing
    let update = setup(
        &nils.path(),
        config.path(),
        &["--update", "--yes", "--channel", &url],
    );
    assert!(update.ok, "{}\n{}", update.stdout, update.stderr);
    let kept = listed();
    assert_eq!(kept.len(), 5, "{kept:#?}");
    assert_eq!(one(&kept, "shared")["role"], "share");
    let recorded = state_of(config.path());
    assert!(recorded.contains("[[site.places]]"), "{recorded}");
    assert!(recorded.contains("workers = 12"), "{recorded}");

    // and the settings reach the unit the install would write
    let unit = setup(
        &nils.path(),
        config.path(),
        &[
            "--yes",
            "--parts",
            "engine",
            "--dir",
            &dir_s,
            "--service",
            "--print",
            "--channel",
            &url,
        ],
    );
    assert!(unit.ok, "{}\n{}", unit.stdout, unit.stderr);
    unit.says(&format!("--backup-dir {archives}"));
    unit.says(&format!("--pack-dir {packs}"));
    unit.says("--workers 12");
    unit.says(&format!("--ingest-root source={dicom}"));
    unit.says(&format!("--ingest-root results={results}"));
    unit.says(&format!("--ingest-root shared={shared}"));
    assert!(
        !unit.stdout.contains("--ingest-root archives="),
        "a place nobody marked is not a directory the engine reads:\n{}",
        unit.stdout
    );
}

/// Places that cannot be the places of this install are refused with the fix
/// in the same sentence, and nothing is written: no registry, no record.
#[test]
fn places_that_cannot_be_declared_stop_the_install_before_it_writes_anything() {
    let nils = Installed::new("nils-setup-site-refused");
    let config = TempDir::new("nils-setup-site-refused-config");
    let base = TempDir::new("nils-setup-site-refused-base");
    let dir = base.path().join("nils");
    let dir_s = dir.display().to_string();
    let registry = format!(
        "registry={},role=registry,backup=archives",
        dir.join("registry").display()
    );
    let archives = format!(
        "archives={},role=backup",
        base.path().join("archives").display()
    );
    let refused = |more: &[&str]| {
        let mut args = vec![
            "--yes",
            "--parts",
            "engine",
            "--dir",
            &dir_s,
            "--no-service",
        ];
        args.extend_from_slice(more);
        let o = setup(&nils.path(), config.path(), &args);
        assert!(!o.ok, "it went ahead:\n{}", o.stdout);
        assert!(!dir.exists(), "it made {} anyway", dir.display());
        assert!(
            !config.path().join("nils").join("setup.toml").exists(),
            "it left a record of an install it did not make"
        );
        format!("{}{}", o.stdout, o.stderr)
    };
    assert!(refused(&["--place", "work=/work,role=vault"]).contains("the roles are"));
    assert!(
        refused(&["--place", "work=/work,role=working,quick"]).contains("says nothing about it")
    );
    assert!(refused(&["--place", "work=/work,role=working"]).contains("keep no registry"));
    assert!(refused(&["--place", &registry]).contains("no backup place is named archives"));
    assert!(
        refused(&[
            "--place",
            &registry,
            "--place",
            &archives,
            "--source",
            "/data/source"
        ])
        .contains("--place NAME=DIR,role=source"),
        "a directory of DICOM is one of the places, not a flag beside them"
    );
    assert!(refused(&["--workers", "0"]).contains("one or more"));
}
