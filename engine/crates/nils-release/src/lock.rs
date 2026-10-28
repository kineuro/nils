// SPDX-License-Identifier: AGPL-3.0-only

//! One writer per tree (Wave 7b, the maintenance track). A release writes a
//! small hidden file into its root while it runs, naming the job it is, and
//! takes it away when it ends, done, failed or stopped alike. The file
//! carries the time of the job's last heartbeat, moved by the heart at each
//! beat, so the folder itself says whether the release that wrote it is
//! still at it, whatever registry that release belongs to.
//!
//! A release that finds one reads it so: a lock of another registry, whose
//! job this registry cannot ask about, refuses while its beat is within the
//! freshness a claim allows ([`job::fresh_secs`]), and is taken over once it
//! is older. A lock of this registry asks the registry about the job it
//! names: a job that is over, or unknown, is taken over; a job the registry
//! still calls running or cancelling refuses while the lock's beat is fresh
//! or the registry has heard from it, and is taken over once both have gone
//! quiet. A lock file that is there but cannot be read refuses, unless it
//! has not been touched for longer than the freshness window, in which case
//! it is taken over as well. Every take-over is reported in one line.
//!
//! The file is a guard beside the registry's claim, which is one job of the
//! kind per registry: the claim fails a job as stale on the word of a
//! heartbeat it has not heard for a minute, and on another node of a
//! cluster that word was wrong once, so the folder's own file is read first,
//! before the claim gets to fail anything, and it holds as long as the other
//! release beats. A file left behind in a copied tree, or by a release that
//! died, blocks nothing for longer than the window: its job is over, or no
//! one beats for it.

use std::path::{Path, PathBuf};

use nils_registry::job;
use nils_registry::store::{Error as StoreError, Store};
use nils_registry::time::{now_iso, now_secs, secs_of};

/// The file's name in the root.
pub const FILE: &str = ".nils-release-lock.json";

/// What the file says.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lock {
    /// The release job, in the registry the lock names.
    pub job: i64,
    /// The registry's identity, so a lock of another registry is known as
    /// one this registry cannot ask about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    pub host: String,
    pub pid: u32,
    pub started_at: String,
    /// When the release last beat its heart, moved by [`beat`]. Absent only
    /// in a lock written before the heart moved it; `started_at` stands in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat_at: Option<String>,
}

impl Lock {
    /// The last time the release that wrote this lock was known to be at it.
    fn last(&self) -> &str {
        self.beat_at.as_deref().unwrap_or(self.started_at.as_str())
    }
}

/// Why a lock found in the root did not refuse: one line for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakenOver {
    /// The job and host the lock named; absent when the lock could not be
    /// read.
    pub job: Option<(i64, String)>,
    /// What was known of it: the job's state, `unknown`, `silent since ...`
    /// (running but not heard from within a claim's freshness), `another
    /// registry's, silent since ...`, or, of a lock that cannot be read,
    /// since when it has not been touched.
    pub was: String,
}

impl TakenOver {
    pub fn line(&self) -> String {
        match &self.job {
            Some((job, host)) => format!(
                "took over a lock left by release job {job} on {host}, which was {}",
                self.was
            ),
            None => format!(
                "took over a release lock in this folder that cannot be read, {}",
                self.was
            ),
        }
    }
}

/// Why a lock refuses the release, or why it could not be read about.
#[derive(Debug)]
pub enum Refusal {
    /// Another release is writing the folder, in words that name it.
    Refused(String),
    /// The registry could not be asked about the job the lock names.
    Store(StoreError),
}

pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// What is in the root.
enum Found {
    Nothing,
    Lock(Lock),
    /// A file is there and does not parse: when it was last written.
    Unreadable(Option<std::time::SystemTime>),
}

fn found(root: &Path) -> Found {
    let path = path(root);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Found::Nothing;
    };
    match serde_json::from_str(&text) {
        Ok(lock) => Found::Lock(lock),
        Err(_) => Found::Unreadable(std::fs::metadata(&path).and_then(|m| m.modified()).ok()),
    }
}

/// The lock in the root, if a readable one is there.
pub fn read(root: &Path) -> Option<Lock> {
    match found(root) {
        Found::Lock(lock) => Some(lock),
        _ => None,
    }
}

/// Whether a stamp is within the freshness a claim allows.
fn fresh(stamp: &str) -> bool {
    secs_of(stamp).is_some_and(|s| now_secs().saturating_sub(s) < job::fresh_secs())
}

/// Whether a lock found in the root refuses this release. `Ok(None)` when
/// the root is free; `Ok(Some(taken))` when a lock is there and may be
/// taken over; `Err(Refusal::Refused(words))` when its release is still
/// writing, or the file cannot be read and is not yet old.
pub fn check(
    store: &mut Store,
    registry_id: &str,
    root: &Path,
) -> Result<Option<TakenOver>, Refusal> {
    let lock = match found(root) {
        Found::Nothing => return Ok(None),
        Found::Unreadable(modified) => {
            let age = modified.and_then(|m| m.elapsed().ok());
            return match age {
                Some(age) if age.as_secs() >= job::fresh_secs() => Ok(Some(TakenOver {
                    job: None,
                    was: format!("untouched for {} s", age.as_secs()),
                })),
                _ => Err(Refusal::Refused(format!(
                    "this folder holds a release lock that cannot be read ({}); remove it if \
                     no release is writing here",
                    path(root).display()
                ))),
            };
        }
        Found::Lock(lock) => lock,
    };
    if let Some(theirs) = lock.registry.as_deref().filter(|id| *id != registry_id) {
        // another registry's job: this one cannot ask about it, so the
        // lock's own beat is the word
        return match fresh(lock.last()) {
            true => Err(Refusal::Refused(format!(
                "another release (job {} on {}, of registry {theirs}) is writing this folder; \
                 wait for it, or cancel it there and run again",
                lock.job, lock.host
            ))),
            false => Ok(Some(TakenOver {
                job: Some((lock.job, lock.host.clone())),
                was: format!("another registry's, silent since {}", lock.last()),
            })),
        };
    }
    let found = job::show(store, lock.job).map_err(|e| match e {
        job::Error::Store(e) => Refusal::Store(e),
        other => Refusal::Refused(other.to_string()),
    })?;
    let Some(found) = found else {
        return Ok(Some(TakenOver {
            job: Some((lock.job, lock.host.clone())),
            was: "unknown".to_string(),
        }));
    };
    let host = found.host.clone().unwrap_or_else(|| lock.host.clone());
    match found.state {
        job::State::Running | job::State::Cancelling
            if fresh(lock.last()) || job::is_live(&found) =>
        {
            Err(Refusal::Refused(format!(
                "another release (job {} on {host}) is writing this folder; wait for it, or \
                 cancel it and run again",
                found.id
            )))
        }
        job::State::Running | job::State::Cancelling => {
            // the registry's last word of it, or the lock's, whichever is later
            let heard = found
                .heartbeat_at
                .as_deref()
                .unwrap_or(found.started_at.as_str());
            Ok(Some(TakenOver {
                job: Some((found.id, host)),
                was: format!(
                    "{} but silent since {}",
                    found.state.name(),
                    heard.max(lock.last())
                ),
            }))
        }
        state => Ok(Some(TakenOver {
            job: Some((found.id, host)),
            was: state.name().to_string(),
        })),
    }
}

/// Write this release's lock into the root, making the root if it is not
/// there yet (the preflight makes it too, a moment later). Written beside
/// and renamed over, so a reader never sees half a file.
pub fn take(root: &Path, lock: &Lock) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let text = serde_json::to_string_pretty(lock).map_err(std::io::Error::other)?;
    let tmp = root.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path(root))
}

/// Move the lock's beat to now and write it again. A write that fails is
/// let go, as a beat the store refuses is: the lock is a guard, and the
/// release is not stopped by a hiccup in writing it.
pub fn beat(root: &Path, lock: &mut Lock) {
    lock.beat_at = Some(now_iso());
    take(root, lock).ok();
}

/// Take the lock away, if it is this job's. Nothing is reported: a lock
/// that is already gone, or someone else's by now, is no failure of the
/// release that ended.
pub fn release(root: &Path, job_id: i64) {
    if read(root).is_some_and(|l| l.job == job_id) {
        std::fs::remove_file(path(root)).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_is_written_read_beaten_and_taken_away_by_its_own_job_alone() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        let mut lock = Lock {
            job: 7,
            registry: Some("r".to_string()),
            host: "h".to_string(),
            pid: 1,
            started_at: "2026-10-28T00:00:00Z".to_string(),
            beat_at: None,
        };
        take(&root, &lock).unwrap();
        assert_eq!(read(&root), Some(lock.clone()));
        assert!(!root.join(format!("{FILE}.tmp")).exists(), "renamed over");
        beat(&root, &mut lock);
        let beaten = read(&root).unwrap();
        assert!(beaten.beat_at.is_some());
        assert_eq!(beaten, lock);
        release(&root, 8);
        assert_eq!(read(&root), Some(lock));
        release(&root, 7);
        assert_eq!(read(&root), None);
    }
}
