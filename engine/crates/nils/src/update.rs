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
    let base = args
        .channel
        .clone()
        .or_else(|| std::env::var("NILS_RELEASES").ok())
        .unwrap_or_else(|| RELEASES.to_string());
    base.trim_end_matches('/').to_string()
}

/// A file of one release: `<base>/download/v<version>/<file>`.
fn asset(base: &str, version: &str, file: &str) -> String {
    format!("{base}/download/v{version}/{file}")
}

/// The version the newest release names, from the one line `VERSION` file
/// it publishes beside its binaries.
fn latest_version(base: &str) -> Result<String, Exit> {
    let url = format!("{base}/latest/download/VERSION");
    match fetch(&url) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            if text.is_empty() || text.lines().count() > 1 {
                return Err(fail(format!("{url} does not hold a version on one line")));
            }
            Ok(text.trim_start_matches('v').to_string())
        }
        // GitHub's own `latest` skips a pre-release, and a pre-release is all
        // there is before 1.0.0, so ask the API for the newest tag instead.
        Err(e) => newest_tag(base).ok_or_else(|| fail(format!("no release to update to: {e}"))),
    }
}

/// The newest tag of a GitHub repository, pre-release or not, from the API.
/// `None` for a channel that is not GitHub, whose own `VERSION` is the answer.
fn newest_tag(base: &str) -> Option<String> {
    let repo = base
        .strip_prefix("https://github.com/")?
        .strip_suffix("/releases")?;
    let body = fetch(&format!(
        "https://api.github.com/repos/{repo}/releases?per_page=10"
    ))
    .ok()?;
    let text = String::from_utf8_lossy(&body);
    let at = text.find("\"tag_name\"")?;
    let rest = &text[at + "\"tag_name\"".len()..];
    let open = rest.find('"')? + 1;
    let close = rest[open..].find('"')? + open;
    let tag = rest[open..close].trim_start_matches('v').to_string();
    (!tag.is_empty()).then_some(tag)
}

/// Fetch one file of a release and check it against that release's sums.
fn fetch_checked(base: &str, version: &str, file: &str) -> Result<Vec<u8>, Exit> {
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
fn writable(dir: &Path) -> bool {
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
fn install_binary(path: &Path, bytes: &[u8]) -> Result<(), Exit> {
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
fn refresh_packs(base: &str, version: &str, dir: &Path) -> Result<String, String> {
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
    let _ = std::fs::remove_dir_all(&aside);
    if dir.exists() {
        std::fs::rename(dir, &aside).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    match std::fs::rename(&fresh, dir) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&aside);
            let _ = std::fs::remove_dir_all(&staging);
            Ok(format!("the packs in {} are the release's", dir.display()))
        }
        Err(e) => {
            if aside.exists() {
                let _ = std::fs::rename(&aside, dir);
            }
            let _ = std::fs::remove_dir_all(&staging);
            Err(format!("{}: {e}", dir.display()))
        }
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

pub(crate) fn update(home: &nils_registry::home::Home, args: UpdateArgs) -> Result<(), Exit> {
    let base = base_of(&args);
    let wanted = match &args.version {
        Some(v) => v.trim().trim_start_matches('v').to_string(),
        None => latest_version(&base)?,
    };
    let asked = args.version.is_some() || args.to.is_some();
    if !asked && !newer(&wanted, VERSION) {
        println!("nils {VERSION} is the newest release");
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

    // The packs go with the binary when the ones in use may be replaced; a
    // deployment that keeps its packs elsewhere is left alone and told so.
    match crate::pack_dir(home, None) {
        Ok(packs) => match refresh_packs(&base, &wanted, &packs) {
            Ok(said) => println!("{said}"),
            Err(why) => println!("the packs were left alone: {why}"),
        },
        Err(_) => println!("no pack directory is in use, so none was refreshed"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_is_newer_by_its_numbers_then_by_its_pre_release() {
        assert!(newer("1.0.1", "1.0.0"));
        assert!(newer("1.10.0", "1.2.0"));
        assert!(newer("1.0.0", "1.0.0-alpha.1"));
        assert!(newer("1.0.0-alpha.2", "1.0.0-alpha.1"));
        assert!(newer("1.0.0-alpha.10", "1.0.0-alpha.2"));
        assert!(newer("1.0.0-beta.1", "1.0.0-alpha.9"));
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
    fn the_sums_file_is_read_in_either_form() {
        let sums = "aa11  nils-linux-x86_64\nbb22 *packs.tar.gz\n";
        assert_eq!(sum_for(sums, "nils-linux-x86_64").as_deref(), Some("aa11"));
        assert_eq!(sum_for(sums, "packs.tar.gz").as_deref(), Some("bb22"));
        assert_eq!(sum_for(sums, "nils-macos-arm64"), None);
    }
}
