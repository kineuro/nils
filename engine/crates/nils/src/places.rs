// SPDX-License-Identifier: AGPL-3.0-only

//! Places at the doors (Wave 5 §12.5, §10.2). The registry keeps the
//! objects (`nils_registry::place`); this module binds every path-taking
//! verb to the role it needs, probes what the filesystem will say about a
//! path, and answers the one question the doors ask: may this verb write
//! here. The rules are enforced once a deployment has declared any place;
//! a registry from before this slice, or a laptop that has declared none,
//! keeps the flags it had, and the capabilities say so.

use std::path::Path;

use nils_registry::place::{self, Place, Role};
use nils_registry::store::Store;
use serde_json::{Value, json};

/// A verb that takes a path, and the role the path must be bound to.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub verb: &'static str,
    pub role: Role,
}

/// The binding of every path-taking verb to a role (§10.2).
pub const BINDINGS: &[Binding] = &[
    Binding {
        verb: "release --out",
        role: Role::Export,
    },
    Binding {
        verb: "handover run --out",
        role: Role::Exchange,
    },
    Binding {
        verb: "backup --dir",
        role: Role::Backup,
    },
    Binding {
        verb: "serve --backup-dir",
        role: Role::Backup,
    },
    Binding {
        verb: "serve --ingest-root",
        role: Role::Source,
    },
    Binding {
        verb: "digest <tree>",
        role: Role::Source,
    },
    Binding {
        verb: "ask export",
        role: Role::Share,
    },
    Binding {
        verb: "working dir",
        role: Role::Working,
    },
];

pub fn bindings_doc() -> Value {
    json!(
        BINDINGS
            .iter()
            .map(|b| json!({"verb": b.verb, "role": b.role.name()}))
            .collect::<Vec<_>>()
    )
}

/// Whether the rules are in force: a deployment has declared at least one
/// place that is not retired.
pub fn enforced(store: &mut Store) -> Result<bool, nils_registry::store::Error> {
    Ok(!place::active(store)?.is_empty())
}

/// The refusal a door answers with: a sentence naming the rule.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub message: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// May a verb that needs `role` write at `path`? The place that holds it
/// when it may; a refusal naming the rule when it may not. Never a
/// refusal while no place is declared, so the flags of a deployment
/// before this slice keep working.
pub fn require(store: &mut Store, role: Role, path: &Path) -> Result<Option<Place>, Refusal> {
    let err = |e: nils_registry::store::Error| Refusal {
        message: e.to_string(),
    };
    if !enforced(store).map_err(err)? {
        return Ok(None);
    }
    if let Some(source) = place::holding(store, Role::Source, path).map_err(err)?
        && role != Role::Source
    {
        return Err(Refusal {
            message: format!(
                "{} is under the source place {}, which the engine never writes (Wave 5 section 10.2)",
                path.display(),
                source.name
            ),
        });
    }
    if let Some(p) = place::holding(store, role, path).map_err(err)? {
        return Ok(Some(p));
    }
    let elsewhere = place::any_holding(store, path).map_err(err)?;
    let places = place::active(store)
        .map_err(err)?
        .into_iter()
        .filter(|p| p.role == role)
        .map(|p| format!("{} ({})", p.name, p.path))
        .collect::<Vec<_>>();
    Err(Refusal {
        message: match elsewhere {
            Some(other) => format!(
                "{} is under {}, a {} place; this verb writes only to a {} place ({}) (Wave 5 section 10.2)",
                path.display(),
                other.name,
                other.role.name(),
                role.name(),
                if places.is_empty() {
                    "none is declared".to_string()
                } else {
                    places.join(", ")
                }
            ),
            None => format!(
                "{} is under no declared place; this verb writes only to a {} place ({}) (Wave 5 section 10.2)",
                path.display(),
                role.name(),
                if places.is_empty() {
                    "none is declared; add one with nils place add".to_string()
                } else {
                    places.join(", ")
                }
            ),
        },
    })
}

/// What the filesystem will say about a path: whether it exists and is a
/// directory, whether the engine may write there, the free bytes, the
/// mount it sits on, and whether a snapshot directory is visible.
pub fn probe(path: &Path) -> Value {
    let exists = path.exists();
    let directory = path.is_dir();
    let writable = directory && writable(path);
    let free = if exists { free_bytes(path) } else { None };
    let mount = if exists { mount_of(path) } else { None };
    let snapshots_seen = if directory {
        Some(path.join(".zfs").join("snapshot").is_dir() || path.join(".snapshot").is_dir())
    } else {
        None
    };
    json!({
        "exists": exists,
        "directory": directory,
        "writable": writable,
        "free_bytes": free,
        "mount": mount,
        "snapshots_seen": snapshots_seen,
    })
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".nils-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The mount point under a path, from the kernel's own table where it is
/// readable; the longest mount prefix wins.
fn mount_of(path: &Path) -> Option<String> {
    let real = std::fs::canonicalize(path).ok()?;
    let table = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mut best: Option<String> = None;
    for line in table.lines() {
        // mount id, parent, major:minor, root, mount point, ...
        let Some(point) = line.split_whitespace().nth(4) else {
            continue;
        };
        let point = point.replace("\\040", " ");
        if real.starts_with(&point) && best.as_ref().is_none_or(|b| point.len() > b.len()) {
            best = Some(point);
        }
    }
    best
}

#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "statvfs fills a plain struct through the pointer it is given"
)]
fn free_bytes(path: &Path) -> Option<i64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a valid NUL-terminated path and the pointer is to a
    // struct of the right type that lives for the whole call.
    let rc = unsafe { libc::statvfs(c.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so the struct is initialised.
    let stat = unsafe { stat.assume_init() };
    i64::try_from(stat.f_bavail)
        .ok()?
        .checked_mul(i64::try_from(stat.f_frsize).ok()?)
}

#[cfg(not(unix))]
fn free_bytes(_path: &Path) -> Option<i64> {
    None
}

/// The paths a running engine has bound, for the listing: which of the
/// deployment's configured paths sit under each place.
pub fn bound_paths(place: &Place, configured: &[(&str, &Path)]) -> Vec<Value> {
    configured
        .iter()
        .filter(|(_, p)| place.holds_path(p))
        .map(|(verb, p)| json!({"verb": verb, "path": p.display().to_string()}))
        .collect()
}

/// The capabilities block: whether the rules are in force and how many
/// places are declared.
pub fn capabilities(store: &mut Store) -> Value {
    let places = place::active(store).unwrap_or_default();
    json!({
        "enforced": !places.is_empty(),
        "count": places.len(),
        "roles": Role::ALL.iter().map(|r| r.name()).collect::<Vec<_>>(),
    })
}
