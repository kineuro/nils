// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 4c §6.5: backup as an audited job, verify as a job, restore as a
//! command an operator runs with the engine stopped. An archive is one
//! directory: the registry and the linkage store (a VACUUM INTO on SQLite,
//! a custom-format pg_dump per schema on Postgres), the configuration, and
//! a manifest naming the registry, its epoch and schema version, and every
//! file with its size and digest. The key store is never in an archive: it
//! is copied on its own, as custody says.
use std::path::{Path, PathBuf};

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use nils_registry::home::{Home, LINKAGE_DB, REGISTRY_DB, Registry};
use nils_registry::time::now_iso;
use serde_json::{Value, json};

use crate::{Exit, fail, usage};

pub(crate) const MANIFEST: &str = "manifest.json";

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

/// Write one archive under `dir`; answers the manifest.
pub(crate) fn backup(home: &Home, registry: &mut Registry, dir: &Path) -> Result<Value, Exit> {
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
