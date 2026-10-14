// SPDX-License-Identifier: AGPL-3.0-only

//! The slice cache (record 55 E5, spec section 10.4): the small pictures a
//! model reads of a scan, kept per session in the site's working place so a
//! re-score after a model update reads them instead of the files.
//!
//! - **Where:** `<working place>/slice-cache/<input version>/`, one folder
//!   per model input's version (`bodypart-planes-b24:v1`), whose files are
//!   the image's own. A pipeline that declares the versions it keeps
//!   (`x-nils.cache.inputs`) is given the folder read-write at `/cache`;
//!   no other is, and nothing in it leaves the site, since the pixels are
//!   clinical.
//! - **Key:** the image keys each entry by the SOP Instance UIDs of the
//!   files it came from, the sha256 of their bytes and the input's version,
//!   never a registry id, so the cache survives a new registry (spec
//!   section 15.3). The engine names each file's SOP Instance UID in
//!   `stacks.json`, and the sha256 of its content where a run learnt it
//!   (`nils_registry::content`): the image looks there before it reads a
//!   file, and a miss reads the file as before.
//! - **Retention** (ruling R4 of 2026-10-10): a version an admitted or
//!   promoted model reads is kept; one no such model reads any more is
//!   dropped once it has gone unread for [`KEEP_DAYS`] days. When it went
//!   unread is kept beside the versions, in `retention.json`. Looked at
//!   after each run that used the cache, and by `nils cache prune`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nils_registry::store::Store;
use serde_json::{Value, json};

/// The cache's folder under the working place.
pub(crate) const DIR: &str = "slice-cache";

/// The file under the cache's folder that keeps when each version went
/// unread.
const RETENTION: &str = "retention.json";

/// How long a version no admitted model reads is kept (ruling R4).
pub(crate) const KEEP_DAYS: u64 = 30;

/// The slice cache's folder in a working place.
pub(crate) fn folder(working: &Path) -> PathBuf {
    working.join(DIR)
}

/// The input versions the slice cache keeps for a model that answers now:
/// those an active pipeline declares (`x-nils.cache.inputs`) where an
/// admitted or promoted model answers one of the axes it proposes. Answers
/// each version with the pipelines that read it.
pub(crate) fn read_versions(store: &mut Store) -> Result<BTreeMap<String, Vec<String>>, String> {
    let pipelines = nils_registry::pipeline::list(store).map_err(|e| e.to_string())?;
    let answering: BTreeSet<String> = nils_registry::model::list(store, &Default::default())
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|m| m.answers())
        .map(|m| m.task)
        .collect();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in pipelines.iter().filter(|p| p.state == "active") {
        let x = &p.descriptor["x-nils"];
        let served = x["proposals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["axis"].as_str())
            .any(|a| answering.contains(&format!("axis:{a}")));
        if !served {
            continue;
        }
        for v in x["cache"]["inputs"].as_array().into_iter().flatten() {
            if let Some(v) = v.as_str() {
                out.entry(v.to_string()).or_default().push(p.label());
            }
        }
    }
    Ok(out)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The files under a folder and their bytes, counted without following a
/// link.
fn size_of(dir: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(list) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in list.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                continue;
            };
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                files += 1;
                bytes += m.len();
            }
        }
    }
    (files, bytes)
}

/// What the cache in a working place holds, version by version, whether
/// an answering model reads each, since when one has gone unread, and,
/// with `prune`, the versions dropped for having gone unread
/// [`KEEP_DAYS`] days or more. `now` is seconds since the epoch.
pub(crate) fn look(
    store: &mut Store,
    working: &Path,
    prune: bool,
    now: u64,
) -> Result<Value, String> {
    let root = folder(working);
    let read = read_versions(store)?;
    if !root.is_dir() {
        return Ok(json!({"folder": DIR, "versions": [], "dropped": [], "keep_days": KEEP_DAYS}));
    }
    let kept_at = root.join(RETENTION);
    let mut unread: BTreeMap<String, u64> = std::fs::read(&kept_at)
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, u64>>(&b).ok())
        .unwrap_or_default();
    let mut versions = Vec::new();
    let mut dropped = Vec::new();
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut list: Vec<PathBuf> = std::fs::read_dir(&root)
        .map_err(|e| format!("the slice cache's folder: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    list.sort();
    for dir in list {
        let Some(version) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        // a folder that is no input version is the image's own business
        if !nils_pipeline::descriptor::is_cache_version(&version) {
            continue;
        }
        found.insert(version.clone());
        let (files, bytes) = size_of(&dir);
        let readers = read.get(&version).cloned().unwrap_or_default();
        let since = if readers.is_empty() {
            Some(*unread.entry(version.clone()).or_insert(now))
        } else {
            unread.remove(&version);
            None
        };
        let due = since.is_some_and(|s| now.saturating_sub(s) >= KEEP_DAYS * 86_400);
        if prune && due {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("the slice cache's version {version}: {e}"))?;
            unread.remove(&version);
            dropped.push(json!({"version": version, "files": files, "bytes": bytes}));
            continue;
        }
        versions.push(json!({
            "version": version, "files": files, "bytes": bytes,
            "read_by": readers,
            "unread_since": since,
            "dropped_after": since.map(|s| s + KEEP_DAYS * 86_400),
        }));
    }
    // a version whose folder went is forgotten
    unread.retain(|v, _| found.contains(v));
    let text = serde_json::to_vec_pretty(&unread).unwrap_or_default();
    let tmp = root.join(format!("{RETENTION}.partial"));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, &kept_at))
        .map_err(|e| format!("the slice cache's {RETENTION}: {e}"))?;
    Ok(json!({"folder": DIR, "versions": versions, "dropped": dropped, "keep_days": KEEP_DAYS}))
}

/// [`look`] at the cache now, pruning it: what a run that used the cache
/// does once it ended, best-effort.
pub(crate) fn after_run(store: &mut Store, working: &Path) -> Value {
    look(store, working, true, now_secs()).unwrap_or_else(|e| json!({"error": e}))
}

/// `nils cache show|prune`: the cache of each active working place.
pub(crate) fn command(
    home: &nils_registry::home::Home,
    prune: bool,
    json_out: bool,
) -> Result<(), crate::Exit> {
    let mut registry = crate::open(home)?;
    let places =
        nils_registry::place::list(registry.store()).map_err(|e| crate::fail(e.to_string()))?;
    let mut out = Vec::new();
    for pl in places
        .into_iter()
        .filter(|p| p.role == nils_registry::place::Role::Working && p.retired_at.is_none())
    {
        let mut doc =
            look(registry.store(), Path::new(&pl.path), prune, now_secs()).map_err(crate::fail)?;
        doc["place"] = json!(pl.name);
        out.push(doc);
    }
    if json_out {
        println!("{}", json!({"places": out}));
        return Ok(());
    }
    if out.is_empty() {
        println!("no working place: the slice cache lives in one");
    }
    for doc in &out {
        println!("{} ({}/):", doc["place"].as_str().unwrap_or(""), DIR);
        let versions = doc["versions"].as_array().cloned().unwrap_or_default();
        if versions.is_empty() {
            println!("  empty");
        }
        for v in versions {
            let read_by: Vec<String> = v["read_by"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r.as_str().map(str::to_string))
                .collect();
            println!(
                "  {}: {} files, {} bytes, {}",
                v["version"].as_str().unwrap_or(""),
                v["files"],
                v["bytes"],
                if read_by.is_empty() {
                    format!(
                        "no answering model reads it; dropped {} days after it went unread",
                        KEEP_DAYS
                    )
                } else {
                    format!("read by {}", read_by.join(", "))
                }
            );
        }
        for d in doc["dropped"].as_array().into_iter().flatten() {
            println!(
                "  dropped {}: {} files, {} bytes",
                d["version"].as_str().unwrap_or(""),
                d["files"],
                d["bytes"]
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ruling R4: a version no answering model reads is kept until it has
    /// gone unread for 30 days, then dropped; a folder that is no version
    /// is left alone.
    #[test]
    fn a_version_nobody_reads_is_dropped_after_thirty_days() {
        let dir = std::env::temp_dir().join(format!("nils-slice-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = folder(&dir);
        for v in ["old-planes:v1", "not a version"] {
            std::fs::create_dir_all(root.join(v)).unwrap();
            std::fs::write(root.join(v).join("s.npz"), b"1234").unwrap();
        }
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        let t0 = 1_000_000;
        let first = look(&mut store, &dir, true, t0).unwrap();
        assert_eq!(first["versions"][0]["version"], "old-planes:v1", "{first}");
        assert_eq!(first["versions"][0]["unread_since"], t0, "{first}");
        assert_eq!(first["versions"][0]["files"], 1, "{first}");
        assert_eq!(first["versions"].as_array().unwrap().len(), 1, "{first}");
        let later = look(&mut store, &dir, true, t0 + 29 * 86_400).unwrap();
        assert!(later["dropped"].as_array().unwrap().is_empty(), "{later}");
        assert_eq!(later["versions"][0]["unread_since"], t0, "{later}");
        let shown = look(&mut store, &dir, false, t0 + 31 * 86_400).unwrap();
        assert!(
            shown["dropped"].as_array().unwrap().is_empty(),
            "shown only: {shown}"
        );
        let pruned = look(&mut store, &dir, true, t0 + 31 * 86_400).unwrap();
        assert_eq!(pruned["dropped"][0]["version"], "old-planes:v1", "{pruned}");
        assert!(!root.join("old-planes:v1").exists());
        assert!(root.join("not a version").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
