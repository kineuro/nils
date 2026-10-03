// SPDX-License-Identifier: AGPL-3.0-only

//! The packs this process has loaded, kept while their files stay the same.
//!
//! Loading a pack reads every file it names, compiles its patterns and runs
//! its whole corpus, which is most of what a short command costs and what a
//! server paid again for every request that named a pack. A pack is a
//! function of its files, so a load that reads the same bytes as an earlier
//! one in this process can hand back that load's pack: the loader records
//! what it read, every file's bytes and the corpus folder's listing, and a
//! later load of the same directory reads them again and compares. Anything
//! different, or anything that can no longer be read, and the pack is built
//! afresh, corpus and all. Only a pack that loaded is kept, so a pack that
//! fails fails every time; and only the pack as its author wrote it, never
//! one under an overlay.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::pack::Pack;

/// What a load read: a file and its bytes, or a folder and the files it
/// listed.
#[derive(PartialEq)]
enum Seen {
    File(PathBuf, Vec<u8>),
    Listing(PathBuf, Vec<PathBuf>),
}

impl Seen {
    /// Whether the disk still says what it said.
    fn holds(&self) -> bool {
        match self {
            Seen::File(path, bytes) => std::fs::read(path).is_ok_and(|now| now == *bytes),
            Seen::Listing(dir, names) => listing(dir).is_ok_and(|now| now == *names),
        }
    }
}

thread_local! {
    /// What the load running on this thread has read so far, while one is
    /// recording.
    static READ: RefCell<Option<Vec<Seen>>> = const { RefCell::new(None) };
}

fn record(seen: Seen) {
    READ.with(|r| {
        if let Some(list) = r.borrow_mut().as_mut() {
            list.push(seen);
        }
    });
}

/// Read a file as text for the loader, noting its bytes when a load is
/// recording.
pub(crate) fn read_to_string(path: &Path) -> std::io::Result<String> {
    let text = std::fs::read_to_string(path)?;
    record(Seen::File(path.to_path_buf(), text.clone().into_bytes()));
    Ok(text)
}

/// The paths a folder holds, sorted, noting them when a load is recording.
pub(crate) fn read_dir(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let names = listing(dir)?;
    record(Seen::Listing(dir.to_path_buf(), names.clone()));
    Ok(names)
}

fn listing(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    names.sort();
    Ok(names)
}

/// While a load records: what was recording before it, put back when the
/// load is over, however it ends.
struct Recording(Option<Option<Vec<Seen>>>);

impl Drop for Recording {
    fn drop(&mut self) {
        if let Some(outer) = self.0.take() {
            READ.with(|r| *r.borrow_mut() = outer);
        }
    }
}

struct Kept {
    dir: PathBuf,
    read: Vec<Seen>,
    pack: Pack,
}

/// How many packs a process keeps. A server names one or two; a test binary
/// that writes a pack per test should not hold every one of them.
const KEEP: usize = 4;

static KEPT: Mutex<Vec<Kept>> = Mutex::new(Vec::new());

/// The pack in `dir` as its author wrote it: the one kept, when every file
/// it read still reads the same, and otherwise `build`'s, kept for next
/// time when it loads.
pub(crate) fn load(
    dir: &Path,
    build: impl FnOnce() -> crate::error::R<Pack>,
) -> crate::error::R<Pack> {
    {
        let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = kept.iter().position(|k| k.dir == dir) {
            if kept[i].read.iter().all(Seen::holds) {
                // the most recently used last, the first to go first
                let k = kept.remove(i);
                let pack = k.pack.clone();
                kept.push(k);
                return Ok(pack);
            }
            kept.remove(i);
        }
    }
    let recording = Recording(Some(READ.with(|r| r.borrow_mut().replace(Vec::new()))));
    let built = build();
    let read = READ.with(|r| r.borrow_mut().take()).unwrap_or_default();
    drop(recording);
    let pack = built?;
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    kept.retain(|k| k.dir != dir);
    if kept.len() >= KEEP {
        kept.remove(0);
    }
    kept.push(Kept {
        dir: dir.to_path_buf(),
        read,
        pack: pack.clone(),
    });
    Ok(pack)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
    }

    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let at = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &at);
            } else {
                std::fs::copy(e.path(), &at).unwrap();
            }
        }
    }

    /// A pack's files changed after a load are read again, and a pack whose
    /// corpus then fails does not load from what was kept.
    #[test]
    fn a_changed_file_or_a_new_case_is_read_again() {
        let tmp = std::env::temp_dir().join(format!("nils-pack-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        copy(&pack_dir(), &tmp);
        let first = crate::load(&tmp, None).unwrap();
        let again = crate::load(&tmp, None).unwrap();
        assert_eq!(first.cases, again.cases);

        // a case that cannot hold, in a new corpus file: the listing moved
        let bad = "cases:\n  - name: never\n    stack: {series_description: 't1 sag'}\n    flags: {no_such_flag: true}\n";
        std::fs::write(tmp.join("corpus").join("zz-never.yml"), bad).unwrap();
        assert!(
            crate::load(&tmp, None).is_err(),
            "a new failing case is read"
        );
        std::fs::remove_file(tmp.join("corpus").join("zz-never.yml")).unwrap();
        assert_eq!(crate::load(&tmp, None).unwrap().cases, first.cases);

        // the same bytes changed in place, the same length: the version
        let manifest = tmp.join("pack.yml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        let version = first.version.to_string();
        let bumped = text.replacen(
            &format!("version: {version}"),
            &format!("version: {}", version.replace('0', "9")),
            1,
        );
        assert_ne!(text, bumped, "the manifest names its version");
        std::fs::write(&manifest, &bumped).unwrap();
        let moved = crate::load(&tmp, None).unwrap();
        assert_ne!(moved.version.to_string(), version);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
