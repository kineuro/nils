// SPDX-License-Identifier: AGPL-3.0-only
//! What the pack crate's tests share: the MRI pack the repository ships, and
//! a scratch directory that removes itself however the test ends.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The MRI pack as the repository ships it.
pub fn shipped_mri() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

static N: AtomicU64 = AtomicU64::new(0);

/// A scratch directory for one test, removed when it drops. It holds the
/// `corpus`, `axes` and `rules` folders a small pack puts files in.
pub struct Dir(PathBuf);

impl Dir {
    /// A new directory in the system's temporary place, named for `tag`.
    pub fn new(tag: &str) -> Dir {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("nils-{tag}-{}-{n}", std::process::id()));
        for sub in ["corpus", "axes", "rules"] {
            std::fs::create_dir_all(p.join(sub)).expect("a directory");
        }
        Dir(p)
    }

    /// A copy of the shipped MRI pack, to change.
    pub fn mri_copy(tag: &str) -> Dir {
        fn copy(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for e in std::fs::read_dir(from).unwrap() {
                let e = e.unwrap();
                let p = e.path();
                if p.is_dir() {
                    copy(&p, &to.join(e.file_name()));
                } else {
                    std::fs::copy(&p, to.join(e.file_name())).unwrap();
                }
            }
        }
        let d = Dir::new(tag);
        copy(&shipped_mri(), &d.0);
        d
    }

    /// Write `body` to `name` inside the directory, making its folders.
    pub fn file(&self, name: &str, body: &str) -> &Dir {
        let at = self.0.join(name);
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("a directory");
        std::fs::write(at, body).expect("a file");
        self
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
