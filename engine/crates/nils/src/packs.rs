// SPDX-License-Identifier: AGPL-3.0-only
//! The rule packs an engine binary was released with, beside the ones in the
//! directory the engine reads, and the packs released on their own.
//!
//! A machine install keeps its packs in a directory of their own, and the
//! binary carries none: each release publishes them as one `packs.tar.gz`.
//! So an engine update is only whole once that directory holds the packs of
//! the release the engine now is, and an update that replaced the binary and
//! left the packs of an older release behind classifies with rules the
//! release no longer has.
//!
//! What is installed is measured, not assumed. Each pack is read for the
//! version its `pack.yml` states and a digest of every file in it, and the
//! same is read out of the release's tarball. A pack whose digest differs is
//! stale and is replaced. A pack the release does not carry is the site's
//! own and is never touched. A first-party pack changed on the machine after
//! an update put it there is kept, and said: every refresh writes a manifest
//! of what it put in place, `.nils-packs.json`, and a pack that no longer
//! matches its manifest line was edited here. Where there is no manifest, as
//! on an install from before this, nothing can say a pack was edited, and a
//! pack that differs from the release is taken to be an older release's.
//!
//! A first-party pack also has releases of its own (record 55 B5, the rules
//! releases of [`crate::rules`]): a rules fix ships as `pack-mri-v1.0.2` with
//! no engine release beside it. So the engine's release is the floor and not
//! the last word. A pack in the directory newer than the copy the release
//! carries is kept, and so is one the manifest says came from its own
//! release at the same version; a pack's own release replaces what is in
//! place where it is at least as new. A pack this engine would refuse, by
//! its contract or by the engines it names (pack contract 10), is replaced
//! by the release's whatever its version, and a release of its own that this
//! engine would refuse is never put in place.
//!
//! A refresh is renames inside one parent directory: the packs it brings are
//! written beside the directory in use and swapped in, and the packs they
//! replace are kept whole in `<dir>.previous`, one update deep.
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::supervise::sha256_hex;
use crate::update;

/// The file a refresh writes in the pack directory, naming what it put there.
pub(crate) const MANIFEST: &str = ".nils-packs.json";

/// The sibling of a pack directory that holds the packs the last refresh
/// replaced.
pub(crate) fn previous_of(dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .map_or_else(|| "packs".to_string(), |n| n.to_string_lossy().into_owned());
    dir.with_file_name(format!("{name}.previous"))
}

/// One pack: its folder's name, the version its `pack.yml` states where it
/// has one, a digest of every file in it, and what it says it works with.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Pack {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) digest: String,
    /// The pack contract its `pack.yml` declares, where it declares one.
    pub(crate) contract: Option<u32>,
    /// The engines its `pack.yml` says it works with (pack contract 10).
    pub(crate) engine: Option<String>,
}

impl Pack {
    /// `mri 0.8.0`, or for a pack that states no version the start of its
    /// digest, which is what tells two of them apart.
    pub(crate) fn said(&self) -> String {
        match &self.version {
            Some(v) => format!("{} {v}", self.name),
            None => format!(
                "{} ({})",
                self.name,
                &self.digest[..12.min(self.digest.len())]
            ),
        }
    }
}

/// A value a `pack.yml` states on a line of its own at the top level.
fn top_level(text: &str, key: &str) -> Option<String> {
    let key = format!("{key}:");
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(&key)?;
        let v = rest.split('#').next()?.trim().trim_matches(['"', '\'']);
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// A digest of a pack from its files, each named by its path inside the
/// pack. The same files give the same digest, on disk or in a tarball.
fn digest_of(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut listing = String::new();
    for (path, bytes) in files {
        listing.push_str(&format!("{} {path}\n", sha256_hex(bytes)));
    }
    sha256_hex(listing.as_bytes())
}

fn pack_of(name: String, files: &BTreeMap<String, Vec<u8>>) -> Pack {
    let manifest = files
        .get("pack.yml")
        .map(|b| String::from_utf8_lossy(b).into_owned());
    let stated = |key: &str| manifest.as_deref().and_then(|t| top_level(t, key));
    Pack {
        name,
        version: stated("version"),
        digest: digest_of(files),
        contract: stated("contract").and_then(|c| c.parse().ok()),
        engine: stated("engine"),
    }
}

/// Every regular file under `dir`, by its path relative to `root`.
fn files_under(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            files_under(root, &path, out);
        } else if kind.is_file()
            && let (Ok(bytes), Ok(rel)) = (std::fs::read(&path), path.strip_prefix(root))
        {
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.insert(rel, bytes);
        }
    }
}

/// The packs in a directory: every folder in it whose name does not start
/// with a dot, in name order. An absent directory holds none.
pub(crate) fn on_disk(dir: &Path) -> Vec<Pack> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Pack> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|k| k.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .map(|name| {
            let mut files = BTreeMap::new();
            let at = dir.join(&name);
            files_under(&at, &at, &mut files);
            pack_of(name, &files)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// One pack an update may put in place: what it is, the release it comes
/// from (`v1.0.0-alpha.80` for the engine's, `pack-mri-v1.0.2` for the
/// pack's own), and its files, which a look that only compares leaves empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Offer {
    pub(crate) pack: Pack,
    pub(crate) from: String,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
}

impl Offer {
    /// Whether it comes from the pack's own release rather than the
    /// engine's.
    pub(crate) fn own_release(&self) -> bool {
        own_release(&self.from)
    }

    /// The same offer without its files, for a look that only compares.
    pub(crate) fn described(&self) -> Offer {
        Offer {
            pack: self.pack.clone(),
            from: self.from.clone(),
            files: BTreeMap::new(),
        }
    }
}

/// Whether a release's tag is a pack's own (`pack-mri-v1.0.2`) rather than
/// the engine's (`v1.0.0-alpha.80`).
fn own_release(from: &str) -> bool {
    from.starts_with("pack-")
}

/// The packs a tarball carries, each with its files, from the release
/// `from`, read without unpacking it.
pub(crate) fn offers_in(bytes: &[u8], from: &str) -> Result<Vec<Offer>, String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut by_pack: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    let entries = archive
        .entries()
        .map_err(|e| format!("reading the packs: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("reading the packs: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("reading the packs: {e}"))?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .filter(|c| c != ".")
            .collect::<Vec<_>>();
        // packs/<pack>/... ; the directories themselves hold nothing to read
        let [top, pack, rest @ ..] = path.as_slice() else {
            continue;
        };
        if top != "packs" || pack.starts_with('.') {
            continue;
        }
        // a name that climbs out of the pack is no file of it
        if rest.iter().any(|c| c == ".." || c.is_empty()) {
            return Err(format!(
                "reading the packs: {} names a place outside its pack",
                path.join("/")
            ));
        }
        let files = by_pack.entry(pack.clone()).or_default();
        if entry.header().entry_type().is_file() && !rest.is_empty() {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("reading the packs: {e}"))?;
            files.insert(rest.join("/"), bytes);
        }
    }
    if by_pack.is_empty() {
        return Err("the release's packs.tar.gz holds no packs directory".to_string());
    }
    Ok(by_pack
        .into_iter()
        .map(|(name, files)| Offer {
            pack: pack_of(name, &files),
            from: from.to_string(),
            files,
        })
        .collect())
}

/// The packs a release's `packs.tar.gz` carries, read without unpacking it.
pub(crate) fn in_tarball(bytes: &[u8]) -> Result<Vec<Pack>, String> {
    Ok(offers_in(bytes, "")?.into_iter().map(|o| o.pack).collect())
}

/// The packs the engine's release `release` carries, from `base`, checked
/// against the release's sums.
pub(crate) fn bundled(base: &str, release: &str) -> Result<Vec<Offer>, String> {
    let bytes = update::fetch_checked(base, release, "packs.tar.gz").map_err(|e| e.message)?;
    offers_in(&bytes, &format!("v{release}"))
}

/// The engine a pack directory is read by, as far as an update knows it: its
/// version, and the pack contract it implements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Engine {
    pub(crate) version: String,
    pub(crate) contract: u32,
}

impl Engine {
    /// This binary.
    pub(crate) fn this() -> Engine {
        Engine {
            version: update::VERSION.to_string(),
            contract: nils_pack::CONTRACT,
        }
    }

    /// The engine of the release `version`, which an update may be putting
    /// in place from another binary. Its contract is known for this binary;
    /// for another it is the least it is known to implement: a later engine
    /// implements this one's, and any engine the contracts of the packs its
    /// own release carries.
    pub(crate) fn of_release(version: &str, carried: &[Pack]) -> Engine {
        if version == update::VERSION {
            return Engine::this();
        }
        let packs = carried.iter().filter_map(|p| p.contract).max().unwrap_or(0);
        let contract = if update::newer(version, update::VERSION) {
            packs.max(nils_pack::CONTRACT)
        } else {
            packs
        };
        Engine {
            version: version.to_string(),
            contract,
        }
    }

    /// Why this engine would refuse a pack, in words, or `None` where it
    /// reads it: a contract above its own, or a range of engines it is
    /// outside (pack contract 10), as the loader refuses them.
    pub(crate) fn refusal(&self, pack: &Pack) -> Option<String> {
        if let Some(c) = pack.contract
            && c > self.contract
        {
            return Some(format!(
                "{} needs pack contract {c}, and engine {} implements {}",
                pack.said(),
                self.version,
                self.contract
            ));
        }
        let text = pack.engine.as_deref()?;
        match nils_pack::engines::Range::parse(text) {
            Err(e) => Some(format!("{} names its engines as {e}", pack.said())),
            Ok(range) if !range.admits(&self.version) => Some(format!(
                "{} works with engines {range}, and this engine is {}",
                pack.said(),
                self.version
            )),
            Ok(_) => None,
        }
    }
}

/// What a refresh put in place for one pack, as the manifest keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) version: Option<String>,
    pub(crate) digest: String,
    /// The release it came from; none in a manifest written before packs
    /// had releases of their own, which were all the engine's.
    pub(crate) from: Option<String>,
}

impl Line {
    fn of(offer: &Offer) -> Line {
        Line {
            version: offer.pack.version.clone(),
            digest: offer.pack.digest.clone(),
            from: Some(offer.from.clone()),
        }
    }

    /// Whether it came from the pack's own release.
    fn own_release(&self) -> bool {
        self.from.as_deref().is_some_and(own_release)
    }
}

/// What a refresh put in place, as the manifest keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub(crate) release: String,
    pub(crate) packs: BTreeMap<String, Line>,
}

impl Manifest {
    pub(crate) fn read(dir: &Path) -> Option<Manifest> {
        let text = std::fs::read_to_string(dir.join(MANIFEST)).ok()?;
        let doc: Value = serde_json::from_str(&text).ok()?;
        let packs = doc["packs"]
            .as_object()?
            .iter()
            .filter_map(|(name, p)| {
                Some((
                    name.clone(),
                    Line {
                        version: p["version"].as_str().map(str::to_string),
                        digest: p["digest"].as_str()?.to_string(),
                        from: p["from"].as_str().map(str::to_string),
                    },
                ))
            })
            .collect();
        Some(Manifest {
            release: doc["release"].as_str().unwrap_or_default().to_string(),
            packs,
        })
    }

    fn write(&self, dir: &Path) -> Result<(), String> {
        let packs: serde_json::Map<String, Value> = self
            .packs
            .iter()
            .map(|(name, line)| {
                let mut doc = json!({ "version": line.version, "digest": line.digest });
                if let Some(from) = &line.from {
                    doc["from"] = json!(from);
                }
                (name.clone(), doc)
            })
            .collect();
        let doc = json!({ "release": self.release, "packs": packs });
        let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
        std::fs::write(dir.join(MANIFEST), text + "\n")
            .map_err(|e| format!("{}: {e}", dir.join(MANIFEST).display()))
    }
}

/// The packs in a directory beside the ones a release carries, and beside
/// the packs' own releases where an update looked at them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) dir: PathBuf,
    /// The release the bundled packs are measured against.
    pub(crate) release: String,
    pub(crate) installed: Vec<Pack>,
    pub(crate) bundled: Vec<Pack>,
    /// The packs' own releases this engine reads, each with the tag it is
    /// published under.
    pub(crate) offered: Vec<(Pack, String)>,
    /// Packs the release carries that differ from, or are missing in, the
    /// directory, and that an update replaces with the release's.
    pub(crate) stale: Vec<String>,
    /// Packs an update replaces with their own release (record 55 B5).
    pub(crate) rules: Vec<String>,
    /// First-party packs changed on this machine after an update put them
    /// there, which are kept.
    pub(crate) edited: Vec<String>,
    /// Packs the release does not carry: the site's own, which are kept.
    pub(crate) own: Vec<String>,
    /// Packs kept over the release's copy: a later version, or the same
    /// version from the pack's own release.
    pub(crate) ahead: Vec<String>,
    /// Packs in the directory this engine would refuse, with why; the
    /// release's copy replaces each.
    pub(crate) refused: Vec<(String, String)>,
}

/// Whether a pack in place stays over the copy the engine's release
/// carries: a later version, or the same version from the pack's own release.
fn kept_over(installed: &Pack, from_own: bool, bundled: &Pack) -> bool {
    match (&installed.version, &bundled.version) {
        (Some(i), Some(b)) => match nils_pack::engines::order(i, b) {
            Ordering::Greater => true,
            Ordering::Equal => from_own,
            Ordering::Less => false,
        },
        _ => false,
    }
}

/// Whether a pack's own release replaces what is, or would be, in place: it
/// differs, and it is at least as new. The same version replaces the
/// engine's copy, which a pack's own release stands above, and another build
/// of its own, as a development channel serves one.
fn replaces(offered: &Pack, there: &Pack) -> bool {
    if offered.digest == there.digest {
        return false;
    }
    match (&offered.version, &there.version) {
        (Some(o), Some(t)) => nils_pack::engines::order(o, t) != Ordering::Less,
        (Some(_), None) => true,
        _ => false,
    }
}

/// The directory's packs beside the release's and the packs' own releases,
/// with the manifest a previous refresh wrote, where there is one. An own
/// release this engine would refuse is not looked at.
pub(crate) fn compare(
    dir: &Path,
    release: &str,
    installed: Vec<Pack>,
    bundled: Vec<Pack>,
    offered: Vec<(Pack, String)>,
    manifest: Option<&Manifest>,
    engine: &Engine,
) -> Status {
    let offered: Vec<(Pack, String)> = offered
        .into_iter()
        .filter(|(p, _)| engine.refusal(p).is_none())
        .collect();
    let mut status = Status {
        dir: dir.to_path_buf(),
        release: release.to_string(),
        installed: Vec::new(),
        bundled: Vec::new(),
        offered: Vec::new(),
        stale: Vec::new(),
        rules: Vec::new(),
        edited: Vec::new(),
        own: Vec::new(),
        ahead: Vec::new(),
        refused: Vec::new(),
    };
    let mut names: Vec<&String> = bundled
        .iter()
        .map(|b| &b.name)
        .chain(offered.iter().map(|(p, _)| &p.name))
        .collect();
    names.sort();
    names.dedup();
    for name in names {
        let i = installed.iter().find(|p| &p.name == name);
        let b = bundled.iter().find(|p| &p.name == name);
        let r = offered
            .iter()
            .find(|(p, _)| &p.name == name)
            .map(|(p, _)| p);
        let line = i.and_then(|i| manifest.and_then(|m| m.packs.get(&i.name)));
        if let Some(i) = i
            && line.is_some_and(|l| l.digest != i.digest)
        {
            status.edited.push(name.clone());
            continue;
        }
        // what the engine's release does with it
        let refused = i.and_then(|i| engine.refusal(i));
        let release_replaces = match (i, b) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(i), Some(b)) if i.digest == b.digest => false,
            (Some(i), Some(b)) => {
                refused.is_some() || !kept_over(i, line.is_some_and(Line::own_release), b)
            }
        };
        let there = if release_replaces { b } else { i };
        // and then its own release, which stands on whatever that leaves
        let own_replaces = match (r, there) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(r), Some(t)) => replaces(r, t),
        };
        if own_replaces {
            status.rules.push(name.clone());
        } else if release_replaces {
            status.stale.push(name.clone());
        } else if let (Some(i), Some(b)) = (i, b)
            && i.digest != b.digest
        {
            status.ahead.push(name.clone());
        }
        if let (Some(why), true) = (refused, own_replaces || release_replaces) {
            status.refused.push((name.clone(), why));
        }
    }
    status.own = installed
        .iter()
        .filter(|i| {
            !bundled.iter().any(|b| b.name == i.name)
                && !offered.iter().any(|(p, _)| p.name == i.name)
        })
        .map(|i| i.name.clone())
        .collect();
    status.installed = installed;
    status.bundled = bundled;
    status.offered = offered;
    status
}

impl Status {
    /// Whether the release's packs would change the directory.
    pub(crate) fn behind(&self) -> bool {
        !self.stale.is_empty()
    }

    fn installed_of(&self, name: &str) -> Option<&Pack> {
        self.installed.iter().find(|p| p.name == name)
    }

    fn bundled_of(&self, name: &str) -> Option<&Pack> {
        self.bundled.iter().find(|p| p.name == name)
    }

    fn offered_of(&self, name: &str) -> Option<&(Pack, String)> {
        self.offered.iter().find(|(p, _)| p.name == name)
    }

    /// The lines `nils update --check` says: one for the directory, then one
    /// per pack the release carries, then the site's own.
    pub(crate) fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "packs in {}, beside the ones engine {} was released with:",
            self.dir.display(),
            self.release
        )];
        for b in &self.bundled {
            let line = match self.installed_of(&b.name) {
                None => match self.offered_of(&b.name) {
                    Some((r, tag)) if self.rules.contains(&b.name) => format!(
                        "{}: not installed; its own release {tag} brings {}",
                        b.name,
                        r.said()
                    ),
                    _ => format!("{}: not installed; the release brings {}", b.name, b.said()),
                },
                Some(i) if self.edited.contains(&b.name) => format!(
                    "{}: changed on this machine, kept (the release brings {})",
                    i.said(),
                    b.said()
                ),
                Some(i) if self.rules.contains(&b.name) => match self.offered_of(&b.name) {
                    Some((r, tag)) => {
                        format!("{}: its own release {tag} brings {}", i.said(), r.said())
                    }
                    None => format!("{}: its own release replaces it", i.said()),
                },
                Some(i) if self.stale.contains(&b.name) => {
                    match self.refused.iter().find(|(n, _)| n == &b.name) {
                        Some((_, why)) => format!(
                            "{}: the engine would refuse it ({why}); the release brings {}",
                            i.said(),
                            b.said()
                        ),
                        None => format!("{}: the release brings {}", i.said(), b.said()),
                    }
                }
                Some(i) if self.ahead.contains(&b.name) => {
                    let same = match (&i.version, &b.version) {
                        (Some(x), Some(y)) => nils_pack::engines::order(x, y) == Ordering::Equal,
                        _ => false,
                    };
                    if same {
                        format!(
                            "{}: its own release of that version, kept over the release's copy",
                            i.said()
                        )
                    } else {
                        format!("{}: newer than the release's {}, kept", i.said(), b.said())
                    }
                }
                Some(i) => format!("{}: the release's", i.said()),
            };
            out.push(format!("  {line}"));
        }
        for name in &self.own {
            if let Some(i) = self.installed_of(name) {
                out.push(format!("  {}: the site's own, kept", i.said()));
            }
        }
        out
    }

    pub(crate) fn doc(&self) -> Value {
        let named = |list: &[Pack]| -> Value {
            list.iter()
                .map(|p| json!({ "name": p.name, "version": p.version, "digest": p.digest }))
                .collect::<Vec<_>>()
                .into()
        };
        json!({
            "dir": self.dir.display().to_string(),
            "release": self.release,
            "installed": named(&self.installed),
            "bundled": named(&self.bundled),
            "stale": self.stale,
            "edited": self.edited,
            "own": self.own,
            "ahead": self.ahead,
            "rules": self.rules,
            "refused": self
                .refused
                .iter()
                .map(|(name, why)| json!({ "name": name, "why": why }))
                .collect::<Vec<_>>(),
            "behind": self.behind(),
            "command": "nils update --all",
        })
    }
}

/// What an update would do in a pack directory: the directory beside the
/// engine's release and the packs' own releases, and the packs it would put
/// in place, each from the release that brings it.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub(crate) status: Status,
    pub(crate) take: Vec<Offer>,
    manifest: Option<Manifest>,
}

impl Plan {
    /// Whether an update would change the directory.
    pub(crate) fn behind(&self) -> bool {
        !self.take.is_empty()
    }
}

/// What an update would put in `dir`: the engine release's packs where they
/// are behind, and a pack's own release where it stands above what that
/// leaves (only those this engine reads are passed, by [`crate::rules`]).
pub(crate) fn plan(
    dir: &Path,
    release: &str,
    bundled: Vec<Offer>,
    own: Vec<Offer>,
    engine: &Engine,
) -> Plan {
    let manifest = Manifest::read(dir);
    let status = compare(
        dir,
        release,
        on_disk(dir),
        bundled.iter().map(|o| o.pack.clone()).collect(),
        own.iter()
            .map(|o| (o.pack.clone(), o.from.clone()))
            .collect(),
        manifest.as_ref(),
        engine,
    );
    let mut take: Vec<Offer> = bundled
        .into_iter()
        .filter(|o| status.stale.contains(&o.pack.name))
        .collect();
    take.extend(
        own.into_iter()
            .filter(|o| status.rules.contains(&o.pack.name)),
    );
    take.sort_by(|a, b| a.pack.name.cmp(&b.pack.name));
    Plan {
        status,
        take,
        manifest,
    }
}

/// Put in `dir` the packs `release` carries, where they are behind what is
/// there. The site's own packs, first-party packs edited here and packs kept
/// over the release's copy stay; the packs replaced are kept in
/// `<dir>.previous`. What is said is one line.
pub(crate) fn refresh(base: &str, release: &str, dir: &Path) -> Result<String, String> {
    let parent = dir.parent().ok_or("the pack directory has no parent")?;
    if !update::writable(parent) {
        return Err(format!("{} is not writable by this user", parent.display()));
    }
    let carried = bundled(base, release)?;
    let packs: Vec<Pack> = carried.iter().map(|o| o.pack.clone()).collect();
    let engine = Engine::of_release(release, &packs);
    apply(dir, &plan(dir, release, carried, Vec::new(), &engine))
}

/// Do what a plan says in `dir`, and say it in one line.
pub(crate) fn apply(dir: &Path, plan: &Plan) -> Result<String, String> {
    let status = &plan.status;
    if !plan.behind() {
        return Ok(format!(
            "the packs in {} are the ones engine {} was released with",
            dir.display(),
            status.release
        ));
    }
    let parent = dir.parent().ok_or("the pack directory has no parent")?;
    if !update::writable(parent) {
        return Err(format!("{} is not writable by this user", parent.display()));
    }
    let staging = parent.join(format!(".nils-packs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let fresh = staging.join("packs");
    let written = plan.take.iter().try_for_each(|offer| {
        if offer.files.is_empty() {
            return Err(format!(
                "{} from {} came without its files",
                offer.pack.said(),
                offer.from
            ));
        }
        for (rel, bytes) in &offer.files {
            let path = fresh.join(&offer.pack.name).join(rel);
            if let Some(at) = path.parent() {
                std::fs::create_dir_all(at).map_err(|e| format!("{}: {e}", at.display()))?;
            }
            std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    });
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("writing the packs: {e}"));
    }
    let aside = parent.join(format!(".nils-packs-old-{}", std::process::id()));
    let swapped = swap_in(&fresh, dir, &aside);
    let _ = std::fs::remove_dir_all(&staging);
    let swapped = swapped?;

    // What this refresh put in place, beside what an earlier one put there
    // for the packs it left: a pack kept as edited keeps the line of the one
    // before it, so it is still known to be edited, and a pack kept over the
    // release's keeps where it came from.
    let mut packs: BTreeMap<String, Line> = BTreeMap::new();
    for offer in &plan.take {
        packs.insert(offer.pack.name.clone(), Line::of(offer));
    }
    for i in &status.installed {
        if packs.contains_key(&i.name) || status.own.contains(&i.name) {
            continue;
        }
        let earlier = plan.manifest.as_ref().and_then(|m| m.packs.get(&i.name));
        match (earlier, status.bundled_of(&i.name)) {
            (Some(line), _) => {
                packs.insert(i.name.clone(), line.clone());
            }
            // the release's own copy, in place before a manifest named it
            (None, Some(b)) if b.digest == i.digest => {
                packs.insert(
                    i.name.clone(),
                    Line {
                        version: b.version.clone(),
                        digest: b.digest.clone(),
                        from: Some(format!("v{}", status.release)),
                    },
                );
            }
            _ => {}
        }
    }
    let written = Manifest {
        release: status.release.clone(),
        packs,
    }
    .write(dir);

    let replaced: Vec<String> = plan
        .take
        .iter()
        .map(|offer| {
            let to = if offer.own_release() {
                format!("{}, its own release {}", offer.pack.said(), offer.from)
            } else {
                offer.pack.said()
            };
            match status.installed_of(&offer.pack.name) {
                Some(i) => format!("{} to {to}", i.said()),
                None => to,
            }
        })
        .collect();
    let mut said = if plan.take.iter().any(Offer::own_release) {
        format!(
            "the packs in {} are brought up to date ({})",
            dir.display(),
            replaced.join(", ")
        )
    } else {
        format!(
            "the packs in {} are the release's ({})",
            dir.display(),
            replaced.join(", ")
        )
    };
    if !swapped.stranded.is_empty() {
        said.push_str(&format!(
            "; the deployment's own {} could not be put back and are in {}",
            swapped.stranded.join(", "),
            aside.display()
        ));
    } else if let Some(previous) = &swapped.previous {
        said.push_str(&format!("; the ones before are in {}", previous.display()));
    }
    let own: Vec<&str> = status
        .own
        .iter()
        .filter(|n| dir.join(n).is_dir())
        .map(String::as_str)
        .collect();
    if !own.is_empty() {
        said.push_str(&format!(", and its own are kept: {}", own.join(", ")));
    }
    if !status.edited.is_empty() {
        said.push_str(&format!(
            "; {} changed on this machine and kept as it is, so remove it and update again to \
             take the release's",
            status.edited.join(", ")
        ));
    }
    if !status.ahead.is_empty() {
        let kept: Vec<String> = status
            .ahead
            .iter()
            .filter_map(|n| status.installed_of(n).map(Pack::said))
            .collect();
        said.push_str(&format!(
            "; {} kept over the copy engine {} carries",
            kept.join(", "),
            status.release
        ));
    }
    if let Err(e) = written {
        said.push_str(&format!("; the manifest was not written: {e}"));
    }
    Ok(said)
}

/// What a swap did: where the packs it replaced went, and what of the
/// directory's own could not be put back.
struct Swapped {
    previous: Option<PathBuf>,
    stranded: Vec<String>,
}

/// Put the packs in `fresh` in `dir`. Every other pack in `dir` is kept: an
/// update replaces only the packs it brings, and a deployment's own are never
/// touched. Everything is a rename inside one parent, so nothing is copied,
/// and what could not be put back is left where it was set aside and never
/// removed. The packs replaced are kept in `<dir>.previous`.
fn swap_in(fresh: &Path, dir: &Path, aside: &Path) -> Result<Swapped, String> {
    let _ = std::fs::remove_dir_all(aside);
    let mut kept: Vec<std::ffi::OsString> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .filter(|name| !name.to_string_lossy().starts_with('.'))
                .filter(|name| std::fs::symlink_metadata(fresh.join(name)).is_err())
                .collect()
        })
        .unwrap_or_default();
    kept.sort();
    if dir.exists() {
        std::fs::rename(dir, aside).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if let Err(e) = std::fs::rename(fresh, dir) {
        if aside.exists() {
            let _ = std::fs::rename(aside, dir);
        }
        return Err(format!("{}: {e}", dir.display()));
    }
    let stranded: Vec<String> = kept
        .iter()
        .filter(|name| std::fs::rename(aside.join(name), dir.join(name)).is_err())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    if !stranded.is_empty() {
        return Ok(Swapped {
            previous: None,
            stranded,
        });
    }
    // the packs replaced, kept one update deep; the old manifest goes with
    // them, since it named them
    let mut previous = None;
    if aside.exists() {
        let at = previous_of(dir);
        let _ = std::fs::remove_dir_all(&at);
        if std::fs::rename(aside, &at).is_ok() {
            previous = Some(at);
        } else {
            let _ = std::fs::remove_dir_all(aside);
        }
    }
    Ok(Swapped {
        previous,
        stranded: Vec::new(),
    })
}

/// The first-party packs in a directory that state a version: those a
/// refresh put there, by its manifest, and those named in `also`. What a
/// pack's own releases are measured against where the engine's release
/// cannot be read.
pub(crate) fn first_party_in(dir: &Path, also: &[&str]) -> Vec<Pack> {
    let manifest = Manifest::read(dir);
    on_disk(dir)
        .into_iter()
        .filter(|p| p.version.is_some())
        .filter(|p| {
            also.contains(&p.name.as_str())
                || manifest
                    .as_ref()
                    .is_some_and(|m| m.packs.contains_key(&p.name))
        })
        .collect()
}

/// The rules in use in a pack directory, as the setup record notes them:
/// each pack that states a version and that a refresh put there, or that is
/// one of `first_party`, with the release it came from where that is known.
pub(crate) fn rules_in(dir: &Path, first_party: &[&str]) -> BTreeMap<String, (String, String)> {
    let manifest = Manifest::read(dir);
    on_disk(dir)
        .into_iter()
        .filter_map(|p| {
            let line = manifest.as_ref().and_then(|m| m.packs.get(&p.name));
            if line.is_none() && !first_party.contains(&p.name.as_str()) {
                return None;
            }
            let from = line.and_then(|l| l.from.clone()).unwrap_or_else(|| {
                manifest
                    .as_ref()
                    .filter(|m| line.is_some() && !m.release.is_empty())
                    .map(|m| format!("v{}", m.release))
                    .unwrap_or_default()
            });
            Some((p.name.clone(), (p.version.clone()?, from)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn tarball(root: &Path, packs: &[(&str, &str, &str)]) -> Vec<u8> {
        let src = root.join("src");
        let _ = std::fs::remove_dir_all(&src);
        for (pack, file, text) in packs {
            write(&src.join("packs").join(pack).join(file), text);
        }
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        tar.append_dir_all("packs", src.join("packs")).unwrap();
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        let _ = std::fs::remove_dir_all(&src);
        bytes
    }

    /// A release laid out as a channel serves it, with its sums.
    fn publish(root: &Path, version: &str, tar: &[u8]) -> String {
        let into = root.join("download").join(format!("v{version}"));
        std::fs::create_dir_all(&into).unwrap();
        std::fs::write(into.join("packs.tar.gz"), tar).unwrap();
        std::fs::write(
            into.join("SHA256SUMS"),
            format!("{}  packs.tar.gz\n", sha256_hex(tar)),
        )
        .unwrap();
        format!("file://{}", root.display())
    }

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nils-packs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// The packs in `dir` beside the ones `release` carries, as a check
    /// reads them.
    fn status(base: &str, release: &str, dir: &Path) -> Result<Status, String> {
        let carried = bundled(base, release)?;
        let packs: Vec<Pack> = carried.iter().map(|o| o.pack.clone()).collect();
        let engine = Engine::of_release(release, &packs);
        Ok(plan(dir, release, carried, Vec::new(), &engine).status)
    }

    /// Whether an update would change the directory at all.
    fn moves(s: &Status) -> bool {
        !s.stale.is_empty() || !s.rules.is_empty()
    }

    fn pack(name: &str, version: &str, digest: &str) -> Pack {
        Pack {
            name: name.to_string(),
            version: Some(version.to_string()),
            digest: digest.to_string(),
            ..Pack::default()
        }
    }

    #[test]
    fn a_pack_reads_the_same_on_disk_and_in_the_tarball() {
        let root = temp("same");
        let tar = tarball(
            &root,
            &[
                (
                    "mri",
                    "pack.yml",
                    "# a comment\npack: mri\nversion: 0.8.0\ncontract: 10\n\
                     engine: \">=1.0.0-alpha.80, <2.0.0\" # the range\n",
                ),
                ("mri", "rules/a.yml", "a\n"),
                ("clinical", "vocabulary.yml", "words\n"),
            ],
        );
        let bundled = in_tarball(&tar).unwrap();
        let dir = root.join("packs");
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar.as_slice()));
        archive.unpack(&root).unwrap();
        assert_eq!(on_disk(&dir), bundled);
        assert_eq!(bundled[1].version.as_deref(), Some("0.8.0"));
        assert_eq!(bundled[1].contract, Some(10));
        assert_eq!(
            bundled[1].engine.as_deref(),
            Some(">=1.0.0-alpha.80, <2.0.0")
        );
        assert_eq!(bundled[0].name, "clinical");
        assert_eq!(bundled[0].version, None);
        assert!(bundled[0].said().starts_with("clinical ("));
        let offers = offers_in(&tar, "pack-mri-v0.8.0").unwrap();
        assert!(offers[1].own_release() && offers[1].files.contains_key("rules/a.yml"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pack_is_stale_edited_or_the_sites_own_by_what_is_on_disk() {
        let this = Engine::this();
        let bundled = vec![pack("clinical", "1", "c2"), pack("mri", "0.8.0", "m2")];
        let installed = vec![
            pack("clinical", "1", "c1"),
            pack("lab", "1", "l1"),
            pack("mri", "0.7.0", "m1"),
        ];
        // with no manifest, nothing can say a pack was edited here
        let s = compare(
            Path::new("/p"),
            "9",
            installed.clone(),
            bundled.clone(),
            Vec::new(),
            None,
            &this,
        );
        assert_eq!(s.stale, ["clinical", "mri"]);
        assert!(s.edited.is_empty());
        assert_eq!(s.own, ["lab"]);
        assert!(s.behind());
        let lines = s.lines().join("\n");
        assert!(
            lines.contains("mri 0.7.0: the release brings mri 0.8.0"),
            "{lines}"
        );
        assert!(lines.contains("lab 1: the site's own, kept"), "{lines}");

        // a manifest that put c1 there and m0 there: mri was edited
        let line = |version: Option<&str>, digest: &str| Line {
            version: version.map(str::to_string),
            digest: digest.to_string(),
            from: None,
        };
        let manifest = Manifest {
            release: "8".to_string(),
            packs: [
                ("clinical".to_string(), line(None, "c1")),
                ("mri".to_string(), line(Some("0.7.0"), "m0")),
            ]
            .into(),
        };
        let s = compare(
            Path::new("/p"),
            "9",
            installed,
            bundled.clone(),
            Vec::new(),
            Some(&manifest),
            &this,
        );
        assert_eq!(s.stale, ["clinical"]);
        assert_eq!(s.edited, ["mri"]);
        assert!(
            s.lines()
                .join("\n")
                .contains("changed on this machine, kept")
        );
        assert_eq!(s.doc()["edited"][0], "mri");

        // the release's packs in place: nothing to do
        let s = compare(
            Path::new("/p"),
            "9",
            bundled.clone(),
            bundled,
            Vec::new(),
            None,
            &this,
        );
        assert!(!s.behind());
        assert_eq!(s.doc()["behind"], false);
    }

    /// Record 55 B5: the engine's release is the floor. A pack its own
    /// release put in place stays over an older copy the release carries, and
    /// over the same version; a newer copy replaces it; and its own release
    /// replaces the release's copy where it is at least as new.
    #[test]
    fn a_packs_own_release_stands_on_the_engines_and_never_below_it() {
        let this = Engine::this();
        let own = |version: &str, digest: &str| Line {
            version: Some(version.to_string()),
            digest: digest.to_string(),
            from: Some(format!("pack-mri-v{version}")),
        };
        let manifest = |line: Line| Manifest {
            release: "9".to_string(),
            packs: [("mri".to_string(), line)].into(),
        };
        let compare_with = |installed: Pack, bundled: Pack, offered: Option<Pack>, m: &Manifest| {
            compare(
                Path::new("/p"),
                "9",
                vec![installed],
                vec![bundled],
                offered
                    .map(|p| {
                        let tag = format!("pack-mri-v{}", p.version.clone().unwrap_or_default());
                        vec![(p, tag)]
                    })
                    .unwrap_or_default(),
                Some(m),
                &this,
            )
        };

        // 1.0.2 from its own release, the engine's release carries 1.0.1
        let m = manifest(own("1.0.2", "m2"));
        let s = compare_with(
            pack("mri", "1.0.2", "m2"),
            pack("mri", "1.0.1", "m1"),
            None,
            &m,
        );
        assert!(s.stale.is_empty() && s.rules.is_empty(), "{s:?}");
        assert_eq!(s.ahead, ["mri"]);
        assert!(
            s.lines()
                .join("\n")
                .contains("mri 1.0.2: newer than the release's mri 1.0.1, kept"),
            "{:?}",
            s.lines()
        );
        // the same version, from its own release, stays too
        let m = manifest(own("1.0.1", "m1b"));
        let s = compare_with(
            pack("mri", "1.0.1", "m1b"),
            pack("mri", "1.0.1", "m1"),
            None,
            &m,
        );
        assert_eq!(s.ahead, ["mri"], "{s:?}");
        assert!(
            s.lines().join("\n").contains(
                "mri 1.0.1: its own release of that version, kept over the release's copy"
            ),
            "{:?}",
            s.lines()
        );
        // a newer copy in the engine's release replaces it
        let m = manifest(own("1.0.2", "m2"));
        let s = compare_with(
            pack("mri", "1.0.2", "m2"),
            pack("mri", "1.0.3", "m3"),
            None,
            &m,
        );
        assert_eq!(s.stale, ["mri"], "{s:?}");

        // its own release replaces the engine's copy at the same version and above
        let m = manifest(Line {
            version: Some("1.0.1".to_string()),
            digest: "m1".to_string(),
            from: Some("v9".to_string()),
        });
        for (version, digest) in [("1.0.1", "r1"), ("1.0.2", "r2")] {
            let s = compare_with(
                pack("mri", "1.0.1", "m1"),
                pack("mri", "1.0.1", "m1"),
                Some(pack("mri", version, digest)),
                &m,
            );
            assert_eq!(s.rules, ["mri"], "{version}: {s:?}");
            assert!(s.stale.is_empty() && !s.behind() && moves(&s));
        }
        // and never an older one, nor one that is what is there already
        for (version, digest) in [("1.0.0", "r0"), ("1.0.1", "m1")] {
            let s = compare_with(
                pack("mri", "1.0.1", "m1"),
                pack("mri", "1.0.1", "m1"),
                Some(pack("mri", version, digest)),
                &m,
            );
            assert!(!moves(&s), "{version}: {s:?}");
        }
        // where the engine's copy is behind and its own release newer still,
        // its own release is what goes in, once
        let s = compare_with(
            pack("mri", "1.0.0", "m0"),
            pack("mri", "1.0.1", "m1"),
            Some(pack("mri", "1.0.2", "r2")),
            &manifest(Line {
                version: Some("1.0.0".to_string()),
                digest: "m0".to_string(),
                from: None,
            }),
        );
        assert_eq!(
            (s.rules.clone(), s.stale.clone()),
            (vec!["mri".to_string()], vec![])
        );
    }

    /// A pack this engine would refuse is replaced by the release's copy
    /// whatever its version, and a release of its own the engine would
    /// refuse is never put in place.
    #[test]
    fn what_this_engine_would_refuse_is_never_kept_or_taken() {
        let engine = Engine {
            version: "1.0.0-alpha.80".to_string(),
            contract: 9,
        };
        let with = |version: &str, digest: &str, contract: u32, range: Option<&str>| Pack {
            contract: Some(contract),
            engine: range.map(str::to_string),
            ..pack("mri", version, digest)
        };
        let m = Manifest {
            release: "9".to_string(),
            packs: [(
                "mri".to_string(),
                Line {
                    version: Some("1.0.3".to_string()),
                    digest: "m3".to_string(),
                    from: Some("pack-mri-v1.0.3".to_string()),
                },
            )]
            .into(),
        };
        // a newer pack in place that needs a contract this engine lacks
        let s = compare(
            Path::new("/p"),
            "1.0.0-alpha.80",
            vec![with("1.0.3", "m3", 10, None)],
            vec![with("1.0.1", "m1", 8, None)],
            Vec::new(),
            Some(&m),
            &engine,
        );
        assert_eq!(s.stale, ["mri"], "{s:?}");
        assert!(s.refused[0].1.contains("needs pack contract 10"), "{s:?}");
        assert!(
            s.lines().join("\n").contains("the engine would refuse it"),
            "{:?}",
            s.lines()
        );
        // own releases outside the engine's reach are not looked at
        for offered in [
            with("1.0.4", "r4", 10, None),
            with("1.0.4", "r4", 9, Some(">=1.0.0-alpha.90")),
            with("1.0.4", "r4", 9, Some("not a range")),
        ] {
            assert!(engine.refusal(&offered).is_some());
            let s = compare(
                Path::new("/p"),
                "1.0.0-alpha.80",
                vec![with("1.0.1", "m1", 8, None)],
                vec![with("1.0.1", "m1", 8, None)],
                vec![(offered, "pack-mri-v1.0.4".to_string())],
                None,
                &engine,
            );
            assert!(!moves(&s) && s.offered.is_empty(), "{s:?}");
        }
        let words = engine
            .refusal(&with("1.0.4", "r4", 9, Some(">=1.0.0-alpha.90, <2.0.0")))
            .unwrap();
        assert_eq!(
            words,
            "mri 1.0.4 works with engines >=1.0.0-alpha.90, <2.0.0, and this engine is \
             1.0.0-alpha.80"
        );
        // an engine this binary is about to put in place implements at least
        // what its release's packs declare, and a later one this one's
        let carried = [with("1.0.1", "m1", 7, None)];
        assert_eq!(Engine::of_release("0.0.1", &carried).contract, 7);
        assert!(Engine::of_release("99.0.0", &carried).contract >= nils_pack::CONTRACT);
        assert_eq!(
            Engine::of_release(update::VERSION, &carried),
            Engine::this()
        );
    }

    #[test]
    fn a_refresh_replaces_stale_packs_keeps_the_old_ones_and_the_sites_own() {
        let root = temp("refresh");
        let dir = root.join("engine").join("packs");
        // what an older release put there, and a pack of the site's own
        write(&dir.join("mri/pack.yml"), "pack: mri\nversion: 0.7.0\n");
        write(&dir.join("clinical/vocabulary.yml"), "old words\n");
        write(&dir.join("lab/pack.yml"), "pack: lab\nversion: 1\n");
        let tar = tarball(
            &root,
            &[
                ("mri", "pack.yml", "pack: mri\nversion: 0.8.0\n"),
                ("clinical", "vocabulary.yml", "new words\n"),
            ],
        );
        let base = publish(&root.join("channel"), "9.0.0", &tar);

        let before = status(&base, "9.0.0", &dir).unwrap();
        assert_eq!(before.stale, ["clinical", "mri"]);

        let said = refresh(&base, "9.0.0", &dir).unwrap();
        assert!(said.contains("mri 0.7.0 to mri 0.8.0"), "{said}");
        assert!(said.contains("its own are kept: lab"), "{said}");
        assert!(said.contains("packs.previous"), "{said}");
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        assert!(read(&dir.join("mri/pack.yml")).contains("0.8.0"));
        assert_eq!(read(&dir.join("clinical/vocabulary.yml")), "new words\n");
        assert!(read(&dir.join("lab/pack.yml")).contains("lab"));
        let previous = previous_of(&dir);
        assert!(read(&previous.join("mri/pack.yml")).contains("0.7.0"));
        assert!(
            !previous.join("lab").exists(),
            "the site's own stays in use"
        );
        assert!(!status(&base, "9.0.0", &dir).unwrap().behind());
        let manifest = Manifest::read(&dir).unwrap();
        assert_eq!(manifest.release, "9.0.0");
        assert_eq!(manifest.packs["mri"].version.as_deref(), Some("0.8.0"));
        assert_eq!(manifest.packs["mri"].from.as_deref(), Some("v9.0.0"));
        // nothing left in the parent but the two
        let mut left: Vec<String> = std::fs::read_dir(root.join("engine"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["packs", "packs.previous"]);

        // the same release again changes nothing, and keeps the old copy
        let said = refresh(&base, "9.0.0", &dir).unwrap();
        assert!(said.contains("are the ones engine 9.0.0"), "{said}");
        assert!(read(&previous.join("mri/pack.yml")).contains("0.7.0"));

        // an edit made here after the refresh is kept and said
        write(&dir.join("mri/rules/local.yml"), "a site's rule\n");
        let tar = tarball(
            &root,
            &[
                ("mri", "pack.yml", "pack: mri\nversion: 0.9.0\n"),
                ("clinical", "vocabulary.yml", "newer words\n"),
            ],
        );
        let base = publish(&root.join("channel"), "10.0.0", &tar);
        let s = status(&base, "10.0.0", &dir).unwrap();
        assert_eq!(s.edited, ["mri"]);
        assert_eq!(s.stale, ["clinical"]);
        let said = refresh(&base, "10.0.0", &dir).unwrap();
        assert!(
            said.contains("mri changed on this machine and kept"),
            "{said}"
        );
        assert!(dir.join("mri/rules/local.yml").exists());
        assert_eq!(read(&dir.join("clinical/vocabulary.yml")), "newer words\n");
        // and it is still known to be edited on the next look
        assert_eq!(status(&base, "10.0.0", &dir).unwrap().edited, ["mri"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A pack's own release put in place by the same swap, beside the
    /// engine's copies it leaves alone: one swap, the replaced pack one
    /// update deep in `.previous`, the manifest naming where each came from,
    /// and the record's view of the rules in use.
    #[test]
    fn a_packs_own_release_goes_in_with_one_swap_and_is_named_in_the_manifest() {
        let root = temp("own");
        let dir = root.join("engine").join("packs");
        let tar = tarball(
            &root,
            &[
                (
                    "mri",
                    "pack.yml",
                    "pack: mri\nversion: 1.0.1\ncontract: 8\n",
                ),
                ("clinical", "vocabulary.yml", "words\n"),
            ],
        );
        let base = publish(&root.join("channel"), "9.0.0", &tar);
        write(&dir.join("lab/pack.yml"), "pack: lab\nversion: 1.0.0\n");
        refresh(&base, "9.0.0", &dir).unwrap();
        let carried = bundled(&base, "9.0.0").unwrap();
        let own_tar = tarball(
            &root,
            &[(
                "mri",
                "pack.yml",
                "pack: mri\nversion: 1.0.2\ncontract: 8\n# a rules fix\n",
            )],
        );
        let own = offers_in(&own_tar, "pack-mri-v1.0.2").unwrap();
        let engine = Engine::this();
        let p = plan(&dir, "9.0.0", carried.clone(), own.clone(), &engine);
        assert_eq!(p.status.rules, ["mri"]);
        assert!(p.status.stale.is_empty());
        assert_eq!(p.take.len(), 1);
        let said = apply(&dir, &p).unwrap();
        assert!(
            said.contains("mri 1.0.1 to mri 1.0.2, its own release pack-mri-v1.0.2"),
            "{said}"
        );
        assert!(said.contains("its own are kept: lab"), "{said}");
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        assert!(read(&dir.join("mri/pack.yml")).contains("1.0.2"));
        assert_eq!(read(&dir.join("clinical/vocabulary.yml")), "words\n");
        let previous = previous_of(&dir);
        assert!(read(&previous.join("mri/pack.yml")).contains("1.0.1"));
        assert!(
            !previous.join("clinical").exists(),
            "only what was replaced goes aside"
        );
        let manifest = Manifest::read(&dir).unwrap();
        assert_eq!(
            manifest.packs["mri"].from.as_deref(),
            Some("pack-mri-v1.0.2")
        );
        assert_eq!(manifest.packs["clinical"].from.as_deref(), Some("v9.0.0"));
        assert!(!manifest.packs.contains_key("lab"));
        let rules = rules_in(&dir, &["mri", "clinical"]);
        assert_eq!(
            rules.get("mri"),
            Some(&("1.0.2".to_string(), "pack-mri-v1.0.2".to_string()))
        );
        assert!(!rules.contains_key("lab") && !rules.contains_key("clinical"));

        // the engine's release again keeps it, and so does its own
        assert!(!plan(&dir, "9.0.0", carried.clone(), Vec::new(), &engine).behind());
        assert!(!plan(&dir, "9.0.0", carried, own, &engine).behind());
        assert!(
            refresh(&base, "9.0.0", &dir)
                .unwrap()
                .contains("are the ones engine 9.0.0")
        );
        assert!(read(&dir.join("mri/pack.yml")).contains("1.0.2"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_update_replaces_the_releases_packs_and_keeps_a_deployments_own() {
        let root = temp("swap");
        let (fresh, dir, aside) = (root.join("fresh"), root.join("packs"), root.join("aside"));
        for (path, text) in [
            (fresh.join("mri/pack.toml"), "the release's"),
            (fresh.join("clinical/pack.toml"), "the release's"),
            (dir.join("mri/pack.toml"), "the one before"),
            (dir.join("mri/gone.toml"), "no longer in the release"),
            (dir.join("clinical/pack.toml"), "the one before"),
            (dir.join("lab/pack.toml"), "the deployment's own"),
        ] {
            write(&path, text);
        }
        let swapped = swap_in(&fresh, &dir, &aside).unwrap();
        assert!(swapped.stranded.is_empty());
        assert_eq!(swapped.previous, Some(previous_of(&dir)));
        let read = |p: &str| std::fs::read_to_string(dir.join(p)).unwrap();
        assert_eq!(read("mri/pack.toml"), "the release's");
        assert_eq!(read("clinical/pack.toml"), "the release's");
        assert!(
            !dir.join("mri/gone.toml").exists(),
            "a pack the release brings is the release's, whole"
        );
        assert_eq!(read("lab/pack.toml"), "the deployment's own");
        assert!(!aside.exists() && !fresh.exists());
        assert!(previous_of(&dir).join("mri/gone.toml").exists());
        assert!(
            !previous_of(&dir).join("lab").exists(),
            "the deployment's own stays in use"
        );

        // where there were no packs, the release's are put in place
        let fresh = root.join("fresh-again");
        std::fs::create_dir_all(fresh.join("mri")).unwrap();
        let none = root.join("none");
        let swapped = swap_in(&fresh, &none, &aside).unwrap();
        assert_eq!(swapped.previous, None);
        assert!(none.join("mri").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tarball_that_names_a_place_outside_its_pack_is_refused() {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let body = b"x";
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        // written as raw bytes: the builder itself refuses such a name
        let name = b"packs/mri/../../escaped";
        header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        tar.append(&header, &body[..]).unwrap();
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        let e = offers_in(&bytes, "pack-mri-v1.0.0").unwrap_err();
        assert!(e.contains("outside its pack"), "{e}");
    }
}
