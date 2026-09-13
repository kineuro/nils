// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 5 §10.4 (D76, C50): `nils supervise`, a subcommand of the one binary
//! run as a service on a host with the privilege to replace the parts on
//! that host and nothing else. It reports what is installed, watches a
//! release channel naming signed artifacts, fetches an artifact, verifies
//! its signature against a key the deployment holds, applies it, restarts
//! the part, and answers the desk over the same contract shape everything
//! else does. The product ships the verifier and the format; the channel
//! and the signing key are the deployment's.
//!
//! An artifact is three files beside each other, named after the part, its
//! version and its target: `<name>.tar.gz` (the files), `<name>.json` (the
//! manifest: part, version, target, contracts, the tarball's sha256, when
//! it was built, the files it holds) and `<name>.sig` (an ed25519 signature
//! over the manifest bytes, hex). The channel is one directory per part
//! with a `latest.json` naming the version and the three URLs.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use ring::signature::{self, Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::{Exit, fail, usage};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The manifest beside an artifact: everything the verifier and the
/// supervisor need to know before they open the tarball.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub part: String,
    pub version: String,
    pub target: String,
    #[serde(default)]
    pub contracts: BTreeMap<String, String>,
    pub sha256: String,
    pub built_at: String,
    #[serde(default)]
    pub files: Vec<String>,
}

impl Manifest {
    /// The stem the three files share: `<part>-<version>-<target>`.
    pub fn stem(&self) -> String {
        format!("{}-{}-{}", self.part, self.version, self.target)
    }
}

/// The host's target, the way the manifest spells it.
pub(crate) fn host_target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The file beside a manifest with another suffix: a version has dots, so
/// the stem is what is left when `.json` goes, never what `with_extension`
/// would guess.
fn beside_file(manifest: &Path, suffix: &str) -> PathBuf {
    let name = manifest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = name.strip_suffix(".json").unwrap_or(&name);
    manifest.with_file_name(format!("{stem}.{suffix}"))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

fn read(path: &Path) -> Result<Vec<u8>, Exit> {
    std::fs::read(path).map_err(|e| fail(format!("{}: {e}", path.display())))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Exit> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| fail(format!("{}: {e}", parent.display())))?;
    }
    std::fs::write(path, bytes).map_err(|e| fail(format!("{}: {e}", path.display())))
}

// ---------------------------------------------------------------------
// keys and signatures

/// A key pair on disk: `<out>/supervise.key` (PKCS#8, private, mode 600)
/// and `<out>/supervise.pub` (the public key, hex, one line). The public
/// file is what a deployment puts in its trust file.
pub(crate) fn keygen(out: &Path) -> Result<(PathBuf, PathBuf), Exit> {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 =
        Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| fail("the random source failed"))?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| fail("the generated key does not parse"))?;
    std::fs::create_dir_all(out).map_err(|e| fail(format!("{}: {e}", out.display())))?;
    let private = out.join("supervise.key");
    let public = out.join("supervise.pub");
    write(&private, pkcs8.as_ref())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| fail(format!("{}: {e}", private.display())))?;
    }
    write(
        &public,
        format!("{}\n", hex::encode(pair.public_key().as_ref())).as_bytes(),
    )?;
    Ok((private, public))
}

/// Sign a manifest with the private key; the signature goes beside it as `<stem>.sig`.
pub(crate) fn sign(key: &Path, manifest: &Path) -> Result<PathBuf, Exit> {
    let pair = Ed25519KeyPair::from_pkcs8(&read(key)?).map_err(|_| {
        fail(format!(
            "{}: not an ed25519 key made by keygen",
            key.display()
        ))
    })?;
    let bytes = read(manifest)?;
    let sig = pair.sign(&bytes);
    let out = beside_file(manifest, "sig");
    write(&out, format!("{}\n", hex::encode(sig.as_ref())).as_bytes())?;
    Ok(out)
}

/// The public keys a deployment trusts: one hex key per line, blank lines
/// and `#` comments skipped.
pub(crate) fn trust_keys(trust: &Path) -> Result<Vec<Vec<u8>>, Exit> {
    let text = String::from_utf8_lossy(&read(trust)?).to_string();
    let mut keys = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = hex::decode(line)
            .map_err(|_| fail(format!("{}: a line is not a hex key", trust.display())))?;
        if key.len() != 32 {
            return Err(fail(format!("{}: a key is not 32 bytes", trust.display())));
        }
        keys.push(key);
    }
    if keys.is_empty() {
        return Err(fail(format!("{}: names no key", trust.display())));
    }
    Ok(keys)
}

/// Why an artifact is refused: every reason has a name the desk shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    NoSignature,
    BadSignature,
    DigestMismatch {
        expected: String,
        found: String,
    },
    TargetMismatch {
        wanted: String,
        found: String,
    },
    ContractMismatch {
        contract: String,
        installed: String,
        artifact: String,
    },
    NoTarball(String),
    BadManifest(String),
}

impl Refusal {
    pub fn name(&self) -> &'static str {
        match self {
            Refusal::NoSignature => "no signature",
            Refusal::BadSignature => "bad signature",
            Refusal::DigestMismatch { .. } => "digest mismatch",
            Refusal::TargetMismatch { .. } => "target mismatch",
            Refusal::ContractMismatch { .. } => "contract mismatch",
            Refusal::NoTarball(_) => "no tarball",
            Refusal::BadManifest(_) => "bad manifest",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoSignature => write!(f, "no signature: the manifest has no .sig beside it"),
            Refusal::BadSignature => {
                write!(f, "bad signature: no trusted key signed this manifest")
            }
            Refusal::DigestMismatch { expected, found } => write!(
                f,
                "digest mismatch: the manifest names sha256 {expected}, the tarball is {found}"
            ),
            Refusal::TargetMismatch { wanted, found } => write!(
                f,
                "target mismatch: this host is {wanted}, the artifact is built for {found}"
            ),
            Refusal::ContractMismatch {
                contract,
                installed,
                artifact,
            } => write!(
                f,
                "contract mismatch: {contract} is {installed} installed and the artifact speaks {artifact}; pass allow_contract_change to take it"
            ),
            Refusal::NoTarball(p) => write!(f, "no tarball: {p} is not beside the manifest"),
            Refusal::BadManifest(why) => write!(f, "bad manifest: {why}"),
        }
    }
}

/// What a verification checks beyond the signature and the digest.
#[derive(Debug, Clone, Default)]
pub(crate) struct Expect {
    /// The target the artifact must be built for; none skips the check.
    pub target: Option<String>,
    /// The contracts of what is installed; a major that differs is refused unless allowed.
    pub installed: Option<BTreeMap<String, String>>,
    pub allow_contract_change: bool,
}

fn major(v: &str) -> &str {
    v.trim_start_matches('v')
        .split(['.', '-'])
        .next()
        .unwrap_or(v)
}

/// Verify an artifact by its manifest path: the signature against the
/// trusted keys, the tarball's digest, then what `expect` asks. Answers the
/// manifest and the tarball's path.
pub(crate) fn verify(
    manifest_path: &Path,
    keys: &[Vec<u8>],
    expect: &Expect,
) -> Result<(Manifest, PathBuf), Refusal> {
    let bytes = std::fs::read(manifest_path)
        .map_err(|e| Refusal::BadManifest(format!("{}: {e}", manifest_path.display())))?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| Refusal::BadManifest(e.to_string()))?;
    let sig_path = beside_file(manifest_path, "sig");
    let sig_text = std::fs::read_to_string(&sig_path).map_err(|_| Refusal::NoSignature)?;
    let sig = hex::decode(sig_text.trim()).map_err(|_| Refusal::BadSignature)?;
    let signed = keys.iter().any(|k| {
        signature::UnparsedPublicKey::new(&signature::ED25519, k)
            .verify(&bytes, &sig)
            .is_ok()
    });
    if !signed {
        return Err(Refusal::BadSignature);
    }
    let tarball = beside_file(manifest_path, "tar.gz");
    let tar_bytes =
        std::fs::read(&tarball).map_err(|_| Refusal::NoTarball(tarball.display().to_string()))?;
    let found = sha256_hex(&tar_bytes);
    if found != manifest.sha256 {
        return Err(Refusal::DigestMismatch {
            expected: manifest.sha256.clone(),
            found,
        });
    }
    if let Some(wanted) = &expect.target
        && wanted != &manifest.target
    {
        return Err(Refusal::TargetMismatch {
            wanted: wanted.clone(),
            found: manifest.target.clone(),
        });
    }
    if let Some(installed) = &expect.installed
        && !expect.allow_contract_change
    {
        for (name, have) in installed {
            if let Some(theirs) = manifest.contracts.get(name)
                && major(have) != major(theirs)
            {
                return Err(Refusal::ContractMismatch {
                    contract: name.clone(),
                    installed: have.clone(),
                    artifact: theirs.clone(),
                });
            }
        }
    }
    Ok((manifest, tarball))
}

// ---------------------------------------------------------------------
// packing

/// Make an artifact from a directory: the tarball of its files (relative
/// paths, sorted), the manifest, and nothing else; signing is a second
/// step with a key the deployment holds.
pub(crate) fn pack(
    part: &str,
    version: &str,
    target: &str,
    dir: &Path,
    out: &Path,
    contracts: BTreeMap<String, String>,
) -> Result<PathBuf, Exit> {
    let mut files: Vec<PathBuf> = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        tar.mode(tar::HeaderMode::Deterministic);
        for rel in &files {
            let full = dir.join(rel);
            let mut f =
                std::fs::File::open(&full).map_err(|e| fail(format!("{}: {e}", full.display())))?;
            tar.append_file(rel, &mut f)
                .map_err(|e| fail(format!("{}: {e}", full.display())))?;
        }
        tar.finish().map_err(|e| fail(e.to_string()))?;
    }
    let bytes = gz.finish().map_err(|e| fail(e.to_string()))?;
    let manifest = Manifest {
        part: part.to_string(),
        version: version.to_string(),
        target: target.to_string(),
        contracts,
        sha256: sha256_hex(&bytes),
        built_at: nils_registry::time::now_iso(),
        files: files
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect(),
    };
    std::fs::create_dir_all(out).map_err(|e| fail(format!("{}: {e}", out.display())))?;
    let manifest_path = out.join(format!("{}.json", manifest.stem()));
    write(&beside_file(&manifest_path, "tar.gz"), &bytes)?;
    write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest)
            .unwrap_or_default()
            .as_bytes(),
    )?;
    Ok(manifest_path)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Exit> {
    for entry in std::fs::read_dir(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))? {
        let entry = entry.map_err(|e| fail(e.to_string()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// the channel and the fetcher

/// What `<channel>/<part>/latest.json` says.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Latest {
    pub version: String,
    pub manifest_url: String,
    pub artifact_url: String,
    pub signature_url: String,
}

/// One fetch: `file://` paths are read from disk (a channel on a share, and
/// the tests); anything else goes through the one HTTP client the engine has.
pub(crate) fn fetch(url: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(|e| format!("{url}: {e}"));
    }
    let mut response = ureq::get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(bytes)
}

/// A URL beside another: `latest.json` names its files relative to the part's directory when they are not absolute.
fn beside(base: &str, name: &str) -> String {
    if name.contains("://") {
        return name.to_string();
    }
    let dir = base.rsplit_once('/').map(|(d, _)| d).unwrap_or(base);
    format!("{dir}/{name}")
}

pub(crate) fn latest(channel: &str, part: &str) -> Result<(Latest, String), String> {
    let url = format!("{}/{part}/latest.json", channel.trim_end_matches('/'));
    let bytes = fetch(&url)?;
    let mut latest: Latest = serde_json::from_slice(&bytes).map_err(|e| format!("{url}: {e}"))?;
    latest.manifest_url = beside(&url, &latest.manifest_url);
    latest.artifact_url = beside(&url, &latest.artifact_url);
    latest.signature_url = beside(&url, &latest.signature_url);
    Ok((latest, url))
}

/// Bring an artifact's three files into a directory, named by the manifest's stem.
pub(crate) fn download(latest: &Latest, into: &Path) -> Result<PathBuf, String> {
    let manifest_bytes = fetch(&latest.manifest_url)?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| format!("{}: {e}", latest.manifest_url))?;
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let manifest_path = into.join(format!("{}.json", manifest.stem()));
    let put = |path: PathBuf, bytes: Vec<u8>| {
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    };
    put(manifest_path.clone(), manifest_bytes)?;
    match fetch(&latest.signature_url) {
        Ok(sig) => put(beside_file(&manifest_path, "sig"), sig)?,
        Err(_) => {
            // no signature on the channel: the verifier names it
            let _ = std::fs::remove_file(beside_file(&manifest_path, "sig"));
        }
    }
    put(
        beside_file(&manifest_path, "tar.gz"),
        fetch(&latest.artifact_url)?,
    )?;
    Ok(manifest_path)
}

// ---------------------------------------------------------------------
// the service

/// `supervise.toml`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Config {
    #[serde(default = "default_bind")]
    pub bind: String,
    /// The trust file: the public keys, one hex key per line.
    pub trust: PathBuf,
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
    /// Where every apply is recorded, one JSON line each.
    pub log: PathBuf,
    /// The host's target; detected when absent.
    pub target: Option<String>,
    /// Seconds to wait for a part to answer with its new version after a restart.
    #[serde(default = "default_settle")]
    pub settle_seconds: u64,
    /// Bearer tokens and who they name; every door asks for one (the admin entitlement).
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    #[serde(default)]
    pub parts: Vec<PartConfig>,
}

fn default_bind() -> String {
    "127.0.0.1:8470".to_string()
}
fn default_poll() -> u64 {
    900
}
fn default_settle() -> u64 {
    30
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PartConfig {
    pub name: String,
    /// The directory the part's files live in; `installed.json` there is the manifest of what was applied.
    pub install: PathBuf,
    /// Run by `sh -c` after the files are in place.
    pub restart: String,
    /// The channel: `<channel>/<name>/latest.json` names the latest artifact.
    pub channel: String,
    /// The part's own capabilities door; after a restart it must answer with the new version.
    pub capabilities: Option<String>,
    /// A file under `install` whose content is the version, for a part without a door.
    pub version_file: Option<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, Exit> {
        let text =
            std::fs::read_to_string(path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
        let mut config: Config =
            toml::from_str(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?;
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let absolute = |p: &PathBuf| {
            if p.is_absolute() {
                p.clone()
            } else {
                base.join(p)
            }
        };
        config.trust = absolute(&config.trust);
        config.log = absolute(&config.log);
        for part in &mut config.parts {
            part.install = absolute(&part.install);
        }
        if config.tokens.is_empty() {
            return Err(usage(
                "supervise.toml names no token; every door asks for one",
            ));
        }
        Ok(config)
    }
}

/// How the supervisor tells a part came back with the new version.
pub(crate) enum Check {
    Capabilities(String),
    File(PathBuf),
    None,
}

impl PartConfig {
    fn check(&self) -> Check {
        if let Some(url) = &self.capabilities {
            Check::Capabilities(url.clone())
        } else if let Some(f) = &self.version_file {
            Check::File(self.install.join(f))
        } else {
            Check::None
        }
    }

    pub(crate) fn installed(&self) -> Option<Manifest> {
        let bytes = std::fs::read(self.install.join("installed.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// The version a part answers with now, the way its check reads it.
fn observed(check: &Check) -> Option<String> {
    match check {
        Check::Capabilities(url) => {
            let bytes = fetch(url).ok()?;
            let doc: Value = serde_json::from_slice(&bytes).ok()?;
            doc.pointer("/engine/version")
                .or_else(|| doc.pointer("/desk/version"))
                .or_else(|| doc.get("version"))
                .and_then(Value::as_str)
                .map(str::to_string)
        }
        Check::File(path) => std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string()),
        Check::None => None,
    }
}

fn settled(check: &Check, version: &str, within: Duration) -> bool {
    if matches!(check, Check::None) {
        return true;
    }
    let start = Instant::now();
    loop {
        if observed(check).as_deref() == Some(version) {
            return true;
        }
        if start.elapsed() > within {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn restart(command: &str) -> Result<(), String> {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .status()
        .map_err(|e| format!("{command}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command}: exited {status}"))
    }
}

/// One row of the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Applied {
    pub at: String,
    pub part: String,
    pub from: Option<String>,
    pub to: String,
    pub digest: String,
    pub ok: bool,
    pub why: Option<String>,
}

fn log_row(log: &Path, row: &Applied) {
    if let Some(parent) = log.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = writeln!(f, "{}", serde_json::to_string(row).unwrap_or_default());
    }
}

pub(crate) fn log_rows(log: &Path, limit: usize) -> Vec<Applied> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut rows: Vec<Applied> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if rows.len() > limit {
        rows.drain(..rows.len() - limit);
    }
    rows
}

/// Unpack a verified artifact into `.staging/<version>` under the install
/// directory and check every file the manifest names arrived.
fn stage(part: &PartConfig, manifest: &Manifest, tarball: &Path) -> Result<PathBuf, String> {
    let staging = part.install.join(".staging").join(&manifest.version);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let bytes = std::fs::read(tarball).map_err(|e| format!("{}: {e}", tarball.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    archive
        .unpack(&staging)
        .map_err(|e| format!("unpacking {}: {e}", tarball.display()))?;
    for f in &manifest.files {
        if f.contains("..") || f.starts_with('/') {
            return Err(format!(
                "the manifest names a file outside the artifact: {f}"
            ));
        }
        if !staging.join(f).is_file() {
            return Err(format!(
                "the manifest names {f} and the tarball does not hold it"
            ));
        }
    }
    Ok(staging)
}

/// Move the staged files into place, keeping the previous ones under `.previous`.
fn swap(part: &PartConfig, manifest: &Manifest, staging: &Path) -> Result<(), String> {
    let previous = part.install.join(".previous");
    let _ = std::fs::remove_dir_all(&previous);
    for f in &manifest.files {
        let live = part.install.join(f);
        let kept = previous.join(f);
        let fresh = staging.join(f);
        if let Some(p) = kept.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        if let Some(p) = live.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        if live.exists() {
            std::fs::rename(&live, &kept).map_err(|e| format!("{}: {e}", live.display()))?;
        }
        std::fs::rename(&fresh, &live).map_err(|e| format!("{}: {e}", fresh.display()))?;
    }
    let installed = part.install.join("installed.json");
    std::fs::write(
        &installed,
        serde_json::to_string_pretty(manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("{}: {e}", installed.display()))?;
    Ok(())
}

/// Put `.previous` back after a failed settle.
fn roll_back(
    part: &PartConfig,
    manifest: &Manifest,
    before: Option<&Manifest>,
) -> Result<(), String> {
    let previous = part.install.join(".previous");
    for f in &manifest.files {
        let live = part.install.join(f);
        let kept = previous.join(f);
        let _ = std::fs::remove_file(&live);
        if kept.exists() {
            std::fs::rename(&kept, &live).map_err(|e| format!("{}: {e}", kept.display()))?;
        }
    }
    let installed = part.install.join("installed.json");
    match before {
        Some(m) => std::fs::write(
            &installed,
            serde_json::to_string_pretty(m).unwrap_or_default(),
        )
        .map_err(|e| format!("{}: {e}", installed.display()))?,
        None => {
            let _ = std::fs::remove_file(&installed);
        }
    }
    Ok(())
}

/// The whole update of one part: fetch the channel's latest (or refuse a
/// version the channel does not name), verify, stage, swap, restart, settle,
/// else roll back. Every ending is a row of the log.
pub(crate) fn update(
    config: &Config,
    part: &PartConfig,
    want: Option<&str>,
    allow_contract_change: bool,
) -> Result<Applied, String> {
    let (latest, _) = latest(&part.channel, &part.name)?;
    if let Some(v) = want
        && v != latest.version
    {
        return Err(format!(
            "the channel names {} for {}, not {v}",
            latest.version, part.name
        ));
    }
    let before = part.installed();
    let into = part.install.join(".staging").join("fetched");
    let manifest_path = download(&latest, &into)?;
    let keys = trust_keys(&config.trust).map_err(|e| e.message)?;
    let expect = Expect {
        target: Some(config.target.clone().unwrap_or_else(host_target)),
        installed: before.as_ref().map(|m| m.contracts.clone()),
        allow_contract_change,
    };
    let refuse = |why: String, to: &str, digest: &str| -> String {
        log_row(
            &config.log,
            &Applied {
                at: nils_registry::time::now_iso(),
                part: part.name.clone(),
                from: before.as_ref().map(|m| m.version.clone()),
                to: to.to_string(),
                digest: digest.to_string(),
                ok: false,
                why: Some(why.clone()),
            },
        );
        why
    };
    let (manifest, tarball) = match verify(&manifest_path, &keys, &expect) {
        Ok(v) => v,
        Err(r) => return Err(refuse(r.to_string(), &latest.version, "")),
    };
    if manifest.part != part.name {
        return Err(refuse(
            format!("the artifact is for {}, not {}", manifest.part, part.name),
            &manifest.version,
            &manifest.sha256,
        ));
    }
    let staging = stage(part, &manifest, &tarball)
        .map_err(|w| refuse(w, &manifest.version, &manifest.sha256))?;
    swap(part, &manifest, &staging).map_err(|w| refuse(w, &manifest.version, &manifest.sha256))?;
    let restarted = restart(&part.restart);
    let ok = restarted.is_ok()
        && settled(
            &part.check(),
            &manifest.version,
            Duration::from_secs(config.settle_seconds),
        );
    if !ok {
        let why = match restarted {
            Err(e) => format!("the restart failed: {e}"),
            Ok(()) => format!(
                "{} did not answer with {} within {} s",
                part.name, manifest.version, config.settle_seconds
            ),
        };
        let back =
            roll_back(part, &manifest, before.as_ref()).and_then(|()| restart(&part.restart));
        let why = match back {
            Ok(()) => format!(
                "{why}; rolled back to {}",
                before
                    .as_ref()
                    .map(|m| m.version.as_str())
                    .unwrap_or("what was there")
            ),
            Err(e) => format!("{why}; the roll back failed too: {e}"),
        };
        return Err(refuse(why, &manifest.version, &manifest.sha256));
    }
    let _ = std::fs::remove_dir_all(&staging);
    let row = Applied {
        at: nils_registry::time::now_iso(),
        part: part.name.clone(),
        from: before.map(|m| m.version),
        to: manifest.version.clone(),
        digest: manifest.sha256.clone(),
        ok: true,
        why: None,
    };
    log_row(&config.log, &row);
    Ok(row)
}

/// What is installed and what the channel offers, per part.
pub(crate) fn capabilities(config: &Config, node: &str) -> Value {
    let target = config.target.clone().unwrap_or_else(host_target);
    let parts: Vec<Value> = config
        .parts
        .iter()
        .map(|p| {
            let installed = p.installed();
            let health = match p.check() {
                Check::Capabilities(url) => {
                    json!({ "door": url, "version": observed(&Check::Capabilities(url.clone())) })
                }
                Check::File(path) => json!({ "version": observed(&Check::File(path)) }),
                Check::None => Value::Null,
            };
            let newer = match latest(&p.channel, &p.name) {
                Ok((l, _))
                    if installed
                        .as_ref()
                        .map(|m| m.version != l.version)
                        .unwrap_or(true) =>
                {
                    json!({ "version": l.version, "manifest_url": l.manifest_url })
                }
                Ok(_) => Value::Null,
                Err(e) => json!({ "error": e }),
            };
            json!({
                "name": p.name,
                "install": p.install.display().to_string(),
                "version": installed.as_ref().map(|m| m.version.clone()),
                "contracts": installed.as_ref().map(|m| m.contracts.clone()),
                "digest": installed.as_ref().map(|m| m.sha256.clone()),
                "channel": p.channel,
                "health": health,
                "newer": newer,
            })
        })
        .collect();
    json!({
        "supervisor": { "version": VERSION, "target": target, "node": node, "poll_seconds": config.poll_seconds },
        "parts": parts,
        "doors": [
            "GET /api/supervise/capabilities",
            "POST /api/supervise/update",
            "GET /api/supervise/log",
            "GET /api/supervise/install",
            "POST /api/supervise/restart",
            "POST /api/supervise/reapply",
            "POST /api/supervise/update-all",
            "GET /api/supervise/runs/{id}",
            "POST /api/supervise/look",
        ],
        "log": log_rows(&config.log, 5),
    })
}

fn reply(request: Request, status: u16, body: Value) {
    let response = Response::from_string(serde_json::to_string_pretty(&body).unwrap_or_default())
        .with_status_code(StatusCode(status))
        .with_chunked_threshold(usize::MAX)
        .with_header(Header::from_bytes("Content-Type", "application/json").expect("header"));
    let _ = request.respond(response);
}

fn error(status: u16, message: impl Into<String>) -> (u16, Value) {
    let disclosure = if status >= 500 { "internal" } else { "safe" };
    (
        status,
        json!({ "error": message.into(), "disclosure": disclosure }),
    )
}

fn principal(config: &Config, request: &Request) -> Option<String> {
    let header = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_string())?;
    let token = header.strip_prefix("Bearer ")?.trim();
    config.tokens.get(token).cloned()
}

// ---------------------------------------------------------------------
// the install: what nils setup made, restarted and kept up to date

/// Work the supervisor starts and does not wait for: a restart, the engine
/// made to follow the registry, an update of everything. Each is a process
/// of its own with its output in a file, so a restart of the desk that asked
/// for it does not cut it short, and the desk reads how it went afterwards.
struct Runs {
    dir: PathBuf,
    children: Mutex<HashMap<String, Child>>,
}

impl Runs {
    fn start(&self, what: &str, args: &[String]) -> Result<Value, (u16, String)> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| (500, "the runs are held".to_string()))?;
        let mut going = None;
        for (id, child) in children.iter_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                going = Some(id.clone());
                break;
            }
        }
        if let Some(id) = going {
            return Err((409, format!("run {id} is still going; one run at a time")));
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| (500, format!("{}: {e}", self.dir.display())))?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let id = format!("r{millis}");
        let doc = json!({
            "id": id,
            "what": what,
            "command": args,
            "state": "running",
            "pid": null,
            "started_at": nils_registry::time::now_iso(),
            "finished_at": null,
            "exit": null,
        });
        write_run(&self.dir, &id, &doc);
        // the recorder runs the command and writes down how it ended, so the
        // ending is kept even when this service is restarted meanwhile
        let mut recorder = vec![
            "supervise".to_string(),
            "record".to_string(),
            "--dir".to_string(),
            self.dir.display().to_string(),
            "--id".to_string(),
            id.clone(),
            "--".to_string(),
        ];
        recorder.extend(args.iter().cloned());
        let me = std::env::current_exe().map_err(|e| (500, e.to_string()))?;
        let child = Command::new(me)
            .args(&recorder)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| (500, format!("{what}: {e}")))?;
        children.insert(id, child);
        Ok(doc)
    }

    /// A run as it stands, with the last lines of its output.
    fn show(&self, id: &str) -> Option<Value> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        let path = self.dir.join(format!("{id}.json"));
        let read = || -> Option<Value> { serde_json::from_slice(&std::fs::read(&path).ok()?).ok() };
        let mut doc = read()?;
        if doc["state"] == "running" {
            let mut children = self.children.lock().ok()?;
            let gone = match children.get_mut(id).map(Child::try_wait) {
                Some(Ok(Some(_))) => {
                    children.remove(id);
                    true
                }
                Some(Ok(None)) => false,
                // a run from before this service started: its recorder says whether it still runs
                _ => !doc["pid"]
                    .as_u64()
                    .is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists()),
            };
            if gone {
                // the recorder wrote the ending before it exited
                doc = read()?;
                if doc["state"] == "running" {
                    doc["state"] = json!("ended");
                    doc["finished_at"] = json!(nils_registry::time::now_iso());
                    write_run(&self.dir, id, &doc);
                }
            }
        }
        let log = std::fs::read_to_string(self.dir.join(format!("{id}.log"))).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        doc["tail"] = json!(lines[lines.len().saturating_sub(40)..].to_vec());
        Some(doc)
    }
}

/// Run one command of this binary for a run the service started, and write
/// down how it ended. The recorder writes the ending rather than the
/// service, so a run that outlives a restart of the service still says how
/// it went.
fn record(dir: &Path, id: &str, command: &[String]) -> Result<(), Exit> {
    let path = dir.join(format!("{id}.json"));
    let read = || -> Value {
        std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| json!({ "id": id }))
    };
    let mut doc = read();
    doc["pid"] = json!(std::process::id());
    write_run(dir, id, &doc);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{id}.log")))
        .map_err(|e| fail(e.to_string()))?;
    let err = log.try_clone().map_err(|e| fail(e.to_string()))?;
    let me = std::env::current_exe().map_err(|e| fail(e.to_string()))?;
    let status = Command::new(me)
        .args(command)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .status();
    let mut doc = read();
    match status {
        Ok(s) => {
            doc["state"] = json!(if s.success() { "done" } else { "failed" });
            doc["exit"] = json!(s.code());
        }
        Err(e) => {
            doc["state"] = json!("failed");
            doc["error"] = json!(e.to_string());
        }
    }
    doc["finished_at"] = json!(nils_registry::time::now_iso());
    write_run(dir, id, &doc);
    Ok(())
}

fn write_run(dir: &Path, id: &str, doc: &Value) {
    let _ = std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec_pretty(doc).unwrap_or_default(),
    );
}

type Slot = OnceLock<Mutex<Option<(Instant, Value)>>>;
static NEWEST: Slot = OnceLock::new();
static MACHINE: Slot = OnceLock::new();

/// A slow look kept for a while: the newest release, the card.
fn cached(slot: &'static Slot, every: Duration, make: impl FnOnce() -> Value) -> Value {
    let held = slot.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = held.lock()
        && let Some((at, value)) = guard.as_ref()
        && at.elapsed() < every
    {
        return value.clone();
    }
    let value = make();
    if let Ok(mut guard) = held.lock() {
        *guard = Some((Instant::now(), value.clone()));
    }
    value
}

/// The install door: the setup record, each service with whether it runs,
/// the newest release beside the one installed, and the machine's card.
fn install(config: &Config) -> (u16, Value) {
    let Some(state) = crate::setup::read_state() else {
        return error(
            404,
            format!(
                "no setup is recorded at {}; this door reports an install nils setup made",
                crate::setup::state_path().display()
            ),
        );
    };
    let mut doc = crate::setup::install_doc(&state);
    let every = Duration::from_secs(config.poll_seconds.max(60));
    let newest = cached(&NEWEST, every, || {
        match crate::update::newest_version(&crate::update::engine_base(None)) {
            Ok(v) => json!({ "version": v }),
            Err(e) => json!({ "error": e.message }),
        }
    });
    let installed = state.parts.get("engine").map(|p| p.version.clone());
    let newer = match (newest["version"].as_str(), installed.as_deref()) {
        (Some(n), Some(have)) if crate::update::newer(n, have) => json!(n),
        _ => Value::Null,
    };
    doc["release"] = json!({
        "installed": installed,
        "newest": newest["version"],
        "newer": newer,
        "error": newest["error"],
        "command": "nils update --all",
    });
    doc["machine"] = cached(&MACHINE, Duration::from_secs(3600), || {
        let card = crate::setup::probe_card();
        json!({
            "card": card.as_ref().map(|c| json!({ "name": c.name, "memory_gb": c.memory_gb })),
            "advice": crate::setup::card_advice(card.as_ref().map(|c| c.memory_gb)),
        })
    });
    (200, doc)
}

const LOOK_FOLDERS: usize = 64;
const LOOK_COUNT_TO: usize = 50_000;
const LOOK_SAMPLE: usize = 16;

/// What a folder holds, before it is added as a source: each folder inside
/// with how many files it has, how many of a sample are DICOM, and what those
/// are by modality and scanner. Only the equipment fields of a header are
/// read, never a person's, and links are not followed.
pub(crate) fn look(path: &Path) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    let shown = path.display().to_string();
    let meta = std::fs::metadata(path);
    if !meta.as_ref().is_ok_and(|m| m.is_dir()) {
        return json!({ "path": shown, "exists": meta.is_ok(), "directory": false, "readable": false, "folders": [], "here": null, "partial": false });
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return json!({ "path": shown, "exists": true, "directory": true, "readable": false, "folders": [], "here": null, "partial": false });
    };
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            dirs.push(entry.path());
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
    dirs.sort();
    files.sort();
    let mut partial = dirs.len() > LOOK_FOLDERS;
    let mut folders = Vec::new();
    for dir in dirs.iter().take(LOOK_FOLDERS) {
        if Instant::now() > deadline {
            partial = true;
            break;
        }
        let (listed, capped) = files_of(dir, deadline);
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        folders.push(describe(&name, &listed, capped));
    }
    json!({
        "path": shown,
        "exists": true,
        "directory": true,
        "readable": true,
        "folders": folders,
        "here": describe("", &files, false),
        "partial": partial,
    })
}

/// The files under a folder, links not followed, to a count and a deadline.
fn files_of(root: &Path, deadline: Instant) -> (Vec<PathBuf>, bool) {
    let mut out = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        if Instant::now() > deadline {
            return (out, true);
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<_> = entries.flatten().collect();
        here.sort_by_key(|e| e.file_name());
        for entry in here {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                queue.push(entry.path());
            } else if kind.is_file() {
                out.push(entry.path());
                if out.len() >= LOOK_COUNT_TO {
                    return (out, true);
                }
            }
        }
    }
    (out, false)
}

/// One folder in the words a person decides by: its files, a sample sniffed
/// for DICOM, and what the DICOM in the sample is.
fn describe(name: &str, files: &[PathBuf], capped: bool) -> Value {
    use dicom_dictionary_std::tags;
    let step = (files.len() / LOOK_SAMPLE).max(1);
    let sample: Vec<&PathBuf> = files.iter().step_by(step).take(LOOK_SAMPLE).collect();
    let mut dicom = 0u64;
    let mut modalities: BTreeMap<String, u64> = BTreeMap::new();
    let mut scanners: BTreeSet<String> = BTreeSet::new();
    for file in &sample {
        match nils_dicom::sniff::sniff(file) {
            nils_dicom::sniff::Sniff::Part10 => {}
            nils_dicom::sniff::Sniff::BareDataset => {
                dicom += 1;
                continue;
            }
            _ => continue,
        }
        dicom += 1;
        let Ok(object) = dicom_object::OpenFileOptions::new()
            .read_until(tags::PIXEL_DATA)
            .open_file(file)
        else {
            continue;
        };
        let word = |tag| {
            object
                .element(tag)
                .ok()
                .and_then(|e| e.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        *modalities
            .entry(word(tags::MODALITY).unwrap_or_else(|| "unknown".to_string()))
            .or_default() += 1;
        let scanner = format!(
            "{} {}",
            word(tags::MANUFACTURER).unwrap_or_default(),
            word(tags::MANUFACTURER_MODEL_NAME).unwrap_or_default()
        );
        if !scanner.trim().is_empty() {
            scanners.insert(scanner.trim().to_string());
        }
    }
    json!({
        "name": name,
        "files": files.len(),
        "capped": capped,
        "sampled": sample.len(),
        "dicom": dicom,
        "modalities": modalities,
        "scanners": scanners.len(),
    })
}

fn body_json(request: &mut Request) -> Value {
    let mut text = String::new();
    let _ = request.as_reader().read_to_string(&mut text);
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

fn started(result: Result<Value, (u16, String)>) -> (u16, Value) {
    match result {
        Ok(doc) => (202, doc),
        Err((status, why)) => error(status, why),
    }
}

fn handle(config: &Config, node: &str, busy: &Mutex<()>, runs: &Runs, mut request: Request) {
    let method = request.method().clone();
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let Some(who) = principal(config, &request) else {
        let (s, b) = error(401, "a bearer token the supervisor knows is required");
        return reply(request, s, b);
    };
    let (status, body) = match (method.clone(), path.as_str()) {
        (Method::Get, "/api/supervise/capabilities") => (200, capabilities(config, node)),
        (Method::Get, "/api/supervise/log") => (200, json!({ "rows": log_rows(&config.log, 200) })),
        (Method::Get, "/api/supervise/install") => install(config),
        (Method::Post, "/api/supervise/restart") => {
            let doc = body_json(&mut request);
            let mut args = vec!["supervise".to_string(), "restart".to_string()];
            if let Some(part) = doc["part"].as_str() {
                if !["engine", "desk", "gateway", "assistant", "postgres", "all"].contains(&part) {
                    let (s, b) = error(
                        400,
                        "part: engine, desk, gateway, assistant, postgres or all",
                    );
                    return reply(request, s, b);
                }
                args.extend(["--part".to_string(), part.to_string()]);
            }
            started(runs.start("restart", &args))
        }
        (Method::Post, "/api/supervise/reapply") => {
            let doc = body_json(&mut request);
            let mut args = vec!["supervise".to_string(), "reapply".to_string()];
            match doc["part"].as_str() {
                None | Some("all") => {}
                Some("engine") => args.extend(["--part".to_string(), "engine".to_string()]),
                Some(_) => {
                    let (s, b) = error(400, "part: engine, or none for every part");
                    return reply(request, s, b);
                }
            }
            started(runs.start("reapply", &args))
        }
        (Method::Post, "/api/supervise/update-all") => {
            started(runs.start("update", &["update".to_string(), "--all".to_string()]))
        }
        (Method::Get, p) if p.starts_with("/api/supervise/runs/") => {
            match runs.show(&p["/api/supervise/runs/".len()..]) {
                Some(doc) => (200, doc),
                None => error(404, "no run by that id"),
            }
        }
        (Method::Post, "/api/supervise/look") => {
            let doc = body_json(&mut request);
            match doc["path"]
                .as_str()
                .map(str::trim)
                .filter(|p| p.starts_with('/'))
            {
                Some(p) => (200, look(Path::new(p))),
                None => error(400, "path: an absolute path on this machine"),
            }
        }
        (Method::Post, "/api/supervise/update") => {
            let mut text = String::new();
            let _ = request.as_reader().read_to_string(&mut text);
            let doc: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let Some(name) = doc["part"].as_str() else {
                let (s, b) = error(400, "part: the name of a part this supervisor manages");
                return reply(request, s, b);
            };
            let Some(part) = config.parts.iter().find(|p| p.name == name) else {
                let (s, b) = error(404, format!("{name} is not a part this supervisor manages"));
                return reply(request, s, b);
            };
            let _guard = match busy.try_lock() {
                Ok(g) => g,
                Err(_) => {
                    let (s, b) = error(409, "an update is already running");
                    return reply(request, s, b);
                }
            };
            match update(
                config,
                part,
                doc["version"].as_str(),
                doc["allow_contract_change"].as_bool().unwrap_or(false),
            ) {
                Ok(row) => (200, json!({ "applied": row, "by": who })),
                Err(why) => (
                    409,
                    json!({ "error": why, "disclosure": "safe", "part": name }),
                ),
            }
        }
        _ => error(
            404,
            format!("{method} {path} is not a door; GET /api/supervise/capabilities lists them"),
        ),
    };
    reply(request, status, body);
}

/// The service: the door on the bind address, and a poll that logs a newer version when the channel names one.
pub(crate) fn run(config_path: &Path, bind: Option<String>) -> Result<(), Exit> {
    let mut config = Config::load(config_path)?;
    if let Some(b) = bind {
        config.bind = b;
    }
    let server = tiny_http::Server::http(&config.bind)
        .map_err(|e| fail(format!("cannot listen on {}: {e}", config.bind)))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| config.bind.clone());
    println!(
        "nils supervise   {bound}   parts {}   trust {}",
        config.parts.len(),
        config.trust.display()
    );
    let node = nils_registry::job::hostname();
    let runs = Runs {
        dir: config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join("runs"),
        children: Mutex::new(HashMap::new()),
    };
    let config = Arc::new(config);
    let busy = Arc::new(Mutex::new(()));
    {
        let config = Arc::clone(&config);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(config.poll_seconds.max(5)));
                for p in &config.parts {
                    if let Ok((l, _)) = latest(&p.channel, &p.name)
                        && p.installed()
                            .map(|m| m.version != l.version)
                            .unwrap_or(true)
                    {
                        eprintln!("supervise: {} has {} on its channel", p.name, l.version);
                    }
                }
            }
        });
    }
    for request in server.incoming_requests() {
        handle(&config, &node, &busy, &runs, request);
    }
    Ok(())
}

// ---------------------------------------------------------------------
// the command line

#[derive(Debug, Subcommand)]
pub(crate) enum SuperviseCommand {
    /// Make a signing key pair: supervise.key (private, mode 600) and supervise.pub (for the trust file)
    Keygen {
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// Make an artifact from a directory: the tarball and the manifest, unsigned
    Pack(PackArgs),
    /// Sign a manifest; the signature goes beside it as .sig
    Sign {
        #[arg(long, value_name = "FILE")]
        key: PathBuf,
        manifest: PathBuf,
    },
    /// Verify an artifact by its manifest: the signature, the tarball's digest, the target; exit 1 with the reason
    Verify(VerifyArtifactArgs),
    /// Run the service: the door on the bind address and the poll of the channel
    Run {
        #[arg(long, value_name = "FILE")]
        config: PathBuf,
        /// Overrides the config's bind; port 0 picks a free one and prints it
        #[arg(long, value_name = "ADDR")]
        bind: Option<String>,
    },
    /// Restart one part of the install nils setup made, or every part in order
    #[command(hide = true)]
    Restart {
        #[arg(long)]
        part: Option<String>,
    },
    /// Make the install follow the registry: the engine's unit or container
    /// written again with every source place, or every part's
    #[command(hide = true)]
    Reapply {
        #[arg(long)]
        part: Option<String>,
    },
    /// Run a command of this binary for a run the service started, and record how it ended
    #[command(hide = true)]
    Record {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// One update by hand, without the service: what the door does for a part
    Update {
        #[arg(long, value_name = "FILE")]
        config: PathBuf,
        part: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        allow_contract_change: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct PackArgs {
    #[arg(long)]
    pub part: String,
    #[arg(long)]
    pub version: String,
    /// The target the files are built for; this host's when absent
    #[arg(long)]
    pub target: Option<String>,
    /// A contract the part speaks, as NAME=VERSION; repeatable
    #[arg(long = "contract", value_name = "NAME=VERSION")]
    pub contracts: Vec<String>,
    /// The directory whose files become the artifact
    #[arg(long, value_name = "DIR")]
    pub dir: PathBuf,
    /// Where the three files go
    #[arg(long, value_name = "DIR")]
    pub out: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct VerifyArtifactArgs {
    /// The trust file: public keys, one hex key per line
    #[arg(long, value_name = "FILE")]
    pub trust: PathBuf,
    pub manifest: PathBuf,
    /// The target the artifact must be built for; this host's when absent, `any` to skip
    #[arg(long)]
    pub target: Option<String>,
    /// The manifest of what is installed, to refuse a contract whose major moved
    #[arg(long, value_name = "FILE")]
    pub installed: Option<PathBuf>,
    #[arg(long)]
    pub allow_contract_change: bool,
}

/// The install nils setup recorded, for the commands that act on it.
fn recorded() -> Result<crate::setup::State, Exit> {
    crate::setup::read_state().ok_or_else(|| {
        usage(format!(
            "no setup is recorded at {}",
            crate::setup::state_path().display()
        ))
    })
}

pub(crate) fn command(command: SuperviseCommand) -> Result<(), Exit> {
    match command {
        SuperviseCommand::Keygen { out } => {
            let (private, public) = keygen(&out)?;
            println!(
                "private key {} (mode 600, keep it off the hosts)\npublic key  {} (put its line in the trust file)",
                private.display(),
                public.display()
            );
            Ok(())
        }
        SuperviseCommand::Pack(args) => {
            let mut contracts = BTreeMap::new();
            for c in &args.contracts {
                let (k, v) = c
                    .split_once('=')
                    .ok_or_else(|| usage(format!("--contract takes NAME=VERSION, not {c}")))?;
                contracts.insert(k.to_string(), v.to_string());
            }
            let manifest = pack(
                &args.part,
                &args.version,
                &args.target.clone().unwrap_or_else(host_target),
                &args.dir,
                &args.out,
                contracts,
            )?;
            println!("{}", manifest.display());
            Ok(())
        }
        SuperviseCommand::Sign { key, manifest } => {
            let sig = sign(&key, &manifest)?;
            println!("{}", sig.display());
            Ok(())
        }
        SuperviseCommand::Verify(args) => {
            let keys = trust_keys(&args.trust)?;
            let installed = match &args.installed {
                Some(p) => Some(
                    serde_json::from_slice::<Manifest>(&read(p)?)
                        .map_err(|e| usage(format!("{}: {e}", p.display())))?
                        .contracts,
                ),
                None => None,
            };
            let target = match args.target.as_deref() {
                Some("any") => None,
                Some(t) => Some(t.to_string()),
                None => Some(host_target()),
            };
            match verify(
                &args.manifest,
                &keys,
                &Expect {
                    target,
                    installed,
                    allow_contract_change: args.allow_contract_change,
                },
            ) {
                Ok((m, tarball)) => {
                    println!("{}", serde_json::to_string_pretty(&json!({ "verified": true, "manifest": m, "tarball": tarball.display().to_string() })).unwrap_or_default());
                    Ok(())
                }
                Err(r) => Err(fail(format!("refused ({}): {r}", r.name()))),
            }
        }
        SuperviseCommand::Run { config, bind } => run(&config, bind),
        SuperviseCommand::Record { dir, id, command } => record(&dir, &id, &command),
        SuperviseCommand::Restart { part } => {
            let state = recorded()?;
            crate::setup::restart_units(&state, part.as_deref()).map(|_| ())
        }
        SuperviseCommand::Reapply { part } => {
            let state = recorded()?;
            match part.as_deref() {
                Some("engine") => crate::setup::reapply_engine(&state),
                None | Some("all") => {
                    crate::setup::restart_after_update(None);
                    Ok(())
                }
                Some(other) => Err(usage(format!("{other}: reapply takes engine or all"))),
            }
        }
        SuperviseCommand::Update {
            config,
            part,
            version,
            allow_contract_change,
        } => {
            let config = Config::load(&config)?;
            let p = config
                .parts
                .iter()
                .find(|p| p.name == part)
                .ok_or_else(|| usage(format!("{part} is not a part the config names")))?;
            match update(&config, p, version.as_deref(), allow_contract_change) {
                Ok(row) => {
                    println!("{}", serde_json::to_string_pretty(&row).unwrap_or_default());
                    Ok(())
                }
                Err(why) => Err(fail(why)),
            }
        }
    }
}
