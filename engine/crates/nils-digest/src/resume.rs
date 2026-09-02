// SPDX-License-Identifier: AGPL-3.0-only

//! The resume stage (§5.2): one thread between the walker and the parsers,
//! with its own connection, asks once per directory what an earlier run
//! recorded there and lets through only what needs reading again. A file
//! whose size and modification time match its record is `unchanged` (a
//! quarantined one stays skipped unless the run retries quarantine); a file
//! that differs is read again with its record's instance beside it; a file
//! with no record is new. With `--restart` every file is read again, its
//! record's instance still beside it, so a file keeps its status.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;

use crossbeam_channel::{Receiver, Sender};
use lru::LruCache;
use nils_registry::schema::Type;
use nils_registry::store::{Cell, Error, Param, Store};

use crate::batch::{Prior, Task};
use crate::report::Counts;
use crate::walk::{SkipReason, WalkEvent};

/// How many directories' records stay in memory.
pub const DIR_CACHE: usize = 512;

/// The statuses of `source_file.status` (§4.2).
pub mod status {
    pub const INGESTED: &str = "ingested";
    pub const DUPLICATE: &str = "duplicate";
    pub const QUARANTINED: &str = "quarantined";
    pub const SKIPPED: &str = "skipped";
    pub const GONE: &str = "gone";

    /// The static name of a status read back, if it is one.
    pub fn of(text: &str) -> Option<&'static str> {
        [INGESTED, DUPLICATE, QUARANTINED, SKIPPED, GONE]
            .into_iter()
            .find(|s| *s == text)
    }
}

/// One `source_file` row, as the resume check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub id: i64,
    pub size: i64,
    pub mtime_ns: i64,
    pub status: &'static str,
    pub instance_id: Option<i64>,
    /// The instance's `source_file_id` points back at this row: the file is
    /// the instance's own, not a duplicate of it.
    pub own: bool,
}

/// What to do with a file, given its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Parse(Option<Prior>),
    /// The row to touch, and whether it is a quarantined file kept as it is.
    Unchanged {
        id: i64,
        quarantined: bool,
    },
}

/// The decision for one file (§5.2).
pub fn decide(
    recorded: Option<&Recorded>,
    size: u64,
    mtime_ns: i64,
    retry_quarantine: bool,
    restart: bool,
) -> Decision {
    let Some(r) = recorded else {
        return Decision::Parse(None);
    };
    let same = r.size == size as i64 && r.mtime_ns == mtime_ns;
    // only the instance's own file carries its instance forward; a duplicate
    // is filed afresh against whatever it holds now
    let own = if r.own { r.instance_id } else { None };
    if restart {
        return Decision::Parse(match (own, same) {
            (None, true) => None,
            (instance_id, _) => Some(Prior {
                instance_id,
                changed: !same,
            }),
        });
    }
    match r.status {
        status::INGESTED | status::DUPLICATE if same => Decision::Unchanged {
            id: r.id,
            quarantined: false,
        },
        status::QUARANTINED if same && !retry_quarantine => Decision::Unchanged {
            id: r.id,
            quarantined: true,
        },
        status::QUARANTINED if same => Decision::Parse(None),
        // a file that was gone and is back as it was: its instance's own file
        // again, not a duplicate of it
        status::GONE if same => Decision::Parse(own.map(|id| Prior {
            instance_id: Some(id),
            changed: false,
        })),
        _ if same => Decision::Parse(None),
        _ => Decision::Parse(Some(Prior {
            instance_id: own,
            changed: true,
        })),
    }
}

/// The records of a source, read one directory at a time.
pub struct Records {
    store: Store,
    source_id: i64,
    dirs: LruCache<String, HashMap<String, Recorded>>,
    /// The source has no rows at all: nothing to ask.
    empty: bool,
    sql: String,
}

impl Records {
    pub fn new(mut store: Store, source_id: i64) -> Result<Records, Error> {
        let table = store.qualified("source_file");
        let d = store.dialect();
        let probe = format!(
            "SELECT 1 FROM {table} WHERE source_id = {} LIMIT 1",
            d.param(1, Type::Int)
        );
        let empty = store.query_opt(&probe, &[Param::Int(source_id)])?.is_none();
        let instance = store.qualified("instance");
        let sql = format!(
            "SELECT f.path, f.size, f.mtime_ns, f.status, f.instance_id, i.source_file_id = f.id, f.id \
             FROM {table} AS f LEFT JOIN {instance} AS i ON i.id = f.instance_id \
             WHERE f.source_id = {} AND f.dir = {}",
            d.param(1, Type::Int),
            d.param(2, Type::Text)
        );
        Ok(Records {
            store,
            source_id,
            dirs: LruCache::new(NonZeroUsize::new(DIR_CACHE).unwrap_or(NonZeroUsize::MIN)),
            empty,
            sql,
        })
    }

    /// The record of `path` (relative, in `dir`), if an earlier run left one.
    pub fn get(&mut self, dir: &str, path: &str) -> Result<Option<Recorded>, Error> {
        if self.empty {
            return Ok(None);
        }
        if !self.dirs.contains(dir) {
            let rows = self
                .store
                .query(&self.sql, &[Param::Int(self.source_id), Param::from(dir)])?;
            let mut map = HashMap::with_capacity(rows.len());
            for r in &rows {
                let Some(status) = status::of(r.text(3)?) else {
                    continue;
                };
                let own = match r.get(5) {
                    Cell::Bool(b) => *b,
                    Cell::Int(i) => *i != 0,
                    _ => false,
                };
                map.insert(
                    r.text(0)?.to_string(),
                    Recorded {
                        id: r.int(6)?,
                        size: r.int(1)?,
                        mtime_ns: r.int(2)?,
                        status,
                        instance_id: r.opt_int(4)?,
                        own,
                    },
                );
            }
            self.dirs.put(dir.to_string(), map);
        }
        Ok(self.dirs.get(dir).and_then(|m| m.get(path)).cloned())
    }
}

/// A path under the root as `source_file` stores it: relative, forward
/// slashes; and its directory part.
pub fn relative(root: &Path, path: &Path) -> (String, String) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let dir = parts[..parts.len().saturating_sub(1)].join("/");
    (parts.join("/"), dir)
}

/// Run the stage: every walk event in, a task out, until the walker is done.
/// With no records (a dry run) every file is parsed. Returns what the stage
/// counted itself: the filtered files.
pub fn run(
    root: &Path,
    rx: &Receiver<WalkEvent>,
    tx: &Sender<Task>,
    mut records: Option<Records>,
    retry_quarantine: bool,
    restart: bool,
) -> Result<Counts, Error> {
    let mut counts = Counts::default();
    for event in rx {
        let task = match event {
            WalkEvent::File {
                path,
                size,
                mtime_ns,
            } => {
                let (rel, dir) = relative(root, &path);
                let recorded = match records.as_mut() {
                    Some(r) => r.get(&dir, &rel)?,
                    None => None,
                };
                match decide(recorded.as_ref(), size, mtime_ns, retry_quarantine, restart) {
                    Decision::Parse(prior) => Task::Parse {
                        path,
                        rel,
                        dir,
                        size,
                        mtime_ns,
                        prior,
                    },
                    Decision::Unchanged { id, quarantined } => Task::Unchanged { id, quarantined },
                }
            }
            WalkEvent::Skipped {
                reason: SkipReason::Filtered,
                ..
            } => {
                counts.skipped(SkipReason::Filtered);
                continue;
            }
            WalkEvent::Skipped {
                path,
                reason,
                size,
                mtime_ns,
            } => {
                let (rel, dir) = relative(root, &path);
                Task::Skipped {
                    rel,
                    dir,
                    size,
                    mtime_ns,
                    reason,
                }
            }
            WalkEvent::WalkError { error, .. } => Task::WalkError { error },
        };
        if tx.send(task).is_err() {
            break;
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(status: &'static str, instance: Option<i64>) -> Recorded {
        Recorded {
            id: 1,
            size: 10,
            mtime_ns: 5,
            status,
            instance_id: instance,
            own: instance.is_some(),
        }
    }

    #[test]
    fn decisions_follow_the_spec() {
        assert_eq!(decide(None, 10, 5, false, false), Decision::Parse(None));
        let ingested = rec(status::INGESTED, Some(7));
        assert_eq!(
            decide(Some(&ingested), 10, 5, false, false),
            Decision::Unchanged {
                id: 1,
                quarantined: false
            }
        );
        assert_eq!(
            decide(Some(&ingested), 11, 5, false, false),
            Decision::Parse(Some(Prior {
                instance_id: Some(7),
                changed: true
            }))
        );
        let quarantined = rec(status::QUARANTINED, None);
        assert_eq!(
            decide(Some(&quarantined), 10, 5, false, false),
            Decision::Unchanged {
                id: 1,
                quarantined: true
            }
        );
        assert_eq!(
            decide(Some(&quarantined), 10, 5, true, false),
            Decision::Parse(None)
        );
        assert_eq!(
            decide(Some(&quarantined), 10, 6, false, false),
            Decision::Parse(Some(Prior {
                instance_id: None,
                changed: true
            }))
        );
        let gone = rec(status::GONE, Some(3));
        assert_eq!(
            decide(Some(&gone), 10, 5, false, false),
            Decision::Parse(Some(Prior {
                instance_id: Some(3),
                changed: false
            }))
        );
        let skipped = rec(status::SKIPPED, None);
        assert_eq!(
            decide(Some(&skipped), 10, 5, false, false),
            Decision::Parse(None)
        );
        // a duplicate carries no instance forward, changed or back from gone
        let duplicate = Recorded {
            own: false,
            ..rec(status::DUPLICATE, Some(7))
        };
        assert_eq!(
            decide(Some(&duplicate), 11, 5, false, false),
            Decision::Parse(Some(Prior {
                instance_id: None,
                changed: true
            }))
        );
        let gone_duplicate = Recorded {
            own: false,
            ..rec(status::GONE, Some(7))
        };
        assert_eq!(
            decide(Some(&gone_duplicate), 10, 5, false, false),
            Decision::Parse(None)
        );

        // a restart reads everything again, the record's instance beside it
        assert_eq!(
            decide(Some(&ingested), 10, 5, false, true),
            Decision::Parse(Some(Prior {
                instance_id: Some(7),
                changed: false
            }))
        );
        assert_eq!(
            decide(Some(&ingested), 10, 6, false, true),
            Decision::Parse(Some(Prior {
                instance_id: Some(7),
                changed: true
            }))
        );
        assert_eq!(
            decide(Some(&quarantined), 10, 5, false, true),
            Decision::Parse(None)
        );
        assert_eq!(
            decide(Some(&quarantined), 11, 5, false, true),
            Decision::Parse(Some(Prior {
                instance_id: None,
                changed: true
            }))
        );
    }

    #[test]
    fn paths_are_relative_with_forward_slashes() {
        let root = Path::new("/data/root");
        assert_eq!(
            relative(root, Path::new("/data/root/a/b/c.dcm")),
            ("a/b/c.dcm".into(), "a/b".into())
        );
        assert_eq!(
            relative(root, Path::new("/data/root/c.dcm")),
            ("c.dcm".into(), String::new())
        );
    }

    #[test]
    fn records_come_one_directory_at_a_time() {
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        store
            .execute(
                "INSERT INTO source_file (id, source_id, batch_id, dir, path, size, mtime_ns, status, instance_id, seen_at) VALUES (1, 1, 1, 'a', 'a/x', 10, 5, 'ingested', 42, 't'), (2, 1, 1, 'a', 'a/y', 1, 1, 'quarantined', NULL, 't'), (3, 2, 1, 'a', 'a/z', 1, 1, 'ingested', 9, 't'), (4, 1, 1, 'a', 'a/w', 10, 5, 'duplicate', 42, 't')",
                &[],
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO instance (id, sop_instance_uid, series_id, source_file_id, first_batch_id) VALUES (42, '1.2.3', 1, 1, 1)",
                &[],
            )
            .unwrap();
        let mut records = Records::new(store, 1).unwrap();
        assert!(!records.empty);
        assert_eq!(
            records.get("a", "a/x").unwrap(),
            Some(Recorded {
                id: 1,
                size: 10,
                mtime_ns: 5,
                status: "ingested",
                instance_id: Some(42),
                own: true,
            })
        );
        assert_eq!(
            records.get("a", "a/y").unwrap().map(|r| r.status),
            Some("quarantined")
        );
        assert_eq!(
            records
                .get("a", "a/w")
                .unwrap()
                .map(|r| (r.id, r.status, r.own)),
            Some((4, "duplicate", false))
        );
        assert_eq!(records.get("a", "a/z").unwrap(), None);
        assert_eq!(records.get("b", "b/q").unwrap(), None);
        assert_eq!(records.dirs.len(), 2);

        let mut none = Records::new(Store::sqlite_in_memory().unwrap(), 1);
        assert!(none.is_err() || none.as_mut().unwrap().empty);
    }
}
