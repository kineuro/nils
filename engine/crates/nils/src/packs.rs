// SPDX-License-Identifier: AGPL-3.0-only
//! The rule packs an engine binary was released with, beside the ones in the
//! directory the engine reads.
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
//! A refresh is renames inside one parent directory: the release's packs are
//! unpacked beside the directory in use and swapped in, and the packs they
//! replace are kept whole in `<dir>.previous`, one update deep.
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
/// has one, and a digest of every file in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pack {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) digest: String,
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

/// The version a `pack.yml` states on a line of its own at the top level.
fn version_in(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("version:")?;
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
    let version = files
        .get("pack.yml")
        .and_then(|b| version_in(&String::from_utf8_lossy(b)));
    Pack {
        name,
        version,
        digest: digest_of(files),
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

/// The packs a release's `packs.tar.gz` carries, read without unpacking it.
pub(crate) fn in_tarball(bytes: &[u8]) -> Result<Vec<Pack>, String> {
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
        .map(|(name, files)| pack_of(name, &files))
        .collect())
}

/// What a refresh put in place, as the manifest keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub(crate) release: String,
    pub(crate) packs: BTreeMap<String, (Option<String>, String)>,
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
                    (
                        p["version"].as_str().map(str::to_string),
                        p["digest"].as_str()?.to_string(),
                    ),
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
            .map(|(name, (version, digest))| {
                (
                    name.clone(),
                    json!({ "version": version, "digest": digest }),
                )
            })
            .collect();
        let doc = json!({ "release": self.release, "packs": packs });
        let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
        std::fs::write(dir.join(MANIFEST), text + "\n")
            .map_err(|e| format!("{}: {e}", dir.join(MANIFEST).display()))
    }
}

/// The packs in a directory beside the ones a release carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) dir: PathBuf,
    /// The release the bundled packs are measured against.
    pub(crate) release: String,
    pub(crate) installed: Vec<Pack>,
    pub(crate) bundled: Vec<Pack>,
    /// Packs the release carries that differ from, or are missing in, the
    /// directory, and that an update replaces.
    pub(crate) stale: Vec<String>,
    /// First-party packs changed on this machine after an update put them
    /// there, which are kept.
    pub(crate) edited: Vec<String>,
    /// Packs the release does not carry: the site's own, which are kept.
    pub(crate) own: Vec<String>,
}

/// The directory's packs beside the release's, with the manifest a previous
/// refresh wrote, where there is one.
pub(crate) fn compare(
    dir: &Path,
    release: &str,
    installed: Vec<Pack>,
    bundled: Vec<Pack>,
    manifest: Option<&Manifest>,
) -> Status {
    let mut stale = Vec::new();
    let mut edited = Vec::new();
    for b in &bundled {
        match installed.iter().find(|i| i.name == b.name) {
            None => stale.push(b.name.clone()),
            Some(i) if i.digest == b.digest => {}
            Some(i) => {
                let put = manifest.and_then(|m| m.packs.get(&i.name));
                match put {
                    Some((_, digest)) if *digest != i.digest => edited.push(i.name.clone()),
                    _ => stale.push(i.name.clone()),
                }
            }
        }
    }
    let own = installed
        .iter()
        .filter(|i| !bundled.iter().any(|b| b.name == i.name))
        .map(|i| i.name.clone())
        .collect();
    Status {
        dir: dir.to_path_buf(),
        release: release.to_string(),
        installed,
        bundled,
        stale,
        edited,
        own,
    }
}

impl Status {
    /// Whether an update would change the directory.
    pub(crate) fn behind(&self) -> bool {
        !self.stale.is_empty()
    }

    fn installed_of(&self, name: &str) -> Option<&Pack> {
        self.installed.iter().find(|p| p.name == name)
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
                None => format!("{}: not installed; the release brings {}", b.name, b.said()),
                Some(i) if self.edited.contains(&b.name) => format!(
                    "{}: changed on this machine, kept (the release brings {})",
                    i.said(),
                    b.said()
                ),
                Some(i) if self.stale.contains(&b.name) => {
                    format!("{}: the release brings {}", i.said(), b.said())
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
            "behind": self.behind(),
            "command": "nils update --all",
        })
    }
}

/// The packs in `dir` beside the ones `release` carries, fetched from `base`
/// and checked against the release's sums.
pub(crate) fn status(base: &str, release: &str, dir: &Path) -> Result<Status, String> {
    let bytes = update::fetch_checked(base, release, "packs.tar.gz").map_err(|e| e.message)?;
    let bundled = in_tarball(&bytes)?;
    Ok(compare(
        dir,
        release,
        on_disk(dir),
        bundled,
        Manifest::read(dir).as_ref(),
    ))
}

/// Put the packs `release` carries in `dir`, where they differ from what is
/// there. The site's own packs and first-party packs edited here are kept;
/// the packs replaced are kept in `<dir>.previous`. What is said is one line.
pub(crate) fn refresh(base: &str, release: &str, dir: &Path) -> Result<String, String> {
    let parent = dir.parent().ok_or("the pack directory has no parent")?;
    if !update::writable(parent) {
        return Err(format!("{} is not writable by this user", parent.display()));
    }
    let bytes = update::fetch_checked(base, release, "packs.tar.gz").map_err(|e| e.message)?;
    let bundled = in_tarball(&bytes)?;
    let manifest = Manifest::read(dir);
    let status = compare(dir, release, on_disk(dir), bundled, manifest.as_ref());
    if !status.behind() {
        return Ok(format!(
            "the packs in {} are the ones engine {release} was released with",
            dir.display()
        ));
    }
    let staging = parent.join(format!(".nils-packs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    if let Err(e) = archive.unpack(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("unpacking the packs: {e}"));
    }
    let fresh = staging.join("packs");
    if !fresh.is_dir() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("the release's packs.tar.gz holds no packs directory".to_string());
    }
    // a pack edited here stays as it is: the release's copy of it is left
    // out, so the swap keeps the one in use as it keeps the site's own
    for name in &status.edited {
        let _ = std::fs::remove_dir_all(fresh.join(name));
    }
    let aside = parent.join(format!(".nils-packs-old-{}", std::process::id()));
    let swapped = swap(&fresh, dir, &aside);
    let _ = std::fs::remove_dir_all(&staging);
    let mut said = swapped?;
    // what this refresh put in place, and for a pack kept as edited, what
    // the one before it put there, so it is still known to be edited
    let mut packs = BTreeMap::new();
    for b in &status.bundled {
        if status.edited.contains(&b.name) {
            if let Some(line) = manifest.as_ref().and_then(|m| m.packs.get(&b.name)) {
                packs.insert(b.name.clone(), line.clone());
            }
        } else {
            packs.insert(b.name.clone(), (b.version.clone(), b.digest.clone()));
        }
    }
    let written = Manifest {
        release: release.to_string(),
        packs,
    }
    .write(dir);
    let replaced: Vec<String> = status
        .stale
        .iter()
        .map(|name| {
            let to = status.bundled.iter().find(|b| &b.name == name);
            match (status.installed_of(name), to) {
                (Some(i), Some(b)) => format!("{} to {}", i.said(), b.said()),
                (None, Some(b)) => b.said(),
                _ => name.clone(),
            }
        })
        .collect();
    said = format!("{said} ({})", replaced.join(", "));
    if !status.edited.is_empty() {
        said.push_str(&format!(
            "; {} changed on this machine and kept as it is, so remove it and update again to \
             take the release's",
            status.edited.join(", ")
        ));
    }
    if let Err(e) = written {
        said.push_str(&format!("; the manifest was not written: {e}"));
    }
    Ok(said)
}

/// Put the release's packs in `dir`. A pack the release does not carry, one
/// a deployment wrote for scans of its own, is kept: an update replaces only
/// the packs it brings. Everything is a rename inside one parent, so nothing
/// is copied, and what could not be put back is left where it was set aside
/// and never removed. The packs replaced are kept in `<dir>.previous`.
pub(crate) fn swap(fresh: &Path, dir: &Path, aside: &Path) -> Result<String, String> {
    let _ = std::fs::remove_dir_all(aside);
    let mut own: Vec<std::ffi::OsString> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .filter(|name| !name.to_string_lossy().starts_with('.'))
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
    // the packs replaced, kept one update deep; the old manifest goes with
    // them, since it named them
    let mut said = said;
    if aside.exists() {
        let previous = previous_of(dir);
        let _ = std::fs::remove_dir_all(&previous);
        if std::fs::rename(aside, &previous).is_ok() {
            said.push_str(&format!("; the ones before are in {}", previous.display()));
        } else {
            let _ = std::fs::remove_dir_all(aside);
        }
    }
    if own.is_empty() {
        Ok(said)
    } else {
        Ok(format!("{said}, and its own are kept: {}", names(&own)))
    }
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

    #[test]
    fn a_pack_reads_the_same_on_disk_and_in_the_tarball() {
        let root = temp("same");
        let tar = tarball(
            &root,
            &[
                (
                    "mri",
                    "pack.yml",
                    "# a comment\npack: mri\nversion: 0.8.0\n",
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
        assert_eq!(bundled[0].name, "clinical");
        assert_eq!(bundled[0].version, None);
        assert!(bundled[0].said().starts_with("clinical ("));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pack_is_stale_edited_or_the_sites_own_by_what_is_on_disk() {
        let pack = |name: &str, version: &str, digest: &str| Pack {
            name: name.to_string(),
            version: Some(version.to_string()),
            digest: digest.to_string(),
        };
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
            None,
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
        let manifest = Manifest {
            release: "8".to_string(),
            packs: [
                ("clinical".to_string(), (None, "c1".to_string())),
                (
                    "mri".to_string(),
                    (Some("0.7.0".to_string()), "m0".to_string()),
                ),
            ]
            .into(),
        };
        let s = compare(
            Path::new("/p"),
            "9",
            installed,
            bundled.clone(),
            Some(&manifest),
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
        let s = compare(Path::new("/p"), "9", bundled.clone(), bundled, None);
        assert!(!s.behind());
        assert_eq!(s.doc()["behind"], false);
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
        assert_eq!(manifest.packs["mri"].0.as_deref(), Some("0.8.0"));
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
        let said = swap(&fresh, &dir, &aside).unwrap();
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
        assert!(previous_of(&dir).join("mri/gone.toml").exists());

        // where there were no packs, the release's are put in place
        let fresh = root.join("fresh-again");
        std::fs::create_dir_all(fresh.join("mri")).unwrap();
        let none = root.join("none");
        let said = swap(&fresh, &none, &aside).unwrap();
        assert!(!said.contains("kept"), "{said}");
        assert!(none.join("mri").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }
}
