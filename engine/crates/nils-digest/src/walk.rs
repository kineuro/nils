// SPDX-License-Identifier: AGPL-3.0-only

//! The walker (`docs/specs/wave1-parse-and-digest.md`, §5.1 and §5.2): a pool
//! of threads over a queue of directories, one listing per task, breadth on the
//! pool and depth inside a directory. Every regular file whose name passes the
//! filter goes down the channel in directory order; symbolic links and special
//! files are reported as skipped; a directory that cannot be listed is a
//! `walk_error` and the walk goes on; a root that cannot be listed is the
//! caller's error.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use crossbeam_channel::Sender;

use crate::cancel::Cancel;

/// The `files` knob: which names are candidates.
#[derive(Debug, Clone)]
pub enum Filter {
    /// Every regular file.
    All,
    /// Names ending in `.dcm`, any case.
    Dcm,
    /// Names with no extension.
    NoExt,
    /// Names matching a glob.
    Glob(glob::Pattern),
    /// Names matching any of several filters (the knob's comma-separated
    /// form, `dcm,no-ext`): the union an earlier tool's "all" mode meant.
    Any(Vec<Filter>),
}

impl Filter {
    /// Parse the knob's text: `all`, `dcm`, `no-ext`, or a glob; a
    /// comma-separated list of those is their union (a glob cannot contain a
    /// comma in this form).
    pub fn parse(text: &str) -> Result<Filter, glob::PatternError> {
        if text.contains(',') {
            let parts = text
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(Filter::parse)
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(match parts.len() {
                1 => parts.into_iter().next().expect("one part"),
                _ => Filter::Any(parts),
            });
        }
        Ok(match text {
            "all" => Filter::All,
            "dcm" => Filter::Dcm,
            "no-ext" => Filter::NoExt,
            other => Filter::Glob(glob::Pattern::new(other)?),
        })
    }

    /// Whether a file of this name is a candidate.
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Filter::All => true,
            Filter::Dcm => Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dcm")),
            Filter::NoExt => Path::new(name).extension().is_none(),
            Filter::Glob(p) => p.matches(name),
            Filter::Any(filters) => filters.iter().any(|filter| filter.matches(name)),
        }
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Filter::All => f.write_str("all"),
            Filter::Dcm => f.write_str("dcm"),
            Filter::NoExt => f.write_str("no-ext"),
            Filter::Glob(p) => f.write_str(p.as_str()),
            Filter::Any(filters) => {
                for (i, filter) in filters.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{filter}")?;
                }
                Ok(())
            }
        }
    }
}

/// Why a directory entry was not handed to the parsers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// A symbolic link; never followed.
    Symlink,
    /// Neither a regular file nor a directory (a socket, a pipe, a device).
    Special,
    /// A regular file whose name the filter did not select.
    Filtered,
}

impl SkipReason {
    pub fn name(self) -> &'static str {
        match self {
            SkipReason::Symlink => "symlink",
            SkipReason::Special => "special",
            SkipReason::Filtered => "filtered",
        }
    }
}

/// What the walker sends. The size and the modification time come from the
/// walker's own `stat` (never following a link), so the resume check (§5.2)
/// and the `source_file` row cost no second call.
#[derive(Debug)]
pub enum WalkEvent {
    /// A candidate file.
    File {
        path: PathBuf,
        size: u64,
        mtime_ns: i64,
    },
    /// An entry not handed on.
    Skipped {
        path: PathBuf,
        reason: SkipReason,
        size: u64,
        mtime_ns: i64,
    },
    /// A directory that could not be listed, or an entry that could not be read.
    WalkError { path: PathBuf, error: String },
}

/// A modification time as nanoseconds since the Unix epoch, negative before
/// it.
pub fn mtime_ns_of(metadata: &std::fs::Metadata) -> i64 {
    use std::time::UNIX_EPOCH;
    let Ok(modified) = metadata.modified() else {
        return 0;
    };
    match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
    }
}

struct Queue {
    dirs: Mutex<(VecDeque<PathBuf>, usize)>,
    ready: Condvar,
}

impl Queue {
    fn push(&self, dir: PathBuf) {
        let mut q = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        q.0.push_back(dir);
        q.1 += 1;
        self.ready.notify_one();
    }

    /// The next directory, or none when every directory is done.
    fn pop(&self) -> Option<PathBuf> {
        let mut q = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(dir) = q.0.pop_front() {
                return Some(dir);
            }
            if q.1 == 0 {
                return None;
            }
            q = self.ready.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn done(&self) {
        let mut q = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        q.1 -= 1;
        if q.1 == 0 {
            self.ready.notify_all();
        }
    }
}

/// Walk `root` with `threads` listing threads, sending events to `tx`, and
/// return when the tree is done, or once a stop is asked: the directories
/// still queued are then let go unlisted. The root must be listable.
pub fn walk(
    root: &Path,
    threads: usize,
    filter: &Filter,
    tx: &Sender<WalkEvent>,
    cancel: &Cancel,
) -> io::Result<()> {
    // The root's failure is the job's, so it is tried here, before the pool.
    std::fs::read_dir(root)?;
    let queue = Queue {
        dirs: Mutex::new((VecDeque::new(), 0)),
        ready: Condvar::new(),
    };
    queue.push(root.to_path_buf());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                while let Some(dir) = queue.pop() {
                    if !cancel.stop() {
                        list(&dir, filter, &queue, tx);
                    }
                    queue.done();
                }
            });
        }
    });
    Ok(())
}

fn list(dir: &Path, filter: &Filter, queue: &Queue, tx: &Sender<WalkEvent>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            let _ = tx.send(WalkEvent::WalkError {
                path: dir.to_path_buf(),
                error: e.to_string(),
            });
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                let _ = tx.send(WalkEvent::WalkError {
                    path: dir.to_path_buf(),
                    error: e.to_string(),
                });
                continue;
            }
        };
        let path = entry.path();
        let kind = match entry.file_type() {
            Ok(k) => k,
            Err(e) => {
                let _ = tx.send(WalkEvent::WalkError {
                    path,
                    error: e.to_string(),
                });
                continue;
            }
        };
        if kind.is_dir() {
            queue.push(path);
            continue;
        }
        let reason = if kind.is_symlink() {
            Some(SkipReason::Symlink)
        } else if !kind.is_file() {
            Some(SkipReason::Special)
        } else if filter.matches(&entry.file_name().to_string_lossy()) {
            None
        } else {
            Some(SkipReason::Filtered)
        };
        // the size and the mtime of the entry itself: a link's own, not its
        // target's
        let (size, mtime_ns) = match entry.metadata() {
            Ok(m) => (m.len(), mtime_ns_of(&m)),
            Err(e) if reason.is_none() => {
                let _ = tx.send(WalkEvent::WalkError {
                    path,
                    error: e.to_string(),
                });
                continue;
            }
            Err(_) => (0, 0),
        };
        let event = match reason {
            None => WalkEvent::File {
                path,
                size,
                mtime_ns,
            },
            Some(reason) => WalkEvent::Skipped {
                path,
                reason,
                size,
                mtime_ns,
            },
        };
        if tx.send(event).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::synth::TempDir;
    use std::collections::BTreeSet;

    fn run(root: &Path, filter: &Filter) -> Vec<WalkEvent> {
        let (tx, rx) = crossbeam_channel::bounded(64);
        let root = root.to_path_buf();
        let filter = filter.clone();
        let walker = std::thread::spawn(move || walk(&root, 3, &filter, &tx, &Cancel::new()));
        let events: Vec<WalkEvent> = rx.iter().collect();
        walker.join().unwrap().unwrap();
        events
    }

    fn files(events: &[WalkEvent]) -> BTreeSet<String> {
        events
            .iter()
            .filter_map(|e| match e {
                WalkEvent::File { path, .. } => {
                    Some(path.file_name().unwrap().to_string_lossy().into_owned())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn filters_select_by_name() {
        let all = Filter::parse("all").unwrap();
        let dcm = Filter::parse("dcm").unwrap();
        let no_ext = Filter::parse("no-ext").unwrap();
        let glob = Filter::parse("IM_*").unwrap();
        assert!(all.matches("anything.txt"));
        assert!(dcm.matches("a.DCM") && dcm.matches("b.dcm") && !dcm.matches("c.dcm.bak"));
        assert!(
            no_ext.matches("IM_0001") && no_ext.matches(".DS_Store") && !no_ext.matches("a.dcm")
        );
        assert!(glob.matches("IM_0001") && !glob.matches("XX_0001"));
        assert!(Filter::parse("[").is_err());
        assert_eq!(all.to_string(), "all");
        assert_eq!(glob.to_string(), "IM_*");
    }

    #[test]
    fn a_comma_separated_list_is_a_union() {
        let union = Filter::parse("dcm, no-ext").unwrap();
        assert!(union.matches("a.DCM") && union.matches("IM_0001"));
        assert!(!union.matches("a.dcm.bak") && !union.matches("notes.txt"));
        assert_eq!(union.to_string(), "dcm,no-ext");
        let with_glob = Filter::parse("dcm,IM_*").unwrap();
        assert!(with_glob.matches("IM_0001.txt") && !with_glob.matches("XX_0001"));
        assert!(matches!(Filter::parse("dcm,").unwrap(), Filter::Dcm));
        assert!(Filter::parse("dcm,[").is_err());
    }

    #[test]
    fn walks_the_tree_in_directory_order_and_skips_links() {
        let dir = TempDir::new("walk");
        dir.file("a/one.dcm", b"x");
        dir.file("a/two", b"x");
        dir.file("a/b/three.DCM", b"x");
        dir.file("c/.hidden", b"x");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("a/one.dcm"), dir.path().join("link")).unwrap();

        let events = run(dir.path(), &Filter::All);
        let names = files(&events);
        assert_eq!(
            names,
            ["one.dcm", "two", "three.DCM", ".hidden"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        assert!(events.iter().all(|e| match e {
            WalkEvent::File { size, mtime_ns, .. } => *size == 1 && *mtime_ns > 0,
            _ => true,
        }));
        #[cfg(unix)]
        assert!(events.iter().any(|e| matches!(
            e,
            WalkEvent::Skipped {
                reason: SkipReason::Symlink,
                ..
            }
        )));

        let events = run(dir.path(), &Filter::Dcm);
        assert_eq!(
            files(&events),
            ["one.dcm", "three.DCM"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        let filtered = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    WalkEvent::Skipped {
                        reason: SkipReason::Filtered,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(filtered, 2);
    }

    #[test]
    fn an_unlistable_root_is_an_error() {
        let dir = TempDir::new("walk-missing");
        let (tx, _rx) = crossbeam_channel::bounded(1);
        assert!(
            walk(
                &dir.path().join("nope"),
                2,
                &Filter::All,
                &tx,
                &Cancel::new()
            )
            .is_err()
        );
    }

    #[test]
    fn a_stop_lets_the_queued_directories_go() {
        let dir = TempDir::new("walk-stop");
        for d in 0..20 {
            for f in 0..5 {
                dir.file(&format!("d{d:02}/f{f}"), b"x");
            }
        }
        let cancel = Cancel::new();
        cancel.request();
        let (tx, rx) = crossbeam_channel::bounded(1_000);
        walk(dir.path(), 2, &Filter::All, &tx, &cancel).unwrap();
        drop(tx);
        // the threads check before every listing, the root's included
        assert_eq!(rx.iter().count(), 0);
        let (tx, rx) = crossbeam_channel::bounded(1_000);
        walk(dir.path(), 2, &Filter::All, &tx, &Cancel::new()).unwrap();
        drop(tx);
        assert_eq!(rx.iter().count(), 100);
    }

    #[cfg(unix)]
    #[test]
    fn an_unlistable_directory_is_a_walk_error() {
        use std::os::unix::fs::PermissionsExt;
        // root can list anything, so the check has nothing to show there
        if unsafe_is_root() {
            return;
        }
        let dir = TempDir::new("walk-perm");
        dir.file("ok/a", b"x");
        dir.file("closed/b", b"x");
        let closed = dir.path().join("closed");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let events = run(dir.path(), &Filter::All);
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            files(&events),
            ["a"].into_iter().map(String::from).collect()
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WalkEvent::WalkError { path, .. } if path == &closed))
        );
    }

    #[cfg(unix)]
    fn unsafe_is_root() -> bool {
        std::fs::metadata("/proc/self")
            .map(|_| std::env::var("USER").as_deref() == Ok("root"))
            .unwrap_or(false)
            || std::fs::read_to_string("/proc/self/status")
                .map(|s| {
                    s.lines()
                        .any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(1) == Some("0"))
                })
                .unwrap_or(false)
    }
}
