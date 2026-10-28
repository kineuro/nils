// SPDX-License-Identifier: AGPL-3.0-only

//! One writer per tree (Wave 7b, the maintenance track). A release writes a
//! small hidden file into its root while it runs, naming the job it is, and
//! takes it away when it ends, done, failed or stopped alike. The file
//! carries the time of the job's last heartbeat, moved by the heart at each
//! beat, so the folder itself says whether the release that wrote it is
//! still at it, whatever registry that release belongs to.
//!
//! A release that finds one reads it so: a lock of another registry, or one
//! that names no registry, whose job this registry cannot ask about (a job
//! number of its own would be the wrong job), refuses while its beat is
//! within the freshness a claim allows ([`job::is_fresh`]), and is taken
//! over once it is older. A lock of this registry asks the registry about the job it
//! names: a job that is over, or unknown, is taken over; a job the registry
//! still calls running or cancelling refuses while the lock's beat is fresh
//! or the registry has heard from it, and is taken over once both have gone
//! quiet. A lock file that is there but cannot be read, for whatever reason
//! short of not being there (it does not parse, it cannot be opened, it is
//! a directory), refuses in words that name the file and the reason, unless
//! it has not been touched for longer than the freshness window, or cannot
//! be dated at all, in which case it is taken over. Every take-over is
//! reported in one line.
//!
//! The file is taken with an exclusive create, so two releases starting
//! into one root within the same second cannot both hold it: the second
//! finds the first's file and refuses. A lock that `check` said may be
//! taken over is put out of the way first, only while it still reads as the
//! one `check` saw, by a rename to a name of the taker's own (one step,
//! which exactly one of two takers wins; the other finds nothing there and
//! refuses), and the exclusive create follows. After writing, the file is
//! read back and must name this release, or the release refuses as a second
//! writer. The lock is taken away only by the release whose whole
//! identity it carries, never on the job's number alone.
//!
//! The file is a guard beside the registry's claim, which is one job of the
//! kind per registry: the claim fails a job as stale on the word of a
//! heartbeat it has not heard for a minute, and on another node of a
//! cluster that word was wrong once, so the folder's own file is read first,
//! before the claim gets to fail anything, and it holds as long as the other
//! release beats. A file left behind in a copied tree, or by a release that
//! died, blocks nothing for longer than the window: its job is over, or no
//! one beats for it.

use std::io::Write;
use std::path::{Path, PathBuf};

use nils_registry::job;
use nils_registry::store::{Error as StoreError, Store};
use nils_registry::time::now_iso;

/// The file's name in the root.
pub const FILE: &str = ".nils-release-lock.json";

/// What the file says.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lock {
    /// The release job, in the registry the lock names.
    pub job: i64,
    /// The registry's identity, so a lock of another registry is known as
    /// one this registry cannot ask about; a lock that names none is read
    /// the same way, by its beat alone.
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

    /// Whether the two name the same release: job, registry, host and pid,
    /// whatever the beats say.
    pub fn same_release(&self, other: &Lock) -> bool {
        self.job == other.job
            && self.registry == other.registry
            && self.host == other.host
            && self.pid == other.pid
    }
}

/// Why a lock found in the root did not refuse: one line for the report,
/// and what was found, so [`take`] puts out of the way only what `check`
/// saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakenOver {
    /// The job and host the lock named; absent when the lock could not be
    /// read.
    pub job: Option<(i64, String)>,
    /// What was known of it: the job's state, `unknown`, `silent since ...`
    /// (running but not heard from within a claim's freshness), `another
    /// registry's, silent since ...`, or, of a lock that cannot be read,
    /// since when it has not been touched, or that it could not be dated.
    pub was: String,
    /// The lock as it read; none when it could not be read.
    pub lock: Option<Lock>,
}

impl TakenOver {
    pub fn line(&self) -> String {
        match &self.job {
            Some((job, host)) => format!(
                "took over a lock left by release job {job} on {host}, which was {}",
                self.was
            ),
            None => format!(
                "took over a release lock in this folder that could not be read and {}",
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

/// Why the lock was not taken.
#[derive(Debug)]
pub enum NotTaken {
    /// Another release's lock is in the root: it got there first, or it
    /// was written over this one's before the read-back. What it says,
    /// when it reads.
    Theirs(Option<Lock>),
    /// The root or the file could not be written.
    Io(std::io::Error),
}

impl std::fmt::Display for NotTaken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotTaken::Theirs(Some(lock)) => write!(
                f,
                "another release (job {} on {}{}) took this folder first; wait for it, or \
                 cancel it and run again",
                lock.job,
                lock.host,
                match &lock.registry {
                    Some(r) => format!(", of registry {r}"),
                    None => String::new(),
                }
            ),
            NotTaken::Theirs(None) => write!(
                f,
                "another release took this folder first; wait for it, or cancel it and run again"
            ),
            NotTaken::Io(e) => write!(f, "{e}"),
        }
    }
}

pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// What is in the root.
enum Found {
    Nothing,
    Lock(Lock),
    /// A file is there and cannot be read: why, naming the file, and when
    /// it was last written, where that can be told.
    Unreadable {
        why: String,
        modified: Option<std::time::SystemTime>,
    },
}

fn found(root: &Path) -> Found {
    found_at(&path(root))
}

/// What is in the file at `path`, read as a lock.
fn found_at(path: &Path) -> Found {
    let modified = || std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Found::Nothing,
        // anything else is a file that is there and cannot be read: never
        // "no lock"
        Err(e) => {
            return Found::Unreadable {
                why: format!("{}: {}", path.display(), e.kind()),
                modified: modified(),
            };
        }
    };
    match serde_json::from_str(&text) {
        Ok(lock) => Found::Lock(lock),
        Err(e) => Found::Unreadable {
            why: format!("{}: does not parse, {e}", path.display()),
            modified: modified(),
        },
    }
}

/// The lock in the root, if a readable one is there.
pub fn read(root: &Path) -> Option<Lock> {
    match found(root) {
        Found::Lock(lock) => Some(lock),
        _ => None,
    }
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
        Found::Unreadable { why, modified } => {
            // the file's age, where it has one: none when the file cannot
            // be dated or its date lies in the future, which refuses nobody
            // for ever
            let age = modified.and_then(|m| m.elapsed().ok());
            return match age {
                None => Ok(Some(TakenOver {
                    job: None,
                    was: "could not be dated".to_string(),
                    lock: None,
                })),
                Some(age) if age.as_secs() >= job::fresh_secs() => Ok(Some(TakenOver {
                    job: None,
                    was: format!("was untouched for {} s", age.as_secs()),
                    lock: None,
                })),
                Some(_) => Err(Refusal::Refused(format!(
                    "this folder holds a release lock that cannot be read ({why}); remove it \
                     if no release is writing here"
                ))),
            };
        }
        Found::Lock(lock) => lock,
    };
    if lock.registry.as_deref() != Some(registry_id) {
        // another registry's job, or one that names no registry: this one
        // cannot ask about it (a job number of its own would be the wrong
        // job), so the lock's own beat is the word
        let theirs = match &lock.registry {
            Some(id) => format!(", of registry {id}"),
            None => ", of a registry it does not name".to_string(),
        };
        return match job::is_fresh(lock.last()) {
            true => Err(Refusal::Refused(format!(
                "another release (job {} on {}{theirs}) is writing this folder; wait for it, \
                 or cancel it there and run again",
                lock.job, lock.host
            ))),
            false => Ok(Some(TakenOver {
                job: Some((lock.job, lock.host.clone())),
                was: format!("another registry's, silent since {}", lock.last()),
                lock: Some(lock),
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
            lock: Some(lock),
        }));
    };
    let host = found.host.clone().unwrap_or_else(|| lock.host.clone());
    match found.state {
        job::State::Running | job::State::Cancelling
            if job::is_fresh(lock.last()) || job::is_live(&found) =>
        {
            // a job already asked to stop needs no second asking
            let next = match found.state {
                job::State::Cancelling => "it is stopping; wait for it and run again",
                _ => "wait for it, or cancel it and run again",
            };
            Err(Refusal::Refused(format!(
                "another release (job {} on {host}) is writing this folder; {next}",
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
                lock: Some(lock),
            }))
        }
        state => Ok(Some(TakenOver {
            job: Some((found.id, host)),
            was: state.name().to_string(),
            lock: Some(lock),
        })),
    }
}

/// Write this release's lock into the root with an exclusive create, so
/// that of two releases starting into one root exactly one holds it. With
/// `over`, the lock `check` found and said may be taken over is put out of
/// the way first, and only while the file still reads as that one; a file
/// that reads otherwise by now is another release's, which got there
/// first. After writing, the file is read back and must name this release.
/// The root is made if it is not there yet; the caller has preflighted it.
pub fn take(root: &Path, lock: &Lock, over: Option<&TakenOver>) -> Result<(), NotTaken> {
    std::fs::create_dir_all(root).map_err(NotTaken::Io)?;
    let path = path(root);
    let text =
        serde_json::to_string_pretty(lock).map_err(|e| NotTaken::Io(std::io::Error::other(e)))?;
    let exclusive = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
    };
    // what `check` saw is the one file this take may put out of the way
    let saw = |what: &Found| match (what, over.and_then(|t| t.lock.as_ref())) {
        (Found::Lock(now), Some(then)) => now == then,
        (Found::Unreadable { .. }, None) => true,
        _ => false,
    };
    let mut aside: Option<PathBuf> = None;
    let created = match exclusive() {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && over.is_some() => {
            match found(root) {
                // gone meanwhile (taken over and released already): nothing
                // to put out of the way, and the exclusive create decides
                Found::Nothing => exclusive(),
                // Still the lock `check` saw: out of the way, and the
                // exclusive create once more. Out of the way is a rename to
                // a name of this taker's own, which exactly one of two takers
                // wins (the other finds nothing there, and refuses: the
                // winner is writing), where a remove would let both pass
                // and both create after it.
                what if saw(&what) => {
                    let mine = root.join(aside_name());
                    match std::fs::rename(&path, &mine) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            return Err(NotTaken::Theirs(settled(root)));
                        }
                        Err(e) => return Err(NotTaken::Io(e)),
                    }
                    // what was moved aside is read once more: a lock another
                    // taker wrote between the look and the rename is put
                    // back, and this one refuses, since that taker is writing
                    if !saw(&found_at(&mine)) {
                        std::fs::rename(&mine, &path).ok();
                        return Err(NotTaken::Theirs(settled(root)));
                    }
                    aside = Some(mine);
                    exclusive()
                }
                // one that reads otherwise is someone else's, there first
                _ => return Err(NotTaken::Theirs(read(root))),
            }
        }
        other => other,
    };
    let mut file = match created {
        Ok(file) => file,
        Err(e) => {
            if let Some(mine) = &aside {
                remove_aside(mine);
            }
            return Err(match e.kind() {
                std::io::ErrorKind::AlreadyExists => NotTaken::Theirs(settled(root)),
                _ => NotTaken::Io(e),
            });
        }
    };
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    // the one `check` saw is of no further use once ours is in place
    if let Some(mine) = &aside {
        remove_aside(mine);
    }
    written.map_err(NotTaken::Io)?;
    // Ours, and nobody else's, before a byte of the release is written. A
    // file briefly not there is given the moment another taker needs to put
    // back what it moved aside by mistake (see above), and is read again.
    for _ in 0..10 {
        match read(root) {
            Some(now) if now.same_release(lock) => return Ok(()),
            Some(theirs) => return Err(NotTaken::Theirs(Some(theirs))),
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    Err(NotTaken::Theirs(None))
}

/// Take away what was put aside: a file, or a directory that stood at the
/// lock's name (which `check` saw as a lock that cannot be read, and took
/// over once it was old), which `remove_file` refuses. Nothing is reported:
/// what was put aside is nobody's by now.
fn remove_aside(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() == std::io::ErrorKind::IsADirectory
    {
        std::fs::remove_dir_all(path).ok();
    }
}

/// A name of this taker's own for the lock it puts out of the way: in the
/// root, so the rename is one step on one filesystem, and unlike any other
/// taker's, in or out of this process.
fn aside_name() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{FILE}.{}.{nanos}.{}.aside",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// The other release's lock, given a moment to be written: the one that
/// created the file first may not have written it yet, and the refusal
/// should name it.
fn settled(root: &Path) -> Option<Lock> {
    for _ in 0..10 {
        if let Some(lock) = read(root) {
            return Some(lock);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    read(root)
}

/// How a release lost its folder's lock between two beats: the file names
/// another release by now (one that took the folder over on a word this
/// release's beats should have contradicted), is gone, or was written over
/// with something that does not read. The release that lost it reads this
/// as an abort: whatever it has in hand is given up, nothing more is
/// written, and the other's lock is left as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lost {
    Theirs(Lock),
    Gone,
    Unreadable(String),
}

impl std::fmt::Display for Lost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Lost::Theirs(lock) => write!(
                f,
                "the folder's lock was lost to release job {} on {}{}",
                lock.job,
                lock.host,
                match &lock.registry {
                    Some(r) => format!(", of registry {r}"),
                    None => String::new(),
                }
            ),
            Lost::Gone => f.write_str("the folder's lock was removed"),
            Lost::Unreadable(why) => write!(
                f,
                "the folder's lock was written over and cannot be read ({why})"
            ),
        }
    }
}

/// Move the lock's beat to now and write it again, where the file is still
/// this release's. The answer says whether it is: a lock that names
/// another release by now is left as it is, and a file that is gone is not
/// put back, since either means the folder is no longer this release's to
/// write ([`Lost`]). A write that fails is let go, as a beat the store
/// refuses is: the lock is a guard, and the release is not stopped by a
/// hiccup in writing it.
///
/// The write never goes over another's lock. A rename over the file would:
/// a taker that found this lock stale can put it aside and write its own
/// between the look and the rename, and the rename would then bury the
/// taker's lock under a fresh copy of this one, two writers after it. So
/// the fresh copy is written the way [`take`] writes: this release's file
/// is moved aside by a rename to a name of this beat's own (the one step a
/// taker's own rename and this cannot both win), read once more to be sure
/// it is still this release's (a taker's lock that got in between goes
/// back where it was), and the fresh copy is made with an exclusive create,
/// which a taker that got the moment in between wins; then the file is read
/// back and must name this release.
pub fn beat(root: &Path, lock: &mut Lock) -> Result<(), Lost> {
    beat_between(root, lock, || {})
}

/// [`beat`], with `between` run after the lock is found to be this
/// release's and before it is moved aside: the gap a take-over can fall
/// into, which the tests fill.
fn beat_between(root: &Path, lock: &mut Lock, between: impl FnOnce()) -> Result<(), Lost> {
    match found(root) {
        Found::Lock(now) if now.same_release(lock) => {}
        other => return Err(lost_to(other)),
    }
    between();
    let path = path(root);
    let mine = root.join(aside_name());
    match std::fs::rename(&path, &mine) {
        Ok(()) => {}
        // someone moved it first: whoever holds the folder now, it is not
        // this release
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(whose(root, lock)),
        // the file could not be moved: the beat is let go, the lock as it was
        Err(_) => return Ok(()),
    }
    match found_at(&mine) {
        Found::Lock(now) if now.same_release(lock) => {}
        // not this release's: a taker's lock got between the look and the
        // rename; back it goes, and the folder is the taker's
        other => {
            std::fs::rename(&mine, &path).ok();
            return Err(lost_to(other));
        }
    }
    lock.beat_at = Some(now_iso());
    let text = match serde_json::to_string_pretty(lock) {
        Ok(text) => text,
        Err(_) => {
            std::fs::rename(&mine, &path).ok();
            return Ok(());
        }
    };
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path);
    match created {
        Ok(mut file) => {
            let written = file
                .write_all(text.as_bytes())
                .and_then(|()| file.sync_all());
            drop(file);
            if written.is_err() {
                // half a file is worse than the copy before: that goes back
                // over it, and the beat is let go
                std::fs::rename(&mine, &path).ok();
                return Ok(());
            }
            remove_aside(&mine);
        }
        // a taker got the moment the folder read as free: theirs, and the
        // copy put aside is nobody's
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            remove_aside(&mine);
            return Err(whose(root, lock));
        }
        Err(_) => {
            std::fs::rename(&mine, &path).ok();
            return Ok(());
        }
    }
    match found(root) {
        Found::Lock(now) if now.same_release(lock) => Ok(()),
        other => Err(lost_to(other)),
    }
}

/// What a beat lost the folder to, by what the file holds.
fn lost_to(found: Found) -> Lost {
    match found {
        Found::Lock(theirs) => Lost::Theirs(theirs),
        Found::Nothing => Lost::Gone,
        Found::Unreadable { why, .. } => Lost::Unreadable(why),
    }
}

/// Whose the folder is when this release's file was found moved or
/// replaced under its beat: the other's lock, given a moment to be written
/// ([`settled`]), or gone. A lock that reads as this release's all the
/// same is a lock nobody else wrote, and no loss.
fn whose(root: &Path, lock: &Lock) -> Lost {
    match settled(root) {
        Some(now) if now.same_release(lock) => Lost::Gone,
        Some(theirs) => Lost::Theirs(theirs),
        None => lost_to(found(root)),
    }
}

/// Take the lock away, if it is this release's by its whole identity (job,
/// registry, host, pid). Nothing is reported: a lock that is already gone,
/// or someone else's by now, is no failure of the release that ended.
pub fn release(root: &Path, lock: &Lock) {
    if read(root).is_some_and(|l| l.same_release(lock)) {
        std::fs::remove_file(path(root)).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_lock(job: i64) -> Lock {
        Lock {
            job,
            registry: Some("r".to_string()),
            host: "h".to_string(),
            pid: 1,
            started_at: "2026-10-28T00:00:00Z".to_string(),
            beat_at: None,
        }
    }

    #[test]
    fn a_lock_is_written_read_beaten_and_taken_away_by_its_own_release_alone() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        let mut lock = a_lock(7);
        take(&root, &lock, None).unwrap();
        assert_eq!(read(&root), Some(lock.clone()));
        assert_eq!(beat(&root, &mut lock), Ok(()));
        let beaten = read(&root).unwrap();
        assert!(beaten.beat_at.is_some());
        assert_eq!(beaten, lock);
        assert!(
            std::fs::read_dir(&root).unwrap().count() == 1,
            "no temporary left beside the lock"
        );
        // the same job of another process, or another job: not its to take
        release(
            &root,
            &Lock {
                pid: 2,
                ..lock.clone()
            },
        );
        release(
            &root,
            &Lock {
                host: "k".into(),
                ..lock.clone()
            },
        );
        release(
            &root,
            &Lock {
                registry: None,
                ..lock.clone()
            },
        );
        release(&root, &a_lock(8));
        assert_eq!(read(&root), Some(lock.clone()));
        release(&root, &lock);
        assert_eq!(read(&root), None);
    }

    #[test]
    fn the_second_release_into_a_root_finds_the_first_one_there() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        take(&root, &a_lock(1), None).unwrap();
        let second = take(&root, &a_lock(2), None).unwrap_err();
        assert!(
            matches!(&second, NotTaken::Theirs(Some(l)) if l.job == 1),
            "{second:?}"
        );
        assert!(second.to_string().contains("job 1 on h, of registry r"));
        assert_eq!(read(&root).map(|l| l.job), Some(1));
    }

    /// Two releases starting into one root at once: exactly one holds the
    /// lock, and the file names the one that does.
    #[test]
    fn two_takes_at_once_have_exactly_one_winner() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        std::fs::create_dir_all(&root).unwrap();
        let go = std::sync::Barrier::new(2);
        let (first, second) = std::thread::scope(|s| {
            let a = s.spawn(|| {
                go.wait();
                take(&root, &a_lock(1), None)
            });
            let b = s.spawn(|| {
                go.wait();
                take(&root, &a_lock(2), None)
            });
            (a.join().unwrap(), b.join().unwrap())
        });
        let winners = [first.is_ok(), second.is_ok()]
            .iter()
            .filter(|w| **w)
            .count();
        assert_eq!(winners, 1, "{first:?} {second:?}");
        let held = read(&root).unwrap();
        assert_eq!(held.job, if first.is_ok() { 1 } else { 2 });
        let loser = if first.is_ok() { second } else { first };
        assert!(matches!(loser, Err(NotTaken::Theirs(Some(_)))));
    }

    /// Two releases that both saw the same stale lock and both come to take
    /// it over: exactly one holds the folder after, the other refuses as a
    /// second writer, and nothing is left beside the lock.
    #[test]
    fn two_take_overs_of_one_stale_lock_have_exactly_one_winner() {
        for _ in 0..20 {
            let dir = nils_dicom::synth::TempDir::new("release-lock");
            let root = dir.path().join("tree");
            let stale = a_lock(1);
            take(&root, &stale, None).unwrap();
            let over = TakenOver {
                job: Some((1, "h".into())),
                was: "done".into(),
                lock: Some(stale.clone()),
            };
            let go = std::sync::Barrier::new(2);
            let (first, second) = std::thread::scope(|s| {
                let a = s.spawn(|| {
                    go.wait();
                    take(&root, &a_lock(2), Some(&over))
                });
                let b = s.spawn(|| {
                    go.wait();
                    take(&root, &a_lock(3), Some(&over))
                });
                (a.join().unwrap(), b.join().unwrap())
            });
            let winners = [first.is_ok(), second.is_ok()]
                .iter()
                .filter(|w| **w)
                .count();
            assert_eq!(winners, 1, "{first:?} {second:?}");
            let held = read(&root).unwrap();
            assert_eq!(held.job, if first.is_ok() { 2 } else { 3 });
            let loser = if first.is_ok() { second } else { first };
            assert!(matches!(loser, Err(NotTaken::Theirs(_))), "{loser:?}");
            // the one put aside is gone: the lock is the folder's only file
            let left: Vec<String> = std::fs::read_dir(&root)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            assert_eq!(left, vec![FILE.to_string()], "{left:?}");
        }
    }

    /// A lock `check` said may be taken over goes out of the way, but only
    /// while it is still the one `check` saw.
    #[test]
    fn a_take_over_replaces_the_lock_check_saw_and_nothing_else() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        let stale = a_lock(1);
        take(&root, &stale, None).unwrap();
        let over = TakenOver {
            job: Some((1, "h".into())),
            was: "done".into(),
            lock: Some(stale.clone()),
        };
        // someone else took it over between the check and the take
        take(&root, &a_lock(3), Some(&over)).unwrap();
        let late = take(&root, &a_lock(2), Some(&over)).unwrap_err();
        assert!(
            matches!(&late, NotTaken::Theirs(Some(l)) if l.job == 3),
            "{late:?}"
        );
        assert_eq!(read(&root).map(|l| l.job), Some(3));
        // the same again from the stale lock itself
        release(&root, &a_lock(3));
        take(&root, &stale, None).unwrap();
        take(&root, &a_lock(2), Some(&over)).unwrap();
        assert_eq!(read(&root).map(|l| l.job), Some(2));
        // and an unreadable one check saw goes out of the way as well
        std::fs::write(path(&root), "{ not json").unwrap();
        let unreadable = TakenOver {
            job: None,
            was: "could not be dated".into(),
            lock: None,
        };
        take(&root, &a_lock(4), Some(&unreadable)).unwrap();
        assert_eq!(read(&root).map(|l| l.job), Some(4));
    }

    /// A beat says when the lock is no longer this release's: another's
    /// is left as it is, a file that is gone is not put back, one written
    /// over with something that does not read is named.
    #[test]
    fn a_beat_never_writes_over_another_releases_lock_and_says_it_was_lost() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        let mut mine = a_lock(1);
        take(&root, &mine, None).unwrap();
        release(&root, &mine);
        assert_eq!(beat(&root, &mut mine), Err(Lost::Gone));
        assert_eq!(read(&root), None, "not put back");
        let theirs = a_lock(2);
        take(&root, &theirs, None).unwrap();
        let lost = beat(&root, &mut mine);
        assert_eq!(lost, Err(Lost::Theirs(theirs.clone())));
        assert_eq!(
            lost.unwrap_err().to_string(),
            "the folder's lock was lost to release job 2 on h, of registry r"
        );
        assert_eq!(read(&root), Some(theirs));
        std::fs::write(path(&root), "{ not json").unwrap();
        assert!(matches!(
            beat(&root, &mut mine),
            Err(Lost::Unreadable(why)) if why.contains("does not parse")
        ));
        assert_eq!(std::fs::read_to_string(path(&root)).unwrap(), "{ not json");
    }

    /// What `check` would have found of `lock` as the file holds it now:
    /// the lock a taker may put out of the way.
    fn as_found(root: &Path) -> TakenOver {
        TakenOver {
            job: None,
            was: "stale".into(),
            lock: read(root),
        }
    }

    /// Every name in the root.
    fn names_in(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    /// A take-over that falls into the beat's gap, between the look and the
    /// write, is not written over: the beat finds the file no longer its
    /// own, puts the taker's lock back where it was and says the folder was
    /// lost to it, and nothing is left beside the lock.
    #[test]
    fn a_beat_never_buries_a_take_over_that_fell_into_its_gap() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        let mut mine = a_lock(1);
        take(&root, &mine, None).unwrap();
        let theirs = a_lock(2);
        let over = as_found(&root);
        let lost = beat_between(&root, &mut mine, || {
            take(&root, &theirs, Some(&over)).unwrap();
        });
        assert_eq!(lost, Err(Lost::Theirs(theirs.clone())));
        assert_eq!(read(&root), Some(theirs.clone()), "the taker's lock stays");
        assert_eq!(names_in(&root), vec![FILE.to_string()]);
        // a take-over that fell in and let go again (the taker's release)
        // leaves the folder empty: the beat finds its lock gone
        release(&root, &theirs);
        take(&root, &mine, None).unwrap();
        let over = as_found(&root);
        let lost = beat_between(&root, &mut mine, || {
            take(&root, &theirs, Some(&over)).unwrap();
            release(&root, &theirs);
        });
        assert_eq!(lost, Err(Lost::Gone));
        assert_eq!(read(&root), None, "not put back");
        assert_eq!(names_in(&root), Vec::<String>::new());
        // and with nothing in the gap the beat is this release's: the beat
        // moved, the file is the one file in the folder
        take(&root, &mine, None).unwrap();
        assert_eq!(beat(&root, &mut mine), Ok(()));
        let now = read(&root).unwrap();
        assert!(now.same_release(&mine) && now.beat_at.is_some(), "{now:?}");
        assert_eq!(names_in(&root), vec![FILE.to_string()]);
    }

    /// A beat and a take-over racing for the same lock, twenty rounds on
    /// two threads: never both win, the file holds the winner's lock, and
    /// nothing is left beside it.
    #[test]
    fn a_beat_and_a_take_over_never_both_win() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        let theirs = a_lock(2);
        for round in 0..20 {
            let mut mine = a_lock(1);
            take(&root, &mine, None).unwrap();
            let over = as_found(&root);
            let gate = std::sync::Barrier::new(2);
            let (beaten, taken) = std::thread::scope(|s| {
                let beating = s.spawn(|| {
                    gate.wait();
                    beat(&root, &mut mine)
                });
                let taking = s.spawn(|| {
                    gate.wait();
                    take(&root, &theirs, Some(&over))
                });
                (beating.join().unwrap(), taking.join().unwrap())
            });
            assert!(
                !(beaten.is_ok() && taken.is_ok()),
                "round {round}: two writers ({beaten:?}, {taken:?})"
            );
            let holds = read(&root).unwrap_or_else(|| panic!("round {round}: no lock"));
            match (&beaten, &taken) {
                (Ok(()), Err(_)) => assert!(holds.same_release(&mine), "round {round}: {holds:?}"),
                (Err(_), Ok(())) => assert_eq!(holds, theirs, "round {round}"),
                (Err(lost), Err(not)) => {
                    panic!("round {round}: nobody holds the folder ({lost:?}, {not:?})")
                }
                (Ok(()), Ok(())) => unreachable!(),
            }
            assert_eq!(names_in(&root), vec![FILE.to_string()], "round {round}");
            std::fs::remove_file(path(&root)).unwrap();
        }
    }

    /// A directory at the lock's name is a lock that cannot be read: it
    /// refuses while it is fresh, is taken over once it is old, and the
    /// take-over puts it aside and takes it away, a directory or not.
    #[test]
    fn a_directory_at_the_locks_name_is_taken_over_once_old_and_taken_away() {
        let dir = nils_dicom::synth::TempDir::new("release-lock");
        let root = dir.path().join("tree");
        std::fs::create_dir_all(path(&root)).unwrap();
        std::fs::write(path(&root).join("inside"), "a file in it").unwrap();
        let home = nils_dicom::synth::TempDir::new("release-lock-home");
        let mut reg = {
            let home = nils_registry::home::Home::new(home.path());
            home.keys(None).add("k", b"a key of some length").unwrap();
            home.init(&nils_registry::home::InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: 12,
                session_scheme: None,
            })
            .unwrap();
            home.open().unwrap()
        };
        let fresh = check(reg.store(), "r", &root).unwrap_err();
        assert!(
            matches!(&fresh, Refusal::Refused(w) if w.contains("cannot be read") && w.contains("is a directory")),
            "{fresh:?}"
        );
        // old: untouched for longer than the window
        let long_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(job::fresh_secs() + 3600);
        std::fs::File::open(path(&root))
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        let over = check(reg.store(), "r", &root).unwrap().expect("taken over");
        assert!(over.was.starts_with("was untouched for"), "{over:?}");
        assert_eq!(over.lock, None);
        take(&root, &a_lock(5), Some(&over)).unwrap();
        assert_eq!(read(&root).map(|l| l.job), Some(5));
        let left: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(left, vec![FILE.to_string()], "{left:?}");
    }
}
