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
        o.says("the gateway marks that backend remote");
    }
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
