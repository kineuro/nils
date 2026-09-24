// SPDX-License-Identifier: AGPL-3.0-only

//! The container runtimes (record 43 R2, D18), behind one trait.
//!
//! The order is rootless podman, then apptainer, then docker only where an
//! operator opted in (`nils pipeline runtime --set docker`), since docker's
//! daemon is root on the host. None found is not an error: the capability is
//! off and says why (D1). Every run carries the same guarantees whatever the
//! runtime: no network, the input and every typed input read-only, one
//! output folder, and a process that is not root on the host (podman's
//! `--userns keep-id` with `--user`, apptainer's own user, docker's
//! `--user`). A GPU is
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
    /// The uid and gid the process runs as: the engine's user (for podman
    /// the engine process's own uid and gid, the ids keep-id maps). Docker
    /// and podman are told with `--user`; podman's `--userns keep-id` maps
    /// the user to the same ids inside, and `--user` makes the process that
    /// user even where the image names a `USER` of its own (wave 43's
    /// proof: `USER 65534` won over keep-id, the outputs belonged to a
    /// sub-uid, and the next run could not hard-link them). Apptainer runs
    /// the user by construction.
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
    /// Where this runtime keeps its images here, for an error that has to
    /// say where an image was looked for; none where it cannot say.
    fn store(&self) -> Option<String> {
        None
    }
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

    fn store(&self) -> Option<String> {
        let format = match self.kind {
            Kind::Podman => "{{.Store.GraphRoot}}",
            Kind::Docker => "{{.DockerRootDir}}",
            Kind::Apptainer => return None,
        };
        match answer(&self.program, &["info", "--format", format], PROBE_CAP) {
            Said::Line(l) if !l.is_empty() => Some(l),
            _ => None,
        }
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
            }
            if let Some((uid, gid)) = inv.user {
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
    /// Whether a runtime did not answer in time, so none is found for now
    /// and a later look may find one: not a finding to keep.
    pub unknown: bool,
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

/// How long a runtime is given to answer a question about itself. Wave 43's
/// proof: `podman info` took 7 to 8 s on a busy host, and a cap of 10 s
/// turned pipelines off two probes in three.
pub const PROBE_CAP: Duration = Duration::from_secs(30);

/// What a program said.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Said {
    /// Its first line.
    Line(String),
    /// It failed, or could not be started.
    Failed,
    /// It did not answer within the cap: nothing is known.
    TimedOut,
}

/// A program's answer, its first line, within `cap`.
fn answer(program: &Path, args: &[&str], cap: Duration) -> Said {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Said::Failed;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > cap => {
                let _ = child.kill();
                let _ = child.wait();
                return Said::TimedOut;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return Said::Failed,
        }
    }
    let Ok(out) = child.wait_with_output() else {
        return Said::Failed;
    };
    if !out.status.success() {
        return Said::Failed;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map_or(Said::Failed, |l| Said::Line(l.trim().to_string()))
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
    match answer(
        &smi,
        &["--query-gpu=name", "--format=csv,noheader"],
        Duration::from_secs(10),
    ) {
        Said::Line(n) if !n.is_empty() => Some(format!("cuda:{n}")),
        _ => None,
    }
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
    detect_within(choice, path, PROBE_CAP)
}

/// [`detect`], each runtime given `cap` to answer. One that does not answer
/// in time is not found for now, and the finding says so: unknown, and
/// worth a retry, never "not rootless".
pub fn detect_within(choice: Choice, path: Option<&OsStr>, cap: Duration) -> Detected {
    let mut looked: Vec<(String, String)> = Vec::new();
    let mut unknown = false;
    let none = |choice, looked, reason: String, unknown| Detected {
        choice,
        runtime: None,
        reason: Some(reason),
        looked,
        unknown,
    };
    let order: Vec<Kind> = match choice {
        Choice::Off => {
            return none(
                choice,
                looked,
                "pipelines are off: an operator turned the runtime off (nils pipeline runtime --set auto turns it on)".into(),
                false,
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
        let line = match answer(&program, &["--version"], cap) {
            Said::Line(l) => l,
            Said::Failed => {
                looked.push((kind.name().into(), "does not answer --version".into()));
                continue;
            }
            Said::TimedOut => {
                unknown = true;
                looked.push((
                    kind.name().into(),
                    format!(
                        "did not answer --version within {} s; unknown, retry",
                        cap.as_secs()
                    ),
                ));
                continue;
            }
        };
        let version = version_of(&line);
        let gpu = match kind {
            Kind::Podman => {
                match answer(
                    &program,
                    &["info", "--format", "{{.Host.Security.Rootless}}"],
                    cap,
                ) {
                    Said::Line(l) if l == "true" => {}
                    Said::TimedOut => {
                        unknown = true;
                        looked.push((
                            kind.name().into(),
                            format!(
                                "{version}, did not answer podman info within {} s (a busy host?); unknown, retry",
                                cap.as_secs()
                            ),
                        ));
                        continue;
                    }
                    _ => {
                        looked.push((
                            kind.name().into(),
                            format!("{version}, not rootless here (D18 runs pipelines rootless)"),
                        ));
                        continue;
                    }
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
            unknown: false,
        };
    }
    let reason = match choice {
        _ if unknown => format!(
            "pipelines are off for now: {}; nothing is known, so look again (nils pipeline runtime)",
            looked
                .iter()
                .filter(|(_, s)| s.ends_with("unknown, retry"))
                .map(|(k, s)| format!("{k} {s}"))
                .collect::<Vec<_>>()
                .join("; ")
        ),
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
    none(choice, looked, reason, unknown)
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
    check_mounts(&inv.mounts).map_err(std::io::Error::other)?;
    prepare_mountpoints(&inv.mounts)?;
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

/// Refuse a mount the runtimes' mount syntax would misread: `--volume
/// host:container:ro` and apptainer's `--bind` split on `:` and `,`, so a
/// path holding either would bind something other than what was meant.
pub fn check_mounts(mounts: &[Mount]) -> Result<(), String> {
    for m in mounts {
        let host = m.host.to_string_lossy();
        for (what, path) in [("host", host.as_ref()), ("container", m.container.as_str())] {
            if path.contains(':') || path.contains(',') {
                return Err(format!(
                    "the {what} path {path} of a mount holds a ':' or a ',', which a runtime's mount syntax splits on"
                ));
            }
        }
        if !m.container.starts_with('/') {
            return Err(format!("{} is not an absolute container path", m.container));
        }
    }
    Ok(())
}

/// Make, in the host folder of each mount, the mountpoints of the mounts
/// nested inside it (`/inputs/<id>` inside `/inputs`): a runtime cannot make
/// one inside a read-only bind, and a real runtime then refuses to start.
pub fn prepare_mountpoints(mounts: &[Mount]) -> std::io::Result<()> {
    for inner in mounts {
        for outer in mounts {
            let Some(rest) = inner
                .container
                .strip_prefix(&outer.container)
                .and_then(|r| r.strip_prefix('/'))
            else {
                continue;
            };
            if rest.is_empty() || rest.split('/').any(|s| s.is_empty() || s == "..") {
                continue;
            }
            let at = outer.host.join(rest);
            if inner.host.is_file() {
                if let Some(parent) = at.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if !at.exists() {
                    std::fs::File::create(&at)?;
                }
            } else {
                std::fs::create_dir_all(&at)?;
            }
        }
    }
    Ok(())
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
            // wave 43's proof: an image's USER beat keep-id; --user wins
            &["--user", "1000:1000"],
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

    /// Wave 43's proof: `podman info` took 7 to 8 s on a busy host, and the
    /// 10 s cap then reported podman "not rootless" and turned pipelines
    /// off. A runtime that does not answer in time is unknown, worth a
    /// retry, and never said to be something it was not asked.
    #[test]
    fn a_runtime_that_answers_late_is_unknown_not_refused() {
        let dir = std::env::temp_dir().join(format!("nils-slow-podman-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let podman = dir.join("podman");
        std::fs::write(
            &podman,
            "#!/bin/sh\n[ \"$1\" = --version ] && { echo 'podman version 5.7.0'; exit 0; }\n[ \"$1\" = info ] && { sleep \"${SLOW:-3}\"; echo true; exit 0; }\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&podman, std::fs::Permissions::from_mode(0o755)).unwrap();
            let slow = detect_within(
                Choice::Only(Kind::Podman),
                Some(dir.as_os_str()),
                Duration::from_secs(1),
            );
            assert!(slow.runtime.is_none());
            assert!(slow.unknown, "{slow:?}");
            let reason = slow.reason.unwrap();
            assert!(reason.contains("unknown, retry"), "{reason}");
            assert!(!reason.contains("not rootless"), "{reason}");
            // given the time it needs, the same podman is taken
            let patient = detect_within(
                Choice::Only(Kind::Podman),
                Some(dir.as_os_str()),
                Duration::from_secs(10),
            );
            assert_eq!(patient.runtime.unwrap().version, "5.7.0");
            assert!(!patient.unknown);
            assert!(PROBE_CAP >= Duration::from_secs(30));
        }
        let _ = std::fs::remove_dir_all(&dir);
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

    /// The review of record 43: a path a mount syntax would split is
    /// refused, and a nested mount's point is made in its parent's folder
    /// before the run, since none can be made inside a read-only bind.
    #[test]
    fn a_mount_is_checked_and_its_nested_points_are_made_first() {
        let m = |host: &str, container: &str| Mount {
            host: PathBuf::from(host),
            container: container.into(),
            read_only: true,
        };
        assert!(check_mounts(&[m("/data/a", "/source/0")]).is_ok());
        for bad in [
            m("/data/a:b", "/source/0"),
            m("/data/a,b", "/source/0"),
            m("/a", "/in:x"),
        ] {
            assert!(check_mounts(&[bad]).is_err());
        }
        let root = std::env::temp_dir().join(format!("nils-mountpoints-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (inputs, labels) = (root.join("inputs"), root.join("labels"));
        std::fs::create_dir_all(&inputs).unwrap();
        std::fs::create_dir_all(&labels).unwrap();
        let card = root.join("card.json");
        std::fs::write(&card, "{}").unwrap();
        prepare_mountpoints(&[
            m(inputs.to_str().unwrap(), "/inputs"),
            m(labels.to_str().unwrap(), "/inputs/labels"),
            m(card.to_str().unwrap(), "/inputs/head/card.json"),
        ])
        .unwrap();
        assert!(inputs.join("labels").is_dir());
        assert!(inputs.join("head/card.json").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
