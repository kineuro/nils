// SPDX-License-Identifier: AGPL-3.0-only

//! Named locks of one registry, held by a process across several of its
//! own statements and transactions: what pipeline runs that go on side by
//! side take around the few steps that read and then write what every run
//! shares (a review group a newer run supersedes, a model registered by its
//! name, the lane's budget), so those steps run one run at a time while
//! the rest of each run goes on.
//!
//! A lock is the backend's own, and it goes with the process that held it,
//! however that process ends, so no lock is ever left behind by a run that
//! was killed:
//!
//! - Postgres: a session-level advisory lock (`pg_try_advisory_lock`) on a
//!   key made of the registry's schema and the lock's name. It is held by
//!   the connection, outside any transaction, and the server lets it go
//!   when the connection ends.
//! - SQLite: an exclusive `flock` on a file beside the database,
//!   `<database>-<name>.lock`. The kernel lets it go when the process
//!   ends. SQLite's own write lock is not used, since a statement of
//!   another process that waits on it gives up after five seconds, and a
//!   step held here may take longer.
//!
//! An in-memory SQLite store is one connection of one process and takes
//! every lock at once.
//!
//! A lock is taken with [`try_take`], which never waits, and given back
//! with [`release`]; the caller waits between tries, so it can beat its
//! job's heart and see a cancel meanwhile. A process never takes a lock it
//! holds already: a file lock taken twice by one process waits on itself.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;

use crate::schema::Type;
use crate::store::{Error, Param, Store};

/// A lock this process holds. Give it back with [`release`]; a guard that
/// is dropped instead lets a file lock go with its file, and leaves an
/// advisory lock held until the connection ends.
#[derive(Debug)]
#[must_use = "a lock is held until it is released"]
pub struct Held {
    name: String,
    how: How,
}

#[derive(Debug)]
enum How {
    Advisory(i64),
    File(File),
    Alone,
}

impl Held {
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A lock's name as a file name takes it: letters, digits and `-`.
fn word(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// The advisory key of a lock in a registry's schema: the first eight
/// bytes of a BLAKE2b digest of both, so two registries in one database
/// never share a key.
pub fn advisory_key(schema: &str, name: &str) -> i64 {
    use blake2::digest::consts::U32;
    use blake2::{Blake2b, Digest};
    let mut h = Blake2b::<U32>::new();
    h.update(b"nils-lock\0");
    h.update(schema.as_bytes());
    h.update(b"\0");
    h.update(name.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    i64::from_be_bytes(b)
}

/// The file a SQLite registry's lock is held on, where the database is a
/// file.
fn lock_file(store: &Store, name: &str) -> Option<PathBuf> {
    let Store::Sqlite(conn) = store else {
        return None;
    };
    let db = conn.path().filter(|p| !p.is_empty())?;
    // one file however the registry was named: through a link, or from
    // another working directory
    let db = std::fs::canonicalize(db).unwrap_or_else(|_| PathBuf::from(db));
    Some(PathBuf::from(format!(
        "{}-{}.lock",
        db.display(),
        word(name)
    )))
}

/// Take a lock if no other process holds it. Answers none when another
/// does; never waits.
pub fn try_take(store: &mut Store, name: &str) -> Result<Option<Held>, Error> {
    match store {
        Store::Postgres { schema, .. } => {
            let key = advisory_key(schema, name);
            let sql = format!(
                "SELECT pg_try_advisory_lock({}::bigint)",
                store.dialect().param(1, Type::Int)
            );
            let taken = store
                .query_opt(&sql, &[Param::Int(key)])?
                .map(|r| r.int(0))
                .transpose()?
                == Some(1);
            Ok(taken.then(|| Held {
                name: name.to_string(),
                how: How::Advisory(key),
            }))
        }
        Store::Sqlite(_) => {
            let Some(path) = lock_file(store, name) else {
                return Ok(Some(Held {
                    name: name.to_string(),
                    how: How::Alone,
                }));
            };
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            match file.try_lock() {
                Ok(()) => Ok(Some(Held {
                    name: name.to_string(),
                    how: How::File(file),
                })),
                Err(TryLockError::WouldBlock) => Ok(None),
                Err(TryLockError::Error(e)) => Err(Error::Io(e)),
            }
        }
    }
}

/// Give a lock back. On Postgres a transaction a step left aborted is
/// rolled back first, since an aborted transaction answers no statement.
pub fn release(store: &mut Store, held: Held) -> Result<(), Error> {
    match held.how {
        How::Advisory(key) => {
            let sql = format!(
                "SELECT pg_advisory_unlock({}::bigint)",
                store.dialect().param(1, Type::Int)
            );
            if store.query_opt(&sql, &[Param::Int(key)]).is_err() {
                store.rollback().ok();
                store.query_opt(&sql, &[Param::Int(key)])?;
            }
            Ok(())
        }
        How::File(file) => {
            file.unlock()?;
            Ok(())
        }
        How::Alone => Ok(()),
    }
}
