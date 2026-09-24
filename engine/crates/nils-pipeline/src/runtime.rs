// SPDX-License-Identifier: AGPL-3.0-only

//! The container runtimes (record 43 R2, D18), behind one trait.
//!
//! The order is rootless podman, then apptainer, then docker only where an
//! operator opted in (`nils pipeline runtime --set docker`), since docker's
//! daemon is root on the host. None found is not an error: the capability is
//! off and says why (D1). Every run carries the same guarantees whatever the
//! runtime: no network, the input and every typed input read-only, one
//! output folder, and a process that is not root on the host (podman's
//! `--userns keep-id`, apptainer's own user, docker's `--user`). A GPU is
//! passed where the pipeline asks and the host offers one: CDI for podman,
//! `--nv` for apptainer, `--gpus` for docker.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The runtimes, in the order they are looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Podman,
    Apptainer,
    Docker,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Podman, Kind::Apptainer, Kind::Docker];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Podman => "podman",
            Kind::Apptainer => "apptainer",
            Kind::Docker => "docker",
        }
    }
}

/// What an operator chose: find one (`auto`, podman then apptainer), only
/// this one (the one way docker is taken), or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Auto,
    Only(Kind),
    Off,
}

impl Choice {
    pub const WORDS: [&'static str; 5] = ["auto", "podman", "apptainer", "docker", "off"];

    pub fn parse(text: &str) -> Option<Choice> {
        Some(match text.trim() {
            "" | "auto" => Choice::Auto,
            "podman" => Choice::Only(Kind::Podman),
            "apptainer" => Choice::Only(Kind::Apptainer),
            "docker" => Choice::Only(Kind::Docker),
            "off" => Choice::Off,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Choice::Auto => "auto",
            Choice::Only(k) => k.name(),
            Choice::Off => "off",
        }
    }
}

/// One folder a container sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub host: PathBuf,
    pub container: String,
    pub read_only: bool,
}

/// One run of an image.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The container's name, so a cancel can stop it: `nils-run-<id>`.
    pub name: String,
    /// `repository@sha256:<hex>`.
    pub image: String,
    pub argv: Vec<String>,
    pub mounts: Vec<Mount>,
    /// Pass the host's GPU.
    pub gpu: bool,
    pub env: Vec<(String, String)>,
    /// The uid and gid a runtime that must be told runs the process as
    /// (docker); podman and apptainer run it as the user by construction.
    pub user: Option<(u32, u32)>,
}

/// A container runtime, as the runner uses one.
pub trait Runtime {
    /// `podman`, `apptainer`, `docker`, or a test's own name.
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    /// The GPU this runtime can hand a container here, by name, or none.
    fn gpu(&self) -> Option<&str>;
    /// The process that runs an invocation to its end.
    fn command(&self, inv: &Invocation) -> Command;
    /// Stop a running invocation, for a cancel; best effort.
    fn stop(&self, _inv: &Invocation) {}
}

/// A runtime reached by its command line.
#[derive(Debug, Clone)]
pub struct Cli {
    pub kind: Kind,
    pub program: PathBuf,
    pub version: String,
    pub gpu: Option<String>,
}

impl Runtime for Cli {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn gpu(&self) -> Option<&str> {
        self.gpu.as_deref()
    }

    fn command(&self, inv: &Invocation) -> Command {
        let mut c = Command::new(&self.program);
        c.args(argv(self.kind, inv));
        c
    }

    fn stop(&self, inv: &Invocation) {
        if matches!(self.kind, Kind::Podman | Kind::Docker) {
            let _ = Command::new(&self.program)
                .args(["kill", &inv.name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// The words after the runtime's program for one invocation.
pub fn argv(kind: Kind, inv: &Invocation) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mount = |m: &Mount| {
        format!(
            "{}:{}{}",
            m.host.display(),
            m.container,
            if m.read_only { ":ro" } else { "" }
        )
    };
    match kind {
        Kind::Podman | Kind::Docker => {
            a.extend(["run", "--rm", "--name", &inv.name].map(String::from));
            a.extend(["--network", "none"].map(String::from));
            if kind == Kind::Podman {
                a.extend(["--pull", "missing", "--userns", "keep-id"].map(String::from));
            } else if let Some((uid, gid)) = inv.user {
                a.extend(["--user".to_string(), format!("{uid}:{gid}")]);
            }
            a.extend(
                ["--cap-drop", "all", "--security-opt", "no-new-privileges"].map(String::from),
            );
            if inv.gpu {
                if kind == Kind::Podman {
                    a.extend(["--device", "nvidia.com/gpu=all"].map(String::from));
                } else {
                    a.extend(["--gpus", "all"].map(String::from));
                }
            }
            for m in &inv.mounts {
                a.push("--volume".into());
                a.push(mount(m));
            }
            for (k, v) in &inv.env {
                a.push("--env".into());
                a.push(format!("{k}={v}"));
            }
            a.push(inv.image.clone());
        }
        Kind::Apptainer => {
            a.extend(
                [
                    "exec",
                    "--containall",
                    "--cleanenv",
                    "--no-home",
                    "--net",
                    "--network",
                    "none",
                ]
                .map(String::from),
            );
            if inv.gpu {
                a.push("--nv".into());
            }
            for m in &inv.mounts {
                a.push("--bind".into());
                a.push(mount(m));
            }
            for (k, v) in &inv.env {
                a.push("--env".into());
                a.push(format!("{k}={v}"));
            }
            a.push(format!("docker://{}", inv.image));
        }
    }
    a.extend(inv.argv.iter().cloned());
    a
}

/// What the look for a runtime found.
#[derive(Debug, Clone)]
pub struct Detected {
    pub choice: Choice,
    pub runtime: Option<Cli>,
    /// Why there is none, in a sentence that names the cure.
    pub reason: Option<String>,
    /// What was looked at, in order, and what each said.
    pub looked: Vec<(String, String)>,
}

/// A program on a search path, as a shell would find it.
pub fn which(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let path = path?;
    std::env::split_paths(path)
        .map(|d| d.join(program))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// A program's answer, its first line, within a few seconds.
fn answer(program: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > Duration::from_secs(10) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
}

/// The version word of `<program> --version`: `podman version 5.0.0`,
/// `apptainer version 1.3.0`, `Docker version 29.8.0, build x`.
fn version_of(line: &str) -> String {
    line.split_whitespace()
        .skip_while(|w| !w.eq_ignore_ascii_case("version"))
        .nth(1)
        .unwrap_or(line)
        .trim_end_matches(',')
        .to_string()
}

/// The name of the host's first NVIDIA GPU, when `nvidia-smi` answers.
fn nvidia(path: Option<&OsStr>) -> Option<String> {
    let smi = which("nvidia-smi", path)?;
    answer(&smi, &["--query-gpu=name", "--format=csv,noheader"])
        .filter(|n| !n.is_empty())
        .map(|n| format!("cuda:{n}"))
}

/// Whether a CDI specification here names NVIDIA's GPUs, which is what
/// podman's `--device nvidia.com/gpu=all` needs.
fn cdi_names_nvidia() -> bool {
    ["/etc/cdi", "/var/run/cdi"].iter().any(|dir| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.filter_map(Result::ok).any(|e| {
                std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("nvidia.com/gpu"))
            })
        })
    })
}

/// Look for a runtime on a search path, as the operator's choice says.
pub fn detect(choice: Choice, path: Option<&OsStr>) -> Detected {
    let mut looked: Vec<(String, String)> = Vec::new();
    let none = |choice, looked, reason: String| Detected {
        choice,
        runtime: None,
        reason: Some(reason),
        looked,
    };
    let order: Vec<Kind> = match choice {
        Choice::Off => {
            return none(
                choice,
                looked,
                "pipelines are off: an operator turned the runtime off (nils pipeline runtime --set auto turns it on)".into(),
            );
        }
        Choice::Auto => vec![Kind::Podman, Kind::Apptainer],
        Choice::Only(k) => vec![k],
    };
    for kind in order {
        let Some(program) = which(kind.name(), path) else {
            looked.push((kind.name().into(), "not installed".into()));
            continue;
        };
        let Some(line) = answer(&program, &["--version"]) else {
            looked.push((kind.name().into(), "does not answer --version".into()));
            continue;
        };
        let version = version_of(&line);
        let gpu = match kind {
            Kind::Podman => {
                let rootless = answer(
                    &program,
                    &["info", "--format", "{{.Host.Security.Rootless}}"],
                );
                if rootless.as_deref() != Some("true") {
                    looked.push((
                        kind.name().into(),
                        format!("{version}, not rootless here (D18 runs pipelines rootless)"),
                    ));
                    continue;
                }
                if cdi_names_nvidia() {
                    nvidia(path)
                } else {
                    None
                }
            }
            Kind::Apptainer | Kind::Docker => nvidia(path),
        };
        looked.push((kind.name().into(), format!("{version}, taken")));
        return Detected {
            choice,
            runtime: Some(Cli {
                kind,
                program,
                version,
                gpu,
            }),
            reason: None,
            looked,
        };
    }
    let reason = match choice {
        Choice::Only(Kind::Docker) => {
            "pipelines are off: docker was chosen and does not answer here".to_string()
        }
        Choice::Only(k) => format!(
            "pipelines are off: {} was chosen and is not usable here ({})",
            k.name(),
            looked.last().map(|(_, s)| s.as_str()).unwrap_or("not found")
        ),
        _ => "pipelines are off: no container runtime here; rootless podman or apptainer runs them, and docker only where an operator opts in with nils pipeline runtime --set docker (D18)".to_string(),
    };
    none(choice, looked, reason)
}

/// How an invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended {
    /// The exit code, none when a signal ended it.
    pub code: Option<i32>,
    /// Whether it was stopped because `tick` asked.
    pub stopped: bool,
}

/// Run an invocation to its end, its output and errors into `log`, calling
/// `tick` about once a second; `tick` answering false stops it.
pub fn run(
    rt: &dyn Runtime,
    inv: &Invocation,
    log: &Path,
    tick: &mut dyn FnMut() -> bool,
) -> std::io::Result<Ended> {
    let out = std::fs::File::create(log)?;
    let err = out.try_clone()?;
    let mut child = rt
        .command(inv)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()?;
    let mut ticked = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Ended {
                code: status.code(),
                stopped: false,
            });
        }
        if ticked.elapsed() >= Duration::from_secs(1) {
            ticked = Instant::now();
            if !tick() {
                rt.stop(inv);
                let _ = child.kill();
                let status = child.wait()?;
                return Ok(Ended {
                    code: status.code(),
                    stopped: true,
                });
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A runtime for tests: runs a host program in place of a container, with
/// every container path in the arguments written as its host folder, and
/// `NILS_INPUT`, `NILS_OUTPUT` and `NILS_INPUTS` naming the host folders of
/// `/input`, `/output` and `/inputs`. Nothing is isolated; it proves what the
/// runner does around a container, never what a container does.
#[derive(Debug, Clone)]
pub struct Script {
    pub program: PathBuf,
    pub gpu: Option<String>,
}

impl Runtime for Script {
    fn name(&self) -> &str {
        "script"
    }

    fn version(&self) -> &str {
        "test"
    }

    fn gpu(&self) -> Option<&str> {
        self.gpu.as_deref()
    }

    fn command(&self, inv: &Invocation) -> Command {
        let mut mounts: Vec<&Mount> = inv.mounts.iter().collect();
        mounts.sort_by_key(|m| std::cmp::Reverse(m.container.len()));
        let host = |word: &str| -> String {
            let mut w = word.to_string();
            for m in &mounts {
                w = w.replace(&m.container, &m.host.display().to_string());
            }
            w
        };
        let mut c = Command::new("sh");
        c.arg(&self.program);
        c.args(inv.argv.iter().map(|w| host(w)));
        for (var, at) in [
            ("NILS_INPUT", "/input"),
            ("NILS_OUTPUT", "/output"),
            ("NILS_INPUTS", "/inputs"),
        ] {
            if let Some(m) = inv.mounts.iter().find(|m| m.container == at) {
                c.env(var, &m.host);
            }
        }
        for (k, v) in &inv.env {
            c.env(k, v);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(gpu: bool) -> Invocation {
        Invocation {
            name: "nils-run-7".into(),
            image: format!("busybox@sha256:{}", "a".repeat(64)),
            argv: vec!["sh".into(), "-c".into(), "ls /input > /output/x".into()],
            mounts: vec![
                Mount {
                    host: "/w/runs/7/input".into(),
                    container: "/input".into(),
                    read_only: true,
                },
                Mount {
                    host: "/w/derivatives/p/7".into(),
                    container: "/output".into(),
                    read_only: false,
                },
            ],
            gpu,
            env: vec![("NILS_RUN".into(), "7".into())],
            user: Some((1000, 1000)),
        }
    }

    fn has(a: &[String], pair: &[&str]) -> bool {
        a.windows(pair.len())
            .any(|w| w.iter().zip(pair).all(|(x, y)| x == y))
    }

    #[test]
    fn every_runtime_isolates_the_network_and_mounts_the_input_read_only() {
        let p = argv(Kind::Podman, &inv(false));
        for pair in [
            &["--network", "none"][..],
            &["--userns", "keep-id"],
            &["--volume", "/w/runs/7/input:/input:ro"],
            &["--volume", "/w/derivatives/p/7:/output"],
            &["--cap-drop", "all"],
        ] {
            assert!(has(&p, pair), "{pair:?} in {p:?}");
        }
        assert!(!p.iter().any(|w| w.contains("nvidia")), "{p:?}");
        let image_at = p.iter().position(|w| w.contains("@sha256:")).unwrap();
        assert_eq!(&p[image_at + 1..], ["sh", "-c", "ls /input > /output/x"]);
        assert!(has(
            &argv(Kind::Podman, &inv(true)),
            &["--device", "nvidia.com/gpu=all"]
        ));

        let d = argv(Kind::Docker, &inv(true));
        assert!(has(&d, &["--network", "none"]) && has(&d, &["--user", "1000:1000"]));
        assert!(has(&d, &["--gpus", "all"]) && !d.iter().any(|w| w == "keep-id"));

        let a = argv(Kind::Apptainer, &inv(true));
        assert_eq!(a[0], "exec");
        assert!(has(&a, &["--network", "none"]) && a.iter().any(|w| w == "--nv"));
        assert!(has(&a, &["--bind", "/w/runs/7/input:/input:ro"]));
        assert!(a.iter().any(|w| w.starts_with("docker://busybox@sha256:")));
    }

    #[test]
    fn choices_and_versions_read_as_written() {
        assert_eq!(Choice::parse("docker"), Some(Choice::Only(Kind::Docker)));
        assert_eq!(Choice::parse(""), Some(Choice::Auto));
        assert_eq!(Choice::parse("lxc"), None);
        assert_eq!(version_of("podman version 5.0.3"), "5.0.3");
        assert_eq!(version_of("Docker version 29.8.0, build abc"), "29.8.0");
        assert_eq!(version_of("apptainer version 1.3.4-1"), "1.3.4-1");
    }

    #[test]
    fn with_nothing_on_the_path_the_capability_is_off_and_says_why() {
        let empty = std::env::temp_dir().join(format!("nils-empty-path-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let d = detect(Choice::Auto, Some(empty.as_os_str()));
        assert!(d.runtime.is_none());
        assert!(d.reason.unwrap().contains("rootless podman or apptainer"));
        assert_eq!(d.looked.len(), 2);
        // docker is never found unless chosen
        let fake = empty.join("docker");
        std::fs::write(&fake, "#!/bin/sh\necho 'Docker version 1.0, build x'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(
                detect(Choice::Auto, Some(empty.as_os_str()))
                    .runtime
                    .is_none()
            );
            let chosen = detect(Choice::Only(Kind::Docker), Some(empty.as_os_str()));
            assert_eq!(chosen.runtime.unwrap().version, "1.0");
        }
        assert!(
            detect(Choice::Off, None)
                .reason
                .unwrap()
                .contains("turned the runtime off")
        );
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn a_script_runtime_runs_to_its_end_and_a_tick_stops_it() {
        let dir = std::env::temp_dir().join(format!("nils-script-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let script = dir.join("run.sh");
        std::fs::write(
            &script,
            "echo \"$@\" > \"$NILS_OUTPUT/args\"\n[ \"$1\" = slow ] && sleep 30\nexit 3\n",
        )
        .unwrap();
        let rt = Script {
            program: script,
            gpu: None,
        };
        let mut i = inv(false);
        i.mounts[1].host = dir.join("out");
        i.argv = vec!["fast".into(), "/output/y".into()];
        let ended = run(&rt, &i, &dir.join("log"), &mut || true).unwrap();
        assert_eq!(
            ended,
            Ended {
                code: Some(3),
                stopped: false
            }
        );
        let args = std::fs::read_to_string(dir.join("out/args")).unwrap();
        assert_eq!(args.trim(), format!("fast {}/y", dir.join("out").display()));
        i.argv = vec!["slow".into()];
        let started = Instant::now();
        let ended = run(&rt, &i, &dir.join("log"), &mut || false).unwrap();
        assert!(ended.stopped);
        assert!(started.elapsed() < Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
