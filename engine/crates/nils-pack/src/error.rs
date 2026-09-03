// SPDX-License-Identifier: AGPL-3.0-only

//! Why a pack will not load, said where a pack author can act on it.
//!
//! A pack is written by someone who writes vocabulary, not Rust, so a refusal
//! names the file, the path inside it and, when the last key of that path can
//! be found in the file, the line. "Somewhere in your pack" is not a refusal
//! anyone can act on.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Error {
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    /// Where inside the file, as a dotted path: `flags.has_se[3]`.
    pub path: String,
    pub message: String,
}

impl Error {
    pub fn at(path: impl Into<String>, message: impl Into<String>) -> Error {
        Error {
            file: None,
            line: None,
            path: path.into(),
            message: message.into(),
        }
    }

    /// Attach the file the path lives in, and the line if it can be found.
    pub fn in_file(mut self, file: &Path, source: Option<&str>) -> Error {
        if self.file.is_none() {
            self.line = source.and_then(|s| line_of(s, &self.path));
            self.file = Some(file.to_path_buf());
        }
        self
    }
}

/// The line a path's last key sits on, when exactly one line declares it.
/// A guess that could be wrong is not offered: two candidates means none.
fn line_of(source: &str, path: &str) -> Option<usize> {
    let key = path
        .rsplit('.')
        .find(|p| !p.is_empty() && !p.starts_with('['))?
        .split('[')
        .next()?;
    if key.is_empty() {
        return None;
    }
    let mut hit = None;
    for (i, line) in source.lines().enumerate() {
        let t = line.trim_start();
        let t = t.strip_prefix("- ").unwrap_or(t);
        let declares = t
            .strip_prefix(key)
            .is_some_and(|rest| rest.starts_with(':') || rest.starts_with(" :"));
        if declares {
            if hit.is_some() {
                return None;
            }
            hit = Some(i + 1);
        }
    }
    hit
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.line) {
            (Some(p), Some(l)) => write!(f, "{}:{l}: ", p.display())?,
            (Some(p), None) => write!(f, "{}: ", p.display())?,
            (None, _) => {}
        }
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for Error {}

pub type R<T> = Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "flags:\n  has_se:\n    any: [a, b]\n  has_gre: x\n";

    #[test]
    fn a_unique_key_gives_its_line() {
        assert_eq!(line_of(SRC, "flags.has_se"), Some(2));
        assert_eq!(line_of(SRC, "flags.has_se[3]"), Some(2));
        assert_eq!(line_of(SRC, "flags.has_gre"), Some(4));
    }

    #[test]
    fn a_key_that_is_not_there_gives_nothing() {
        assert_eq!(line_of(SRC, "flags.has_epi"), None);
    }

    #[test]
    fn a_key_declared_twice_gives_nothing_rather_than_a_guess() {
        let twice = "a:\n  k: 1\nb:\n  k: 2\n";
        assert_eq!(line_of(twice, "a.k"), None);
    }

    #[test]
    fn the_message_says_file_line_and_path() {
        let e = Error::at("flags.has_se", "no parser named x")
            .in_file(Path::new("packs/example/flags.yml"), Some(SRC));
        assert_eq!(
            e.to_string(),
            "packs/example/flags.yml:2: flags.has_se: no parser named x"
        );
    }
}
