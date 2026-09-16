// SPDX-License-Identifier: AGPL-3.0-only

//! The resume stage, on the digest's model (`nils-digest/src/resume.rs`):
//! one thread between the walker and the workers, with its own connection,
//! asks once per directory what an earlier run recorded there in
//! `pseudonym_file` and decides each file by its size and modification
//! time. A file recorded as written whose source is unchanged is checked
//! for its output and otherwise left alone; a held file is read again only
//! once a map released it or a person asked for it to be coded anyway; a
//! refused file stays refused until it changes; a changed file is read
//! again with its record beside it, so its old output can be let go.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, Sender};
use lru::LruCache;
use nils_digest::walk::WalkEvent;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Cell, Error, Param, Store};

pub use nils_digest::resume::{DIR_CACHE, relative};

use crate::progress::Progress;
use crate::run::Item;

/// The states of `pseudonym_file.state` (record 26 §3).
pub mod state {
    pub const WRITTEN: &str = "written";
    pub const UNCHANGED: &str = "unchanged";
    pub const HELD: &str = "held";
    pub const REFUSED: &str = "refused";

    pub fn of(text: &str) -> Option<&'static str> {
        [WRITTEN, UNCHANGED, HELD, REFUSED]
            .into_iter()
            .find(|s| *s == text)
    }
}

/// One `pseudonym_file` row, as the resume check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub id: i64,
    pub size: i64,
    pub mtime: i64,
    pub state: &'static str,
    pub out_path: Option<String>,
    pub out_size: Option<i64>,
    pub shape: Option<String>,
    /// A map released the held file.
    pub released: bool,
    /// A person asked for the held file to be coded anyway.
    pub code_anyway: bool,
    /// The keyed lookup a held row carries: the rule's, or the one a map
    /// re-keyed it to when it named the value under another type.
    pub lookup: Option<Vec<u8>>,
}

/// What an earlier run recorded for a path that is read again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prior {
    pub id: i64,
    /// The output the earlier run wrote, to let go of when the new one
    /// lands elsewhere.
    pub out_path: Option<String>,
    /// The size or the modification time differ from the record.
    pub changed: bool,
    pub code_anyway: bool,
    /// The lookup the held row was released under, when a map re-keyed it
    /// to the type it named the value as: the identity is looked for under
    /// it first, and under the rule's own lookup otherwise.
    pub lookup: Option<Vec<u8>>,
}

/// What to do with a file, given its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Read it, resolve it, write it.
    Read(Option<Prior>),
    /// The source is as recorded: the output is looked for, and the file
    /// is unchanged when it is there at its size, read again otherwise.
    Check {
        id: i64,
        out_path: String,
        out_size: i64,
    },
    /// Held and not released: it stays held, its row touched.
    StillHeld { id: i64, shape: Option<String> },
    /// Refused and unchanged: it stays refused, its row touched.
    StillRefused { id: i64 },
}

/// The decision for one file.
pub fn decide(recorded: Option<&Recorded>, size: u64, mtime: i64) -> Decision {
    let Some(r) = recorded else {
        return Decision::Read(None);
    };
    let same = r.size == size as i64 && r.mtime == mtime;
    if !same {
        return Decision::Read(Some(Prior {
            id: r.id,
            out_path: r.out_path.clone(),
            changed: true,
            code_anyway: r.code_anyway,
            lookup: None,
        }));
    }
    match (r.state, &r.out_path, r.out_size) {
        (state::WRITTEN | state::UNCHANGED, Some(out_path), Some(out_size)) => Decision::Check {
            id: r.id,
            out_path: out_path.clone(),
            out_size,
        },
        (state::HELD, _, _) if r.released || r.code_anyway => Decision::Read(Some(Prior {
            id: r.id,
            out_path: None,
            changed: false,
            code_anyway: r.code_anyway,
            lookup: if r.released { r.lookup.clone() } else { None },
        })),
        (state::HELD, _, _) => Decision::StillHeld {
            id: r.id,
            shape: r.shape.clone(),
        },
        (state::REFUSED, _, _) => Decision::StillRefused { id: r.id },
        // a written row with no output on it: read again
        _ => Decision::Read(Some(Prior {
            id: r.id,
            out_path: r.out_path.clone(),
            changed: false,
            code_anyway: r.code_anyway,
            lookup: None,
        })),
    }
}

/// The records of a dataset, read one directory at a time.
pub struct Records {
    store: Store,
    place_id: i64,
    dirs: LruCache<String, HashMap<String, Recorded>>,
    /// The dataset has no rows at all: nothing to ask.
    empty: bool,
    sql: String,
}

impl Records {
    pub fn new(mut store: Store, place_id: i64) -> Result<Records, Error> {
        let t = table("pseudonym_file");
        let qualified = store.qualified("pseudonym_file");
        let d = store.dialect();
        let probe = format!(
            "SELECT 1 FROM {qualified} WHERE place_id = {} LIMIT 1",
            d.param(1, Type::Int)
        );
        let empty = store.query_opt(&probe, &[Param::Int(place_id)])?.is_none();
        let released = d.text_of(t.column("released_at").expect("released_at"));
        let sql = format!(
            "SELECT path, size, mtime, state, out_path, out_size, shape, {released} IS NOT NULL, code_anyway, id, lookup \
             FROM {qualified} WHERE place_id = {} AND dir = {}",
            d.param(1, Type::Int),
            d.param(2, Type::Text)
        );
        Ok(Records {
            store,
            place_id,
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
                .query(&self.sql, &[Param::Int(self.place_id), Param::from(dir)])?;
            let mut map = HashMap::with_capacity(rows.len());
            for r in &rows {
                let path = r.text(0)?;
                let Some(state) = state::of(r.text(3)?) else {
                    continue;
                };
                let flag = |i: usize| match r.get(i) {
                    Cell::Bool(b) => *b,
                    Cell::Int(n) => *n != 0,
                    _ => false,
                };
                map.insert(
                    path.to_string(),
                    Recorded {
                        id: r.int(9)?,
                        size: r.int(1)?,
                        mtime: r.int(2)?,
                        state,
                        out_path: r.opt_text(4)?.map(str::to_string),
                        out_size: r.opt_int(5)?,
                        shape: r.opt_text(6)?.map(str::to_string),
                        released: flag(7),
                        code_anyway: flag(8),
                        lookup: r.opt_bytes(10)?.map(<[u8]>::to_vec),
                    },
                );
            }
            self.dirs.put(dir.to_string(), map);
        }
        Ok(self.dirs.get(dir).and_then(|m| m.get(path)).cloned())
    }
}

/// What the resume stage hands a worker.
pub enum Task {
    /// Read, resolve and write the file at `path`.
    Read {
        path: PathBuf,
        rel: String,
        size: u64,
        mtime: i64,
        prior: Option<Prior>,
    },
    /// Look for the output of an unchanged source.
    Check {
        path: PathBuf,
        rel: String,
        size: u64,
        mtime: i64,
        id: i64,
        out_path: String,
        out_size: i64,
    },
}

/// Run the stage: every walk event in, a task out to the workers, or an
/// item straight to the recorder for a file no worker needs to see, until
/// the walker is done or a stop is asked.
pub fn run(
    root: &Path,
    mut records: Option<Records>,
    rx: &Receiver<WalkEvent>,
    tasks: &Sender<Task>,
    items: &Sender<Item>,
    progress: &Progress,
    cancel: &nils_digest::Cancel,
) -> Result<(), Error> {
    for event in rx {
        if cancel.stop() {
            break;
        }
        match event {
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
                let task = match decide(recorded.as_ref(), size, mtime_ns) {
                    Decision::Read(prior) => Task::Read {
                        path,
                        rel,
                        size,
                        mtime: mtime_ns,
                        prior,
                    },
                    Decision::Check {
                        id,
                        out_path,
                        out_size,
                    } => Task::Check {
                        path,
                        rel,
                        size,
                        mtime: mtime_ns,
                        id,
                        out_path,
                        out_size,
                    },
                    Decision::StillHeld { id, shape } => {
                        progress.file(&progress.held, size);
                        if items.send(Item::StillHeld { id, shape }).is_err() {
                            break;
                        }
                        continue;
                    }
                    Decision::StillRefused { id } => {
                        progress.file(&progress.refused, size);
                        if items.send(Item::StillRefused { id }).is_err() {
                            break;
                        }
                        continue;
                    }
                };
                if tasks.send(task).is_err() {
                    break;
                }
            }
            // a link or a special file is left out, as the digest leaves
            // it out; nothing under the originals is filtered by name
            WalkEvent::Skipped { .. } => {
                if items.send(Item::Skipped).is_err() {
                    break;
                }
            }
            WalkEvent::WalkError { error, .. } => {
                if items.send(Item::WalkError { error }).is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(state: &'static str) -> Recorded {
        Recorded {
            id: 1,
            size: 10,
            mtime: 5,
            state,
            out_path: Some("c/d/001/00001.dcm".into()),
            out_size: Some(9),
            shape: Some("999".into()),
            released: false,
            code_anyway: false,
            lookup: None,
        }
    }

    #[test]
    fn decisions_follow_the_record() {
        assert_eq!(decide(None, 10, 5), Decision::Read(None));
        let written = rec(state::WRITTEN);
        assert_eq!(
            decide(Some(&written), 10, 5),
            Decision::Check {
                id: 1,
                out_path: "c/d/001/00001.dcm".into(),
                out_size: 9
            }
        );
        assert_eq!(
            decide(Some(&written), 11, 5),
            Decision::Read(Some(Prior {
                id: 1,
                out_path: Some("c/d/001/00001.dcm".into()),
                changed: true,
                code_anyway: false,
                lookup: None
            }))
        );
        assert!(matches!(
            decide(Some(&rec(state::UNCHANGED)), 10, 5),
            Decision::Check { .. }
        ));
        let held = Recorded {
            out_path: None,
            out_size: None,
            ..rec(state::HELD)
        };
        assert_eq!(
            decide(Some(&held), 10, 5),
            Decision::StillHeld {
                id: 1,
                shape: Some("999".into())
            }
        );
        let released = Recorded {
            released: true,
            lookup: Some(b"re-keyed".to_vec()),
            ..held.clone()
        };
        assert_eq!(
            decide(Some(&released), 10, 5),
            Decision::Read(Some(Prior {
                id: 1,
                out_path: None,
                changed: false,
                code_anyway: false,
                lookup: Some(b"re-keyed".to_vec())
            }))
        );
        let anyway = Recorded {
            code_anyway: true,
            ..held.clone()
        };
        assert_eq!(
            decide(Some(&anyway), 10, 5),
            Decision::Read(Some(Prior {
                id: 1,
                out_path: None,
                changed: false,
                code_anyway: true,
                lookup: None
            }))
        );
        let refused = Recorded {
            out_path: None,
            out_size: None,
            ..rec(state::REFUSED)
        };
        assert_eq!(
            decide(Some(&refused), 10, 5),
            Decision::StillRefused { id: 1 }
        );
        assert!(matches!(
            decide(Some(&refused), 10, 6),
            Decision::Read(Some(_))
        ));
        // a written row that lost its output is read again
        let lost = Recorded {
            out_path: None,
            ..rec(state::WRITTEN)
        };
        assert!(matches!(
            decide(Some(&lost), 10, 5),
            Decision::Read(Some(_))
        ));
    }

    #[test]
    fn records_come_one_directory_at_a_time() {
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        store
            .execute(
                "INSERT INTO pseudonym_file (id, place_id, path, dir, size, mtime, state, out_path, out_size, shape, first_seen, released_at, code_anyway) VALUES \
                 (1, 3, 'a/x', 'a', 10, 5, 'written', 'c/1.dcm', 9, NULL, 't', NULL, 0), \
                 (2, 3, 'a/y', 'a', 1, 1, 'held', NULL, NULL, '999', 't', '2026-09-16T00:00:00Z', 0), \
                 (3, 3, 'a/b/z', 'a/b', 1, 1, 'held', NULL, NULL, '999', 't', NULL, 1), \
                 (4, 4, 'a/w', 'a', 1, 1, 'refused', NULL, NULL, NULL, 't', NULL, 0), \
                 (5, 3, 'q', '', 2, 2, 'refused', NULL, NULL, NULL, 't', NULL, 0)",
                &[],
            )
            .unwrap();
        let mut records = Records::new(store, 3).unwrap();
        assert!(!records.empty);
        let x = records.get("a", "a/x").unwrap().unwrap();
        assert_eq!((x.id, x.state, x.out_size), (1, "written", Some(9)));
        let y = records.get("a", "a/y").unwrap().unwrap();
        assert!(y.released && !y.code_anyway && y.shape.as_deref() == Some("999"));
        // one level deep: a/b/z is not in a
        assert_eq!(records.get("a", "a/b/z").unwrap(), None);
        let z = records.get("a/b", "a/b/z").unwrap().unwrap();
        assert!(z.code_anyway && !z.released);
        assert_eq!(records.get("a", "a/w").unwrap(), None, "another dataset's");
        let q = records.get("", "q").unwrap().unwrap();
        assert_eq!(q.state, "refused");
        assert_eq!(records.dirs.len(), 3);
        let mut none = Records::new(Store::sqlite_in_memory().unwrap(), 3);
        assert!(none.is_err() || none.as_mut().unwrap().empty);
    }
}
