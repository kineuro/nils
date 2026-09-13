// SPDX-License-Identifier: AGPL-3.0-only

//! The folders inside a folder on the host, for a path browser
//! (`POST /api/supervise/folders`). Each folder says whether this account may
//! open it, whether a disk is mounted there, and whether /etc/fstab names a
//! disk there that nothing is mounted for; the folder itself says the disk it
//! is on, with its room. With no path the answer says where to start. The
//! supervisor serves one call at a time, and a network mount that does not
//! answer would hold every call behind it, so the disk is read in a thread of
//! its own, and past a few seconds the answer says so.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::places::{Mount, Tables, may_enter, space};

/// How long a listing may take before the call is answered without it.
const WAIT: Duration = Duration::from_secs(5);
/// The folders a listing shows before it says it is partial.
const SHOWN: usize = 500;
/// The files a listing counts before it says it is partial.
const COUNTED: usize = 10_000;
/// Past this many entries of any kind a listing stops reading.
const READ: usize = 200_000;

/// The door's answer: the folders inside `path`, or where to start.
pub(crate) fn answer(path: Option<&Path>) -> Value {
    match path {
        Some(p) => listed(p, Tables::read(), WAIT),
        None => starts(
            Tables::read(),
            std::env::var_os("HOME").map(PathBuf::from),
            WAIT,
        ),
    }
}

/// Work that may wait on a disk, done in a thread of its own: its answer when
/// it ends within `wait`, else None, and the thread is left to end in its own
/// time.
fn within<T: Send + 'static>(
    wait: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(wait).ok()
}

/// A disk, with its room where it was measured.
fn disk(m: &Mount, measured: bool) -> Value {
    let room = if measured {
        space(Path::new(&m.point))
    } else {
        None
    };
    json!({
        "point": m.point,
        "fs": m.fs,
        "source": m.source,
        "network": m.network(),
        "free_bytes": room.map(|(free, _)| free),
        "total_bytes": room.map(|(_, total)| total),
    })
}

/// A disk /etc/fstab names that nothing is mounted for.
fn unmounted(m: &Mount) -> Value {
    json!({ "fs": m.fs, "source": m.source, "network": m.network() })
}

/// The folders inside a folder, given `wait` to read them.
fn listed(path: &Path, tables: Tables, wait: Duration) -> Value {
    // what is known without the disk, for an answer past the wait
    let known = json!({
        "path": path.display().to_string(),
        "parent": path.parent().map(|p| p.display().to_string()),
        "exists": null,
        "directory": null,
        "readable": null,
        "mount": tables.holding(path).map(|m| disk(m, false)),
        "folders": [],
        "files": 0,
        "partial": false,
        "timed_out": true,
    });
    let owned = path.to_path_buf();
    within(wait, move || listing(&owned, &tables)).unwrap_or(known)
}

/// The listing itself, which reads the disk.
fn listing(path: &Path, tables: &Tables) -> Value {
    let real = std::fs::canonicalize(path).ok();
    let doc = |exists: bool,
               directory: bool,
               readable: bool,
               folders: Vec<Value>,
               files: usize,
               partial: bool| {
        json!({
            "path": path.display().to_string(),
            "parent": path.parent().map(|p| p.display().to_string()),
            "exists": exists,
            "directory": directory,
            "readable": readable,
            "mount": real.as_deref().and_then(|r| tables.holding(r)).map(|m| disk(m, true)),
            "folders": folders,
            "files": files,
            "partial": partial,
            "timed_out": false,
        })
    };
    let meta = std::fs::metadata(path);
    if !meta.as_ref().is_ok_and(std::fs::Metadata::is_dir) {
        return doc(meta.is_ok(), false, false, Vec::new(), 0, false);
    }
    let entries = if may_enter(path) {
        std::fs::read_dir(path).ok()
    } else {
        None
    };
    let Some(entries) = entries else {
        return doc(true, true, false, Vec::new(), 0, false);
    };
    let mut names: Vec<(String, bool)> = Vec::new();
    let (mut files, mut partial) = (0, false);
    for (n, entry) in entries.flatten().enumerate() {
        if n >= READ {
            partial = true;
            break;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let link = kind.is_symlink();
        if kind.is_dir() || (link && entry.path().is_dir()) {
            names.push((entry.file_name().to_string_lossy().into_owned(), link));
        } else if files < COUNTED {
            files += 1;
        } else {
            partial = true;
        }
    }
    names.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then_with(|| a.0.cmp(&b.0))
    });
    if names.len() > SHOWN {
        names.truncate(SHOWN);
        partial = true;
    }
    let base = real.clone().unwrap_or_else(|| path.to_path_buf());
    let folders = names
        .iter()
        .map(|(name, link)| {
            let inside = path.join(name);
            // the folder in the mount table's own terms, a link followed
            let point = if *link {
                std::fs::canonicalize(&inside).ok()
            } else {
                Some(base.join(name))
            };
            let point = point.as_deref();
            json!({
                "name": name,
                "readable": may_enter(&inside),
                "hidden": name.starts_with('.'),
                "link": link,
                "mount": point.and_then(|p| tables.at(p)).map(|m| disk(m, true)),
                "unmounted": point.and_then(|p| tables.unmounted_at(p)).map(unmounted),
            })
        })
        .collect();
    doc(true, true, true, folders, files, partial)
}

/// What a starting point is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Root,
    Home,
    Mount,
    Fstab,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Root => "root",
            Kind::Home => "home",
            Kind::Mount => "mount",
            Kind::Fstab => "fstab",
        }
    }
}

#[derive(Debug)]
struct Start {
    path: PathBuf,
    kind: Kind,
}

impl Start {
    fn doc(&self, tables: &Tables, measured: bool) -> Value {
        let on = match self.kind {
            Kind::Fstab => None,
            // a home folder may be a link; its disk is the one it leads to
            Kind::Home if measured => std::fs::canonicalize(&self.path)
                .ok()
                .and_then(|p| tables.holding(&p)),
            _ => tables.holding(&self.path),
        };
        let named = match self.kind {
            Kind::Fstab => tables.unmounted_at(&self.path).map(unmounted),
            _ => None,
        };
        json!({
            "path": self.path.display().to_string(),
            "kind": self.kind.name(),
            "mount": on.map(|m| disk(m, measured)),
            "unmounted": named,
        })
    }
}

/// Where a browser starts, given `wait` to measure the disks.
fn starts(tables: Tables, home: Option<PathBuf>, wait: Duration) -> Value {
    // what is known without the disk, for an answer past the wait
    let known = json!({
        "path": null,
        "roots": points(&tables, None).iter().map(|s| s.doc(&tables, false)).collect::<Vec<_>>(),
        "timed_out": true,
    });
    within(wait, move || {
        let home = home.filter(|h| h.is_dir());
        let roots: Vec<Value> = points(&tables, home.as_deref())
            .iter()
            .map(|s| s.doc(&tables, true))
            .collect();
        json!({ "path": null, "roots": roots, "timed_out": false })
    })
    .unwrap_or(known)
}

/// The root, then the home folder, every disk mounted that holds data, and
/// every disk /etc/fstab names that nothing is mounted for, each sorted.
fn points(tables: &Tables, home: Option<&Path>) -> Vec<Start> {
    let mut out = vec![Start {
        path: PathBuf::from("/"),
        kind: Kind::Root,
    }];
    if let Some(home) = home.filter(|h| *h != Path::new("/")) {
        out.push(Start {
            path: home.to_path_buf(),
            kind: Kind::Home,
        });
    }
    let data = |m: &&Mount| !m.pseudo() && !m.system();
    let mut mounted: Vec<PathBuf> = tables
        .mounts
        .iter()
        .filter(data)
        .map(|m| PathBuf::from(&m.point))
        .collect();
    mounted.sort();
    mounted.dedup();
    let mut named: Vec<PathBuf> = tables
        .fstab
        .iter()
        .filter(data)
        .map(|m| PathBuf::from(&m.point))
        .filter(|p| tables.at(p).is_none())
        .collect();
    named.sort();
    named.dedup();
    for (paths, kind) in [(mounted, Kind::Mount), (named, Kind::Fstab)] {
        for path in paths {
            if !out.iter().any(|s| s.path == path) {
                out.push(Start { path, kind });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::places::{parse_fstab, parse_mountinfo};
    use nils_dicom::synth::TempDir;

    const MOUNTINFO: &str = r"22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw,errors=remount-ro
23 22 0:22 / /proc rw,nosuid,nodev,noexec shared:12 - proc proc rw
24 22 0:23 / /sys rw,nosuid,nodev,noexec shared:2 - sysfs sysfs rw
25 22 0:24 / /run rw,nosuid,nodev shared:5 - tmpfs tmpfs rw,size=1628k
26 22 0:45 / /srv rw,noatime shared:7 - zfs pool/srv rw,xattr
27 22 8:17 / /data rw,relatime - xfs /dev/sdb1 rw
28 22 0:46 / /archive rw,relatime shared:9 - autofs systemd-1 rw,fd=51
29 28 0:47 / /archive rw,relatime shared:10 master:3 - nfs4 files:/export/archive rw,vers=4.2
30 22 0:48 / /var/lib/docker/overlay2/abc/merged rw,relatime - overlay overlay rw
31 22 8:33 / /media/usb\040disk rw,relatime shared:11 - exfat /dev/sdc1 rw
32 22 8:2 / /boot rw,relatime shared:13 - ext4 /dev/sda2 rw
";

    const FSTAB: &str = r"# made at install
UUID=0a1b / ext4 defaults 0 1
/dev/sdb1   /data/   xfs defaults 0 2
files:/export/archive /archive nfs4 x-systemd.automount,_netdev 0 0
/dev/sdd1 /media/backup ext4 noauto 0 0
tmpfs /tmp tmpfs defaults 0 0
/swapfile none swap sw 0 0
//files/share /srv/share\040store cifs credentials=/etc/cifs 0 0
";

    fn mount(point: &Path, fs: &str, source: &str) -> Mount {
        Mount {
            point: point.display().to_string(),
            fs: fs.to_string(),
            source: source.to_string(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_lists_the_folders_inside_it_with_their_disks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = TempDir::new("folders");
        let here = std::fs::canonicalize(dir.path()).unwrap();
        for name in ["Beta", "alpha", ".cache", "scans", "locked"] {
            std::fs::create_dir(here.join(name)).unwrap();
        }
        std::fs::write(here.join("one.txt"), "1").unwrap();
        std::fs::write(here.join("two.txt"), "2").unwrap();
        symlink(here.join("alpha"), here.join("gamma")).unwrap();
        symlink(here.join("one.txt"), here.join("three.txt")).unwrap();
        let tables = Tables {
            mounts: vec![
                mount(Path::new("/"), "ext4", "/dev/sda1"),
                mount(&here.join("scans"), "nfs4", "files:/export/scans"),
            ],
            fstab: vec![mount(&here.join("Beta"), "nfs4", "files:/export/beta")],
        };
        let locked = here.join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root opens a folder whatever its mode, so the refusal is looked for only below root
        let refused = std::fs::read_dir(&locked).is_err();
        let doc = listed(&here, tables, WAIT);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(doc["path"], here.display().to_string(), "{doc}");
        assert_eq!(
            doc["parent"],
            here.parent().unwrap().display().to_string(),
            "{doc}"
        );
        for key in ["exists", "directory", "readable"] {
            assert_eq!(doc[key], true, "{key}: {doc}");
        }
        assert_eq!(doc["timed_out"], false, "{doc}");
        assert_eq!(doc["partial"], false, "{doc}");
        assert_eq!(doc["files"], 3, "two files and a link to one: {doc}");
        assert_eq!(doc["mount"]["point"], "/", "{doc}");
        assert_eq!(doc["mount"]["fs"], "ext4", "{doc}");
        assert_eq!(doc["mount"]["network"], false, "{doc}");
        assert!(doc["mount"]["total_bytes"].is_i64(), "{doc}");

        let folders = doc["folders"].as_array().unwrap();
        let names: Vec<&str> = folders
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [".cache", "alpha", "Beta", "gamma", "locked", "scans"]
        );
        let at = |name: &str| folders.iter().find(|f| f["name"] == name).unwrap();
        assert_eq!(at(".cache")["hidden"], true);
        assert_eq!(at("alpha")["hidden"], false);
        assert_eq!(at("gamma")["link"], true);
        assert_eq!(at("alpha")["link"], false);
        assert_eq!(at("alpha")["readable"], true);
        assert_eq!(at("alpha")["mount"], Value::Null);
        assert_eq!(at("alpha")["unmounted"], Value::Null);
        assert_eq!(at("scans")["mount"]["fs"], "nfs4");
        assert_eq!(at("scans")["mount"]["network"], true);
        assert_eq!(at("Beta")["mount"], Value::Null);
        assert_eq!(
            at("Beta")["unmounted"],
            json!({"fs": "nfs4", "source": "files:/export/beta", "network": true})
        );
        if refused {
            assert_eq!(at("locked")["readable"], false);
        }

        let nowhere = listed(&here.join("nowhere"), Tables::default(), WAIT);
        for key in ["exists", "directory", "readable"] {
            assert_eq!(nowhere[key], false, "{key}: {nowhere}");
        }
        assert_eq!(nowhere["folders"], json!([]));
        let file = listed(&here.join("one.txt"), Tables::default(), WAIT);
        assert_eq!(file["exists"], true, "{file}");
        assert_eq!(file["directory"], false, "{file}");
        assert_eq!(file["readable"], false, "{file}");
        assert_eq!(file["folders"], json!([]));
    }

    #[test]
    fn a_browser_starts_at_the_root_the_home_folder_and_every_disk() {
        let tables = Tables {
            mounts: parse_mountinfo(MOUNTINFO),
            fstab: parse_fstab(FSTAB),
        };
        let points = points(&tables, Some(Path::new("/home/someone")));
        let listed: Vec<String> = points
            .iter()
            .map(|s| format!("{} {}", s.path.display(), s.kind.name()))
            .collect();
        assert_eq!(
            listed,
            [
                "/ root",
                "/home/someone home",
                "/archive mount",
                "/data mount",
                "/media/usb disk mount",
                "/srv mount",
                "/media/backup fstab",
                "/srv/share store fstab",
            ]
        );
        let doc = |path: &str| {
            points
                .iter()
                .find(|s| s.path == Path::new(path))
                .unwrap()
                .doc(&tables, false)
        };
        let archive = doc("/archive");
        assert_eq!(
            archive["mount"]["fs"], "nfs4",
            "the disk over the automount: {archive}"
        );
        assert_eq!(archive["mount"]["network"], true, "{archive}");
        assert_eq!(archive["mount"]["free_bytes"], Value::Null, "{archive}");
        assert_eq!(archive["unmounted"], Value::Null, "{archive}");
        let backup = doc("/media/backup");
        assert_eq!(backup["kind"], "fstab", "{backup}");
        assert_eq!(backup["mount"], Value::Null, "{backup}");
        assert_eq!(
            backup["unmounted"],
            json!({"fs": "ext4", "source": "/dev/sdd1", "network": false})
        );
        assert_eq!(doc("/srv/share store")["unmounted"]["network"], true);
        assert_eq!(doc("/")["mount"]["source"], "/dev/sda1");
    }

    #[test]
    fn the_starting_points_answer_with_a_real_home_folder() {
        let home = TempDir::new("folders-home");
        let doc = starts(Tables::default(), Some(home.path().to_path_buf()), WAIT);
        assert_eq!(doc["timed_out"], false, "{doc}");
        assert_eq!(doc["roots"][0]["path"], "/", "{doc}");
        assert_eq!(doc["roots"][1]["kind"], "home", "{doc}");
        let gone = starts(Tables::default(), Some(home.path().join("gone")), WAIT);
        assert_eq!(gone["roots"].as_array().unwrap().len(), 1, "{gone}");
    }

    #[test]
    fn a_path_is_made_plain_without_the_disk() {
        use crate::places::plain;
        assert_eq!(plain("/srv/../data/"), Some(PathBuf::from("/data")));
        assert_eq!(
            plain("/srv/./imaging//"),
            Some(PathBuf::from("/srv/imaging"))
        );
        assert_eq!(plain("/.."), Some(PathBuf::from("/")));
        assert_eq!(plain("/").unwrap().parent(), None);
        assert_eq!(
            plain("/srv").unwrap().parent(),
            Some(Path::new("/")),
            "the parent of a folder at the root is the root"
        );
        assert_eq!(plain("relative"), None);
        assert_eq!(plain(""), None);
    }

    #[test]
    fn the_tables_are_read_with_their_escapes_undone() {
        use crate::places::unescape;
        let mounts = parse_mountinfo(MOUNTINFO);
        assert_eq!(mounts.len(), 11);
        assert_eq!(
            mounts[0],
            mount(Path::new("/"), "ext4", "/dev/sda1"),
            "no optional field before the dash"
        );
        assert_eq!(
            mounts[7],
            mount(Path::new("/archive"), "nfs4", "files:/export/archive"),
            "two optional fields before the dash"
        );
        assert_eq!(mounts[9].point, "/media/usb disk");
        assert!(parse_mountinfo("not a mount line\n").is_empty());
        let named = parse_fstab(FSTAB);
        let points: Vec<&str> = named.iter().map(|m| m.point.as_str()).collect();
        assert_eq!(
            points,
            [
                "/",
                "/data",
                "/archive",
                "/media/backup",
                "/tmp",
                "/srv/share store"
            ],
            "comments and swap left out, a trailing slash dropped"
        );
        assert_eq!(unescape(r"a\134b\011c\012"), "a\\b\tc\n");
        assert_eq!(unescape(r"short\04"), r"short\04");
        assert_eq!(unescape(r"not\8octal"), r"not\8octal");
    }

    #[test]
    fn work_that_does_not_end_in_time_is_answered_without_it() {
        assert_eq!(within(WAIT, || 2), Some(2));
        let slow = within(Duration::from_millis(20), || {
            std::thread::sleep(Duration::from_millis(400));
            1
        });
        assert_eq!(slow, None);
    }
}
