// SPDX-License-Identifier: AGPL-3.0-only

//! How far a running unit is (2026-11-05): what a container may say of
//! itself while it works, so a run whose units each hold many stacks does
//! not stand at "0 of 20,000" until its first unit ends.
//!
//! The protocol (`contracts/job`, amended 2026-11-05, optional on both
//! sides): the engine names a file in the container's `NILS_PROGRESS`,
//! `/output/.nils-progress.json`. A unit may replace that file whole, as
//! often as it likes, with how many of what it was given it has done:
//! `{"done": 312, "of": 870, "what": "stacks"}`. `of` and `what` may be left
//! out. The engine reads the file while the container runs, at most every
//! few seconds and one small read at a time, never a walk of the folder,
//! since the output place may be a slow network share; it removes the file
//! once the container ended, and never registers it. A unit that writes
//! nothing is shown by its units done of its units and its time running, as
//! before.

use std::io::Read as _;
use std::path::Path;

use serde_json::{Value, json};

/// The file a unit writes, under its output folder.
pub const FILE: &str = ".nils-progress.json";

/// The variable that names it inside the container.
pub const ENV: &str = "NILS_PROGRESS";

/// Where the file is inside the container.
pub const CONTAINER_PATH: &str = "/output/.nils-progress.json";

/// The most of the file that is read: a progress line is a few dozen bytes.
pub const MAX_BYTES: u64 = 4096;

/// The longest word for what a unit counts.
const WHAT_MAX: usize = 24;

/// What one unit said of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reported {
    /// How many it has done.
    pub done: u64,
    /// Of how many, where it said.
    pub of: Option<u64>,
    /// What it counts, a short plural word ("stacks", "steps"), where it
    /// said one the engine takes.
    pub what: Option<String>,
}

/// A whole number from a JSON value: an integer, or a float with no
/// fraction, never below zero.
fn count(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        v.as_f64()
            .filter(|f| f.is_finite() && *f >= 0.0 && f.fract() == 0.0 && *f <= 1e15)
            .map(|f| f as u64)
    })
}

/// The word a unit counts in, where it is a short plain word: letters,
/// spaces and hyphens, at most [`WHAT_MAX`] long, kept in lower case. Any
/// other is dropped, since a page shows it as it is.
fn word(v: &Value) -> Option<String> {
    let w = v.as_str()?.trim();
    (!w.is_empty()
        && w.len() <= WHAT_MAX
        && w.chars()
            .all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '-'))
    .then(|| w.to_ascii_lowercase())
}

/// A progress file's text read, or none where it is no progress: an object
/// with a whole `done`. A `done` past its `of` is taken as `of`.
pub fn parse(text: &str) -> Option<Reported> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let done = count(&v["done"])?;
    let of = count(&v["of"]);
    Some(Reported {
        done: of.map_or(done, |of| done.min(of)),
        of,
        what: word(&v["what"]),
    })
}

/// What a unit's output folder says of it now: one open and one small read
/// of [`FILE`], never through a link and never blocking on a pipe a
/// container left there; none where there is no file, or it is no progress
/// (a unit that writes it half way is read again at the next look).
pub fn read(out: &Path) -> Option<Reported> {
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let f = open.open(out.join(FILE)).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    f.take(MAX_BYTES).read_to_string(&mut text).ok()?;
    parse(&text)
}

/// Remove a unit's progress file once its container ended, so it is never
/// taken for an output; nothing where there is none.
pub fn remove(out: &Path) {
    let _ = std::fs::remove_file(out.join(FILE));
}

/// What the running containers of a run said, together: their `done` and
/// `of` summed (`of` only where each said one), the word where they all
/// said the same, and how many said anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sum {
    pub done: u64,
    pub of: Option<u64>,
    pub what: Option<String>,
    pub units: usize,
}

impl Sum {
    /// The reports summed; none where there is none.
    pub fn of<'a>(reports: impl IntoIterator<Item = &'a Reported>) -> Option<Sum> {
        let mut sum: Option<Sum> = None;
        for r in reports {
            sum = Some(match sum {
                None => Sum {
                    done: r.done,
                    of: r.of,
                    what: r.what.clone(),
                    units: 1,
                },
                Some(s) => Sum {
                    done: s.done.saturating_add(r.done),
                    of: s.of.zip(r.of).map(|(a, b)| a.saturating_add(b)),
                    what: s.what.filter(|w| r.what.as_deref() == Some(w.as_str())),
                    units: s.units + 1,
                },
            });
        }
        sum
    }

    /// As the run's progress carries it: `{done, of, what, units}`.
    pub fn doc(&self) -> Value {
        json!({"done": self.done, "of": self.of, "what": self.what, "units": self.units})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_progress_line_is_read_for_its_counts_and_its_word() {
        assert_eq!(
            parse(r#"{"done": 312, "of": 870, "what": "Stacks"}"#),
            Some(Reported {
                done: 312,
                of: Some(870),
                what: Some("stacks".into())
            })
        );
        // a float with no fraction is a count; `of` and `what` may be left out
        assert_eq!(
            parse(r#"{"done": 4.0}"#),
            Some(Reported {
                done: 4,
                of: None,
                what: None
            })
        );
        // a done past its of is taken as of; a word that is no plain word is dropped
        assert_eq!(
            parse(r#"{"done": 9, "of": 5, "what": "<b>x</b>"}"#),
            Some(Reported {
                done: 5,
                of: Some(5),
                what: None
            })
        );
        for bad in [
            "",
            "{",
            "[]",
            r#"{"of": 3}"#,
            r#"{"done": -1}"#,
            r#"{"done": 1.5}"#,
            r#"{"done": "3"}"#,
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert_eq!(
            parse(r#"{"done": 1, "what": "a word far too long to be shown at all"}"#)
                .and_then(|r| r.what),
            None
        );
    }

    #[test]
    fn the_file_is_read_whole_or_not_at_all_and_removed_once_over() {
        let dir = std::env::temp_dir().join(format!("nils-progress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir), None);
        std::fs::write(dir.join(FILE), br#"{"done": 2, "of": 7}"#).unwrap();
        assert_eq!(read(&dir).map(|r| (r.done, r.of)), Some((2, Some(7))));
        // half written: nothing now, read again at the next look
        std::fs::write(dir.join(FILE), br#"{"done": 3, "o"#).unwrap();
        assert_eq!(read(&dir), None);
        remove(&dir);
        assert!(!dir.join(FILE).exists());
        remove(&dir);
        // a link a container planted is not read through
        #[cfg(unix)]
        {
            let target = dir.join("elsewhere.json");
            std::fs::write(&target, br#"{"done": 1}"#).unwrap();
            std::os::unix::fs::symlink(&target, dir.join(FILE)).unwrap();
            assert_eq!(read(&dir), None);
        }
        // nor a folder by its name
        let _ = std::fs::remove_file(dir.join(FILE));
        std::fs::create_dir(dir.join(FILE)).unwrap();
        assert_eq!(read(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_running_units_are_summed_with_their_word_where_they_share_it() {
        let a = Reported {
            done: 3,
            of: Some(10),
            what: Some("steps".into()),
        };
        let b = Reported {
            done: 4,
            of: Some(10),
            what: Some("steps".into()),
        };
        let c = Reported {
            done: 1,
            of: None,
            what: Some("slices".into()),
        };
        assert_eq!(Sum::of([]), None);
        let s = Sum::of([&a, &b]).unwrap();
        assert_eq!(
            s.doc(),
            json!({"done": 7, "of": 20, "what": "steps", "units": 2})
        );
        let s = Sum::of([&a, &b, &c]).unwrap();
        assert_eq!(
            s.doc(),
            json!({"done": 8, "of": null, "what": null, "units": 3})
        );
    }
}
