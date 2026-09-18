// SPDX-License-Identifier: AGPL-3.0-only
//! `nils update`: replace this binary with the newest release, and refresh
//! the packs beside it. The supervisor (§10.4) updates the parts of a whole
//! deployment from a signed channel; this is the smaller thing a person on
//! their own machine wants, the other half of the one line installer.
//!
//! A release is a directory of files named after the target, beside a
//! `SHA256SUMS` that names every one of them and a `VERSION` holding the
//! version on one line. On GitHub that is
//! `https://github.com/kineuro/nils/releases/download/v<version>/<file>`,
//! with `latest/download/<file>` naming the newest; a deployment that
//! publishes its own passes `--channel` or sets `NILS_RELEASES`, and lays
//! the same two paths out itself.
use std::path::{Path, PathBuf};

use clap::Args;

use crate::supervise::{fetch, sha256_hex};
use crate::{Exit, fail, usage};

/// Where releases come from when nothing says otherwise.
pub(crate) const RELEASES: &str = "https://github.com/kineuro/nils/releases";

/// This binary's version, which is what an update is measured against.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Say what an update would do and install nothing
    #[arg(long)]
    check: bool,
    /// Install exactly this version instead of the newest
    #[arg(long, value_name = "VERSION")]
    version: Option<String>,
    /// Install into this directory instead of over the running binary
    #[arg(long, value_name = "DIR")]
    to: Option<PathBuf>,
    /// Where releases come from; NILS_RELEASES sets the same thing
    #[arg(long, value_name = "URL")]
    channel: Option<String>,
    /// Update every part `nils setup` installed: the engine first, then the
    /// rest by the newest binary, with the versions its release pins
    #[arg(long)]
    all: bool,
}

/// The release's name for one part and one target. The engine publishes
/// `nils-<target>`, the desk `nils-desk-<target>`, and Windows adds `.exe`.
pub(crate) fn part_file(part: &str, target: &str) -> String {
    if target.starts_with("windows-") {
        format!("{part}-{target}.exe")
    } else {
        format!("{part}-{target}")
    }
}

/// The release's name for a target: the six `engine-build.yml` publishes.
pub(crate) fn file_of(target: &str) -> String {
    if target.starts_with("windows-") {
        format!("nils-{target}.exe")
    } else {
        format!("nils-{target}")
    }
}

/// This host's target, spelled the way a release file is: `arm64` rather
/// than the compiler's `aarch64`, and `macos` rather than its `darwin`.
pub(crate) fn host_target() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{}-{arch}", std::env::consts::OS)
}

/// A version as numbers and, when it is a pre-release, what follows the hyphen.
fn parts(v: &str) -> (Vec<u64>, Option<String>) {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre.to_string())),
        None => (v, None),
    };
    (
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
        pre,
    )
}

/// One pre-release identifier against another: numbers as numbers, the rest
/// as text, and the longer list wins when it agrees so far (`alpha.2` over
/// `alpha`).
fn pre_order(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut left, mut right) = (a.split('.'), b.split('.'));
    loop {
        match (left.next(), right.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    _ => x.cmp(y),
                };
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Whether `candidate` is a later version than `than`: the numbers left to
/// right, and then a release ahead of the pre-releases that led to it.
pub(crate) fn newer(candidate: &str, than: &str) -> bool {
    let (a, a_pre) = parts(candidate);
    let (b, b_pre) = parts(than);
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    match (a_pre, b_pre) {
        (None, None) => false,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(x), Some(y)) => pre_order(&x, &y) == std::cmp::Ordering::Greater,
    }
}

/// The checksum a `SHA256SUMS` file gives one name, in either of the two
/// forms the tools write (`<sum>  <name>` and `<sum> *<name>`).
fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (sum, rest) = line.split_once(char::is_whitespace)?;
        let named = rest.trim().trim_start_matches('*');
        (named == name).then(|| sum.trim().to_string())
    })
}

fn base_of(args: &UpdateArgs) -> String {
    engine_base(args.channel.as_deref())
}

/// Where the engine's releases come from: what was asked for, else what the
/// environment names, else GitHub.
pub(crate) fn engine_base(channel: Option<&str>) -> String {
    let base = channel
        .map(str::to_string)
        .or_else(|| std::env::var("NILS_RELEASES").ok())
        .unwrap_or_else(|| RELEASES.to_string());
    base.trim_end_matches('/').to_string()
}

/// Where the desk's releases come from. A channel asked for on the command
/// line covers every part, which is what a deployment publishing its own
/// wants and what the tests use; otherwise the desk has its own repository.
pub(crate) fn desk_base(channel: Option<&str>) -> String {
    let base = channel
        .map(str::to_string)
        .or_else(|| std::env::var("NILS_DESK_RELEASES").ok())
        .or_else(|| std::env::var("NILS_RELEASES").ok())
        .unwrap_or_else(|| crate::setup::DESK_RELEASES.to_string());
    base.trim_end_matches('/').to_string()
}

/// A file of one release: `<base>/download/v<version>/<file>`.
fn asset(base: &str, version: &str, file: &str) -> String {
    format!("{base}/download/v{version}/{file}")
}

/// The version the newest release names.
///
/// A repository whose releases are all pre-releases, which is every release
/// before 1.0.0, has no `latest`: GitHub's own skips a pre-release, so
/// `latest/download/VERSION` answers 404 every time and the answer is only
/// ever in the listing. So the listing is asked first, which is one request
/// instead of two and, when the API refuses, a refusal that can be told from
/// a file that is not there.
///
/// A channel that is not GitHub has no listing and keeps its plain `VERSION`
/// file beside its binaries, read exactly as before; so does a GitHub
/// repository whose listing names no release, which is what a repository
/// that does publish a latest release looks like to the API when the
/// listing cannot be had.
pub(crate) fn newest_version(base: &str) -> Result<String, Exit> {
    if let Some(repo) = github_repo(base) {
        match newest_tag(repo) {
            Ok(Some(version)) => return Ok(version),
            // A refusal is said as a refusal, and never as the 404 of the
            // other URL, which is what the file below would answer with.
            Err(refused) => return Err(fail(refused)),
            Ok(None) => {}
        }
    }
    version_file(base)
}

/// The version the one line `VERSION` file of the newest release names.
fn version_file(base: &str) -> Result<String, Exit> {
    let url = format!("{base}/latest/download/VERSION");
    match fetch(&url) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            if text.is_empty() || text.lines().count() > 1 {
                return Err(fail(format!("{url} does not hold a version on one line")));
            }
            Ok(text.trim_start_matches('v').to_string())
        }
        Err(e) => Err(fail(format!("no release to update to: {e}"))),
    }
}

/// The `owner/name` of a base that is a GitHub repository's releases, and
/// `None` for a channel of a deployment's own.
fn github_repo(base: &str) -> Option<&str> {
    base.strip_prefix("https://github.com/")?
        .strip_suffix("/releases")
}

/// The newest tag of a GitHub repository, pre-release or not, from the API:
/// `Ok(None)` where the listing names no release and the `VERSION` file is
/// the answer, `Err` where the API refused, in words that say so.
///
/// The request carries no credential. The engine reads none for GitHub from
/// anywhere, and the limit this runs into is the one for an address without
/// a token.
fn newest_tag(repo: &str) -> Result<Option<String>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=30");
    // A request that never arrived is not a refusal: the file is asked next,
    // and its own words are what a person is told.
    let Ok(answer) = ask(&url) else {
        return Ok(None);
    };
    newest_answered(&url, &answer)
}

/// What one answer from the releases API means: the newest version it names,
/// nothing, or a refusal in the words to report.
fn newest_answered(url: &str, answer: &Answer) -> Result<Option<String>, String> {
    if answer.refused() {
        return Err(answer.refusal(url));
    }
    Ok(newest_of(&answer.body))
}

/// One answer from GitHub's API, kept whole: a refusal is told from a
/// listing by the status and the headers, which [`fetch`] throws away.
#[derive(Default)]
struct Answer {
    status: u16,
    body: String,
    /// `x-ratelimit-limit`: how many requests an hour this address has.
    limit: Option<String>,
    /// `x-ratelimit-remaining`: how many of them are left.
    remaining: Option<String>,
    /// `x-ratelimit-reset`: when the hour begins again, in seconds since
    /// the epoch.
    reset: Option<String>,
    /// `retry-after`: the seconds to wait, which a secondary limit sends
    /// instead.
    retry_after: Option<String>,
}

impl Answer {
    /// Whether the API refused rather than answered: the two statuses it
    /// refuses with.
    fn refused(&self) -> bool {
        matches!(self.status, 403 | 429)
    }

    /// Whether the refusal is the rate limit, by the headers it comes with
    /// or the sentence GitHub sends.
    fn rate_limited(&self) -> bool {
        self.remaining.as_deref() == Some("0")
            || self.retry_after.is_some()
            || self.message().is_some_and(|m| {
                let m = m.to_lowercase();
                m.contains("rate limit") || m.contains("too many requests")
            })
    }

    /// The sentence a refusal's body holds, where it holds one.
    fn message(&self) -> Option<String> {
        let body: serde_json::Value = serde_json::from_str(&self.body).ok()?;
        Some(body["message"].as_str()?.trim().to_string())
    }

    /// When the requests come back, said as a person reads a clock: the
    /// hour's own end where the headers give it, else the wait a secondary
    /// limit asks for.
    fn comes_back(&self) -> Option<String> {
        if let Some(at) = self.reset.as_ref().and_then(|s| s.trim().parse().ok())
            && let Ok(when) = jiff::Timestamp::from_second(at)
        {
            return Some(format!("at {} UTC", when.strftime("%Y-%m-%d %H:%M")));
        }
        let seconds: i64 = self.retry_after.as_ref()?.trim().parse().ok()?;
        Some(format!("in {seconds} seconds"))
    }

    /// The refusal as it is reported: the limit that was reached and when it
    /// resets, never the URL that always answers 404. A refusal for another
    /// reason keeps GitHub's own words.
    fn refusal(&self, url: &str) -> String {
        let mut said = format!(
            "the release listing was refused: {url}: http status {}",
            self.status
        );
        if !self.rate_limited() {
            if let Some(message) = self.message() {
                said.push_str(&format!(": {message}"));
            }
            return said;
        }
        match &self.limit {
            Some(limit) => said.push_str(&format!(
                ": GitHub allows {limit} requests an hour from one address and this hour's are used"
            )),
            None => said
                .push_str(": GitHub's limit of requests an hour from one address has been reached"),
        }
        match self.comes_back() {
            Some(when) => said.push_str(&format!(", and they come back {when}")),
            None => said.push_str(", and it says nothing of when they come back"),
        }
        said
    }
}

/// Ask the API for one URL, keeping the status and the headers a refusal is
/// read from. [`fetch`] keeps neither, so a 403 that says how long the wait
/// is would come back as a line about a status and nothing else.
fn ask(url: &str) -> Result<Answer, String> {
    let response = ureq::get(url)
        .header("accept", "application/vnd.github+json")
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_string())
    };
    let (limit, remaining, reset, retry_after) = (
        header("x-ratelimit-limit"),
        header("x-ratelimit-remaining"),
        header("x-ratelimit-reset"),
        header("retry-after"),
    );
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(Answer {
        status,
        body,
        limit,
        remaining,
        reset,
        retry_after,
    })
}

/// The highest version among the releases a listing names, drafts left out.
/// GitHub lists releases in no order a version can rely on: after
/// 1.0.0-alpha.11 it put alpha.9 first, and taking the first release kept
/// every install at alpha.9.
fn newest_of(listing: &str) -> Option<String> {
    let releases: serde_json::Value = serde_json::from_str(listing).ok()?;
    releases
        .as_array()?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| r["tag_name"].as_str())
        .map(|tag| tag.trim_start_matches('v').to_string())
        .filter(|tag| !tag.is_empty())
        .reduce(|best, tag| if newer(&tag, &best) { tag } else { best })
}

/// Fetch one file of a release and check it against that release's sums.
pub(crate) fn fetch_checked(base: &str, version: &str, file: &str) -> Result<Vec<u8>, Exit> {
    let sums = fetch(&asset(base, version, "SHA256SUMS"))
        .map_err(|e| fail(format!("the release names no checksums: {e}")))?;
    let sums = String::from_utf8_lossy(&sums).to_string();
    let want = sum_for(&sums, file).ok_or_else(|| {
        fail(format!(
            "the release {version} has no {file}: this platform is not one it was built for"
        ))
    })?;
    let bytes = fetch(&asset(base, version, file)).map_err(|e| fail(e.to_string()))?;
    let got = sha256_hex(&bytes);
    if got != want {
        return Err(fail(format!(
            "{file} does not match the release's checksum: {got} against {want}"
        )));
    }
    Ok(bytes)
}

/// Whether a directory takes a file from this user, asked by writing one.
pub(crate) fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".nils-write-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Put `bytes` at `path`, executable, without a moment where the path is
/// half a binary: beside it first, then one rename over.
pub(crate) fn install_binary(path: &Path, bytes: &[u8]) -> Result<(), Exit> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let fresh = dir.join(format!(".nils-update-{}", std::process::id()));
    std::fs::write(&fresh, bytes).map_err(|e| fail(format!("{}: {e}", fresh.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fresh, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| fail(format!("{}: {e}", fresh.display())))?;
    }
    // Windows will not rename over a running image; the old one steps aside
    // first and is forgotten on the next start.
    if std::fs::rename(&fresh, path).is_err() {
        let aside = path.with_extension("old");
        let _ = std::fs::remove_file(&aside);
        if path.exists() {
            std::fs::rename(path, &aside).map_err(|e| fail(format!("{}: {e}", path.display())))?;
        }
        std::fs::rename(&fresh, path).map_err(|e| {
            let _ = std::fs::rename(&aside, path);
            fail(format!("{}: {e}", path.display()))
        })?;
    }
    Ok(())
}

/// The packs the release carries, over the directory in use. The tarball
/// holds one `packs/` directory, so it is unpacked beside the old one and
/// the two are swapped.
pub(crate) fn refresh_packs(base: &str, version: &str, dir: &Path) -> Result<String, String> {
    let parent = dir.parent().ok_or("the pack directory has no parent")?;
    if !writable(parent) {
        return Err(format!("{} is not writable by this user", parent.display()));
    }
    let bytes = fetch_checked(base, version, "packs.tar.gz").map_err(|e| e.message)?;
    let staging = parent.join(format!(".nils-packs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    archive
        .unpack(&staging)
        .map_err(|e| format!("unpacking the packs: {e}"))?;
    let fresh = staging.join("packs");
    if !fresh.is_dir() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("the release's packs.tar.gz holds no packs directory".to_string());
    }
    let aside = parent.join(format!(".nils-packs-old-{}", std::process::id()));
    let swapped = swap_packs(&fresh, dir, &aside);
    let _ = std::fs::remove_dir_all(&staging);
    swapped
}

/// Put the release's packs in `dir`. A pack the release does not carry, one
/// a deployment wrote for scans of its own, is kept: an update replaces only
/// the packs it brings. Everything is a rename inside one parent, so nothing
/// is copied, and what could not be put back is left where it was set aside
/// and never removed.
fn swap_packs(fresh: &Path, dir: &Path, aside: &Path) -> Result<String, String> {
    let _ = std::fs::remove_dir_all(aside);
    let mut own: Vec<std::ffi::OsString> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .filter(|name| std::fs::symlink_metadata(fresh.join(name)).is_err())
                .collect()
        })
        .unwrap_or_default();
    own.sort();
    if dir.exists() {
        std::fs::rename(dir, aside).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if let Err(e) = std::fs::rename(fresh, dir) {
        if aside.exists() {
            let _ = std::fs::rename(aside, dir);
        }
        return Err(format!("{}: {e}", dir.display()));
    }
    let names = |list: &[std::ffi::OsString]| {
        list.iter()
            .map(|n| n.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let stranded: Vec<_> = own
        .iter()
        .filter(|name| std::fs::rename(aside.join(name), dir.join(name)).is_err())
        .cloned()
        .collect();
    let said = format!("the packs in {} are the release's", dir.display());
    if !stranded.is_empty() {
        return Ok(format!(
            "{said}; the deployment's own {} could not be put back and are in {}",
            names(&stranded),
            aside.display()
        ));
    }
    let _ = std::fs::remove_dir_all(aside);
    if own.is_empty() {
        Ok(said)
    } else {
        Ok(format!("{said}, and its own are kept: {}", names(&own)))
    }
}

/// The share directory of the prefix a binary sits in, made if it can be:
/// `~/.local/share/nils/packs` for `~/.local/bin/nils`, and
/// `/usr/local/share/nils/packs` for `/usr/local/bin/nils`. Both are places
/// the engine looks with no flag, which is the whole point of choosing them.
fn beside_the_binary(binary: &Path) -> Option<PathBuf> {
    let prefix = binary.parent().and_then(Path::parent)?;
    let share = prefix.join("share").join("nils");
    if std::fs::create_dir_all(&share).is_ok() && writable(&share) {
        Some(share.join("packs"))
    } else {
        None
    }
}

/// Where this update would write, and whether it may.
fn destination(args: &UpdateArgs) -> Result<PathBuf, Exit> {
    if let Some(dir) = &args.to {
        std::fs::create_dir_all(dir).map_err(|e| usage(format!("{}: {e}", dir.display())))?;
        let name = if cfg!(windows) { "nils.exe" } else { "nils" };
        return Ok(dir.join(name));
    }
    let me = std::env::current_exe()
        .map_err(|e| fail(format!("this binary cannot say where it is: {e}")))?;
    Ok(std::fs::canonicalize(&me).unwrap_or(me))
}

/// What a binary `nils update --all` installed is started with: the version it
/// replaced, so it updates the parts and does not replace itself again.
pub(crate) const HANDED_OVER: &str = "NILS_UPDATE_HANDED_OVER";

/// Hand the rest of an update to the binary just installed, with this
/// process's arguments and the version it replaced. On Unix this process
/// becomes it; elsewhere it runs to its end and this one exits with its
/// status. Returns only when the new binary could not be started.
fn hand_over(path: &Path) -> std::io::Error {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let mut command = std::process::Command::new(path);
    command
        .args(std::env::args_os().skip(1))
        .env(HANDED_OVER, VERSION);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.exec()
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => e,
        }
    }
}

pub(crate) fn update(home: &nils_registry::home::Home, args: UpdateArgs) -> Result<(), Exit> {
    let base = base_of(&args);
    // The engine first, then every other part: the parts are the newest
    // binary's to update, since this one's pins are the release it came with
    // and a part a later release adds is unknown to it. A binary this update
    // installed carries on from here, and its services start again, since the
    // engine's binary is new.
    if args.all && !args.check {
        if let Some(from) = std::env::var_os(HANDED_OVER) {
            println!(
                "nils {VERSION} carries on the update from {}",
                from.to_string_lossy()
            );
            crate::setup::update_all(args.channel.as_deref())?;
            crate::setup::restart_after_update(args.channel.as_deref());
            return Ok(());
        }
        crate::setup::setup_recorded()?;
        // On an install whose services are the machine's own, replacing the
        // parts' files is root's work, and the account the supervisor runs
        // as asks the helper to run this same command as root.
        if let Some(done) = crate::setup::update_by_helper() {
            return done;
        }
    }
    let wanted = match &args.version {
        Some(v) => v.trim().trim_start_matches('v').to_string(),
        None => newest_version(&base)?,
    };
    let asked = args.version.is_some() || args.to.is_some();
    if !asked && !newer(&wanted, VERSION) {
        println!("nils {VERSION} is the newest release");
        if args.all && !args.check && crate::setup::update_all(args.channel.as_deref())? {
            crate::setup::restart_after_update(args.channel.as_deref());
        }
        return Ok(());
    }
    let target = host_target();
    let file = file_of(&target);
    let path = destination(&args)?;

    if args.check {
        println!("nils {wanted} is the release; this binary is {VERSION}");
        println!("  it would install {file} at {}", path.display());
        return Ok(());
    }

    let dir = path.parent().unwrap_or(Path::new("."));
    if !writable(dir) {
        return Err(fail(format!(
            "{} is not writable by this user; run one of\n  sudo nils update\n  nils update --to ~/.local/bin",
            dir.display()
        )));
    }

    let bytes = fetch_checked(&base, &wanted, &file)?;
    install_binary(&path, &bytes)?;
    println!("nils {wanted} at {} (was {VERSION})", path.display());
    let serves = crate::setup::record_engine_version(&path, &wanted);

    // The packs go with the binary when the ones in use may be replaced; a
    // deployment that keeps its packs elsewhere is left alone and told so.
    //
    // Where there are none at all, they are taken now. An engine installed
    // before the wizard fetched packs has none, and an update that moves
    // the binary and leaves it unable to say what a scan is has not
    // updated much.
    match crate::pack_dir(home, None) {
        Ok(packs) => match refresh_packs(&base, &wanted, &packs) {
            Ok(said) => println!("{said}"),
            Err(why) => println!("the packs were left alone: {why}"),
        },
        Err(_) => match beside_the_binary(&path) {
            Some(packs) => match refresh_packs(&base, &wanted, &packs) {
                Ok(_) => println!(
                    "the packs are at {}, where there were none",
                    packs.display()
                ),
                Err(why) => println!("there are no packs, and none could be taken: {why}"),
            },
            None => println!("there are no packs, and nowhere beside the binary to put them"),
        },
    }

    // Every other part is the new binary's to update, with the versions its
    // release pins; where it cannot be started, this one does what it can.
    if args.all {
        let why = hand_over(&path);
        println!("nils {wanted} could not be started ({why}), so nils {VERSION} updates the parts");
        crate::setup::update_all(args.channel.as_deref())?;
        crate::setup::restart_after_update(args.channel.as_deref());
        return Ok(());
    }

    // A running engine keeps the binary it started with until it is
    // restarted, so an update that stopped here would change nothing until
    // the next boot.
    if serves {
        crate::setup::restart_after_update(args.channel.as_deref());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_release_is_the_highest_version_not_the_first_listed() {
        // the order GitHub gave after 1.0.0-alpha.11 was released
        let listing = r#"[
            {"tag_name": "v1.0.0-alpha.9", "draft": false},
            {"tag_name": "v1.0.0-alpha.11", "draft": false},
            {"tag_name": "v1.0.0-alpha.10", "draft": false},
            {"tag_name": "v1.0.0-alpha.8", "draft": false}
        ]"#;
        assert_eq!(newest_of(listing).as_deref(), Some("1.0.0-alpha.11"));
        let with_draft = r#"[
            {"tag_name": "v1.0.0-alpha.11", "draft": false},
            {"tag_name": "v1.0.0-alpha.12", "draft": true}
        ]"#;
        assert_eq!(
            newest_of(with_draft).as_deref(),
            Some("1.0.0-alpha.11"),
            "a draft is not a release"
        );
        let released = r#"[{"tag_name": "v1.0.0-alpha.11"}, {"tag_name": "v1.0.0"}]"#;
        assert_eq!(newest_of(released).as_deref(), Some("1.0.0"));
        assert_eq!(newest_of("[]"), None);
        assert_eq!(newest_of("not json"), None);
    }

    #[test]
    fn a_lookup_the_api_refuses_says_it_was_refused_and_when_the_requests_come_back() {
        let url = "https://api.github.com/repos/kineuro/nils/releases?per_page=30";
        // 2026-09-18 09:20 UTC, as the header gives it: seconds since the epoch
        let refused = Answer {
            status: 403,
            body: r#"{"message": "API rate limit exceeded for this address."}"#.to_string(),
            limit: Some("60".to_string()),
            remaining: Some("0".to_string()),
            reset: Some("1789723200".to_string()),
            ..Answer::default()
        };
        let said = newest_answered(url, &refused).unwrap_err();
        assert!(said.contains("refused"), "{said}");
        assert!(said.contains("60 requests an hour"), "{said}");
        assert!(said.contains("at 2026-09-18 09:20 UTC"), "{said}");
        assert!(
            !said.contains("latest/download/VERSION") && !said.contains("404"),
            "the URL that always answers 404 is not the reason: {said}"
        );

        // a secondary limit sends the wait instead of the hour's end
        let secondary = Answer {
            status: 429,
            body: r#"{"message": "You have exceeded a secondary rate limit."}"#.to_string(),
            retry_after: Some("60".to_string()),
            ..Answer::default()
        };
        let said = newest_answered(url, &secondary).unwrap_err();
        assert!(said.contains("in 60 seconds"), "{said}");

        // a refusal for another reason keeps GitHub's own words
        let other = Answer {
            status: 403,
            body: r#"{"message": "Resource not accessible"}"#.to_string(),
            ..Answer::default()
        };
        let said = newest_answered(url, &other).unwrap_err();
        assert!(
            said.ends_with("http status 403: Resource not accessible"),
            "{said}"
        );

        // and an answer is still read as the listing it is
        let listing = Answer {
            status: 200,
            body: r#"[{"tag_name": "v1.0.0-alpha.35", "draft": false}]"#.to_string(),
            ..Answer::default()
        };
        assert_eq!(
            newest_answered(url, &listing).unwrap().as_deref(),
            Some("1.0.0-alpha.35")
        );
    }

    #[test]
    fn a_channel_that_is_not_github_reads_the_version_file_it_publishes() {
        let root = std::env::temp_dir().join(format!("nils-channel-{}", std::process::id()));
        let dir = root.join("latest").join("download");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("VERSION"), "v1.0.0-alpha.7\n").unwrap();
        let base = format!("file://{}", root.display());
        assert_eq!(github_repo(&base), None, "a channel of its own has no API");
        assert_eq!(newest_version(&base).ok().as_deref(), Some("1.0.0-alpha.7"));

        std::fs::write(dir.join("VERSION"), "1.0.0\nand more\n").unwrap();
        let Err(stopped) = newest_version(&base) else {
            panic!("two lines are not a version")
        };
        assert!(
            stopped.message.contains("a version on one line"),
            "{}",
            stopped.message
        );

        std::fs::remove_file(dir.join("VERSION")).unwrap();
        let Err(stopped) = newest_version(&base) else {
            panic!("there is no VERSION file to read")
        };
        assert!(
            stopped.message.starts_with("no release to update to"),
            "{}",
            stopped.message
        );
        assert_eq!(
            github_repo("https://github.com/kineuro/nils/releases"),
            Some("kineuro/nils"),
            "a GitHub base is asked of the listing first"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_release_is_newer_by_its_numbers_then_by_its_pre_release() {
        assert!(newer("1.0.1", "1.0.0"));
        assert!(newer("1.10.0", "1.2.0"));
        assert!(newer("1.0.0", "1.0.0-alpha.1"));
        assert!(newer("1.0.0-alpha.2", "1.0.0-alpha.1"));
        assert!(newer("1.0.0-alpha.10", "1.0.0-alpha.2"));
        assert!(newer("1.0.0-beta.1", "1.0.0-alpha.9"));
        // a build made between two releases, as a development channel serves
        // it: after the release it is built from, before the next one
        assert!(newer("1.0.0-alpha.35.dev.1", "1.0.0-alpha.35"));
        assert!(newer("1.0.0-alpha.35.dev.2", "1.0.0-alpha.35.dev.1"));
        assert!(newer("1.0.0-alpha.36", "1.0.0-alpha.35.dev.9"));
        assert!(!newer("1.0.0-alpha.35", "1.0.0-alpha.35.dev.1"));
        assert!(newer("v1.0.1", "1.0.0"), "a leading v is not a version");
        assert!(!newer("1.0.0", "1.0.0"));
        assert!(!newer("1.0.0-alpha.1", "1.0.0"));
        assert!(!newer("0.9.9", "1.0.0"));
        assert!(!newer("1.2.0", "1.10.0"), "numbers, not text");
    }

    #[test]
    fn the_file_of_a_target_is_the_one_the_release_publishes() {
        assert_eq!(file_of("linux-x86_64"), "nils-linux-x86_64");
        assert_eq!(file_of("macos-arm64"), "nils-macos-arm64");
        assert_eq!(file_of("windows-x86_64"), "nils-windows-x86_64.exe");
        let target = host_target();
        assert!(!target.contains("aarch64"), "{target} spells arm64");
    }

    #[test]
    fn an_update_replaces_the_releases_packs_and_keeps_a_deployments_own() {
        let root = std::env::temp_dir().join(format!("nils-packs-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (fresh, dir, aside) = (root.join("fresh"), root.join("packs"), root.join("aside"));
        for (path, text) in [
            (fresh.join("mri/pack.toml"), "the release's"),
            (fresh.join("clinical/pack.toml"), "the release's"),
            (dir.join("mri/pack.toml"), "the one before"),
            (dir.join("mri/gone.toml"), "no longer in the release"),
            (dir.join("clinical/pack.toml"), "the one before"),
            (dir.join("lab/pack.toml"), "the deployment's own"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        let said = swap_packs(&fresh, &dir, &aside).unwrap();
        assert!(said.ends_with("its own are kept: lab"), "{said}");
        let read = |p: &str| std::fs::read_to_string(dir.join(p)).unwrap();
        assert_eq!(read("mri/pack.toml"), "the release's");
        assert_eq!(read("clinical/pack.toml"), "the release's");
        assert!(
            !dir.join("mri/gone.toml").exists(),
            "a pack the release brings is the release's, whole"
        );
        assert_eq!(read("lab/pack.toml"), "the deployment's own");
        assert!(!aside.exists() && !fresh.exists());

        // where there were no packs, the release's are put in place
        let fresh = root.join("fresh-again");
        std::fs::create_dir_all(fresh.join("mri")).unwrap();
        let none = root.join("none");
        let said = swap_packs(&fresh, &none, &aside).unwrap();
        assert!(!said.contains("kept"), "{said}");
        assert!(none.join("mri").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_part_is_named_after_itself_and_its_target() {
        assert_eq!(part_file("nils", "linux-x86_64"), "nils-linux-x86_64");
        assert_eq!(
            part_file("nils-desk", "macos-arm64"),
            "nils-desk-macos-arm64"
        );
        assert_eq!(
            part_file("nils-desk", "windows-x86_64"),
            "nils-desk-windows-x86_64.exe"
        );
        // The engine's own name is the same either way.
        assert_eq!(part_file("nils", "macos-arm64"), file_of("macos-arm64"));
    }

    #[test]
    fn the_desks_releases_are_its_own_unless_a_channel_says_otherwise() {
        assert_eq!(desk_base(Some("file:///tmp/rel/")), "file:///tmp/rel");
        assert_eq!(engine_base(Some("file:///tmp/rel/")), "file:///tmp/rel");
        assert_ne!(desk_base(None), engine_base(None));
        assert!(desk_base(None).ends_with("nils-desk/releases"));
    }

    #[test]
    fn the_sums_file_is_read_in_either_form() {
        let sums = "aa11  nils-linux-x86_64\nbb22 *packs.tar.gz\n";
        assert_eq!(sum_for(sums, "nils-linux-x86_64").as_deref(), Some("aa11"));
        assert_eq!(sum_for(sums, "packs.tar.gz").as_deref(), Some("bb22"));
        assert_eq!(sum_for(sums, "nils-macos-arm64"), None);
    }
}
