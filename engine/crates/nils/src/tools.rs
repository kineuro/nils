// SPDX-License-Identifier: AGPL-3.0-only

//! The tools the engine runs beside itself (2026-10-12): dcm2niix, which a
//! BIDS release and a run's bids input convert with. One build is pinned
//! here, with the sha256 of its archive for each platform rordenlab
//! publishes, and `nils setup` and the engine's own install read the same
//! pin: setup places it beside nils on a machine install, and an admin
//! installs it from the desk or with `nils tools install dcm2niix` into the
//! registry's own `tools` folder, which the engine's account writes and no
//! system path is.
//!
//! The engine finds dcm2niix in this order: a path a run or a release
//! names with `--dcm2niix`; the name on the search path; beside the running
//! nils; where the setup record names one; and the registry's tools
//! folder. A unit of systemd has the default search path, which a
//! person's `~/.local/bin` is not on, so a converter beside nils is found
//! there all the same.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The converter's name, as a run and a release name it by default.
pub(crate) const DCM2NIIX: &str = "dcm2niix";

/// The build this version takes, and where its archives are published.
pub(crate) const DCM2NIIX_VERSION: &str = "v1.0.20260724";
pub(crate) const DCM2NIIX_RELEASES: &str =
    "https://github.com/rordenlab/dcm2niix/releases/download";

/// A mirror of the releases, a folder (`file://`) or a server holding
/// `<version>/<archive>`, for a machine that cannot reach GitHub; it is
/// held to the same digests.
pub(crate) const DCM2NIIX_MIRROR_ENV: &str = "NILS_DCM2NIIX_RELEASES";

/// The digest a debug build, as the tests run it, takes in place of the
/// pinned one, so a mirror of a stand-in archive installs. A release build
/// never reads it.
const TEST_SHA256_ENV: &str = "NILS_DCM2NIIX_TEST_SHA256";

/// One platform's archive of the pinned build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pin {
    /// As `std::env::consts::OS` and `ARCH` name the platform.
    pub(crate) os: &'static str,
    pub(crate) arch: &'static str,
    pub(crate) archive: &'static str,
    pub(crate) sha256: &'static str,
}

/// The archives of the pinned build, read from the release's own digests
/// and checked against the downloaded bytes. rordenlab publishes Linux for
/// x86_64 alone and one macOS archive for both processors; a Linux on arm
/// has no build to take, and is told so.
pub(crate) const DCM2NIIX_PINS: [Pin; 3] = [
    Pin {
        os: "linux",
        arch: "x86_64",
        archive: "dcm2niix_lnx.zip",
        sha256: "2916a7bc64511eb20fb976a994799183cc1bfdd03395bf5d8edef54a58dee06f",
    },
    Pin {
        os: "macos",
        arch: "aarch64",
        archive: "dcm2niix_mac.zip",
        sha256: "d393006dd1edfba4013bc4b8eccc875b23d611ecbc53fda5a5542f629591008f",
    },
    Pin {
        os: "macos",
        arch: "x86_64",
        archive: "dcm2niix_mac.zip",
        sha256: "d393006dd1edfba4013bc4b8eccc875b23d611ecbc53fda5a5542f629591008f",
    },
];

/// The pinned archive of a platform, where rordenlab publishes one.
pub(crate) fn pin_for(os: &str, arch: &str) -> Option<Pin> {
    DCM2NIIX_PINS
        .iter()
        .copied()
        .find(|p| p.os == os && p.arch == arch)
}

/// This machine's pinned archive.
pub(crate) fn pin_here() -> Option<Pin> {
    pin_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The sha256 an archive is held to: the pin's.
fn wanted_sha256(pin: &Pin) -> String {
    if cfg!(debug_assertions)
        && let Ok(test) = std::env::var(TEST_SHA256_ENV)
        && !test.trim().is_empty()
    {
        return test.trim().to_string();
    }
    pin.sha256.to_string()
}

/// Where the archives come from: rordenlab's releases, or a mirror.
pub(crate) fn releases_base() -> String {
    std::env::var(DCM2NIIX_MIRROR_ENV)
        .ok()
        .map(|base| base.trim().to_string())
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| DCM2NIIX_RELEASES.to_string())
}

/// One archive of the pinned build under a base: `<base>/<version>/<archive>`.
pub(crate) fn archive_url(base: &str, pin: &Pin) -> String {
    format!(
        "{}/{DCM2NIIX_VERSION}/{}",
        base.trim_end_matches('/'),
        pin.archive
    )
}

/// What a machine with no pinned build is told.
pub(crate) fn no_build_here() -> String {
    format!(
        "rordenlab publishes no dcm2niix build for {} on {}: install the distribution's \
         package (apt install dcm2niix) or build {DCM2NIIX_VERSION} from its source, and \
         put it on the engine's search path or beside nils",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// The way by hand, for an engine that cannot reach the releases: the
/// archive, its digest and where the binary goes.
pub(crate) fn by_hand(pin: &Pin, into: &Path) -> String {
    format!(
        "download {} on a machine that reaches it, check that its sha256 is {}, unpack the \
         dcm2niix in it to {} and make it executable; or point {DCM2NIIX_MIRROR_ENV} at a \
         folder or server that holds {DCM2NIIX_VERSION}/{}",
        archive_url(DCM2NIIX_RELEASES, pin),
        pin.sha256,
        into.display(),
        pin.archive
    )
}

/// The one file of a zip archive at its top by that name: stored or
/// deflated, its CRC checked. The archives this version pins hold one
/// file each, and are read only once their sha256 is the pin's.
pub(crate) fn zip_member(bytes: &[u8], name: &str) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let u16_at = |i: usize| -> Result<usize, String> {
        bytes
            .get(i..i + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            .ok_or_else(|| "the archive ends early".to_string())
    };
    let u32_at = |i: usize| -> Result<usize, String> {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| "the archive ends early".to_string())
    };
    // the end of the central directory, at most a comment's length from the end
    let lowest = bytes.len().saturating_sub(22 + 65_535);
    let end = (lowest..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&i| bytes[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06]))
        .ok_or("it is not a zip archive")?;
    let entries = u16_at(end + 10)?;
    let mut at = u32_at(end + 16)?;
    for _ in 0..entries {
        if u32_at(at)? != 0x0201_4b50 {
            return Err("its directory is damaged".into());
        }
        let method = u16_at(at + 10)?;
        let crc = u32_at(at + 16)? as u32;
        let packed = u32_at(at + 20)?;
        let size = u32_at(at + 24)?;
        let (n, e, c) = (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?);
        let local = u32_at(at + 42)?;
        let entry = bytes
            .get(at + 46..at + 46 + n)
            .ok_or("the archive ends early")?;
        at += 46 + n + e + c;
        if entry != name.as_bytes() {
            continue;
        }
        if u32_at(local)? != 0x0403_4b50 {
            return Err(format!("its entry {name} is damaged"));
        }
        let start = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        let data = bytes
            .get(start..start + packed)
            .ok_or("the archive ends early")?;
        let out = match method {
            0 => data.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(data)
                    .take(size as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("its entry {name}: {e}"))?;
                out
            }
            other => return Err(format!("its entry {name} is packed by method {other}")),
        };
        let mut check = flate2::Crc::new();
        check.update(&out);
        if out.len() != size || check.sum() != crc {
            return Err(format!("its entry {name} does not unpack to what it says"));
        }
        return Ok(out);
    }
    Err(format!("the archive holds no {name}"))
}

/// The binary of a pinned archive, once its sha256 is `want`.
pub(crate) fn binary_of(bytes: &[u8], want: &str) -> Result<Vec<u8>, String> {
    let got = crate::supervise::sha256_hex(bytes);
    if got != want {
        return Err(format!(
            "its sha256 is {got}, and this version of nils takes only {want}"
        ));
    }
    zip_member(bytes, DCM2NIIX)
}

/// The binary written at `into`, executable, by way of a file beside it
/// renamed over it, so nothing half written is ever at `into`.
pub(crate) fn place_binary(binary: &[u8], into: &Path) -> Result<(), String> {
    let dir = into.parent().ok_or("the binary's folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let partial = dir.join(format!(".{DCM2NIIX}.partial-{}", std::process::id()));
    let written = std::fs::write(&partial, binary).and_then(|()| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&partial, into)
    });
    written.map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("{}: {e}", into.display())
    })
}

/// Why a fetch did not place the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotFetched {
    /// No build is pinned for this machine.
    NoBuild(String),
    /// The releases, or the mirror, could not be reached.
    Unreachable { url: String, why: String },
    /// It was reached, and what came is not the pinned build.
    Refused { url: String, why: String },
    /// The binary could not be written.
    Unwritten(String),
}

impl NotFetched {
    /// In one sentence, with the way by hand where the releases were not
    /// reached.
    pub(crate) fn said(&self, into: &Path) -> String {
        match self {
            NotFetched::NoBuild(why) => why.clone(),
            NotFetched::Unreachable { url, why } => {
                let manual = pin_here()
                    .map(|p| by_hand(&p, into))
                    .unwrap_or_else(no_build_here);
                format!("{url} could not be reached ({why}); by hand: {manual}")
            }
            NotFetched::Refused { url, why } => {
                format!("{url} is not the build this version pins: {why}; nothing was written")
            }
            NotFetched::Unwritten(why) => format!("dcm2niix could not be written: {why}"),
        }
    }

    /// Whether the releases could not be reached at all.
    pub(crate) fn offline(&self) -> bool {
        matches!(self, NotFetched::Unreachable { .. })
    }
}

/// The pinned build downloaded, checked and placed at `into`. Nothing is
/// written where the archive is not the pinned one.
pub(crate) fn fetch_into(into: &Path) -> Result<String, NotFetched> {
    let pin = pin_here().ok_or_else(|| NotFetched::NoBuild(no_build_here()))?;
    let url = archive_url(&releases_base(), &pin);
    let bytes = crate::supervise::fetch(&url).map_err(|why| NotFetched::Unreachable {
        url: url.clone(),
        why,
    })?;
    let binary = binary_of(&bytes, &wanted_sha256(&pin)).map_err(|why| NotFetched::Refused {
        url: url.clone(),
        why,
    })?;
    place_binary(&binary, into).map_err(NotFetched::Unwritten)?;
    Ok(url)
}

// ------------------------------------------------------------- the lookup

/// Where the engine installs a tool for itself: the registry's own folder,
/// which its account writes.
pub(crate) fn tools_dir(home: &Path) -> PathBuf {
    home.join("tools")
}

/// Where the engine's own install of the pinned dcm2niix is.
pub(crate) fn installed_at(home: &Path) -> PathBuf {
    tools_dir(home)
        .join(DCM2NIIX)
        .join(DCM2NIIX_VERSION)
        .join(DCM2NIIX)
}

/// How dcm2niix was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) path: PathBuf,
    /// given, path, beside, setup or tools.
    pub(crate) by: &'static str,
}

fn runnable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Where dcm2niix is, as `given` names it and `home` (the registry's
/// folder) keeps it: a path with a folder in it is the site's and taken as
/// it is; a bare name is looked for on the search path, and the default
/// name then beside the running nils, where the setup record names it, and
/// in the registry's tools folder. None where it is in none of them.
pub(crate) fn find_dcm2niix(
    given: &Path,
    home: Option<&Path>,
    search: Option<&std::ffi::OsStr>,
) -> Option<Found> {
    if given.is_absolute() || given.components().count() > 1 {
        return Some(Found {
            path: given.to_path_buf(),
            by: "given",
        });
    }
    let name = given.to_str()?;
    if let Some(path) = nils_pipeline::runtime::which(name, search) {
        return Some(Found { path, by: "path" });
    }
    if name != DCM2NIIX {
        return None;
    }
    let beside = std::env::current_exe()
        .ok()
        .map(|me| std::fs::canonicalize(&me).unwrap_or(me))
        .and_then(|me| me.parent().map(|d| d.join(DCM2NIIX)))
        .filter(|p| runnable(p));
    if let Some(path) = beside {
        return Some(Found { path, by: "beside" });
    }
    let recorded = crate::setup::read_state()
        .and_then(|s| s.dcm2niix)
        .map(|c| PathBuf::from(c.path))
        .filter(|p| runnable(p));
    if let Some(path) = recorded {
        return Some(Found { path, by: "setup" });
    }
    home.map(installed_at)
        .filter(|p| runnable(p))
        .map(|path| Found { path, by: "tools" })
}

/// The converter a run or a release runs: found, or the name as it was
/// given, whose run then says it is missing.
pub(crate) fn dcm2niix_for(given: &Path, home: Option<&Path>) -> (PathBuf, Option<&'static str>) {
    match find_dcm2niix(given, home, std::env::var_os("PATH").as_deref()) {
        Some(f) => (f.path, Some(f.by)),
        None => (given.to_path_buf(), None),
    }
}

/// Whether dcm2niix is here, where, and which build, with the install the
/// engine offers: `GET /api/tools` and `capabilities.tools` (2026-10-12).
pub(crate) fn dcm2niix_status(home: Option<&Path>) -> Value {
    let found = find_dcm2niix(
        Path::new(DCM2NIIX),
        home,
        std::env::var_os("PATH").as_deref(),
    );
    let checked = found
        .as_ref()
        .map(|f| nils_release::bids::convert::Converter::find(&f.path));
    let (present, version, problem) = match &checked {
        Some(Ok(c)) => (true, Some(c.version.clone()), None),
        Some(Err(e)) => (false, None, Some(e.clone())),
        None => (false, None, None),
    };
    let pin = pin_here();
    let into = home.map(installed_at);
    json!({
        "name": DCM2NIIX,
        "present": present,
        "path": found.as_ref().map(|f| f.path.display().to_string()),
        "found_by": found.as_ref().map(|f| f.by),
        "version": version,
        "pinned": DCM2NIIX_VERSION,
        "current": version.as_deref().is_some_and(|v| v.contains(DCM2NIIX_VERSION)),
        "problem": problem,
        "install": {
            "door": nils_pipeline::failure::DCM2NIIX_INSTALL_DOOR,
            "command": nils_pipeline::failure::DCM2NIIX_INSTALL_COMMAND,
            "grant": nils_pipeline::failure::INSTALL_GRANT,
            "available": pin.is_some() && into.is_some(),
            "into": into.as_ref().map(|p| p.display().to_string()),
            "source": pin.map(|p| archive_url(&releases_base(), &p)),
            "sha256": pin.map(|p| p.sha256),
            "why": pin.is_none().then(no_build_here),
        },
    })
}

/// How long the status a capabilities answer carries is kept: asking the
/// converter its version starts a process, and the desk asks often.
const STATUS_KEPT: std::time::Duration = std::time::Duration::from_secs(60);

static STATUS: std::sync::Mutex<Option<(std::time::Instant, PathBuf, Value)>> =
    std::sync::Mutex::new(None);

/// [`dcm2niix_status`] as `capabilities.tools` carries it, kept a minute.
pub(crate) fn capability(home: &Path) -> Value {
    let mut kept = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, of, v)) = kept.as_ref()
        && of == home
        && at.elapsed() < STATUS_KEPT
    {
        return json!({ DCM2NIIX: v });
    }
    let v = dcm2niix_status(Some(home));
    *kept = Some((std::time::Instant::now(), home.to_path_buf(), v.clone()));
    json!({ DCM2NIIX: v })
}

fn forget_status() {
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// An install refused or failed: the status a door answers it with, and
/// its body.
pub(crate) type Refused = (u16, Value);

/// The pinned dcm2niix installed into the registry's tools folder, where
/// the engine finds none: downloaded from rordenlab's releases (or the
/// mirror `NILS_DCM2NIIX_RELEASES` names), checked against the pinned
/// sha256, unpacked and asked its version. One found anywhere is left
/// alone. Nothing is written where the archive is not the pinned one, and
/// a binary that does not run here is taken away again. Audited as
/// `tool.install` under `principal`.
pub(crate) fn install(
    registry: &mut nils_registry::Registry,
    home: &Path,
    principal: &str,
) -> Result<Value, Refused> {
    let refused = |status: u16, error: String, offline: bool| -> Refused {
        (
            status,
            json!({
                "error": error, "disclosure": "safe", "offline": offline,
                "failure": nils_pipeline::failure::Cause::Dcm2niix.doc(),
            }),
        )
    };
    let before = dcm2niix_status(Some(home));
    if before["present"] == true {
        return Ok(json!({
            "installed": false,
            "why": format!(
                "dcm2niix is here already, {} ({}); it is left as it is",
                before["path"].as_str().unwrap_or_default(),
                before["found_by"].as_str().unwrap_or_default()
            ),
            "tool": before,
        }));
    }
    let into = installed_at(home);
    let source = match fetch_into(&into) {
        Ok(url) => url,
        Err(e) => {
            let status = match &e {
                NotFetched::NoBuild(_) => 409,
                NotFetched::Unreachable { .. } | NotFetched::Refused { .. } => 502,
                NotFetched::Unwritten(_) => 500,
            };
            return Err(refused(status, e.said(&into), e.offline()));
        }
    };
    let version = match nils_release::bids::convert::Converter::find(&into) {
        Ok(c) => c.version,
        Err(e) => {
            let _ = std::fs::remove_file(&into);
            return Err(refused(
                409,
                format!(
                    "dcm2niix {DCM2NIIX_VERSION} was downloaded and does not run here, so it was taken away again: {e}"
                ),
                false,
            ));
        }
    };
    // older builds this folder kept go once the pinned one runs
    if let Ok(entries) = std::fs::read_dir(tools_dir(home).join(DCM2NIIX)) {
        for e in entries.flatten() {
            if e.file_name() != DCM2NIIX_VERSION && e.file_name().to_string_lossy().starts_with('v')
            {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    forget_status();
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal,
            action: nils_registry::audit::Action::ToolInstall,
            scope: json!({"tool": DCM2NIIX, "version": DCM2NIIX_VERSION}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "path": into.display().to_string(), "source": source, "said": version,
            })),
        },
    )
    .map_err(|e| {
        (
            500,
            json!({"error": e.to_string(), "disclosure": "internal"}),
        )
    })?;
    Ok(json!({
        "installed": true,
        "source": source,
        "tool": dcm2niix_status(Some(home)),
    }))
}

/// The doors: `GET /api/tools`, what the engine runs beside itself and
/// whether it is here, and `POST /api/tools/dcm2niix/install`, an admin's
/// install of the pinned build (2026-10-12). The grants are the door
/// table's: install:see or pipelines:see to read, install:work to install.
pub(crate) fn route(
    home: &Path,
    registry: &mut nils_registry::Registry,
    principal: &str,
    get: bool,
    post: bool,
    segs: &[&str],
) -> Option<Result<crate::serve::Reply, crate::serve::Reply>> {
    use crate::serve::Reply;
    match segs {
        ["api", "tools"] if get => Some(Ok(Reply::ok(json!({
            "tools": [dcm2niix_status(Some(home))],
        })))),
        ["api", "tools", tool, "install"] if post => Some(if *tool == DCM2NIIX {
            match install(registry, home, principal) {
                Ok(done) => Ok(Reply::ok(done)),
                Err((status, body)) => {
                    let mut r = Reply::error(status, body["error"].as_str().unwrap_or_default());
                    r.body = body;
                    Err(r)
                }
            }
        } else {
            Err(Reply::error(
                404,
                format!("the engine installs dcm2niix alone, not {tool}"),
            ))
        }),
        _ => None,
    }
}

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 2] = ["GET /api/tools", "POST /api/tools/dcm2niix/install"];

/// `nils tools`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum ToolsCommand {
    /// What the engine runs beside itself, dcm2niix, and whether it is here
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Install the dcm2niix build this version pins into the registry's
    /// tools folder, where the engine finds none: downloaded from
    /// rordenlab's releases, or the mirror NILS_DCM2NIIX_RELEASES names,
    /// and checked against its sha256
    Install {
        /// The tool: dcm2niix
        tool: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `nils tools list` and `nils tools install dcm2niix`.
pub(crate) fn command(
    home: &nils_registry::home::Home,
    command: Option<ToolsCommand>,
) -> Result<(), crate::Exit> {
    let print = |v: &Value| {
        println!(
            "{}",
            serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
        );
    };
    let say = |t: &Value| {
        match t["path"].as_str().filter(|_| t["present"] == true) {
            Some(path) => println!(
                "dcm2niix   {} at {path} ({})",
                t["version"].as_str().unwrap_or("its version unknown"),
                t["found_by"].as_str().unwrap_or_default()
            ),
            None => println!(
                "dcm2niix   not found{}; {}",
                t["problem"]
                    .as_str()
                    .map(|p| format!(" ({p})"))
                    .unwrap_or_default(),
                if t["install"]["available"] == true {
                    format!(
                        "nils tools install dcm2niix installs {DCM2NIIX_VERSION} into {}",
                        t["install"]["into"].as_str().unwrap_or_default()
                    )
                } else {
                    t["install"]["why"].as_str().unwrap_or_default().to_string()
                }
            ),
        }
        println!("           this version pins {DCM2NIIX_VERSION}");
    };
    match command.unwrap_or(ToolsCommand::List { json: false }) {
        ToolsCommand::List { json } => {
            let t = dcm2niix_status(Some(home.dir()));
            if json {
                print(&json!({ "tools": [t] }));
            } else {
                say(&t);
            }
            Ok(())
        }
        ToolsCommand::Install { tool, json } => {
            if tool != DCM2NIIX {
                return Err(crate::usage(format!(
                    "nils tools install dcm2niix: the engine installs dcm2niix alone, not {tool}"
                )));
            }
            let mut registry = crate::open(home)?;
            let principal = nils_registry::principal::Principal::current().to_string();
            match install(&mut registry, home.dir(), &principal) {
                Ok(done) => {
                    if json {
                        print(&done);
                    } else {
                        match done["why"].as_str() {
                            Some(why) => println!("{why}"),
                            None => println!(
                                "installed dcm2niix {DCM2NIIX_VERSION} from {}",
                                done["source"].as_str().unwrap_or_default()
                            ),
                        }
                        say(&done["tool"]);
                    }
                    Ok(())
                }
                Err((_, body)) => Err(crate::fail(
                    body["error"]
                        .as_str()
                        .unwrap_or("not installed")
                        .to_string(),
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip archive of the files given, each deflated, as rordenlab's
    /// are made.
    pub(crate) fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
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
            let fields = |sig: u32, central: bool| {
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
                    h.extend(0u16.to_le_bytes());
                    h.extend(0u16.to_le_bytes());
                    h.extend(0u16.to_le_bytes());
                    h.extend(0o100_755u32.wrapping_shl(16).to_le_bytes());
                    h.extend(offset.to_le_bytes());
                }
                h.extend(name.as_bytes());
                h
            };
            out.extend(fields(0x0403_4b50, false));
            out.extend(&packed);
            directory.extend(fields(0x0201_4b50, true));
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

    #[test]
    fn every_platform_with_a_build_has_its_sha256_pinned() {
        assert_eq!(
            pin_for("linux", "x86_64").unwrap().archive,
            "dcm2niix_lnx.zip"
        );
        assert_eq!(
            pin_for("macos", "aarch64").unwrap().archive,
            "dcm2niix_mac.zip"
        );
        let (intel, arm) = (
            pin_for("macos", "x86_64").unwrap(),
            pin_for("macos", "aarch64").unwrap(),
        );
        assert_eq!(
            (intel.archive, intel.sha256),
            (arm.archive, arm.sha256),
            "one archive for both"
        );
        assert_eq!(pin_for("linux", "aarch64"), None, "rordenlab builds none");
        for p in DCM2NIIX_PINS {
            assert_eq!(p.sha256.len(), 64);
            assert!(p.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert_eq!(
            archive_url(DCM2NIIX_RELEASES, &pin_for("linux", "x86_64").unwrap()),
            "https://github.com/rordenlab/dcm2niix/releases/download/v1.0.20260724/dcm2niix_lnx.zip"
        );
        assert_eq!(
            archive_url("file:///m/", &pin_for("linux", "x86_64").unwrap()),
            "file:///m/v1.0.20260724/dcm2niix_lnx.zip"
        );
    }

    #[test]
    fn the_binary_comes_out_of_a_pinned_archive_and_nothing_else() {
        let body = b"#!/bin/sh\necho dcm2niix\n";
        let zip = zip_of(&[("README", b"a readme"), ("dcm2niix", body)]);
        let digest = crate::supervise::sha256_hex(&zip);
        assert_eq!(binary_of(&zip, &digest).unwrap(), body);
        let refused = binary_of(&zip, &"0".repeat(64)).unwrap_err();
        assert!(refused.contains("sha256"), "{refused}");
        let none = zip_of(&[("README", b"x")]);
        let refused = binary_of(&none, &crate::supervise::sha256_hex(&none)).unwrap_err();
        assert!(refused.contains("holds no dcm2niix"), "{refused}");
        assert!(zip_member(b"not a zip at all", "dcm2niix").is_err());
        // an archive cut short is refused rather than read past its end
        let cut = &zip[..zip.len() - 30];
        assert!(zip_member(cut, "dcm2niix").is_err());
    }

    #[test]
    fn a_name_is_looked_for_and_a_path_is_taken_as_given() {
        let root = std::env::temp_dir().join(format!("nils-tools-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let on_path = root.join("bin");
        let home = root.join("registry");
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        // a path is the site's, found or not
        let given = find_dcm2niix(Path::new("/opt/x/dcm2niix"), Some(&home), None).unwrap();
        assert_eq!(
            (given.path, given.by),
            (PathBuf::from("/opt/x/dcm2niix"), "given")
        );
        // the engine's own install, where nothing else has one
        let mine = installed_at(&home);
        place_binary(b"#!/bin/sh\n", &mine).unwrap();
        let search = std::ffi::OsString::from(on_path.as_os_str());
        let found = find_dcm2niix(Path::new(DCM2NIIX), Some(&home), Some(&search));
        // beside the test binary or on record there is none, so it is the
        // tools folder's
        if let Some(f) = &found
            && f.by != "beside"
            && f.by != "setup"
        {
            assert_eq!((f.path.clone(), f.by), (mine.clone(), "tools"));
        }
        // the search path comes before it
        place_binary(b"#!/bin/sh\n", &on_path.join(DCM2NIIX)).unwrap();
        let found = find_dcm2niix(Path::new(DCM2NIIX), Some(&home), Some(&search)).unwrap();
        assert_eq!((found.path, found.by), (on_path.join(DCM2NIIX), "path"));
        // another name is looked for on the search path alone
        assert!(find_dcm2niix(Path::new("dcm2niix-old"), Some(&home), Some(&search)).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
