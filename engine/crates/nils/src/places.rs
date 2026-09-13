// SPDX-License-Identifier: AGPL-3.0-only

//! Places at the doors (Wave 5 §12.5, §10.2). The registry keeps the
//! objects (`nils_registry::place`); this module binds every path-taking
//! verb to the role it needs, probes what the filesystem will say about a
//! path, and answers the one question the doors ask: may this verb write
//! here. The rules are enforced once a deployment has declared any place;
//! a registry from before this slice, or a laptop that has declared none,
//! keeps the flags it had, and the capabilities say so.

use std::path::{Component, Path, PathBuf};

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
    holding(&mount_table(), &real).map(|m| m.point.clone())
}

/// A disk mounted, or named in /etc/fstab: where it is mounted, its
/// filesystem, and what it is a mount of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mount {
    pub point: String,
    pub fs: String,
    pub source: String,
}

/// Filesystems reached over a network.
const NETWORK: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "fuse.sshfs",
    "sshfs",
    "ceph",
    "glusterfs",
    "fuse.glusterfs",
    "lustre",
    "beegfs",
    "9p",
    "afs",
    "davfs",
    "fuse.rclone",
];

/// Filesystems the system keeps for itself, which hold no one's data.
const PSEUDO: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "cgroup",
    "cgroup2",
    "securityfs",
    "pstore",
    "bpf",
    "debugfs",
    "tracefs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "overlay",
    "squashfs",
    "nsfs",
    "efivarfs",
    "binfmt_misc",
    "rpc_pipefs",
    "ramfs",
    "selinuxfs",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "fuse.lxcfs",
    "fuse.snapfuse",
    "nfsd",
    "rootfs",
    "devfs",
    "swap",
    "none",
];

/// Where the system mounts what it keeps for itself.
const SYSTEM: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/boot",
    "/snap",
    "/var/lib/docker",
    "/var/lib/containers",
    "/var/snap",
];

impl Mount {
    /// Whether the disk is reached over a network.
    pub fn network(&self) -> bool {
        NETWORK.contains(&self.fs.as_str())
    }

    /// Whether the filesystem is one the system keeps for itself.
    pub fn pseudo(&self) -> bool {
        PSEUDO.contains(&self.fs.as_str())
    }

    /// Whether the mount point is one the system keeps for itself.
    pub fn system(&self) -> bool {
        SYSTEM.iter().any(|s| Path::new(&self.point).starts_with(s))
    }
}

/// The kernel's mount table and /etc/fstab, read where the system keeps
/// them, on Linux; elsewhere both are empty.
#[derive(Debug, Clone, Default)]
pub(crate) struct Tables {
    pub mounts: Vec<Mount>,
    pub fstab: Vec<Mount>,
}

impl Tables {
    pub fn read() -> Tables {
        Tables {
            mounts: mount_table(),
            fstab: fstab_table(),
        }
    }

    /// The mount a canonical path lives on: the longest mount point it
    /// starts with.
    pub fn holding(&self, path: &Path) -> Option<&Mount> {
        holding(&self.mounts, path)
    }

    /// What is mounted at exactly this point, if anything.
    pub fn at(&self, point: &Path) -> Option<&Mount> {
        in_force(self.mounts.iter().filter(|m| Path::new(&m.point) == point))
    }

    /// The disk /etc/fstab names for a point when nothing is mounted there.
    pub fn unmounted_at(&self, point: &Path) -> Option<&Mount> {
        if self.at(point).is_some() {
            return None;
        }
        self.fstab
            .iter()
            .find(|m| Path::new(&m.point) == point && !m.pseudo())
    }
}

fn mount_table() -> Vec<Mount> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|t| parse_mountinfo(&t))
        .unwrap_or_default()
}

fn fstab_table() -> Vec<Mount> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    std::fs::read_to_string("/etc/fstab")
        .map(|t| parse_fstab(&t))
        .unwrap_or_default()
}

/// Of mounts stacked on one point, the one in force: the last mounted, and a
/// real disk over an automount's placeholder.
fn in_force<'a>(mounts: impl Iterator<Item = &'a Mount>) -> Option<&'a Mount> {
    mounts.fold(None, |best, m| match best {
        Some(b) if m.fs == "autofs" && b.fs != "autofs" => Some(b),
        _ => Some(m),
    })
}

fn holding<'a>(mounts: &'a [Mount], path: &Path) -> Option<&'a Mount> {
    let under = |m: &&Mount| path.starts_with(&m.point);
    let longest = mounts.iter().filter(under).map(|m| m.point.len()).max()?;
    in_force(
        mounts
            .iter()
            .filter(under)
            .filter(|m| m.point.len() == longest),
    )
}

/// The kernel's mount table, `/proc/self/mountinfo`: the mount point is the
/// fifth field, and the filesystem and its source follow the `-` that ends
/// the optional fields.
pub(crate) fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let point = fields.get(4)?;
            let dash = 6 + fields.get(6..)?.iter().position(|f| *f == "-")?;
            Some(Mount {
                point: unescape(point),
                fs: (*fields.get(dash + 1)?).to_string(),
                source: fields
                    .get(dash + 2)
                    .map(|s| unescape(s))
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// /etc/fstab: a source, a mount point and a filesystem on each line that is
/// not a comment. Swap, and anything not mounted on a path, are left out.
pub(crate) fn parse_fstab(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut fields = line.split_whitespace();
            let source = unescape(fields.next()?);
            let point = plain(&unescape(fields.next()?))?;
            let fs = fields.next()?.to_string();
            (fs != "swap").then(|| Mount {
                point: point.display().to_string(),
                fs,
                source,
            })
        })
        .collect()
}

/// A field of the mount table or of fstab with its octal escapes undone:
/// `\040` is a space, `\011` a tab, `\012` a newline and `\134` a backslash.
pub(crate) fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes
            .get(i + 1..i + 4)
            .filter(|d| bytes[i] == b'\\' && d.iter().all(|b| (b'0'..=b'7').contains(b)))
            .and_then(|d| {
                let value = d.iter().fold(0u32, |n, b| n * 8 + u32::from(b - b'0'));
                u8::try_from(value).ok()
            });
        match byte {
            Some(b) => {
                out.push(b);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An absolute path made plain without touching the disk: `.` dropped, `..`
/// taking the folder before it and never going above the root, and no
/// trailing slash but the root's. None for a path that is not absolute.
pub(crate) fn plain(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    Some(out)
}

/// The free and the total bytes of the filesystem a path is on.
#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "statvfs fills a plain struct through the pointer it is given"
)]
pub(crate) fn space(path: &Path) -> Option<(i64, i64)> {
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
    let block = i64::try_from(stat.f_frsize).ok()?;
    let free = i64::try_from(stat.f_bavail).ok()?.checked_mul(block)?;
    let total = i64::try_from(stat.f_blocks).ok()?.checked_mul(block)?;
    Some((free, total))
}

#[cfg(not(unix))]
pub(crate) fn space(_path: &Path) -> Option<(i64, i64)> {
    None
}

fn free_bytes(path: &Path) -> Option<i64> {
    space(path).map(|(free, _)| free)
}

/// Whether this account may list a folder and go inside it.
#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "access reads the NUL-terminated path it is given and nothing else"
)]
pub(crate) fn may_enter(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path that lives for the whole call.
    unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::X_OK) == 0 }
}

#[cfg(not(unix))]
pub(crate) fn may_enter(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok()
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
