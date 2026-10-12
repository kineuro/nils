// SPDX-License-Identifier: AGPL-3.0-only

//! What a failure was (2026-10-12): the error of a run, a unit or a job
//! read for its kind, so a person is told why it failed and what to do,
//! not only that it did. A run that said "failed" and nothing more sent a
//! person guessing at the wrong cause (the test images, when the engine had
//! not found dcm2niix).
//!
//! - `dependency`: something the engine or a container needs is missing or
//!   does not start: dcm2niix, the container runtime, the subordinate ids
//!   rootless podman unpacks an image with, an image that will not pull,
//!   the GPU's driver.
//! - `resource`: the machine ran short: memory (a container killed with
//!   137, or by the kernel's OOM killer), disk, a card's memory, the lane's
//!   budget or its card, time.
//! - `input`: a unit's input is missing or was refused.
//! - `pipeline`: the pipeline's own error, and whatever else is not known.
//!
//! The error is kept as it was written; the kind, the cause and one
//! sentence of what to do stand beside it. The sentence is the engine's
//! own, never a tool's words, so it can be shown at any detail.

use serde_json::{Value, json};

/// The four kinds a failure is told as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dependency,
    Resource,
    Input,
    Pipeline,
}

impl Kind {
    pub const ALL: [Kind; 4] = [
        Kind::Dependency,
        Kind::Resource,
        Kind::Input,
        Kind::Pipeline,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Dependency => "dependency",
            Kind::Resource => "resource",
            Kind::Input => "input",
            Kind::Pipeline => "pipeline",
        }
    }
}

/// What failed: the thing the advice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// dcm2niix missing where the engine looks, or not starting.
    Dcm2niix,
    /// No container runtime, or one that failed on its own account.
    Runtime,
    /// Rootless podman without the subordinate ids an image's files need.
    Subuid,
    /// An image that will not pull or build.
    Image,
    /// No GPU driver, or none a container can reach.
    GpuDriver,
    /// A container killed for memory, or a tool out of it.
    Memory,
    /// A full disk, or a quota reached.
    Disk,
    /// A card out of memory.
    GpuMemory,
    /// More than the pipeline lane holds, or no card it leases.
    Lane,
    /// A time limit.
    Time,
    /// A unit's input missing or refused.
    Input,
    /// The pipeline's own error, and anything not known.
    Pipeline,
}

impl Cause {
    pub const ALL: [Cause; 12] = [
        Cause::Dcm2niix,
        Cause::Runtime,
        Cause::Subuid,
        Cause::Image,
        Cause::GpuDriver,
        Cause::Memory,
        Cause::Disk,
        Cause::GpuMemory,
        Cause::Lane,
        Cause::Time,
        Cause::Input,
        Cause::Pipeline,
    ];

    pub fn kind(self) -> Kind {
        match self {
            Cause::Dcm2niix | Cause::Runtime | Cause::Subuid | Cause::Image | Cause::GpuDriver => {
                Kind::Dependency
            }
            Cause::Memory | Cause::Disk | Cause::GpuMemory | Cause::Lane | Cause::Time => {
                Kind::Resource
            }
            Cause::Input => Kind::Input,
            Cause::Pipeline => Kind::Pipeline,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Cause::Dcm2niix => "dcm2niix",
            Cause::Runtime => "runtime",
            Cause::Subuid => "subuid",
            Cause::Image => "image",
            Cause::GpuDriver => "gpu_driver",
            Cause::Memory => "memory",
            Cause::Disk => "disk",
            Cause::GpuMemory => "gpu_memory",
            Cause::Lane => "lane",
            Cause::Time => "time",
            Cause::Input => "input",
            Cause::Pipeline => "pipeline",
        }
    }

    /// What to do, in one plain sentence.
    pub fn advice(self) -> &'static str {
        match self {
            Cause::Dcm2niix => {
                "dcm2niix is not installed where the engine can find it: an admin installs it \
                 with nils tools install dcm2niix (the desk offers it), nils setup --update \
                 installs it beside nils, or a run names one with --dcm2niix PATH"
            }
            Cause::Runtime => {
                "the container runtime is missing or failed on its own: install rootless podman, \
                 or apptainer 1.1 or later, for the engine's account (nils pipeline runtime says \
                 what the engine found), then run it again"
            }
            Cause::Subuid => {
                "rootless podman could not unpack the image, since the engine's account has too \
                 few subordinate user or group ids: give it a range of 65536 or more in \
                 /etc/subuid and /etc/subgid (usermod --add-subuids and --add-subgids), run \
                 podman system migrate as that account, then run it again"
            }
            Cause::Image => {
                "the pipeline's image could not be pulled: check that the engine's machine \
                 reaches the image's registry, or pull the image once as the engine's account, \
                 then run it again"
            }
            Cause::GpuDriver => {
                "the container could not reach a GPU: install or load the NVIDIA driver and the \
                 container toolkit (for podman, nvidia-ctk cdi generate), or run the pipeline on \
                 the processor where it allows that"
            }
            Cause::Memory => {
                "a container was killed for memory: give the lane more memory (nils pipeline \
                 lane --memory-gb), run fewer units at once, or run it on a machine with more \
                 memory"
            }
            Cause::Disk => {
                "the disk is full: free space in the working place the run writes to, then go \
                 on with nils run --resume RUN"
            }
            Cause::GpuMemory => {
                "the card ran out of memory: free the card of other work, or lease one with \
                 more memory (nils pipeline lane --gpu-card)"
            }
            Cause::Lane => {
                "a unit needs more than the pipeline lane holds, or a card it does not lease: \
                 raise the lane within what the machine has (nils pipeline lane), or name the \
                 card it leases"
            }
            Cause::Time => {
                "a unit ran out of time: run it again on a quieter machine, or with fewer units \
                 at once"
            }
            Cause::Input => {
                "a unit's input is missing or was refused: the pre-flight (nils run --preflight) \
                 names each unit and why"
            }
            Cause::Pipeline => {
                "the pipeline itself failed: the error is what it said, and the run's log in its \
                 working place says more"
            }
        }
    }

    /// The fix the engine offers itself, where it has one: the tool it
    /// installs, with its door, its command and the grant they need.
    pub fn fix(self) -> Value {
        match self {
            Cause::Dcm2niix => json!({
                "tool": "dcm2niix",
                "door": DCM2NIIX_INSTALL_DOOR,
                "command": DCM2NIIX_INSTALL_COMMAND,
                "grant": INSTALL_GRANT,
            }),
            _ => Value::Null,
        }
    }

    /// The failure as the doors and `--json` answer it beside its error:
    /// `{kind, cause, advice, fix}`.
    pub fn doc(self) -> Value {
        json!({
            "kind": self.kind().name(),
            "cause": self.name(),
            "advice": self.advice(),
            "fix": self.fix(),
        })
    }

    /// A cause by its name, as a recorded failure names it.
    pub fn parse(name: &str) -> Option<Cause> {
        Cause::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// The door that installs dcm2niix, and the command line's.
pub const DCM2NIIX_INSTALL_DOOR: &str = "POST /api/tools/dcm2niix/install";
pub const DCM2NIIX_INSTALL_COMMAND: &str = "nils tools install dcm2niix";
/// The grant the install needs: an admin's.
pub const INSTALL_GRANT: &str = "install:work";

/// What a failure said: its error, the exit code and the signal of a
/// container that ended, and the end of its log where the engine read it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Said<'a> {
    pub error: &'a str,
    pub exit_code: Option<i64>,
    pub signal: Option<i32>,
    pub log: Option<&'a str>,
}

impl<'a> Said<'a> {
    /// An error alone.
    pub fn error(error: &'a str) -> Said<'a> {
        Said {
            error,
            ..Said::default()
        }
    }
}

/// How much of a log's end is read for its cause.
pub const LOG_TAIL_BYTES: u64 = 64 * 1024;

fn any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

/// The cause of a failure where its words or its exit say one; none where
/// nothing is known, which a run or a unit reads as the pipeline's own.
/// The more particular causes are asked first: a missing converter before
/// a missing file, the subordinate ids before an image, a card's memory
/// before memory.
pub fn known(said: &Said<'_>) -> Option<Cause> {
    // a refusal the engine wrote names its cause itself
    if let Some(cause) = marked(said.error) {
        return Some(cause);
    }
    let mut text = said.error.to_lowercase();
    if let Some(log) = said.log {
        text.push('\n');
        text.push_str(&log.to_lowercase());
    }
    let t = text.as_str();
    if t.contains("dcm2niix")
        && any(
            t,
            &[
                "not runnable",
                "no such file",
                "not found",
                "cannot execute",
                "permission denied",
                "exec format error",
                "error while loading shared libraries",
                "said nothing about its version",
            ],
        )
    {
        return Some(Cause::Dcm2niix);
    }
    // a container killed (137 as podman and docker report it, a signal 9
    // as apptainer's process ends) ran, so its image was there: the
    // machine's memory, whatever else its log says
    if said.exit_code == Some(137) || said.signal == Some(9) || t.contains("exited 137") {
        return Some(Cause::Memory);
    }
    if any(
        t,
        &[
            "insufficient uids or gids",
            "/etc/subuid",
            "/etc/subgid",
            "newuidmap",
            "newgidmap",
            "not enough ids available",
            "there might not be enough ids",
        ],
    ) {
        return Some(Cause::Subuid);
    }
    if any(
        t,
        &[
            "nvidia-container-cli",
            "could not select device driver",
            "unresolvable cdi devices",
            "found no nvidia driver",
            "no nvidia driver",
            "cuda driver version is insufficient",
            "libcuda.so",
            "couldn't communicate with the nvidia driver",
            "failed to initialize nvml",
            "driver not loaded",
            "this engine's runtime offers none",
        ],
    ) {
        return Some(Cause::GpuDriver);
    }
    if any(
        t,
        &[
            "no space left on device",
            "os error 28",
            "disk quota exceeded",
            "os error 122",
            "enospc",
        ],
    ) {
        return Some(Cause::Disk);
    }
    if any(
        t,
        &[
            "cuda out of memory",
            "cuda error: out of memory",
            "outofmemoryerror: cuda",
            "cublas_status_alloc_failed",
            "out of memory on device",
            "hip out of memory",
        ],
    ) {
        return Some(Cause::GpuMemory);
    }
    if any(
        t,
        &[
            "manifest unknown",
            "pull access denied",
            "toomanyrequests",
            "error pulling image",
            "unable to pull",
            "initializing source docker://",
            "copying system image",
            "failed to resolve reference",
            "requested access to the resource is denied",
            "reading manifest",
            "image not known",
            "no such image",
            "image is not known",
            "unable to find image",
            "could not build",
            "short-name",
        ],
    ) {
        return Some(Cause::Image);
    }
    if any(
        t,
        &[
            "no container runtime",
            "pipelines are off",
            "could not start the container",
            "could not start a build",
            "cannot connect to the docker daemon",
            "is the docker daemon running",
            "cannot connect to podman",
        ],
    ) {
        return Some(Cause::Runtime);
    }
    if any(
        t,
        &[
            "oomkilled",
            "oom-kill",
            "oom killer",
            "out of memory",
            "cannot allocate memory",
            "os error 12",
            "memoryerror",
            "std::bad_alloc",
            "killed process",
        ],
    ) {
        return Some(Cause::Memory);
    }
    // 125 is podman's and docker's own failure, never the pipeline's
    if said.exit_code == Some(125) || t.contains("exited 125") {
        return Some(Cause::Runtime);
    }
    if any(
        t,
        &[
            "the lane's budget",
            "and the lane has",
            "the pipeline lane uses no card",
        ],
    ) {
        return Some(Cause::Lane);
    }
    if said.exit_code == Some(124)
        || any(t, &["time limit", "deadline exceeded", "due to time limit"])
    {
        return Some(Cause::Time);
    }
    if any(
        t,
        &[
            "missing an input",
            "for its input",
            "the selection holds no stacks",
            "no stack of the selection is picked",
            "belongs to no subject",
            "the release of the input failed",
            "nothing to run",
        ],
    ) {
        return Some(Cause::Input);
    }
    None
}

/// The words that end a refusal the engine wrote itself, naming its
/// cause, so the job that carries only its error is read for the same
/// cause: `(failure: <cause>)`.
pub fn mark(cause: Cause) -> String {
    format!("(failure: {})", cause.name())
}

/// The cause a refusal's words name, where they end with [`mark`]'s.
fn marked(error: &str) -> Option<Cause> {
    let (_, rest) = error.rsplit_once("(failure: ")?;
    Cause::parse(rest.split(')').next()?.trim())
}

/// The cause of a run's or a unit's failure: what is known, else the
/// pipeline's own.
pub fn cause_of(said: &Said<'_>) -> Cause {
    known(said).unwrap_or(Cause::Pipeline)
}

/// A recorded failure's cause, where the document names one this version
/// knows.
pub fn recorded(doc: &Value) -> Option<Cause> {
    doc["cause"].as_str().and_then(Cause::parse)
}

/// The end of a text at most `max` bytes long, cut at a character.
pub fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// The end of a log file, [`LOG_TAIL_BYTES`] of it at most; none where it
/// cannot be read.
pub fn log_tail(path: &std::path::Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len > LOG_TAIL_BYTES {
        f.seek(SeekFrom::Start(len - LOG_TAIL_BYTES)).ok()?;
    }
    let mut bytes = Vec::new();
    f.take(LOG_TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(error: &str) -> Cause {
        cause_of(&Said::error(error))
    }

    /// 2026-10-12: the error a bids run gave on a laptop whose engine's
    /// unit did not have dcm2niix on its PATH.
    #[test]
    fn a_converter_the_engine_cannot_run_is_a_dependency_with_its_install() {
        let e = "the release of the input failed: dcm2niix is not runnable (No such file or \
                 directory (os error 2)). dcm2niix is a prerequisite of a deployment: install a \
                 current build from rordenlab/dcm2niix, which a distribution's package lags, and \
                 pass --dcm2niix if it is not on the path.";
        assert_eq!(of(e), Cause::Dcm2niix);
        let doc = of(e).doc();
        assert_eq!(doc["kind"], "dependency");
        assert_eq!(doc["cause"], "dcm2niix");
        assert_eq!(doc["fix"]["door"], "POST /api/tools/dcm2niix/install");
        assert_eq!(doc["fix"]["command"], "nils tools install dcm2niix");
        assert_eq!(doc["fix"]["grant"], "install:work");
        assert!(
            doc["advice"]
                .as_str()
                .unwrap()
                .starts_with("dcm2niix is not installed where the engine can find it")
        );
        // a missing input of the release is not this
        assert_eq!(
            of("the release of the input failed: no stack of the selection is picked"),
            Cause::Input
        );
    }

    /// A container the kernel or the runtime killed for memory: exit 137 as
    /// podman and docker report it, a signal 9 as apptainer's process ends,
    /// and the words a tool or a runtime prints.
    #[test]
    fn a_container_killed_for_memory_is_a_resource() {
        let run = "the container exited 137; its log is runs/7/log.txt in the working place work";
        assert_eq!(of(run), Cause::Memory);
        assert_eq!(
            cause_of(&Said {
                error: "the container exited by a signal and wrote no results.json",
                signal: Some(9),
                ..Said::default()
            }),
            Cause::Memory
        );
        assert_eq!(
            cause_of(&Said {
                error: "the container exited 1 and wrote no results.json",
                exit_code: Some(1),
                log: Some("Traceback (most recent call last):\nMemoryError\n"),
                ..Said::default()
            }),
            Cause::Memory
        );
        assert_eq!(Cause::Memory.kind(), Kind::Resource);
        // a card out of memory is the card's, not the machine's
        assert_eq!(
            of("torch.OutOfMemoryError: CUDA out of memory. Tried to allocate 2.00 GiB"),
            Cause::GpuMemory
        );
    }

    #[test]
    fn a_full_disk_is_a_resource() {
        assert_eq!(
            of("the output folder: No space left on device (os error 28)"),
            Cause::Disk
        );
        assert_eq!(
            of("hashing sub-1/out.nii.gz: Disk quota exceeded (os error 122)"),
            Cause::Disk
        );
        assert_eq!(Cause::Disk.doc()["kind"], "resource");
    }

    /// An image that will not pull, as podman says it in the log of a
    /// container that exited 125, and apptainer's build.
    #[test]
    fn a_missing_image_is_a_dependency() {
        let error = "the container exited 125; its log is runs/3/log.txt in the working place \
                     work; 125 is podman's own failure, as when it cannot find or pull \
                     ghcr.io/x/y@sha256:00: it looked in the image store /s";
        let log = "Trying to pull ghcr.io/x/y@sha256:00...\nError: initializing source \
                   docker://ghcr.io/x/y@sha256:00: reading manifest sha256:00 in ghcr.io/x/y: \
                   manifest unknown\n";
        assert_eq!(
            cause_of(&Said {
                error,
                exit_code: Some(125),
                log: Some(log),
                ..Said::default()
            }),
            Cause::Image
        );
        // the same exit with nothing more said is still the runtime's own
        assert_eq!(
            cause_of(&Said {
                error,
                exit_code: Some(125),
                ..Said::default()
            }),
            Cause::Runtime
        );
        assert_eq!(
            of(
                "apptainer could not build ghcr.io/x/y@sha256:00 from its digest; its log is runs/3/image.log"
            ),
            Cause::Image
        );
    }

    /// Rootless podman with too few subordinate ids for an image's files,
    /// as it happened on a laptop.
    #[test]
    fn too_few_subordinate_ids_are_a_dependency() {
        let log = "Error: copying system image from manifest list: writing blob: adding layer \
                   with blob \"sha256:ab\": processing tar file(potentially insufficient UIDs or \
                   GIDs available in user namespace (requested 0:42 for /etc/shadow): Check \
                   /etc/subuid and /etc/subgid if configured locally and run \"podman system \
                   migrate\": lchown /etc/shadow: invalid argument): exit status 1\n";
        let c = cause_of(&Said {
            error: "the container exited 125; its log is runs/4/log.txt",
            exit_code: Some(125),
            log: Some(log),
            ..Said::default()
        });
        assert_eq!(c, Cause::Subuid);
        assert_eq!(c.kind(), Kind::Dependency);
    }

    #[test]
    fn the_runtime_the_driver_the_lane_and_time() {
        assert_eq!(
            of(
                "pipelines are off: no container runtime here; rootless podman or apptainer runs them"
            ),
            Cause::Runtime
        );
        assert_eq!(
            of("podman could not start the container: No such file or directory (os error 2)"),
            Cause::Runtime
        );
        assert_eq!(
            of("Error: setting up CDI devices: unresolvable CDI devices nvidia.com/gpu=all"),
            Cause::GpuDriver
        );
        assert_eq!(
            of("a unit needs 8 cores and 64 GB, and the lane has 4 cores and 16 GB"),
            Cause::Lane
        );
        assert_eq!(of("fastsurfer: time limit reached"), Cause::Time);
        // a tool's own words that only look like a cause are its own: a
        // command the image lacks, a download a container with no network
        // could not make
        assert_eq!(
            cause_of(&Said {
                error: "the container exited 127 and wrote no results.json",
                exit_code: Some(127),
                log: Some(
                    "Error: crun: executable file `segment` not found in $PATH: No such file or directory: OCI runtime attempted to invoke a command that was not found\n"
                ),
                ..Said::default()
            }),
            Cause::Pipeline
        );
        assert_eq!(
            of("urllib.error.URLError: <urlopen error [Errno 110] Connection timed out>"),
            Cause::Pipeline
        );
        assert_eq!(
            of("missing an input: no stack is picked for the role t1w"),
            Cause::Input
        );
    }

    /// What is not known is the pipeline's own, with the error kept as it
    /// was; a job of another kind has none.
    #[test]
    fn the_rest_is_the_pipeline_s_own() {
        assert_eq!(of("refused on purpose"), Cause::Pipeline);
        assert_eq!(
            of("the container exited 3; its log is runs/2/log.txt"),
            Cause::Pipeline
        );
        assert!(known(&Said::error("the pack mri has no axis x")).is_none());
        assert_eq!(Cause::Pipeline.doc()["fix"], Value::Null);
        for c in Cause::ALL {
            assert_eq!(Cause::parse(c.name()), Some(c));
            assert!(
                !c.advice().contains('\u{2014}'),
                "no dashes in {}",
                c.name()
            );
        }
        assert_eq!(recorded(&Cause::Disk.doc()), Some(Cause::Disk));
        // a refusal the engine wrote is read for the cause it names
        assert_eq!(
            of(&format!(
                "refused before it was queued: 2 GB free {}",
                mark(Cause::Disk)
            )),
            Cause::Disk
        );
    }

    #[test]
    fn a_log_s_tail_is_its_end() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
        // never inside a character
        assert_eq!(tail("aé", 1), "");
    }
}
