// SPDX-License-Identifier: AGPL-3.0-only

//! The container runtimes (record 43 R2, D18), behind one trait.
//!
//! The order is rootless podman, then apptainer, then docker only where an
//! operator opted in (`nils pipeline runtime --set docker`), since docker's
//! daemon is root on the host; `--set apptainer-first` looks for apptainer
//! before podman, as an unprivileged container such as the group's NILS
//! guest wants (record 49 A2). Apptainer runs an image the engine built from
//! the pinned OCI digest into a SIF file or a sandbox folder, kept by that
//! digest, so an image is fetched once and a run never pulls a tag. None found is not an error: the capability is
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

/// What an operator chose: find one (`auto`, podman then apptainer, or
/// `apptainer-first`, apptainer then podman), only this one (the one way
/// docker is taken), or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Auto,
    ApptainerFirst,
    Only(Kind),
    Off,
}

impl Choice {
    pub const WORDS: [&'static str; 6] = [
        "auto",
        "apptainer-first",
        "podman",
        "apptainer",
        "docker",
        "off",
    ];

    pub fn parse(text: &str) -> Option<Choice> {
        Some(match text.trim() {
            "" | "auto" => Choice::Auto,
            "apptainer-first" => Choice::ApptainerFirst,
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
            Choice::ApptainerFirst => "apptainer-first",
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
    /// The one card to pass, by index, where the lane leased one (record 49
    /// A2); none is the lane's default card, 0. Every card is never passed.
    pub card: Option<u32>,
    /// For apptainer, the SIF file or sandbox folder built from the image's
    /// digest; none runs `docker://<image>`.
    pub local_image: Option<PathBuf>,
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
    /// The cores and the memory the runtime holds the container to, from
    /// what the unit declares (record 49, after review); none where the
    /// runtime cannot hold it to them here.
    pub cpus: Option<u32>,
    pub memory_mib: Option<u64>,
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
            if let Some(c) = inv.cpus {
                a.extend(["--cpus".to_string(), c.to_string()]);
            }
            if let Some(m) = inv.memory_mib {
                a.extend(["--memory".to_string(), format!("{m}m")]);
            }
            if inv.gpu {
                // the one card the lease holds: CDI names it alone for
                // podman, and docker is told that device; inside, it is
                // the container's only card
                let card = inv.card.unwrap_or(0);
                if kind == Kind::Podman {
                    a.extend(["--device".to_string(), format!("nvidia.com/gpu={card}")]);
                } else {
                    a.extend(["--gpus".to_string(), format!("device={card}")]);
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
            // `run`, not `exec`: an image built from an OCI image runs its
            // ENTRYPOINT with these words after it (its CMD where there are
            // none), as podman and docker do; `exec` would skip the
            // ENTRYPOINT (record 49 A2). `--no-eval` hands those words to
            // the ENTRYPOINT as they are: without it the image's runscript
            // quotes them in double quotes and evaluates them through a
            // shell first, so a `bash -c '...'` command has its `$(..)`,
            // `$f` and inner quotes taken apart before it runs. `--pwd /`
            // starts in the image's root, since the engine's own working
            // folder is not in a contained image.
            a.extend(
                [
                    "run",
                    "--containall",
                    "--cleanenv",
                    "--no-home",
                    "--no-eval",
                    "--pwd",
                    "/",
                    "--net",
                    "--network",
                    "none",
                ]
                .map(String::from),
            );
            if let Some(c) = inv.cpus {
                a.extend(["--cpus".to_string(), c.to_string()]);
            }
            if let Some(m) = inv.memory_mib {
                a.extend(["--memory".to_string(), format!("{m}M")]);
            }
            if inv.gpu {
                // --nv shows every card of the host: the process is told
                // the one its lease holds, counted in the bus order that
                // nvidia-smi's index counts in
                let card = inv.card.unwrap_or(0);
                a.push("--nv".into());
                for env in [
                    "CUDA_DEVICE_ORDER=PCI_BUS_ID".to_string(),
                    format!("CUDA_VISIBLE_DEVICES={card}"),
                    format!("NVIDIA_VISIBLE_DEVICES={card}"),
                ] {
                    a.push("--env".into());
                    a.push(env);
                }
            }
            for m in &inv.mounts {
                a.push("--bind".into());
                a.push(mount(m));
            }
            for (k, v) in &inv.env {
                a.push("--env".into());
                a.push(format!("{k}={v}"));
            }
            match &inv.local_image {
                Some(local) => a.push(local.display().to_string()),
                None => a.push(format!("docker://{}", inv.image)),
            }
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
        Choice::ApptainerFirst => vec![Kind::Apptainer, Kind::Podman],
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
            Kind::Apptainer => {
                // before 1.1 an unprivileged apptainer cannot keep a
                // container off the network, which every run is
                if !apptainer_isolates(&version) {
                    looked.push((
                        kind.name().into(),
                        format!(
                            "{version}, older than 1.1, which cannot run --network none unprivileged"
                        ),
                    ));
                    continue;
                }
                nvidia(path)
            }
            Kind::Docker => nvidia(path),
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

/// Whether an apptainer version runs `--net --network none` for an
/// unprivileged user: 1.1 and later.
pub fn apptainer_isolates(version: &str) -> bool {
    let mut parts = version
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    (major, minor) >= (1, 1)
}

/// How apptainer keeps an image: a SIF file, which needs squashfuse or a
/// setuid install to run unprivileged, or a sandbox folder, which runs
/// where there is no /dev/fuse, as in an unprivileged container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageForm {
    Sif,
    Sandbox,
}

impl ImageForm {
    pub const WORDS: [&'static str; 2] = ["sif", "sandbox"];

    pub fn parse(text: &str) -> Option<ImageForm> {
        match text.trim() {
            "" | "sif" => Some(ImageForm::Sif),
            "sandbox" => Some(ImageForm::Sandbox),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ImageForm::Sif => "sif",
            ImageForm::Sandbox => "sandbox",
        }
    }
}

/// Where apptainer's copy of an image is kept under the image folder: by
/// the image's manifest digest, `<hex>.sif` or `<hex>.sandbox`.
pub fn local_image(dir: &Path, digest: &str, form: ImageForm) -> PathBuf {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    dir.join(match form {
        ImageForm::Sif => format!("{hex}.sif"),
        ImageForm::Sandbox => format!("{hex}.sandbox"),
    })
}

/// The words after `apptainer` that build an image's local copy at `target`
/// from its pinned reference.
pub fn build_argv(reference: &str, target: &Path, form: ImageForm) -> Vec<String> {
    let mut a = vec!["build".to_string()];
    if form == ImageForm::Sandbox {
        a.push("--sandbox".into());
    }
    a.push(target.display().to_string());
    a.push(format!("docker://{reference}"));
    a
}

/// The cgroup controllers podman names, from `{{.Host.CgroupControllers}}`:
/// `[cpu io memory pids]`.
pub fn parse_controllers(text: &str) -> Vec<String> {
    text.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Which of the cores and the memory this runtime can hold a container to
/// here (record 49, after review): docker's daemon always can; rootless
/// podman where its cgroups delegate the controller; apptainer where the
/// cgroup v2 tree it would run in (the user's delegated one, unless it
/// runs as root) offers it. Answers (cores, memory).
pub fn limits_here(cli: &Cli) -> (bool, bool) {
    let has = |c: &[String], w: &str| c.iter().any(|x| x == w);
    match cli.kind {
        Kind::Docker => (true, true),
        Kind::Podman => match answer(
            &cli.program,
            &["info", "--format", "{{.Host.CgroupControllers}}"],
            PROBE_CAP,
        ) {
            Said::Line(l) => {
                let c = parse_controllers(&l);
                (has(&c, "cpu"), has(&c, "memory"))
            }
            _ => (false, false),
        },
        Kind::Apptainer => {
            let root = Path::new("/sys/fs/cgroup");
            #[cfg(unix)]
            let uid = unsafe_free_uid();
            #[cfg(not(unix))]
            let uid = 1;
            let file = if uid == 0 {
                root.join("cgroup.controllers")
            } else {
                root.join(format!(
                    "user.slice/user-{uid}.slice/user@{uid}.service/cgroup.controllers"
                ))
            };
            let c = std::fs::read_to_string(file)
                .map(|t| parse_controllers(&t))
                .unwrap_or_default();
            (has(&c, "cpu"), has(&c, "memory"))
        }
    }
}

/// This process's uid, read from /proc, so no unsafe call is needed.
#[cfg(unix)]
fn unsafe_free_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|u| u.parse().ok())
        })
        .unwrap_or(1)
}

/// The file beside a cached image that records what the engine built.
fn image_record(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".digest");
    target.with_file_name(name)
}

/// What a cached image is, as a digest: a SIF file by the sha256 of its
/// bytes; a sandbox folder by the sha256 of every entry's path, kind,
/// mode, size and modification time, and a link's target, never followed.
/// A link in the image's own place is refused.
pub fn image_fingerprint(target: &Path, form: ImageForm) -> std::io::Result<String> {
    let meta = std::fs::symlink_metadata(target)?;
    let bad = || std::io::Error::other("not the kind of image the engine builds");
    match form {
        ImageForm::Sif => {
            if !meta.is_file() {
                return Err(bad());
            }
            Ok(crate::files::sha256_file(target)?.1)
        }
        ImageForm::Sandbox => {
            if !meta.is_dir() {
                return Err(bad());
            }
            let mut lines: Vec<String> = Vec::new();
            let mut stack = vec![target.to_path_buf()];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d)? {
                    let e = e?;
                    let p = e.path();
                    let m = std::fs::symlink_metadata(&p)?;
                    let rel = p
                        .strip_prefix(target)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .into_owned();
                    let mtime = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_nanos());
                    #[cfg(unix)]
                    let mode = {
                        use std::os::unix::fs::MetadataExt;
                        m.mode()
                    };
                    #[cfg(not(unix))]
                    let mode = 0u32;
                    let link = if m.file_type().is_symlink() {
                        std::fs::read_link(&p)?.to_string_lossy().into_owned()
                    } else {
                        String::new()
                    };
                    lines.push(format!("{rel}\t{mode:o}\t{}\t{mtime}\t{link}", m.len()));
                    if m.is_dir() {
                        stack.push(p);
                    }
                }
            }
            lines.sort();
            Ok(crate::sha256(lines.join("\n").as_bytes()))
        }
    }
}

/// Record what the engine built, beside it.
pub fn record_image(target: &Path, form: ImageForm) -> std::io::Result<()> {
    let digest = image_fingerprint(target, form)?;
    let record = image_record(target);
    let _ = std::fs::remove_file(&record);
    crate::files::write_new(&record, digest.as_bytes())
}

/// Whether a cached image is what the engine built: its digest now is the
/// one recorded when it was built.
pub fn image_verified(target: &Path, form: ImageForm) -> bool {
    let Ok(recorded) = std::fs::read_to_string(image_record(target)) else {
        return false;
    };
    image_fingerprint(target, form).is_ok_and(|d| d == recorded.trim())
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
    let mut child = spawn(rt, inv, log)?;
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
                let status = kill(&mut child)?;
                return Ok(Ended {
                    code: status.code(),
                    stopped: true,
                });
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Start an invocation and leave it running, its output and errors into
/// `log`: how the lane runs several at once (record 49 A1).
pub fn spawn(
    rt: &dyn Runtime,
    inv: &Invocation,
    log: &Path,
) -> std::io::Result<std::process::Child> {
    check_mounts(&inv.mounts).map_err(std::io::Error::other)?;
    prepare_mountpoints(&inv.mounts)?;
    let out = std::fs::File::create(log)?;
    let err = out.try_clone()?;
    let mut c = rt.command(inv);
    c.stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    // a group of its own, so a stop reaches every process the runtime
    // started (apptainer's starter, a stand-in's child), not the client alone
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c.spawn()
}

/// Stop a spawned invocation and every process in its group; answers how
/// it ended.
pub fn kill(child: &mut std::process::Child) -> std::io::Result<std::process::ExitStatus> {
    kill_group(child.id());
    let _ = child.kill();
    child.wait()
}

/// Send SIGKILL to the process group a spawned invocation leads.
pub fn kill_group(leader: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{leader}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    {
        let _ = leader;
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
            card: None,
            local_image: None,
            env: vec![("NILS_RUN".into(), "7".into())],
            user: Some((1000, 1000)),
            cpus: None,
            memory_mib: None,
        }
    }

    fn has(a: &[String], pair: &[&str]) -> bool {
        a.windows(pair.len())
            .any(|w| w.iter().zip(pair).all(|(x, y)| x == y))
    }

    fn has2(a: &[String], w: &[&str]) -> bool {
        a.windows(w.len())
            .any(|x| x.iter().zip(w).all(|(p, q)| p == q))
    }

    /// Record 49, after review: a GPU unit sees the one card its lease
    /// holds, by the bus order nvidia-smi counts in, and never every card.
    #[test]
    fn a_gpu_unit_is_given_its_leased_card_alone() {
        let mut i = inv(true);
        i.card = Some(1);
        let a = argv(Kind::Apptainer, &i);
        for env in [
            "CUDA_DEVICE_ORDER=PCI_BUS_ID",
            "CUDA_VISIBLE_DEVICES=1",
            "NVIDIA_VISIBLE_DEVICES=1",
        ] {
            assert!(has2(&a, &["--env", env]), "{env}: {a:?}");
        }
        let p = argv(Kind::Podman, &i);
        assert!(has2(&p, &["--device", "nvidia.com/gpu=1"]), "{p:?}");
        // no card named: the lane's default card, never all of them
        let i = inv(true);
        for kind in [Kind::Podman, Kind::Docker, Kind::Apptainer] {
            let a = argv(kind, &i);
            assert!(!a.iter().any(|w| w.ends_with("gpu=all")), "{kind:?}: {a:?}");
            assert!(!has2(&a, &["--gpus", "all"]), "{kind:?}: {a:?}");
        }
        assert!(has2(
            &argv(Kind::Podman, &i),
            &["--device", "nvidia.com/gpu=0"]
        ));
        assert!(has2(&argv(Kind::Docker, &i), &["--gpus", "device=0"]));
    }

    /// Record 49, after review: a unit's declared cores and memory are
    /// enforced by the runtime, not only counted by the lane.
    #[test]
    fn a_unit_s_cores_and_memory_are_enforced() {
        let mut i = inv(false);
        i.cpus = Some(2);
        i.memory_mib = Some(4096);
        for kind in [Kind::Podman, Kind::Docker] {
            let a = argv(kind, &i);
            assert!(
                has2(&a, &["--cpus", "2"]) && has2(&a, &["--memory", "4096m"]),
                "{a:?}"
            );
        }
        let a = argv(Kind::Apptainer, &i);
        assert!(
            has2(&a, &["--cpus", "2"]) && has2(&a, &["--memory", "4096M"]),
            "{a:?}"
        );
        let a = argv(Kind::Podman, &inv(false));
        assert!(!a.iter().any(|w| w == "--cpus" || w == "--memory"), "{a:?}");
        assert_eq!(parse_controllers("[memory pids]"), ["memory", "pids"]);
        assert_eq!(parse_controllers("[cpu io memory pids]").len(), 4);
        assert!(parse_controllers("").is_empty());
    }

    /// Record 49, after review: a cached image is used only where it is
    /// what the engine built, by the digest it recorded then; a changed
    /// file, a changed tree or a link in its place is not.
    #[test]
    fn a_cached_image_is_used_only_as_built() {
        let dir = std::env::temp_dir().join(format!("nils-image-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sif = dir.join("abc.sif");
        std::fs::write(&sif, b"an image").unwrap();
        assert!(
            !image_verified(&sif, ImageForm::Sif),
            "no digest recorded yet"
        );
        record_image(&sif, ImageForm::Sif).unwrap();
        assert!(image_verified(&sif, ImageForm::Sif));
        std::fs::write(&sif, b"an image, changed").unwrap();
        assert!(!image_verified(&sif, ImageForm::Sif));
        let tree = dir.join("abc.sandbox");
        std::fs::create_dir_all(tree.join("bin")).unwrap();
        std::fs::write(tree.join("bin/tool"), b"#!/bin/sh").unwrap();
        record_image(&tree, ImageForm::Sandbox).unwrap();
        assert!(image_verified(&tree, ImageForm::Sandbox));
        std::fs::write(tree.join("bin/tool"), b"#!/bin/sh\necho changed").unwrap();
        assert!(!image_verified(&tree, ImageForm::Sandbox));
        #[cfg(unix)]
        {
            let other = dir.join("other.sif");
            std::fs::write(&other, b"an image").unwrap();
            let linked = dir.join("def.sif");
            std::os::unix::fs::symlink(&other, &linked).unwrap();
            std::fs::write(
                dir.join("def.sif.digest"),
                std::fs::read(dir.join("abc.sif.digest")).unwrap(),
            )
            .unwrap();
            assert!(!image_verified(&linked, ImageForm::Sif));
        }
        let _ = std::fs::remove_dir_all(&dir);
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
            &["--device", "nvidia.com/gpu=0"]
        ));

        let d = argv(Kind::Docker, &inv(true));
        assert!(has(&d, &["--network", "none"]) && has(&d, &["--user", "1000:1000"]));
        assert!(has(&d, &["--gpus", "device=0"]) && !d.iter().any(|w| w == "keep-id"));

        let a = argv(Kind::Apptainer, &inv(true));
        assert_eq!(a[0], "run");
        assert!(has(&a, &["--network", "none"]) && a.iter().any(|w| w == "--nv"));
        assert!(has(&a, &["--bind", "/w/runs/7/input:/input:ro"]));
        assert!(a.iter().any(|w| w.starts_with("docker://busybox@sha256:")));
    }

    /// Record 49 slice G: apptainer's runscript for an OCI image evaluates
    /// the words after the image through a shell unless told `--no-eval`,
    /// so a starter's `bash -c '...'` lost its `$(..)`, `$f` and inner
    /// quotes and every unit failed. The words reach the image as they
    /// are, after the image, with `--no-eval` before it.
    #[test]
    fn apptainer_hands_a_shell_command_over_unevaluated() {
        let script = r#"set -eu; cd /input; for f in sub-*/anat/*_T1w.nii.gz; do d=/output/$(dirname "$f"); mkdir -p "$d"; done; echo "N4 corrected $n images""#;
        let mut i = inv(false);
        i.local_image = Some(PathBuf::from("/w/images/abc.sandbox"));
        i.argv = vec!["bash".into(), "-c".into(), script.into()];
        let a = argv(Kind::Apptainer, &i);
        let image_at = a.iter().position(|w| w == "/w/images/abc.sandbox").unwrap();
        let no_eval = a.iter().position(|w| w == "--no-eval").expect("--no-eval");
        assert!(no_eval < image_at, "{a:?}");
        assert!(has(&a[..image_at], &["--pwd", "/"]), "{a:?}");
        assert_eq!(&a[image_at + 1..], ["bash", "-c", script]);
        // the other runtimes pass words as they are already
        let p = argv(Kind::Podman, &i);
        assert!(!p.iter().any(|w| w == "--no-eval"), "{p:?}");
        assert_eq!(&p[p.len() - 3..], ["bash", "-c", script]);
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

    /// Record 49 A2: a leased card is the one card passed, by each runtime
    /// its own way, and apptainer runs the local copy it was given, with
    /// the flags that keep it apart from the host.
    #[test]
    fn a_leased_card_is_the_one_passed_and_apptainer_runs_its_local_copy() {
        let mut i = inv(true);
        i.card = Some(1);
        assert!(has(
            &argv(Kind::Podman, &i),
            &["--device", "nvidia.com/gpu=1"]
        ));
        assert!(has(&argv(Kind::Docker, &i), &["--gpus", "device=1"]));
        i.local_image = Some(PathBuf::from("/w/images/abc.sif"));
        let a = argv(Kind::Apptainer, &i);
        assert_eq!(
            &a[..10],
            [
                "run",
                "--containall",
                "--cleanenv",
                "--no-home",
                "--no-eval",
                "--pwd",
                "/",
                "--net",
                "--network",
                "none"
            ]
        );
        assert!(has(&a, &["--env", "CUDA_VISIBLE_DEVICES=1"]) && a.iter().any(|w| w == "--nv"));
        // the words after the image follow its ENTRYPOINT, as under podman:
        // `run`, never `exec`, which would skip it
        assert!(!a.iter().any(|w| w == "exec"), "{a:?}");
        let image_at = a.iter().position(|w| w == "/w/images/abc.sif").unwrap();
        assert_eq!(&a[image_at + 1..], ["sh", "-c", "ls /input > /output/x"]);
        assert!(!a.iter().any(|w| w.starts_with("docker://")));
        let target = local_image(
            Path::new("/w/images"),
            &format!("sha256:{}", "b".repeat(64)),
            ImageForm::Sandbox,
        );
        assert_eq!(
            target,
            PathBuf::from(format!("/w/images/{}.sandbox", "b".repeat(64)))
        );
        assert_eq!(
            build_argv("busybox@sha256:00", &target, ImageForm::Sandbox)[..2],
            ["build", "--sandbox"]
        );
        assert_eq!(
            build_argv("busybox@sha256:00", Path::new("/x.sif"), ImageForm::Sif),
            ["build", "/x.sif", "docker://busybox@sha256:00"]
        );
        assert!(apptainer_isolates("1.3.4-1") && apptainer_isolates("1.1.0"));
        assert!(!apptainer_isolates("1.0.3") && !apptainer_isolates("0.9"));
        assert_eq!(
            Choice::parse("apptainer-first"),
            Some(Choice::ApptainerFirst)
        );
        assert_eq!(ImageForm::parse("sandbox"), Some(ImageForm::Sandbox));
    }

    /// `apptainer-first` looks for apptainer before podman; an apptainer
    /// too old to isolate the network is passed over, with the reason.
    #[test]
    fn apptainer_first_prefers_apptainer_and_an_old_one_is_passed_over() {
        let dir = std::env::temp_dir().join(format!("nils-apptainer-first-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let write = |name: &str, body: &str| {
                let f = dir.join(name);
                std::fs::write(&f, body).unwrap();
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            };
            write(
                "podman",
                "#!/bin/sh\n[ \"$1\" = --version ] && { echo 'podman version 5.0.0'; exit 0; }\necho true\n",
            );
            write("apptainer", "#!/bin/sh\necho 'apptainer version 1.3.4'\n");
            write("nvidia-smi", "#!/bin/sh\nexit 1\n");
            let path = Some(dir.as_os_str());
            assert_eq!(
                detect(Choice::Auto, path).runtime.unwrap().kind,
                Kind::Podman
            );
            assert_eq!(
                detect(Choice::ApptainerFirst, path).runtime.unwrap().kind,
                Kind::Apptainer
            );
            write("apptainer", "#!/bin/sh\necho 'apptainer version 1.0.3'\n");
            let d = detect(Choice::ApptainerFirst, path);
            assert_eq!(d.runtime.unwrap().kind, Kind::Podman);
            assert!(d.looked[0].1.contains("older than 1.1"), "{:?}", d.looked);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
