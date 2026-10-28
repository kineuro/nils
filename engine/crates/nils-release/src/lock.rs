// SPDX-License-Identifier: AGPL-3.0-only

//! One writer per tree (Wave 7b, the maintenance track). A release writes a
//! small hidden file into its root while it runs, naming the job it is, and
//! takes it away when it ends, done, failed or stopped alike. A release that
//! finds one asks the registry about the job it names: a job the registry
//! still calls running or cancelling, and has heard from, refuses the
//! release in words that name it; a job that is over, unknown, or silent for
//! longer than a claim allows is taken over, and the report says so.
//!
//! The file is a guard beside the registry's claim, which is one job of the
//! kind per registry: the claim fails a job as stale on the word of a
//! heartbeat it has not heard for a minute, and on another node of a
//! cluster that word was wrong once, so the folder's own file is read first,
//! before the claim gets to fail anything, and it holds as long as the other
//! release beats. A file left behind in a copied tree, or by a release that
//! died, blocks nothing: its job is over, or no one beats for it.

use std::path::{Path, PathBuf};

use nils_registry::job;
use nils_registry::store::Store;

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
}

/// Why a lock found in the root did not refuse: one line for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakenOver {
    pub job: i64,
    pub host: String,
    /// What the registry said of the job: its state, `unknown`, `silent`
    /// (running but not heard from within a claim's freshness) or `another
    /// registry's`.
    pub was: String,
}

impl TakenOver {
    pub fn line(&self) -> String {
        format!(
            "took over a lock left by release job {} on {}, which was {}",
            self.job, self.host, self.was
        )
    }
}

pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// The lock in the root, if a readable one is there.
pub fn read(root: &Path) -> Option<Lock> {
    let text = std::fs::read_to_string(path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether a lock found in the root refuses this release. `Ok(None)` when
/// the root is free; `Ok(Some(taken))` when a lock is there and may be
/// taken over; `Err(words)` when its job is still writing.
pub fn check(
    store: &mut Store,
    registry_id: &str,
    root: &Path,
) -> Result<Option<TakenOver>, String> {
    let Some(lock) = read(root) else {
        return Ok(None);
    };
    if lock.registry.as_deref().is_some_and(|id| id != registry_id) {
        return Ok(Some(TakenOver {
            job: lock.job,
            host: lock.host,
            was: "another registry's".to_string(),
        }));
    }
    let found = job::show(store, lock.job).map_err(|e| e.to_string())?;
    let Some(found) = found else {
        return Ok(Some(TakenOver {
            job: lock.job,
            host: lock.host,
            was: "unknown".to_string(),
        }));
    };
    let host = found.host.clone().unwrap_or_else(|| lock.host.clone());
    match found.state {
        job::State::Running | job::State::Cancelling if job::is_live(&found) => Err(format!(
            "another release (job {} on {host}) is writing this folder; wait for it, or cancel \
             it and run again",
            found.id
        )),
        job::State::Running | job::State::Cancelling => Ok(Some(TakenOver {
            job: found.id,
            host,
            was: format!(
                "{} but silent since {}",
                found.state.name(),
                found
                    .heartbeat_at
                    .as_deref()
                    .unwrap_or(found.started_at.as_str())
            ),
        })),
        state => Ok(Some(TakenOver {
            job: found.id,
            host,
            was: state.name().to_string(),
        })),
    }
}

/// Write this release's lock into the root, making the root if it is not
/// there yet (the preflight makes it too, a moment later).
pub fn take(root: &Path, lock: &Lock) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let text = serde_json::to_string_pretty(lock).map_err(std::io::Error::other)?;
    std::fs::write(path(root), text)
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
    fn a_lock_is_written_read_and_taken_away_by_its_own_job_alone() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        let lock = Lock {
            job: 7,
            registry: Some("r".to_string()),
            host: "h".to_string(),
            pid: 1,
            started_at: "2026-10-28T00:00:00Z".to_string(),
        };
        take(&root, &lock).unwrap();
        assert_eq!(read(&root), Some(lock.clone()));
        release(&root, 8);
        assert_eq!(read(&root), Some(lock));
        release(&root, 7);
        assert_eq!(read(&root), None);
    }
}
