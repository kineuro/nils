// SPDX-License-Identifier: AGPL-3.0-only

//! The software place (record 57 P5; Wave 7b, section 6): one copy of each
//! tool, for NILS and for people, in a plain folder laid out as a site's
//! shared software folder is, extended with manifests and modules:
//!
//! ```text
//! <place>/
//!   <app>/<version>/image            the SIF or sandbox; on docker or podman the manifest names image@sha256
//!   <app>/<version>/manifest.json    source and digest, pack hash, size, installed or registered, by whom, when, verified when
//!   <app>/<version>/bin/<cmd>        one command per tool entry point
//!   <app>/bin/<app>-run              the launcher people use today
//!   bin/                             links for people's PATH
//!   modules/<app>/<version>.lua      Lmod modules: module use <place>/modules
//!   licences/<app>/                  site licence files, readable by the engine's account alone
//!   externals.yaml                   registered installs
//! ```
//!
//! This module is the part that needs no registry: the layout, the
//! manifest, the wrappers and the module file, the fingerprint of an
//! install, the words a runtime is given to pull an image by its digest and
//! to say which digest it got, and the sections of a pipeline pack that
//! describe a tool (`source`, `licence`, `tests`). The binary owns the
//! place's path, the installs themselves, the acceptances and the doors.
//!
//! Wrappers find the place from their own location, so moving the folder
//! does not break them. Nothing here, and nothing in the runner, pushes an
//! image anywhere (record 57 G9): the engine pulls by digest from a tool's
//! own source or registers a copy the site holds.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::runtime::{self, ImageForm};

/// The file names of the layout.
pub const IMAGE: &str = "image";
pub const MANIFEST: &str = "manifest.json";
pub const PACK: &str = "nils-pack.yaml";
pub const COMMANDS: &str = "bin";
pub const LINKS: &str = "bin";
pub const MODULES: &str = "modules";
pub const LICENCES: &str = "licences";
pub const EXTERNALS: &str = "externals.yaml";

/// Where a test run's input is mounted in a container.
pub const TEST_INPUT: &str = "/input";

/// How an install is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An apptainer SIF file.
    Sif,
    /// An apptainer sandbox folder, or a tool's home folder.
    Sandbox,
    /// An image in a docker or podman store, named `repository@sha256:<hex>`.
    Image,
    /// One executable file, such as the converter.
    Binary,
}

impl Kind {
    pub const WORDS: [&'static str; 4] = ["sif", "sandbox", "image", "binary"];

    pub fn parse(text: &str) -> Option<Kind> {
        Some(match text.trim() {
            "sif" => Kind::Sif,
            "sandbox" => Kind::Sandbox,
            "image" => Kind::Image,
            "binary" => Kind::Binary,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Sif => "sif",
            Kind::Sandbox => "sandbox",
            Kind::Image => "image",
            Kind::Binary => "binary",
        }
    }

    /// The kind apptainer's image form is kept as.
    pub fn of_form(form: ImageForm) -> Kind {
        match form {
            ImageForm::Sif => Kind::Sif,
            ImageForm::Sandbox => Kind::Sandbox,
        }
    }

    /// The apptainer image form of a kind that is one.
    pub fn form(self) -> Option<ImageForm> {
        match self {
            Kind::Sif => Some(ImageForm::Sif),
            Kind::Sandbox => Some(ImageForm::Sandbox),
            Kind::Image | Kind::Binary => None,
        }
    }

    /// Whether the install is a file or folder on the host.
    pub fn on_disk(self) -> bool {
        !matches!(self, Kind::Image)
    }
}

/// How an install came: installed by the engine, which may update and
/// remove it, or registered as the site's, which the engine never touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum How {
    Installed,
    Registered,
}

impl How {
    pub fn name(self) -> &'static str {
        match self {
            How::Installed => "installed",
            How::Registered => "registered",
        }
    }
}

/// `<app>@<version>`, as the commands and the doors name a package.
pub fn parse_spec(text: &str) -> Result<(String, String), String> {
    let text = text.trim();
    let Some((app, version)) = text.split_once('@') else {
        return Err(format!(
            "{text} names no package: a package is <app>@<version>, as freesurfer@8.2.0"
        ));
    };
    let (app, version) = (app.trim(), version.trim());
    if app.is_empty() || version.is_empty() {
        return Err(format!(
            "{text} names no package: a package is <app>@<version>, as freesurfer@8.2.0"
        ));
    }
    if !name_ok(app) || !name_ok(version) {
        return Err(format!(
            "{text} is not a package name: an app and a version are letters, digits, '.', '-', \
             '_' and '+', with no slash"
        ));
    }
    Ok((app.to_string(), version.to_string()))
}

/// A word that may be a folder's name under the place.
pub fn name_ok(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with('.')
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

/// The place's folders and files, by their rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub place: PathBuf,
}

impl Layout {
    pub fn new(place: &Path) -> Layout {
        Layout {
            place: place.to_path_buf(),
        }
    }

    /// `<place>/<app>/<version>`.
    pub fn install_dir(&self, app: &str, version: &str) -> PathBuf {
        self.place.join(app).join(version)
    }

    /// `<place>/<app>/<version>/image`.
    pub fn image(&self, app: &str, version: &str) -> PathBuf {
        self.install_dir(app, version).join(IMAGE)
    }

    pub fn manifest(&self, app: &str, version: &str) -> PathBuf {
        self.install_dir(app, version).join(MANIFEST)
    }

    /// The copy of the pack an install was made with, where one was given.
    pub fn pack(&self, app: &str, version: &str) -> PathBuf {
        self.install_dir(app, version).join(PACK)
    }

    /// `<place>/<app>/<version>/bin`, one command per entry point.
    pub fn commands(&self, app: &str, version: &str) -> PathBuf {
        self.install_dir(app, version).join(COMMANDS)
    }

    /// `<place>/<app>/bin/<app>-run`.
    pub fn launcher(&self, app: &str) -> PathBuf {
        self.place.join(app).join("bin").join(format!("{app}-run"))
    }

    /// `<place>/bin`, the links for people's PATH.
    pub fn links(&self) -> PathBuf {
        self.place.join(LINKS)
    }

    /// `<place>/modules/<app>/<version>.lua`.
    pub fn module(&self, app: &str, version: &str) -> PathBuf {
        self.place
            .join(MODULES)
            .join(app)
            .join(format!("{version}.lua"))
    }

    /// `<place>/licences/<app>`.
    pub fn licences(&self, app: &str) -> PathBuf {
        self.place.join(LICENCES).join(app)
    }

    pub fn externals(&self) -> PathBuf {
        self.place.join(EXTERNALS)
    }

    /// Every manifest under the place, `<app>/<version>/manifest.json`, in
    /// name order; a manifest that cannot be read is skipped.
    pub fn installs(&self) -> Vec<Manifest> {
        let mut out = Vec::new();
        let Ok(apps) = std::fs::read_dir(&self.place) else {
            return out;
        };
        let mut apps: Vec<PathBuf> = apps.flatten().map(|e| e.path()).collect();
        apps.sort();
        for app in apps {
            let Some(name) = app.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name_ok(name) || matches!(name, LINKS | MODULES | LICENCES) || !app.is_dir() {
                continue;
            }
            let Ok(versions) = std::fs::read_dir(&app) else {
                continue;
            };
            let mut versions: Vec<PathBuf> = versions.flatten().map(|e| e.path()).collect();
            versions.sort();
            for v in versions {
                if let Some(m) = Manifest::read(&v.join(MANIFEST)) {
                    out.push(m);
                }
            }
        }
        out
    }
}

/// What a version probe said, what the fingerprint was, whether the pack's
/// test ran: one of the three results a verification records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    /// Whether it was run at all; a pack that declares none leaves it unrun.
    pub ran: bool,
    pub passed: Option<bool>,
    /// What was found, or why it did not run.
    pub said: String,
}

impl Checked {
    pub fn not_run(why: &str) -> Checked {
        Checked {
            ran: false,
            passed: None,
            said: why.to_string(),
        }
    }

    pub fn result(passed: bool, said: String) -> Checked {
        Checked {
            ran: true,
            passed: Some(passed),
            said,
        }
    }

    /// Whether it counts against the install: run and failed.
    pub fn failed(&self) -> bool {
        self.ran && self.passed == Some(false)
    }
}

/// The three results of a verification (section 6.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Verification {
    pub probe: Option<Checked>,
    pub fingerprint: Option<Checked>,
    pub test: Option<Checked>,
}

impl Verification {
    pub fn failed(&self) -> bool {
        [&self.probe, &self.fingerprint, &self.test]
            .into_iter()
            .flatten()
            .any(Checked::failed)
    }
}

/// `manifest.json`: what an install is, where it came from, and when it
/// was verified. Never a byte of a licence file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub app: String,
    pub version: String,
    pub kind: Kind,
    pub how: How,
    /// False for a registered install: the engine never updates or removes it.
    pub managed: bool,
    /// The OCI reference the install pulled, `repository@sha256:<hex>`; none
    /// for a registered install.
    #[serde(default)]
    pub source: Option<String>,
    /// The digest the pull asked for and the one it got: equal, or the
    /// install failed.
    #[serde(default)]
    pub digest_asked: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    /// How the digest got was known: the store's repository digest after a
    /// docker or podman pull, or the reference's digest apptainer's OCI
    /// client verified.
    #[serde(default)]
    pub digest_how: Option<String>,
    /// The sha256 of the pack it was installed or registered with, where
    /// one was given.
    #[serde(default)]
    pub pack_sha256: Option<String>,
    /// Where the image is: `image` for a SIF or sandbox the engine built
    /// (relative to the install's folder), the registered path, the
    /// `repository@sha256:<hex>` an image in a docker or podman store is
    /// named by, or a binary's file name.
    pub image: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// The runtime it was pulled with or runs under.
    #[serde(default)]
    pub runtime: Option<String>,
    /// Who installed or registered it, and when.
    pub by: String,
    pub at: String,
    /// The one command people run it by (`recon-all`), where the pack or
    /// the descriptor names one; the wrappers are written for these.
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub verified_at: Option<String>,
    #[serde(default)]
    pub verification: Verification,
    /// The pack's licence section as recorded: spdx, terms, whether an
    /// acceptance is needed and the site file's variable. Never its content.
    #[serde(default)]
    pub licence: Value,
}

impl Manifest {
    pub fn read(path: &Path) -> Option<Manifest> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Written whole, replacing what was there.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let part = path.with_extension("json.part");
        let _ = std::fs::remove_file(&part);
        crate::files::write_new(&part, text.as_bytes())?;
        std::fs::rename(&part, path)
    }

    /// `<app>@<version>`.
    /// Whether the last verification ran and passed: `verified_at` is the
    /// time of the last one, whatever it found, so a run asks this.
    pub fn verified_ok(&self) -> bool {
        self.verified_at.is_some() && !self.verification.failed()
    }

    pub fn spec(&self) -> String {
        format!("{}@{}", self.app, self.version)
    }

    /// Where the image is on the host, for a kind kept on disk: the
    /// install's own `image`, or the registered path.
    pub fn image_path(&self, layout: &Layout) -> Option<PathBuf> {
        if !self.kind.on_disk() {
            return None;
        }
        let p = Path::new(&self.image);
        Some(if p.is_absolute() {
            p.to_path_buf()
        } else {
            layout.install_dir(&self.app, &self.version).join(p)
        })
    }

    /// What a runtime is given to run it: the image path, or the reference.
    pub fn runnable(&self, layout: &Layout) -> String {
        match self.image_path(layout) {
            Some(p) => p.display().to_string(),
            None => self.image.clone(),
        }
    }

    pub fn doc(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

// --------------------------------------------------------------- the pack

/// A version probe: a command run in the tool, and the text its output
/// must hold.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probe {
    pub command: Vec<String>,
    pub expect: String,
}

/// The pack's test: a small public input and what the run must say.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TestRun {
    /// A folder on the host, or an https address of an archive or file;
    /// none runs the command with no input.
    pub input: Option<String>,
    pub command: Vec<String>,
    /// Text the run's output must hold; empty takes a clean exit alone.
    pub expect: String,
}

/// Where a site's licence file is seen by the tool.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SiteFile {
    /// The variable that names the mount, such as `FS_LICENSE`.
    pub env: String,
    /// Where the container sees the file.
    pub mount: String,
    /// The file's name under `licences/<app>/`, where the pack names one.
    pub name: Option<String>,
}

/// The pack's `licence` section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Licence {
    pub spdx: Option<String>,
    pub redistributable: Option<bool>,
    pub site_file: Option<SiteFile>,
    pub terms_address: Option<String>,
    pub terms_version: Option<String>,
    /// Whether an administrator must accept the terms before a run.
    pub accept: bool,
    /// Other variables the terms want set the same way (`FS_ALLOW_DEEP`).
    pub env: Vec<(String, String)>,
}

impl Licence {
    /// `{spdx, redistributable, terms: {address, version}, accept,
    /// site_file: {env, mount}}`, as the manifest records it.
    pub fn doc(&self) -> Value {
        json!({
            "spdx": self.spdx,
            "redistributable": self.redistributable,
            "terms": {"address": self.terms_address, "version": self.terms_version},
            "accept": self.accept,
            "site_file": self.site_file.as_ref().map(|s| json!({"env": s.env, "mount": s.mount, "name": s.name})),
            "env": self.env.iter().map(|(k, v)| json!({"name": k, "value": v})).collect::<Vec<_>>(),
        })
    }
}

/// What a pipeline pack says of its tool: the `source`, `licence` and
/// `tests` sections (Wave 7b, section 5.2), read leniently so a pack
/// written for the full loader reads here as well.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackSoftware {
    /// The app and the version the tool is kept under, where the pack
    /// names them; else the pipeline's name and its tool version.
    pub app: Option<String>,
    pub version: Option<String>,
    /// The image's OCI reference with `@sha256`.
    pub image: Option<String>,
    pub probe: Option<Probe>,
    /// The paths under a sandbox or a home the fingerprint reads.
    pub key_files: Vec<String>,
    pub licence: Option<Licence>,
    pub test: Option<TestRun>,
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A command as a pack writes it: a list of words, or one line split as a
/// shell splits it.
fn command_of(v: &Value) -> Result<Vec<String>, String> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::String(line) => crate::words::split(line),
        Value::Array(words) => words
            .iter()
            .map(|w| text_of(w).ok_or_else(|| format!("a command's word is text, not {w}")))
            .collect(),
        other => Err(format!("a command is words, not {other}")),
    }
}

fn list_of(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(text_of).collect(),
        _ => Vec::new(),
    }
}

/// The software sections of a pack document.
pub fn pack_software(doc: &Value) -> Result<PackSoftware, String> {
    let mut out = PackSoftware::default();
    let source = &doc["source"];
    if !source.is_null() {
        if !source.is_object() {
            return Err("source is a mapping: image, probe and key_files".into());
        }
        out.app = text_of(&source["app"]);
        out.version = text_of(&source["version"]);
        out.image = text_of(&source["image"]);
        if let Some(image) = &out.image
            && !image.contains("@sha256:")
        {
            return Err(format!(
                "source.image is an OCI reference pinned by its digest, repository@sha256:<hex>, not {image}"
            ));
        }
        let probe = &source["probe"];
        if !probe.is_null() {
            let command =
                command_of(&probe["command"]).map_err(|e| format!("source.probe.command: {e}"))?;
            if command.is_empty() {
                return Err("source.probe names a command and the text expected".into());
            }
            out.probe = Some(Probe {
                command,
                expect: text_of(&probe["expect"]).unwrap_or_default(),
            });
        }
        out.key_files = list_of(&source["key_files"]);
    }
    let licence = &doc["licence"];
    if !licence.is_null() {
        if !licence.is_object() {
            return Err(
                "licence is a mapping: spdx, redistributable, site_file, terms, accept".into(),
            );
        }
        let site = &licence["site_file"];
        let site_file = site.is_object().then(|| SiteFile {
            env: text_of(&site["env"]).unwrap_or_default(),
            mount: text_of(&site["mount"]).unwrap_or_default(),
            name: text_of(&site["name"]),
        });
        if let Some(s) = &site_file
            && (s.env.is_empty() || s.mount.is_empty())
        {
            return Err("licence.site_file names env, the variable, and mount, where the tool sees the file".into());
        }
        let terms = &licence["terms"];
        let (address, version) = match terms {
            Value::String(a) => (Some(a.clone()), None),
            Value::Object(_) => (text_of(&terms["address"]), text_of(&terms["version"])),
            _ => (None, None),
        };
        let env = match &licence["env"] {
            Value::Object(m) => m
                .iter()
                .filter_map(|(k, v)| text_of(v).map(|v| (k.clone(), v)))
                .collect(),
            _ => Vec::new(),
        };
        let accept = licence["accept"].as_bool().unwrap_or(false);
        if accept && address.is_none() {
            return Err(
                "licence.accept asks an acceptance, so licence.terms names the terms' address"
                    .into(),
            );
        }
        out.licence = Some(Licence {
            spdx: text_of(&licence["spdx"]),
            redistributable: licence["redistributable"].as_bool(),
            site_file,
            terms_address: address,
            terms_version: version,
            accept,
            env,
        });
    }
    let tests = &doc["tests"];
    if !tests.is_null() {
        let t = if let Value::Array(a) = tests {
            a.first().cloned().unwrap_or(Value::Null)
        } else {
            tests.clone()
        };
        if t.is_object() {
            let command = command_of(&t["command"]).map_err(|e| format!("tests.command: {e}"))?;
            if command.is_empty() {
                return Err("tests names the command of the test run".into());
            }
            out.test = Some(TestRun {
                input: text_of(&t["input"]),
                command,
                expect: text_of(&t["expect"]).unwrap_or_default(),
            });
        }
    }
    Ok(out)
}

/// A pack's text read: its document, its software sections and its sha256.
pub fn parse_pack(text: &str) -> Result<(Value, PackSoftware, String), String> {
    let doc: Value = serde_saphyr::from_str(text)
        .map_err(|e| format!("the pack is not a YAML document: {e}"))?;
    let software = pack_software(&doc)?;
    Ok((doc, software, crate::sha256(text.as_bytes())))
}

// ---------------------------------------------------------- the fingerprint

/// The bytes of every file under a folder, links not followed.
fn tree_size(root: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            let m = std::fs::symlink_metadata(&p)?;
            if m.is_dir() {
                stack.push(p);
            } else if m.is_file() {
                total += m.len();
            }
        }
    }
    Ok(total)
}

fn hash_tree(root: &Path, keys: &[String], lines: &mut Vec<String>) -> std::io::Result<u64> {
    let mut total = 0u64;
    let mut stack: Vec<PathBuf> = keys.iter().map(|k| root.join(k)).collect();
    while let Some(p) = stack.pop() {
        let m = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            // a key file the install does not hold is part of the fingerprint
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let rel = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .into_owned();
                lines.push(format!("{rel}\tmissing"));
                continue;
            }
            Err(e) => return Err(e),
        };
        let rel = p
            .strip_prefix(root)
            .unwrap_or(&p)
            .to_string_lossy()
            .into_owned();
        if m.file_type().is_symlink() {
            let target = std::fs::read_link(&p)?.to_string_lossy().into_owned();
            lines.push(format!("{rel}\tlink\t{target}"));
        } else if m.is_dir() {
            for e in std::fs::read_dir(&p)? {
                stack.push(e?.path());
            }
        } else {
            let (size, hex) = crate::files::sha256_file(&p)?;
            total += size;
            lines.push(format!("{rel}\tfile\t{size}\t{hex}"));
        }
    }
    Ok(total)
}

/// What an install is, as a digest, with its size in bytes: a SIF or a
/// binary by the sha256 of its bytes; a sandbox or a home by the sha256 of
/// its `key_files` (every file under them by path, size and content, a
/// link by its target, a key file missing by name), so that a copy
/// elsewhere with the same bytes is the same tool (57-Q4) and libraries
/// added beside the key files, for a viewer, do not change it. Where the
/// pack names no key files the whole tree is read as the image cache was,
/// by every entry's path, kind, mode, size and modification time
/// ([`runtime::image_fingerprint`]), which a run can afford before it
/// starts. An image in a runtime's store has no file to read: its
/// fingerprint is the digest the runtime reports.
pub fn fingerprint(
    kind: Kind,
    path: &Path,
    key_files: &[String],
) -> std::io::Result<(String, u64)> {
    let meta = std::fs::symlink_metadata(path)?;
    match kind {
        Kind::Sif | Kind::Binary => {
            if !meta.is_file() {
                return Err(std::io::Error::other(format!(
                    "{} is not a file, which a {} is",
                    path.display(),
                    kind.name()
                )));
            }
            let (size, hex) = crate::files::sha256_file(path)?;
            Ok((format!("sha256:{hex}"), size))
        }
        Kind::Sandbox => {
            if !meta.is_dir() {
                return Err(std::io::Error::other(format!(
                    "{} is not a folder, which a sandbox is",
                    path.display()
                )));
            }
            if key_files.is_empty() {
                let digest = runtime::image_fingerprint(path, ImageForm::Sandbox)?;
                return Ok((digest, tree_size(path)?));
            }
            let mut lines = Vec::new();
            let size = hash_tree(path, key_files, &mut lines)?;
            lines.sort();
            Ok((crate::sha256(lines.join("\n").as_bytes()), size))
        }
        Kind::Image => Err(std::io::Error::other(
            "an image in a runtime's store is fingerprinted by the digest the runtime reports",
        )),
    }
}

// ------------------------------------------------- the runtime's words

/// The words after a docker or podman program that pull an image by its
/// pinned reference. Apptainer builds its file instead ([`runtime::build_argv`]).
pub fn pull_argv(reference: &str) -> Vec<String> {
    vec!["pull".to_string(), reference.to_string()]
}

/// The words that ask docker or podman what an image in its store is: its
/// repository digests, its size and its id, tab-separated.
pub fn inspect_argv(reference: &str) -> Vec<String> {
    vec![
        "image".to_string(),
        "inspect".to_string(),
        "--format".to_string(),
        "{{json .RepoDigests}}\t{{.Size}}\t{{.Id}}".to_string(),
        reference.to_string(),
    ]
}

/// What an inspect said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inspected {
    /// `repository@sha256:<hex>`, one per repository the image is known under.
    pub repo_digests: Vec<String>,
    pub size: Option<u64>,
    pub id: Option<String>,
}

impl Inspected {
    /// The digest the store holds the image under for `repository`, or the
    /// one digest it reports where it names one repository alone.
    pub fn digest_for(&self, repository: &str) -> Option<String> {
        let repo = repository.trim_end_matches('/');
        let mut found: Vec<&str> = self
            .repo_digests
            .iter()
            .filter_map(|d| d.rsplit_once('@'))
            .filter(|(r, _)| same_repository(r, repo))
            .map(|(_, d)| d)
            .collect();
        if found.is_empty() && self.repo_digests.len() == 1 {
            found = self
                .repo_digests
                .iter()
                .filter_map(|d| d.rsplit_once('@'))
                .map(|(_, d)| d)
                .collect();
        }
        found.first().map(|d| d.to_string())
    }
}

/// Whether two repository names are one: docker's store writes
/// `docker.io/library/busybox` as `busybox`, and `docker.io/x/y` as `x/y`.
fn same_repository(a: &str, b: &str) -> bool {
    let norm = |r: &str| -> String {
        let r = r
            .strip_prefix("docker.io/")
            .or_else(|| r.strip_prefix("index.docker.io/"))
            .unwrap_or(r);
        let r = r.strip_prefix("library/").unwrap_or(r);
        r.to_string()
    };
    a == b || norm(a) == norm(b)
}

/// [`inspect_argv`]'s answer read.
pub fn parse_inspect(text: &str) -> Inspected {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    let mut parts = line.split('\t');
    let digests: Vec<String> = parts
        .next()
        .and_then(|j| serde_json::from_str::<Vec<String>>(j.trim()).ok())
        .unwrap_or_default();
    let size = parts.next().and_then(|s| s.trim().parse::<u64>().ok());
    let id = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Inspected {
        repo_digests: digests,
        size,
        id,
    }
}

/// The repository and the digest of a pinned reference,
/// `repository@sha256:<hex>`.
pub fn split_reference(reference: &str) -> Option<(&str, &str)> {
    let (repo, digest) = reference.rsplit_once('@')?;
    digest.starts_with("sha256:").then_some((repo, digest))
}

/// The words after a runtime's program that run one command in an install
/// with no network, the input (where there is one) read-only at
/// [`TEST_INPUT`], and the current folder as the work folder: how a version
/// probe and the pack's test run (section 6.4).
pub fn exec_argv(
    rt: runtime::Kind,
    image: &str,
    command: &[String],
    input: Option<&Path>,
    work: &Path,
) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    match rt {
        runtime::Kind::Apptainer => {
            a.extend(
                [
                    "exec",
                    "--containall",
                    "--net",
                    "--network",
                    "none",
                    "--no-home",
                ]
                .map(String::from),
            );
            a.push("--bind".into());
            a.push(format!("{}:/work", work.display()));
            if let Some(input) = input {
                a.push("--bind".into());
                a.push(format!("{}:{TEST_INPUT}:ro", input.display()));
            }
            a.push("--pwd".into());
            a.push("/work".into());
            a.push(image.to_string());
        }
        runtime::Kind::Podman | runtime::Kind::Docker => {
            a.extend(["run", "--rm", "--network", "none"].map(String::from));
            a.push("-v".into());
            a.push(format!("{}:/work", work.display()));
            if let Some(input) = input {
                a.push("-v".into());
                a.push(format!("{}:{TEST_INPUT}:ro", input.display()));
            }
            a.push("-w".into());
            a.push("/work".into());
            a.push(image.to_string());
        }
    }
    a.extend(command.iter().cloned());
    a
}

/// The words that take an installed image out of a docker or podman store,
/// for the remove of an install the engine made.
pub fn remove_image_argv(reference: &str) -> Vec<String> {
    vec!["image".to_string(), "rm".to_string(), reference.to_string()]
}

// --------------------------------------------- the wrappers and the module

/// A word quoted for a POSIX shell.
pub fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '@' | ':' | '+' | '=')
        })
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The lines every wrapper starts with: the place found from the wrapper's
/// own location, `depth` folders above it (3 for `<app>/<version>/bin`, 2
/// for `<app>/bin`).
fn locate_lines(depth: usize) -> String {
    let up = (0..depth).map(|_| "..").collect::<Vec<_>>().join("/");
    format!(
        "set -eu\n\
         here=$(CDPATH= cd -- \"$(dirname -- \"$(readlink -f -- \"$0\")\")\" && pwd)\n\
         place=$(CDPATH= cd -- \"$here/{up}\" && pwd)\n"
    )
}

/// The variable a person sets to add flags to the runtime a wrapper runs.
pub const WRAPPER_FLAGS_ENV: &str = "NILS_SOFTWARE_FLAGS";

/// `<app>/<version>/bin/<cmd>`: runs one command of the install, in the
/// current folder, through the runtime it was installed for. The place is
/// found from the wrapper's own location, so a moved folder still runs;
/// `NILS_SOFTWARE_FLAGS` adds flags for the runtime (`--nv` for a card).
pub fn wrapper_text(m: &Manifest, command: &str) -> String {
    let mut s = format!(
        "#!/bin/sh\n# {} {}: {command}, written by NILS. The place is found from this file's own location.\n",
        m.app, m.version
    );
    s.push_str(&locate_lines(3));
    s.push_str(&format!(
        "install=\"$place/{}/{}\"\n",
        shell_quote(&m.app),
        shell_quote(&m.version)
    ));
    let image = match (m.kind.on_disk(), Path::new(&m.image).is_absolute()) {
        (true, false) => format!("\"$install/{}\"", shell_quote(&m.image)),
        _ => shell_quote(&m.image),
    };
    let flags = format!("${{{WRAPPER_FLAGS_ENV}:-}}");
    let runtime = m.runtime.as_deref().unwrap_or(match m.kind {
        Kind::Sif | Kind::Sandbox => "apptainer",
        Kind::Image => "docker",
        Kind::Binary => "",
    });
    match m.kind {
        Kind::Sif | Kind::Sandbox => s.push_str(&format!(
            "exec {runtime} exec --containall {flags} --bind \"$PWD\" --pwd \"$PWD\" {image} {} \"$@\"\n",
            shell_quote(command)
        )),
        Kind::Image => s.push_str(&format!(
            "exec {runtime} run --rm -i {flags} --user \"$(id -u):$(id -g)\" -v \"$PWD:$PWD\" -w \"$PWD\" {image} {} \"$@\"\n",
            shell_quote(command)
        )),
        Kind::Binary => s.push_str(&format!("exec {image} \"$@\"\n")),
    }
    s
}

/// `<app>/bin/<app>-run`: the launcher people use today, `<app>-run <cmd>
/// args`, which runs the command of the version `<APP>_VERSION` names, else
/// the one written here (the latest installed).
pub fn launcher_text(app: &str, default_version: &str) -> String {
    let var = env_name(app);
    let mut s = format!(
        "#!/bin/sh\n# {app}-run CMD ARGS: runs a command of {app}, written by NILS. The version is \
         ${var}_VERSION, else {default_version}.\n"
    );
    s.push_str(&locate_lines(2));
    s.push_str(&format!(
        "version=${{{var}_VERSION:-{}}}\n\
         if [ $# -eq 0 ]; then echo \"usage: {app}-run CMD [ARGS]; the commands are in $place/{}/$version/bin\" >&2; exit 2; fi\n\
         cmd=$1; shift\n\
         exec \"$place/{}/$version/bin/$cmd\" \"$@\"\n",
        shell_quote(default_version),
        shell_quote(app),
        shell_quote(app)
    ));
    s
}

/// `FREESURFER` for `freesurfer`, `LST_AI` for `lst-ai`.
pub fn env_name(app: &str) -> String {
    app.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// `modules/<app>/<version>.lua`, an Lmod module: `module use
/// <place>/modules`, then `module load <app>/<version>` puts the install's
/// commands on the PATH and names its image. The place is found from the
/// module file's own location.
pub fn module_text(m: &Manifest) -> String {
    let var = env_name(&m.app);
    let image = match (m.kind.on_disk(), Path::new(&m.image).is_absolute()) {
        (true, false) => format!("pathJoin(install, {})", lua_quote(&m.image)),
        _ => lua_quote(&m.image),
    };
    let how = format!(
        "{} by NILS{}",
        m.how.name(),
        m.source
            .as_deref()
            .map(|s| format!(" from {s}"))
            .unwrap_or_default()
    );
    format!(
        "-- {app} {version}: {how}. The place is found from this file's own location.\n\
         local here = myFileName()\n\
         local place = here:match(\"^(.*)/modules/[^/]+/[^/]+%.lua$\")\n\
         local install = pathJoin(place, {app_q}, {version_q})\n\
         whatis(\"Name: {app}\")\n\
         whatis(\"Version: {version}\")\n\
         whatis(\"Kind: {kind}, {how_word}\")\n\
         help([[{app} {version}, {how}.\nThe commands are {app}/{version}/bin under the software place; {app}-run CMD runs one.]])\n\
         prepend_path(\"PATH\", pathJoin(install, \"bin\"))\n\
         setenv(\"NILS_{var}_VERSION\", {version_q})\n\
         setenv(\"NILS_{var}_IMAGE\", {image})\n",
        app = m.app,
        version = m.version,
        app_q = lua_quote(&m.app),
        version_q = lua_quote(&m.version),
        kind = m.kind.name(),
        how_word = m.how.name(),
    )
}

fn lua_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

/// Write the install's wrappers, launcher, links and module from its
/// manifest: `<app>/<version>/bin/<cmd>` for each command, `<app>/bin/
/// <app>-run` (naming this version as its default), `bin/<cmd>` and
/// `bin/<app>-run` as relative links, and `modules/<app>/<version>.lua`.
/// A registered install gets them too, so people reach it the same way;
/// nothing under the registered path is touched.
pub fn write_wrappers(layout: &Layout, m: &Manifest) -> std::io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    let commands = layout.commands(&m.app, &m.version);
    std::fs::create_dir_all(&commands)?;
    for c in &m.commands {
        if !name_ok(c) {
            continue;
        }
        let path = commands.join(c);
        write_executable(&path, wrapper_text(m, c).as_bytes())?;
        written.push(path);
        link_into(
            &layout.links(),
            c,
            &format!("../{}/{}/bin/{c}", m.app, m.version),
        )?;
    }
    let launcher = layout.launcher(&m.app);
    if let Some(dir) = launcher.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_executable(&launcher, launcher_text(&m.app, &m.version).as_bytes())?;
    written.push(launcher);
    link_into(
        &layout.links(),
        &format!("{}-run", m.app),
        &format!("../{}/bin/{}-run", m.app, m.app),
    )?;
    let module = layout.module(&m.app, &m.version);
    if let Some(dir) = module.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_replacing(&module, module_text(m).as_bytes())?;
    written.push(module);
    Ok(written)
}

/// Take an install's wrappers, links and module away, for a remove: the
/// links in `bin/` that point into it, its commands, its module, and the
/// launcher where no other version of the app remains.
pub fn remove_wrappers(layout: &Layout, m: &Manifest) -> std::io::Result<()> {
    let into = format!("../{}/{}/", m.app, m.version);
    if let Ok(links) = std::fs::read_dir(layout.links()) {
        for e in links.flatten() {
            if let Ok(target) = std::fs::read_link(e.path())
                && target.to_string_lossy().starts_with(&into)
            {
                std::fs::remove_file(e.path())?;
            }
        }
    }
    let module = layout.module(&m.app, &m.version);
    if module.exists() {
        std::fs::remove_file(&module)?;
    }
    let others = layout
        .place
        .join(&m.app)
        .read_dir()
        .map(|d| {
            d.flatten().any(|e| {
                e.file_name() != std::ffi::OsStr::new(&m.version)
                    && e.file_name() != std::ffi::OsStr::new("bin")
                    && e.path().join(MANIFEST).is_file()
            })
        })
        .unwrap_or(false);
    if !others {
        let launcher = layout.launcher(&m.app);
        let _ = std::fs::remove_file(&launcher);
        let _ = std::fs::remove_file(layout.links().join(format!("{}-run", m.app)));
        let _ = std::fs::remove_dir(launcher.parent().unwrap_or(&layout.place));
        let _ = std::fs::remove_dir(layout.place.join(MODULES).join(&m.app));
    }
    Ok(())
}

fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let part = path.with_extension("part");
    let _ = std::fs::remove_file(&part);
    crate::files::write_new(&part, bytes)?;
    std::fs::rename(&part, path)
}

fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_replacing(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// A relative link `dir/name -> target`, replacing one there.
fn link_into(dir: &Path, name: &str, target: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let link = dir.join(name);
    if std::fs::symlink_metadata(&link).is_ok() {
        std::fs::remove_file(&link)?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, &link)
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        Ok(())
    }
}

// ------------------------------------------------------------- externals

/// One registered install, as `externals.yaml` records it, the way Spack
/// records externals: the spec, the kind, the path, `managed: false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    pub spec: String,
    pub kind: Kind,
    pub path: String,
    pub by: String,
    pub at: String,
}

/// `externals.yaml` read; a missing file is no externals.
pub fn read_externals(path: &Path) -> Result<Vec<External>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let doc: Value = serde_saphyr::from_str(&text)
        .map_err(|e| format!("{} is not a YAML document: {e}", path.display()))?;
    let Some(list) = doc["externals"].as_array() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in list {
        let Some(spec) = text_of(&e["spec"]) else {
            continue;
        };
        let Some(kind) = text_of(&e["kind"]).and_then(|k| Kind::parse(&k)) else {
            continue;
        };
        out.push(External {
            spec,
            kind,
            path: text_of(&e["path"]).unwrap_or_default(),
            by: text_of(&e["by"]).unwrap_or_default(),
            at: text_of(&e["at"]).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// `externals.yaml` written whole.
pub fn write_externals(path: &Path, externals: &[External]) -> std::io::Result<()> {
    let mut s = String::from(
        "# The installs registered as the site's (record 57 P5): the engine never updates or removes these.\nexternals:\n",
    );
    if externals.is_empty() {
        s = s.replace("externals:\n", "externals: []\n");
    }
    for e in externals {
        s.push_str(&format!(
            "  - spec: {}\n    kind: {}\n    path: {}\n    managed: false\n    by: {}\n    at: {}\n",
            yaml_scalar(&e.spec),
            e.kind.name(),
            yaml_scalar(&e.path),
            yaml_scalar(&e.by),
            yaml_scalar(&e.at)
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_replacing(path, s.as_bytes())
}

/// A scalar YAML reads back as the same text: JSON's quoting, which YAML
/// accepts.
fn yaml_scalar(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!(
            "nils-software-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(kind: Kind, how: How, image: &str) -> Manifest {
        Manifest {
            app: "freesurfer".into(),
            version: "8.2.0".into(),
            kind,
            how,
            managed: how == How::Installed,
            source: Some("docker.io/freesurfer/freesurfer@sha256:5648104ca75a24f6ee5a8786146f99b405c2bda1da12bff723a15056da8ddf6b".into()),
            digest_asked: Some("sha256:5648104ca75a24f6ee5a8786146f99b405c2bda1da12bff723a15056da8ddf6b".into()),
            digest: Some("sha256:5648104ca75a24f6ee5a8786146f99b405c2bda1da12bff723a15056da8ddf6b".into()),
            digest_how: None,
            pack_sha256: None,
            image: image.into(),
            size_bytes: Some(12),
            runtime: Some(match kind {
                Kind::Image => "docker".into(),
                _ => "apptainer".into(),
            }),
            by: "adm".into(),
            at: "2026-10-18T12:00:00Z".into(),
            commands: vec!["recon-all".into(), "mri_synthseg".into()],
            fingerprint: None,
            verified_at: None,
            verification: Verification::default(),
            licence: Value::Null,
        }
    }

    #[test]
    fn a_spec_is_an_app_and_a_version() {
        assert_eq!(
            parse_spec("freesurfer@8.2.0").unwrap(),
            ("freesurfer".to_string(), "8.2.0".to_string())
        );
        assert_eq!(
            parse_spec(" lst-ai@1.2.0+cu12 ").unwrap(),
            ("lst-ai".to_string(), "1.2.0+cu12".to_string())
        );
        for bad in ["freesurfer", "@8", "a@", "../x@1", "a/b@1", ".hidden@1"] {
            assert!(parse_spec(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_layout_is_the_record_s() {
        let l = Layout::new(Path::new("/srv/software"));
        assert_eq!(
            l.image("freesurfer", "8.2.0"),
            PathBuf::from("/srv/software/freesurfer/8.2.0/image")
        );
        assert_eq!(
            l.manifest("freesurfer", "8.2.0"),
            PathBuf::from("/srv/software/freesurfer/8.2.0/manifest.json")
        );
        assert_eq!(
            l.commands("freesurfer", "8.2.0"),
            PathBuf::from("/srv/software/freesurfer/8.2.0/bin")
        );
        assert_eq!(
            l.launcher("freesurfer"),
            PathBuf::from("/srv/software/freesurfer/bin/freesurfer-run")
        );
        assert_eq!(l.links(), PathBuf::from("/srv/software/bin"));
        assert_eq!(
            l.module("freesurfer", "8.2.0"),
            PathBuf::from("/srv/software/modules/freesurfer/8.2.0.lua")
        );
        assert_eq!(
            l.licences("freesurfer"),
            PathBuf::from("/srv/software/licences/freesurfer")
        );
        assert_eq!(l.externals(), PathBuf::from("/srv/software/externals.yaml"));
    }

    #[test]
    fn a_manifest_round_trips_and_is_listed() {
        let dir = tmp("manifest");
        let l = Layout::new(&dir);
        let m = manifest(Kind::Sif, How::Installed, IMAGE);
        m.write(&l.manifest(&m.app, &m.version)).unwrap();
        let back = Manifest::read(&l.manifest(&m.app, &m.version)).unwrap();
        assert_eq!(back, m);
        assert_eq!(
            back.image_path(&l),
            Some(dir.join("freesurfer/8.2.0/image"))
        );
        // the links, modules and licences folders are not apps
        std::fs::create_dir_all(l.links()).unwrap();
        std::fs::create_dir_all(l.place.join(MODULES).join("x")).unwrap();
        let listed = l.installs();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].spec(), "freesurfer@8.2.0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in runtime on the search path that prints the words it is
    /// given, one per line.
    fn stand_in_runtime(dir: &Path, name: &str) -> PathBuf {
        let bin = dir.join("stand-in-bin");
        std::fs::create_dir_all(&bin).unwrap();
        let p = bin.join(name);
        write_executable(
            &p,
            b"#!/bin/sh\nfor w in \"$@\"; do printf '%s\\n' \"$w\"; done\n",
        )
        .unwrap();
        bin
    }

    fn sh(path: &Path, args: &[&str], bin: &Path) -> String {
        let search = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = std::process::Command::new(path)
            .args(args)
            .env("PATH", search)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[cfg(unix)]
    #[test]
    fn wrappers_find_the_place_from_their_own_location_and_a_moved_place_still_runs() {
        let root = tmp("wrappers");
        let a = root.join("a");
        let l = Layout::new(&a);
        let m = manifest(Kind::Sif, How::Installed, IMAGE);
        std::fs::create_dir_all(l.install_dir(&m.app, &m.version)).unwrap();
        std::fs::write(l.image(&m.app, &m.version), b"not a sif").unwrap();
        let written = write_wrappers(&l, &m).unwrap();
        assert!(written.contains(&l.commands("freesurfer", "8.2.0").join("recon-all")));
        assert!(written.contains(&l.launcher("freesurfer")));
        assert!(written.contains(&l.module("freesurfer", "8.2.0")));
        let bin = stand_in_runtime(&root, "apptainer");
        // through the bin/ link, the wrapper runs the image under the place
        let said = sh(&l.links().join("recon-all"), &["-s", "x"], &bin);
        let words: Vec<&str> = said.lines().collect();
        assert_eq!(words[0], "exec");
        assert!(words.contains(&"--containall"));
        assert_eq!(
            words.iter().find(|w| w.ends_with("/image")).copied(),
            Some(a.join("freesurfer/8.2.0/image").to_str().unwrap())
        );
        let at = words.iter().position(|w| w.ends_with("/image")).unwrap();
        assert_eq!(&words[at + 1..], ["recon-all", "-s", "x"]);
        // the launcher people use today
        let said = sh(
            &l.links().join("freesurfer-run"),
            &["mri_synthseg", "--help"],
            &bin,
        );
        let words: Vec<&str> = said.lines().collect();
        assert!(words.contains(&"mri_synthseg"), "{said}");
        assert!(words.contains(&"--help"), "{said}");
        // moved whole, the wrappers find the new place
        let b = root.join("b");
        std::fs::rename(&a, &b).unwrap();
        let said = sh(&b.join("bin/recon-all"), &["-s", "x"], &bin);
        let words: Vec<&str> = said.lines().collect();
        assert_eq!(
            words.iter().find(|w| w.ends_with("/image")).copied(),
            Some(b.join("freesurfer/8.2.0/image").to_str().unwrap())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_module_names_the_install_s_paths_from_its_own_location() {
        let m = manifest(Kind::Sif, How::Installed, IMAGE);
        let lua = module_text(&m);
        assert!(lua.contains("local here = myFileName()"));
        assert!(lua.contains("local install = pathJoin(place, \"freesurfer\", \"8.2.0\")"));
        assert!(lua.contains("prepend_path(\"PATH\", pathJoin(install, \"bin\"))"));
        assert!(lua.contains("setenv(\"NILS_FREESURFER_IMAGE\", pathJoin(install, \"image\"))"));
        assert!(lua.contains("setenv(\"NILS_FREESURFER_VERSION\", \"8.2.0\")"));
        // a registered install names the site's path, an image its reference
        let r = manifest(
            Kind::Sandbox,
            How::Registered,
            "/data/shared/software/freesurfer/8.2.0",
        );
        assert!(module_text(&r).contains(
            "setenv(\"NILS_FREESURFER_IMAGE\", \"/data/shared/software/freesurfer/8.2.0\")"
        ));
        let d = manifest(
            Kind::Image,
            How::Installed,
            "docker.io/freesurfer/freesurfer@sha256:5648104ca75a24f6ee5a8786146f99b405c2bda1da12bff723a15056da8ddf6b",
        );
        assert!(module_text(&d).contains("docker.io/freesurfer/freesurfer@sha256:5648"));
        let w = wrapper_text(&d, "recon-all");
        assert!(w.contains("exec docker run --rm -i"), "{w}");
        assert!(
            w.contains("docker.io/freesurfer/freesurfer@sha256:5648"),
            "{w}"
        );
        assert!(w.contains("--user \"$(id -u):$(id -g)\""), "{w}");
    }

    #[test]
    fn a_sandbox_s_fingerprint_reads_its_key_files_alone() {
        let dir = tmp("fingerprint");
        let home = dir.join("fs");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::create_dir_all(home.join("models")).unwrap();
        std::fs::create_dir_all(home.join("lib")).unwrap();
        std::fs::write(home.join("bin/recon-all"), b"#!/bin/sh\necho 8.2.0\n").unwrap();
        std::fs::write(home.join("models/synthseg.h5"), b"weights").unwrap();
        let keys = ["bin".to_string(), "models".to_string()];
        let (f1, size) = fingerprint(Kind::Sandbox, &home, &keys).unwrap();
        assert!(f1.starts_with("sha256:"));
        assert_eq!(size, 21 + 7);
        // a viewer's library added beside them changes nothing
        std::fs::write(home.join("lib/libGL.so"), b"viewer").unwrap();
        let (f2, _) = fingerprint(Kind::Sandbox, &home, &keys).unwrap();
        assert_eq!(f1, f2);
        // a copy elsewhere with the same bytes is the same tool (57-Q4)
        let copy = dir.join("copy");
        std::fs::create_dir_all(copy.join("bin")).unwrap();
        std::fs::create_dir_all(copy.join("models")).unwrap();
        std::fs::write(copy.join("bin/recon-all"), b"#!/bin/sh\necho 8.2.0\n").unwrap();
        std::fs::write(copy.join("models/synthseg.h5"), b"weights").unwrap();
        assert_eq!(fingerprint(Kind::Sandbox, &copy, &keys).unwrap().0, f1);
        // a key file changed, or missing, changes it
        std::fs::write(copy.join("models/synthseg.h5"), b"other weights").unwrap();
        assert_ne!(fingerprint(Kind::Sandbox, &copy, &keys).unwrap().0, f1);
        std::fs::remove_file(copy.join("models/synthseg.h5")).unwrap();
        assert_ne!(fingerprint(Kind::Sandbox, &copy, &keys).unwrap().0, f1);
        // with no key files the whole tree is read, as the image cache was
        let (whole, size) = fingerprint(Kind::Sandbox, &home, &[]).unwrap();
        assert_eq!(
            whole,
            runtime::image_fingerprint(&home, ImageForm::Sandbox).unwrap()
        );
        assert_eq!(size, 21 + 7 + 6);
        // a SIF by its bytes
        std::fs::write(dir.join("x.sif"), b"sif bytes").unwrap();
        let (sif, n) = fingerprint(Kind::Sif, &dir.join("x.sif"), &[]).unwrap();
        assert_eq!(sif, crate::sha256(b"sif bytes"));
        assert_eq!(n, 9);
        assert!(fingerprint(Kind::Sif, &home, &[]).is_err());
        assert!(fingerprint(Kind::Image, &home, &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_runtime_s_words_pull_by_digest_and_say_what_came() {
        let reference = "docker.io/library/busybox@sha256:ab12";
        assert_eq!(pull_argv(reference), ["pull", reference]);
        let inspect = inspect_argv(reference);
        assert_eq!(&inspect[..3], ["image", "inspect", "--format"]);
        assert_eq!(inspect[4], reference);
        let said = parse_inspect(
            "[\"busybox@sha256:ab12\",\"docker.io/library/busybox@sha256:ab12\"]\t4261550\tsha256:deadbeef\n",
        );
        assert_eq!(said.size, Some(4_261_550));
        assert_eq!(said.id.as_deref(), Some("sha256:deadbeef"));
        assert_eq!(
            said.digest_for("docker.io/library/busybox").as_deref(),
            Some("sha256:ab12")
        );
        // docker's store writes the library name short
        let short = parse_inspect("[\"busybox@sha256:ab12\"]\t1\tsha256:x");
        assert_eq!(
            short.digest_for("docker.io/library/busybox").as_deref(),
            Some("sha256:ab12")
        );
        // another repository's digest is not this one's
        let other = parse_inspect(
            "[\"example.org/x/y@sha256:ff\",\"example.org/x/z@sha256:ee\"]\t1\tsha256:x",
        );
        assert_eq!(
            other.digest_for("example.org/x/y").as_deref(),
            Some("sha256:ff")
        );
        assert_eq!(other.digest_for("example.org/q").as_deref(), None);
        assert_eq!(parse_inspect("").digest_for("x"), None);
        assert_eq!(
            split_reference(reference),
            Some(("docker.io/library/busybox", "sha256:ab12"))
        );
        assert_eq!(split_reference("busybox:latest"), None);
        let probe = ["recon-all".to_string(), "--version".to_string()];
        let a = exec_argv(
            runtime::Kind::Apptainer,
            "/p/freesurfer/8.2.0/image",
            &probe,
            Some(Path::new("/in")),
            Path::new("/w"),
        );
        assert_eq!(a[0], "exec");
        assert!(a.windows(2).any(|w| w == ["--network", "none"]));
        assert!(a.contains(&"/in:/input:ro".to_string()));
        assert_eq!(
            &a[a.len() - 3..],
            ["/p/freesurfer/8.2.0/image", "recon-all", "--version"]
        );
        let d = exec_argv(
            runtime::Kind::Docker,
            reference,
            &probe,
            None,
            Path::new("/w"),
        );
        assert_eq!(&d[..4], ["run", "--rm", "--network", "none"]);
        assert_eq!(&d[d.len() - 3..], [reference, "recon-all", "--version"]);
        assert_eq!(remove_image_argv(reference), ["image", "rm", reference]);
    }

    /// Record 57 G9: no code path pushes an image anywhere. The runtime
    /// module and this one are the only places the engine speaks to a
    /// runtime about images; neither holds the word `push` as a runtime
    /// verb, and every argv builder here is one of pull, build, inspect,
    /// exec, run and rm.
    #[test]
    fn no_code_path_pushes_an_image() {
        // the verb, spelled so this test's own line is not a hit
        let verb = ["pu", "sh"].concat();
        let needles = [
            format!("\"{verb}\""),
            format!("{verb}_argv"),
            format!("\"--{verb}\""),
        ];
        for (name, source) in [
            ("runtime.rs", include_str!("runtime.rs")),
            ("software.rs", include_str!("software.rs")),
        ] {
            for (n, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                assert!(
                    needles.iter().all(|needle| !code.contains(needle.as_str())),
                    "{name}:{} speaks of pushing an image: {line}",
                    n + 1
                );
            }
        }
        let reference = "example.org/x@sha256:00";
        let verbs: Vec<String> = [
            pull_argv(reference)[0].clone(),
            inspect_argv(reference)[0].clone(),
            remove_image_argv(reference)[0].clone(),
            exec_argv(runtime::Kind::Docker, reference, &[], None, Path::new("/w"))[0].clone(),
            exec_argv(runtime::Kind::Apptainer, "/i", &[], None, Path::new("/w"))[0].clone(),
            runtime::build_argv(reference, Path::new("/x.sif"), ImageForm::Sif)[0].clone(),
        ]
        .into_iter()
        .collect();
        for v in verbs {
            assert!(
                ["pull", "image", "run", "exec", "build"].contains(&v.as_str()),
                "{v}"
            );
        }
    }

    #[test]
    fn externals_round_trip() {
        let dir = tmp("externals");
        let path = dir.join(EXTERNALS);
        assert!(read_externals(&path).unwrap().is_empty());
        let e = vec![External {
            spec: "freesurfer@8.2.0".into(),
            kind: Kind::Sandbox,
            path: "/data/shared/software/freesurfer/8.2.0".into(),
            by: "adm".into(),
            at: "2026-10-18T12:00:00Z".into(),
        }];
        write_externals(&path, &e).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("managed: false"), "{text}");
        assert_eq!(read_externals(&path).unwrap(), e);
        write_externals(&path, &[]).unwrap();
        assert!(read_externals(&path).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pack_s_software_sections_are_read() {
        let text = r#"
nils_pack: 1
pipeline: {name: freesurfer-recon-all, descriptor_sha256: "sha256:00"}
source:
  app: freesurfer
  version: 8.2.0
  image: "docker.io/freesurfer/freesurfer@sha256:5648104ca75a24f6ee5a8786146f99b405c2bda1da12bff723a15056da8ddf6b"
  probe: {command: "recon-all --version", expect: "freesurfer-linux-ubuntu22_x86_64-8.2.0"}
  key_files: [bin, models]
licence:
  spdx: LicenseRef-FreeSurfer
  redistributable: true
  site_file: {env: FS_LICENSE, mount: /license.txt, name: license.txt}
  terms: {address: "https://surfer.nmr.mgh.harvard.edu/registration.html", version: "2026"}
  accept: true
  env: {FS_ALLOW_DEEP: "1"}
tests:
  input: "https://example.org/small/input.tar.gz"
  command: ["mri_synthseg", "--i", "/input/t1.nii.gz", "--o", "out.nii.gz"]
  expect: "SynthSeg"
"#;
        let (_, s, digest) = parse_pack(text).unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(s.app.as_deref(), Some("freesurfer"));
        assert_eq!(s.version.as_deref(), Some("8.2.0"));
        assert!(s.image.as_deref().unwrap().contains("@sha256:5648"));
        let probe = s.probe.unwrap();
        assert_eq!(probe.command, ["recon-all", "--version"]);
        assert_eq!(probe.expect, "freesurfer-linux-ubuntu22_x86_64-8.2.0");
        assert_eq!(s.key_files, ["bin", "models"]);
        let l = s.licence.unwrap();
        assert_eq!(l.spdx.as_deref(), Some("LicenseRef-FreeSurfer"));
        assert!(l.accept);
        assert_eq!(l.site_file.as_ref().unwrap().env, "FS_LICENSE");
        assert_eq!(l.site_file.as_ref().unwrap().mount, "/license.txt");
        assert_eq!(l.terms_version.as_deref(), Some("2026"));
        assert_eq!(l.env, [("FS_ALLOW_DEEP".to_string(), "1".to_string())]);
        let t = s.test.unwrap();
        assert_eq!(t.command[0], "mri_synthseg");
        assert_eq!(t.expect, "SynthSeg");
        // a pack with none of the sections reads as nothing
        let (_, none, _) = parse_pack("nils_pack: 1\n").unwrap();
        assert_eq!(none, PackSoftware::default());
        // what is wrong is said
        assert!(
            parse_pack("source: {image: busybox:latest}\n")
                .unwrap_err()
                .contains("@sha256")
        );
        assert!(
            parse_pack("licence: {accept: true}\n")
                .unwrap_err()
                .contains("terms")
        );
        assert!(
            parse_pack("licence: {site_file: {env: X}}\n")
                .unwrap_err()
                .contains("mount")
        );
    }
}
