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

use crate::releases::PartRelease;
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
    /// Update only this part, against its own newest release; repeatable
    /// (engine, desk, assistant, kvasir, rules)
    #[arg(long = "part", value_name = "PART")]
    parts: Vec<String>,
}

impl UpdateArgs {
    /// Whether the parts beside the engine are in this update.
    fn takes_parts(&self) -> bool {
        self.all || !self.parts.is_empty()
    }

    /// Whether the engine's own binary is in this update.
    fn takes_engine(&self) -> bool {
        self.parts.is_empty() || self.parts.iter().any(|p| p == "engine")
    }

    /// The parts beside the engine named, or none for every one.
    fn only(&self) -> Vec<String> {
        self.parts.clone()
    }
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

/// Whether `candidate` is a later version than `than`: the numbers left to
/// right, and then a release ahead of the pre-releases that led to it. The
/// order is the pack crate's, which reads a pack's range of engines by it
/// (pack contract 10), so a release and a range never disagree.
pub(crate) fn newer(candidate: &str, than: &str) -> bool {
    nils_pack::engines::newer(candidate, than)
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

/// A file of the release a tag names: `<base>/download/<tag>/<file>`. The
/// engine's tags are `v<version>`; a pack's own are `pack-<name>-v<version>`.
pub(crate) fn asset_at(base: &str, tag: &str, file: &str) -> String {
    format!("{base}/download/{tag}/{file}")
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

/// Every release a base publishes, newest first. A GitHub repository's
/// listing names them all (the thirty newest, which is more than a desk that
/// waits for its engine ever passes over); a channel of a deployment's own
/// keeps only the `VERSION` file of its newest, so that one is all it has.
pub(crate) fn versions(base: &str) -> Result<Vec<String>, Exit> {
    if let Some(repo) = github_repo(base) {
        let url = format!("https://api.github.com/repos/{repo}/releases?per_page=30");
        if let Ok(answer) = ask(&url) {
            if answer.refused() {
                return Err(fail(answer.refusal(&url)));
            }
            let listed = listed(&answer.body);
            if !listed.is_empty() {
                return Ok(listed);
            }
        }
    }
    version_file(base).map(|v| vec![v])
}

/// Every release of one kind a GitHub base lists, newest first: the versions
/// of the tags that start with `prefix` (`pack-mri-v` for the MRI pack's own
/// releases). `None` for a channel of a deployment's own, which has no
/// listing; a refusal is said as one. A pack's releases share the listing
/// with the engine's, so it is read a hundred deep, the most one request
/// gives, where the engine's newest is always near the top.
pub(crate) fn tagged(base: &str, prefix: &str) -> Option<Result<Vec<String>, String>> {
    let repo = github_repo(base)?;
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=100");
    Some(match ask(&url) {
        Ok(answer) if answer.refused() => Err(answer.refusal(&url)),
        Ok(answer) => Ok(listed_as(&answer.body, Some(prefix))),
        Err(e) => Err(format!("the release listing could not be read: {e}")),
    })
}

/// A file a channel may or may not have: `Ok(None)` where it answers that
/// there is no such file, which is a channel that publishes none, and an
/// error only where it could not be asked.
pub(crate) fn fetch_if_there(url: &str) -> Result<Option<Vec<u8>>, String> {
    if let Some(path) = url.strip_prefix("file://") {
        return match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{url}: {e}")),
        };
    }
    let response = ureq::get(url)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    match response.status().as_u16() {
        404 => Ok(None),
        status if (200..300).contains(&status) => response
            .into_body()
            .read_to_vec()
            .map(Some)
            .map_err(|e| format!("{url}: {e}")),
        status => Err(format!("{url}: http status {status}")),
    }
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
    listed(listing).into_iter().next()
}

/// The releases a listing names, drafts left out, newest first by version
/// rather than in the order GitHub gave them. Only the engine's and the
/// desk's own tags are versions here (`v1.0.0-alpha.80`): a pack released
/// on its own (`pack-mri-v1.0.2`) shares the engine's repository, and is
/// never taken for an engine.
fn listed(listing: &str) -> Vec<String> {
    listed_as(listing, None)
}

/// The releases a listing names under one kind of tag, newest first: the
/// versions of the engine's own tags where `prefix` is `None`, and of a
/// pack's own (`pack-mri-v`) where it names that pack's.
fn listed_as(listing: &str, prefix: Option<&str>) -> Vec<String> {
    let Ok(releases) = serde_json::from_str::<serde_json::Value>(listing) else {
        return Vec::new();
    };
    let versioned = |v: &str| v.starts_with(|c: char| c.is_ascii_digit());
    let mut out: Vec<String> = releases
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| r["tag_name"].as_str())
        .filter_map(|tag| match prefix {
            None => Some(tag.trim_start_matches('v')),
            Some(p) => tag.strip_prefix(p),
        })
        .filter(|v| versioned(v))
        .map(str::to_string)
        .collect();
    out.sort_by(|a, b| {
        if newer(a, b) {
            std::cmp::Ordering::Less
        } else if newer(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    out.dedup();
    out
}

/// Fetch one file of a release and check it against that release's sums.
pub(crate) fn fetch_checked(base: &str, version: &str, file: &str) -> Result<Vec<u8>, Exit> {
    fetch_checked_at(base, &format!("v{version}"), file)
}

/// Fetch one file of the release a tag names, checked against its sums.
pub(crate) fn fetch_checked_at(base: &str, tag: &str, file: &str) -> Result<Vec<u8>, Exit> {
    let sums = fetch(&asset_at(base, tag, "SHA256SUMS"))
        .map_err(|e| fail(format!("the release names no checksums: {e}")))?;
    let sums = String::from_utf8_lossy(&sums).to_string();
    let want = sum_for(&sums, file).ok_or_else(|| match tag.strip_prefix('v') {
        // an engine release names a binary for each platform it was built for
        Some(version) => fail(format!(
            "the release {version} has no {file}: this platform is not one it was built for"
        )),
        None => fail(format!("the release {tag} names no {file} in its sums")),
    })?;
    let bytes = fetch(&asset_at(base, tag, file)).map_err(|e| fail(e.to_string()))?;
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

/// `nils update --check`: what an update would do, and nothing done. The
/// engine as always, and where a setup is recorded, every part beside its own
/// newest release, since the desk, the assistant and Kvasir release on their
/// own and any one of them may be behind while the engine is not.
fn check(args: &UpdateArgs, base: &str) -> Result<(), Exit> {
    let state = crate::setup::read_state();
    let engine = match &args.version {
        Some(v) => Ok(v.trim().trim_start_matches('v').to_string()),
        None => newest_version(base),
    };
    // With no setup recorded the engine is all there is, and a release that
    // cannot be read is the answer, as it always was.
    let Some(state) = state else {
        return engine_check(args, &engine?);
    };
    match &engine {
        Ok(wanted) => engine_check(args, wanted)?,
        Err(e) => println!(
            "nils {VERSION}: the newest release could not be read: {}",
            e.message
        ),
    }
    let channel = args.channel.as_deref();
    let rows = crate::releases::part_releases(
        &state,
        &mut |part| match part {
            "engine" => engine
                .as_ref()
                .map(Clone::clone)
                .map_err(|e| e.message.clone()),
            other => crate::releases::newest_of(other, channel),
        },
        &mut |v| crate::releases::desk_floor(&desk_base(channel), v, false),
        &mut || versions(&desk_base(channel)).map_err(|e| e.message),
        crate::releases::engine_contracts(),
    );
    let asked: Vec<&PartRelease> = rows
        .iter()
        .filter(|r| args.parts.is_empty() || args.parts.contains(&r.part))
        .collect();
    for row in &asked {
        println!("  {}", row.line());
    }
    let mut behind: Vec<String> = asked
        .iter()
        .filter(|r| r.behind())
        .filter_map(|r| r.to_take().map(|n| format!("{} {n}", r.part)))
        .collect();
    // The packs beside the engine's release: the one an update takes, or the
    // one installed where the engine is at its newest; and beside each
    // first-party pack's own releases, which an update takes with no engine
    // release (record 55 B5).
    let asks = |part: &str| args.parts.is_empty() || args.parts.iter().any(|p| p == part);
    let (packs, rules) = (asks("engine"), asks(crate::rules::PART));
    if packs || rules {
        let engine_row = rows.iter().find(|r| r.part == "engine");
        let release = engine_row
            .and_then(|r| r.to_take().map(str::to_string))
            .or_else(|| crate::setup::engine_version(&state));
        let dir = crate::setup::engine_pack_dir(&state);
        match (dir, release) {
            (Some(dir), Some(release)) => {
                // where the engine's release cannot be read, a pack's own
                // releases are still measured, against what is in place
                let carried = match crate::packs::bundled(base, &release) {
                    Ok(carried) => Some(carried),
                    Err(why) => {
                        println!(
                            "  packs in {}: the ones engine {release} was released with could not be read: {why}",
                            dir.display()
                        );
                        None
                    }
                };
                if carried.is_some() || rules {
                    let read = carried.is_some();
                    let carried = carried.unwrap_or_default();
                    let carried_packs: Vec<crate::packs::Pack> =
                        carried.iter().map(|o| o.pack.clone()).collect();
                    let engine = crate::packs::Engine::of_release(&release, &carried_packs);
                    let found = if rules {
                        let measured = if read {
                            carried_packs
                        } else {
                            crate::packs::first_party_in(&dir, &crate::setup::FIRST_PARTY_PACKS)
                        };
                        crate::rules::find_all(&crate::rules::base(channel), &measured, &engine)
                    } else {
                        Vec::new()
                    };
                    let plan = crate::packs::plan(
                        &dir,
                        &release,
                        carried,
                        crate::rules::takes(&found),
                        &engine,
                    );
                    if packs && read {
                        for line in plan.status.lines() {
                            println!("  {line}");
                        }
                        if plan.status.behind() {
                            behind.push(format!(
                                "packs of {release} ({})",
                                plan.status.stale.join(", ")
                            ));
                        }
                    }
                    for row in crate::rules::rows(&plan, &found) {
                        println!("  {}", row.line());
                        if let Some(version) = &row.takes {
                            behind.push(format!("{} {} {version}", crate::rules::PART, row.pack));
                        }
                    }
                }
            }
            (None, _) if rules && !args.parts.is_empty() => println!(
                "  rules: this install's engine runs in a container, whose image carries its packs"
            ),
            _ => {}
        }
    }
    if behind.is_empty() {
        println!("every part is at its newest release");
    } else if args.parts.is_empty() {
        println!("nils update --all would take {}", behind.join(", "));
    } else {
        let named: Vec<String> = args.parts.iter().map(|p| format!("--part {p}")).collect();
        println!(
            "nils update {} would take {}",
            named.join(" "),
            behind.join(", ")
        );
    }
    Ok(())
}

/// The engine's own lines of a check.
fn engine_check(args: &UpdateArgs, wanted: &str) -> Result<(), Exit> {
    let asked = args.version.is_some() || args.to.is_some();
    if !asked && !newer(wanted, VERSION) {
        println!("nils {VERSION} is the newest release");
        return Ok(());
    }
    let file = file_of(&host_target());
    let path = destination(args)?;
    println!("nils {wanted} is the release; this binary is {VERSION}");
    println!("  it would install {file} at {}", path.display());
    Ok(())
}

pub(crate) fn update(home: &nils_registry::home::Home, args: UpdateArgs) -> Result<(), Exit> {
    let base = base_of(&args);
    for part in &args.parts {
        if !crate::releases::OWN_RELEASES.contains(&part.as_str()) {
            return Err(usage(format!(
                "--part {part}: a part with releases of its own is one of {}",
                crate::releases::OWN_RELEASES.join(", ")
            )));
        }
    }
    if args.check {
        return check(&args, &base);
    }
    let only = args.only();
    // The engine first, then every other part: the parts are the newest
    // binary's to update, since this one's pins are the release it came with
    // and a part a later release adds is unknown to it. A binary this update
    // installed carries on from here, and its services start again, since the
    // engine's binary is new. Each part moves to its own newest release, so a
    // part is updated whether or not the engine has a newer one.
    if args.takes_parts() {
        if let Some(from) = std::env::var_os(HANDED_OVER) {
            println!(
                "nils {VERSION} carries on the update from {}",
                from.to_string_lossy()
            );
            crate::setup::update_all(args.channel.as_deref(), &only)?;
            crate::setup::restart_after_update(args.channel.as_deref());
            return Ok(());
        }
        crate::setup::setup_recorded()?;
        // On an install whose services are the machine's own, replacing the
        // parts' files is root's work, and the account the supervisor runs
        // as asks the helper to run this same command as root.
        if let Some(done) = crate::setup::update_by_helper(&only) {
            return done;
        }
        // Parts named without the engine leave its binary where it is.
        if !args.takes_engine() {
            if crate::setup::update_all(args.channel.as_deref(), &only)? {
                if only.iter().all(|p| p == crate::rules::PART) {
                    // The engine reads a pack again whenever its files have
                    // changed, so new rules need nothing started again.
                    println!("the engine reads the new rules from its next look at them");
                } else {
                    crate::setup::restart_after_update(args.channel.as_deref());
                }
            }
            return Ok(());
        }
    }
    let wanted = match &args.version {
        Some(v) => v.trim().trim_start_matches('v').to_string(),
        None => newest_version(&base)?,
    };
    let asked = args.version.is_some() || args.to.is_some();
    if !asked && !newer(&wanted, VERSION) {
        println!("nils {VERSION} is the newest release");
        if args.takes_parts() && crate::setup::update_all(args.channel.as_deref(), &only)? {
            crate::setup::restart_after_update(args.channel.as_deref());
        }
        return Ok(());
    }
    let target = host_target();
    let file = file_of(&target);
    let path = destination(&args)?;

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
    // Where a setup is recorded and this is its engine, they go where that
    // engine reads them, which a site may have named: the directory this
    // process would look in is the one of whoever runs it, and an update the
    // supervisor asks for runs as root, who finds none there.
    //
    // Where there are none at all, they are taken now. An engine installed
    // before the wizard fetched packs has none, and an update that moves
    // the binary and leaves it unable to say what a scan is has not
    // updated much.
    let recorded = crate::setup::read_state()
        .filter(|state| {
            state.parts.get("engine").is_some_and(|p| {
                Path::new(&p.path) == path
                    || std::fs::canonicalize(&p.path).is_ok_and(|c| c == path)
            })
        })
        .and_then(|state| crate::setup::engine_pack_dir(&state));
    match recorded {
        Some(packs) => {
            if let Some(parent) = packs.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match crate::packs::refresh(&base, &wanted, &packs) {
                Ok(said) => println!("{said}"),
                Err(why) => println!("the packs were left alone: {why}"),
            }
            crate::setup::record_rules(&packs);
        }
        None => match crate::pack_dir(home, None) {
            Ok(packs) => match crate::packs::refresh(&base, &wanted, &packs) {
                Ok(said) => println!("{said}"),
                Err(why) => println!("the packs were left alone: {why}"),
            },
            Err(_) => match beside_the_binary(&path) {
                Some(packs) => match crate::packs::refresh(&base, &wanted, &packs) {
                    Ok(_) => println!(
                        "the packs are at {}, where there were none",
                        packs.display()
                    ),
                    Err(why) => println!("there are no packs, and none could be taken: {why}"),
                },
                None => println!("there are no packs, and nowhere beside the binary to put them"),
            },
        },
    }

    // Every other part is the new binary's to update, with the versions its
    // release pins; where it cannot be started, this one does what it can.
    if args.takes_parts() {
        let why = hand_over(&path);
        println!("nils {wanted} could not be started ({why}), so nils {VERSION} updates the parts");
        crate::setup::update_all(args.channel.as_deref(), &only)?;
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
        // every release, newest first, for a desk that has to pass one over
        assert_eq!(
            listed(listing),
            [
                "1.0.0-alpha.11",
                "1.0.0-alpha.10",
                "1.0.0-alpha.9",
                "1.0.0-alpha.8"
            ]
        );
        assert_eq!(listed(with_draft), ["1.0.0-alpha.11"]);
    }

    /// Record 55 B5: a pack's own releases share the engine's repository,
    /// and its listing. The engine's lookups never take one for an engine,
    /// and a pack's lookup finds its own by their tag.
    #[test]
    fn a_packs_own_release_is_never_taken_for_an_engine_and_is_found_by_its_tag() {
        let listing = r#"[
            {"tag_name": "pack-mri-v1.0.3", "draft": false},
            {"tag_name": "v1.0.0-alpha.80", "draft": false},
            {"tag_name": "pack-mri-v1.0.2", "draft": false},
            {"tag_name": "pack-ct-v0.1.0", "draft": false},
            {"tag_name": "pack-mri-v1.0.4", "draft": true},
            {"tag_name": "vendor-x", "draft": false}
        ]"#;
        assert_eq!(listed(listing), ["1.0.0-alpha.80"]);
        assert_eq!(newest_of(listing).as_deref(), Some("1.0.0-alpha.80"));
        assert_eq!(listed_as(listing, Some("pack-mri-v")), ["1.0.3", "1.0.2"]);
        assert_eq!(listed_as(listing, Some("pack-ct-v")), ["0.1.0"]);
        assert!(listed_as(listing, Some("pack-pet-v")).is_empty());
        assert_eq!(
            tagged("file:///nowhere", "pack-mri-v"),
            None,
            "a channel of its own has no listing"
        );
    }

    #[test]
    fn a_file_a_channel_does_not_have_is_none_and_not_an_error() {
        let root = std::env::temp_dir().join(format!("nils-if-there-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("here"), "1.0.2\n").unwrap();
        let url = |name: &str| format!("file://{}", root.join(name).display());
        assert_eq!(fetch_if_there(&url("here")), Ok(Some(b"1.0.2\n".to_vec())));
        assert_eq!(fetch_if_there(&url("gone")), Ok(None));
        let _ = std::fs::remove_dir_all(&root);
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
