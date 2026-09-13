// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 4c §6.5: backup as an audited job, verify as a job, restore as a
//! command an operator runs with the engine stopped. An archive is one
//! directory: the registry and the linkage store (a VACUUM INTO on SQLite,
//! a custom-format pg_dump per schema on Postgres), the configuration, and
//! a manifest naming the registry, its epoch and schema version, and every
//! file with its size and digest. The key store is never in an archive: it
//! is copied on its own, as custody says.
//!
//! Wave 5 §10.3: the archives in a directory are listed with what each
//! holds, how long it took and its last check; a backup can keep only the
//! newest few of its registry; and a restore can be rehearsed, every store
//! opened as a restore would open it, without applying it. The outcome of a
//! check is kept beside the manifest, which is never touched.
use std::path::{Path, PathBuf};

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use nils_registry::home::{Home, LINKAGE_DB, REGISTRY_DB, Registry};
use nils_registry::store::Store;
use nils_registry::time::{now_iso, secs_of};
use serde_json::{Value, json};

use crate::{Exit, fail, usage};

pub(crate) const MANIFEST: &str = "manifest.json";
/// The last check of an archive, kept beside its manifest.
pub(crate) const CHECKED: &str = "checked.json";

fn digest_file(path: &Path) -> Result<(u64, String), Exit> {
    let bytes = std::fs::read(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let mut h = Blake2b::<U32>::new();
    h.update(&bytes);
    Ok((bytes.len() as u64, hex::encode(h.finalize())))
}

fn stamp() -> String {
    now_iso()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(14)
        .collect()
}

fn manifest_of(archive: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(archive.join(MANIFEST)).ok()?).ok()
}

/// Write one archive under `dir`; answers the manifest.
pub(crate) fn backup(home: &Home, registry: &mut Registry, dir: &Path) -> Result<Value, Exit> {
    let started_at = now_iso();
    let meta = registry.meta().clone();
    let backend = format!("{:?}", registry.config().backend).to_lowercase();
    let archive = dir.join(format!(
        "{}-{}",
        &meta.registry_id[..8.min(meta.registry_id.len())],
        stamp()
    ));
    std::fs::create_dir_all(&archive).map_err(|e| fail(format!("{}: {e}", archive.display())))?;
    let mut files: Vec<PathBuf> = Vec::new();
    match backend.as_str() {
        "sqlite" => {
            for (name, kind) in [(REGISTRY_DB, "registry"), (LINKAGE_DB, "linkage")] {
                let target = archive.join(name);
                let sql = format!(
                    "VACUUM INTO \'{}\'",
                    target.display().to_string().replace('\'', "''")
                );
                let r = if kind == "registry" {
                    registry.store().batch(&sql)
                } else {
                    let mut linkage = registry.open_linkage().map_err(|e| fail(e.to_string()))?;
                    linkage.batch(&sql)
                };
                r.map_err(|e| fail(format!("{kind}: {e}")))?;
                files.push(target);
            }
        }
        _ => {
            let dsn = registry.dsn().map_err(|e| fail(e.to_string()))?;
            let schema = registry.config().schema.clone();
            for (suffix, schema_name) in [
                ("registry.dump", schema.clone()),
                ("linkage.dump", format!("{schema}_linkage")),
            ] {
                let target = archive.join(suffix);
                let status = std::process::Command::new("pg_dump")
                    .args(["--format=custom", "--no-owner", "--schema"])
                    .arg(&schema_name)
                    .arg("--file")
                    .arg(&target)
                    .arg(&dsn)
                    .status()
                    .map_err(|e| {
                        fail(format!(
                            "pg_dump: {e}; a Postgres backup needs pg_dump on the path"
                        ))
                    })?;
                if !status.success() {
                    return Err(fail(format!("pg_dump {schema_name} exited {status}")));
                }
                files.push(target);
            }
        }
    }
    let config = home.dir().join("nils.toml");
    if config.is_file() {
        let target = archive.join("nils.toml");
        std::fs::copy(&config, &target).map_err(|e| fail(format!("{}: {e}", config.display())))?;
        files.push(target);
    }
    let mut listed = Vec::new();
    for f in &files {
        let (bytes, digest) = digest_file(f)?;
        listed.push(json!({
            "name": f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "bytes": bytes,
            "blake2b": digest,
        }));
    }
    let manifest = json!({
        "registry_id": meta.registry_id,
        "epoch": meta.epoch,
        "schema_version": meta.schema_version,
        "backend": backend,
        "started_at": started_at,
        "created_at": now_iso(),
        "files": listed,
        "keys": "not included: copy the key store on its own (nils custody)",
        "archive": archive.display().to_string(),
    });
    std::fs::write(
        archive.join(MANIFEST),
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| fail(format!("{}: {e}", archive.join(MANIFEST).display())))?;
    Ok(manifest)
}

/// The archives in a directory, newest first: what each holds, how long it
/// took, whether it is this registry's, and its last check.
pub(crate) fn archives(dir: &Path, registry_id: &str) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let manifest = manifest_of(&path)?;
            let files = manifest["files"].as_array().cloned().unwrap_or_default();
            let started = manifest["started_at"].as_str().and_then(secs_of);
            let created = manifest["created_at"].as_str().and_then(secs_of);
            let checked = std::fs::read_to_string(path.join(CHECKED))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            Some(json!({
                "name": entry.file_name().to_string_lossy(),
                "created_at": manifest["created_at"],
                "seconds": match (started, created) {
                    (Some(a), Some(b)) if b >= a => Some(b - a),
                    _ => None,
                },
                "bytes": files.iter().filter_map(|f| f["bytes"].as_u64()).sum::<u64>(),
                "files": files.len(),
                "epoch": manifest["epoch"],
                "schema_version": manifest["schema_version"],
                "backend": manifest["backend"],
                "ours": manifest["registry_id"] == registry_id,
                "checked": checked,
            }))
        })
        .collect();
    out.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
    out
}

/// Keep the newest `keep` archives of this registry in a directory and
/// remove the others. Only a directory holding this registry's manifest is
/// ever removed, and never the archive just written. Answers the names
/// removed.
pub(crate) fn prune(
    dir: &Path,
    registry_id: &str,
    keep: usize,
    written: &Path,
) -> Result<Vec<String>, Exit> {
    let entries = std::fs::read_dir(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let mut ours: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter_map(|path| {
            let manifest = manifest_of(&path)?;
            (manifest["registry_id"] == registry_id).then(|| {
                let created = manifest["created_at"].as_str().unwrap_or_default();
                (created.to_string(), path)
            })
        })
        .collect();
    // the archive just written first, then the newest
    ours.sort_by(|a, b| {
        (b.1 == written)
            .cmp(&(a.1 == written))
            .then_with(|| b.0.cmp(&a.0))
    });
    let mut removed = Vec::new();
    for (_, path) in ours.into_iter().skip(keep.max(1)) {
        std::fs::remove_dir_all(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
        removed.push(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
    }
    removed.sort();
    Ok(removed)
}

/// Read an archive's manifest and check every file against it.
pub(crate) fn verify(archive: &Path) -> Result<Value, Exit> {
    let text = std::fs::read_to_string(archive.join(MANIFEST))
        .map_err(|e| usage(format!("{}: {e}", archive.join(MANIFEST).display())))?;
    let manifest: Value = serde_json::from_str(&text)
        .map_err(|e| fail(format!("{}: not a manifest: {e}", archive.display())))?;
    let mut checked = Vec::new();
    let mut ok = true;
    for f in manifest["files"].as_array().cloned().unwrap_or_default() {
        let name = f["name"].as_str().unwrap_or_default().to_string();
        let path = archive.join(&name);
        let state = match digest_file(&path) {
            Ok((bytes, digest)) => {
                if f["blake2b"] == digest && f["bytes"] == bytes {
                    "ok"
                } else {
                    ok = false;
                    "differs"
                }
            }
            Err(_) => {
                ok = false;
                "missing"
            }
        };
        checked.push(json!({"name": name, "state": state}));
    }
    Ok(json!({
        "ok": ok,
        "archive": archive.display().to_string(),
        "registry_id": manifest["registry_id"],
        "epoch": manifest["epoch"],
        "schema_version": manifest["schema_version"],
        "backend": manifest["backend"],
        "created_at": manifest["created_at"],
        "files": checked,
    }))
}

/// Rehearse a restore without applying it: every file checked against the
/// manifest, then each store opened as a restore would open it, a SQLite
/// file read only with its integrity checked and its registry named, a
/// Postgres dump read back by pg_restore. Nothing is written.
pub(crate) fn rehearse(archive: &Path) -> Result<Value, Exit> {
    let mut doc = verify(archive)?;
    let mut opened = Vec::new();
    if doc["ok"] == true {
        if doc["backend"] == "sqlite" {
            let id = doc["registry_id"].as_str().unwrap_or_default().to_string();
            for name in [REGISTRY_DB, LINKAGE_DB] {
                let named = (name == REGISTRY_DB).then_some(id.as_str());
                opened
                    .push(json!({"name": name, "state": open_sqlite(&archive.join(name), named)}));
            }
        } else {
            for name in ["registry.dump", "linkage.dump"] {
                opened.push(json!({"name": name, "state": read_dump(&archive.join(name))}));
            }
        }
    }
    let ok = doc["ok"] == true && opened.iter().all(|o| o["state"] == "opens");
    doc["ok"] = json!(ok);
    doc["rehearsed"] = json!(true);
    doc["opened"] = json!(opened);
    Ok(doc)
}

/// A SQLite store in an archive, opened read only: `opens`, or what stops it.
fn open_sqlite(path: &Path, registry_id: Option<&str>) -> String {
    let mut store = match Store::open_sqlite_read_only(path) {
        Ok(store) => store,
        Err(e) => return format!("does not open: {e}"),
    };
    match store.query("PRAGMA quick_check", &[]) {
        Ok(rows) if rows.first().and_then(|r| r.text(0).ok()) == Some("ok") => {}
        Ok(_) => return "fails its integrity check".to_string(),
        Err(e) => return format!("does not open: {e}"),
    }
    if let Some(id) = registry_id {
        let named = store
            .query(
                "SELECT value FROM registry_meta WHERE key = 'registry_id'",
                &[],
            )
            .ok()
            .and_then(|rows| {
                rows.first()
                    .and_then(|r| r.text(0).ok().map(str::to_string))
            });
        if named.as_deref() != Some(id) {
            return "names another registry".to_string();
        }
    }
    "opens".to_string()
}

/// A Postgres dump in an archive, read back by pg_restore: `opens`, or what
/// stops it.
fn read_dump(path: &Path) -> String {
    match std::process::Command::new("pg_restore")
        .arg("--list")
        .arg(path)
        .output()
    {
        Ok(out)
            if out.status.success()
                && String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .any(|l| !l.trim().is_empty() && !l.starts_with(';')) =>
        {
            "opens".to_string()
        }
        Ok(out) => format!(
            "pg_restore cannot read it: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("no entries")
        ),
        Err(e) => {
            format!("pg_restore: {e}; rehearsing a Postgres archive needs pg_restore on the path")
        }
    }
}

/// Keep the outcome of a check beside the archive, for the list of archives.
pub(crate) fn record(archive: &Path, doc: &Value) -> Result<(), Exit> {
    let kept = json!({
        "at": now_iso(),
        "ok": doc["ok"],
        "rehearsed": doc["rehearsed"] == true,
        "files": doc["files"],
        "opened": doc["opened"],
    });
    let path = archive.join(CHECKED);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&kept).unwrap_or_default(),
    )
    .map_err(|e| fail(format!("{}: {e}", path.display())))
}

/// Put an archive back, with the engine stopped: a pre-restore archive is
/// written first, then the stores are replaced. Never a job.
pub(crate) fn restore(
    home: &Home,
    registry: &mut Registry,
    archive: &Path,
    yes: bool,
) -> Result<Value, Exit> {
    let verified = verify(archive)?;
    if verified["ok"] != true {
        return Err(usage(format!(
            "{}: the archive does not verify; nothing was restored",
            archive.display()
        )));
    }
    let backend = format!("{:?}", registry.config().backend).to_lowercase();
    if verified["backend"] != backend {
        return Err(usage(format!(
            "the archive is {} and this registry is {backend}",
            verified["backend"]
        )));
    }
    if verified["registry_id"] != registry.meta().registry_id {
        return Err(usage(
            "the archive belongs to another registry; nothing was restored",
        ));
    }
    if !yes {
        return Err(usage(
            "restore replaces the registry and the linkage store; stop nils serve, then run again with --yes",
        ));
    }
    let before = home.dir().join("backups-before-restore");
    let pre = backup(home, registry, &before)?;
    match backend.as_str() {
        "sqlite" => {
            for name in [REGISTRY_DB, LINKAGE_DB] {
                let src = archive.join(name);
                let dst = home.dir().join(name);
                for side in ["-wal", "-shm"] {
                    let _ = std::fs::remove_file(home.dir().join(format!("{name}{side}")));
                }
                std::fs::copy(&src, &dst).map_err(|e| fail(format!("{}: {e}", src.display())))?;
            }
        }
        _ => {
            let dsn = registry.dsn().map_err(|e| fail(e.to_string()))?;
            for suffix in ["registry.dump", "linkage.dump"] {
                let status = std::process::Command::new("pg_restore")
                    .args(["--clean", "--if-exists", "--no-owner", "--dbname"])
                    .arg(&dsn)
                    .arg(archive.join(suffix))
                    .status()
                    .map_err(|e| fail(format!("pg_restore: {e}")))?;
                if !status.success() {
                    return Err(fail(format!("pg_restore {suffix} exited {status}")));
                }
            }
        }
    }
    Ok(json!({"restored": archive.display().to_string(), "pre_restore": pre["archive"]}))
}
