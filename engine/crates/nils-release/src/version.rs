// SPDX-License-Identifier: AGPL-3.0-only

//! What version a release is, and what changed since the last one
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.6).
//!
//! Everything upstream of a release changes. A technique is renamed, a body
//! part is corrected by QC and a brain becomes a spinal cord, a
//! vendor-specific `FLASH` reads better as a plain `GRE`, a decision is
//! recorded, the scheme is retuned, the grammar is improved. Every one of
//! those changes some files and leaves most alone, and v0 re-exports
//! everything or nothing.

use blake2::Digest;

/// The version of the code that decides what a file's **bytes** are.
///
/// Bumped deliberately when the de-identification changes what it writes: a
/// tag added to a category, a fix to the date arithmetic, a change to which
/// UIDs are remapped. Without it, a re-run after such a change compares
/// unchanged inputs, finds them unchanged, and leaves a tree that is a mixture
/// of two engines with nothing saying so.
///
/// The **naming** grammar needs no such constant, because a name is recomputed
/// from scratch on every run and compared as a path: a grammar change moves
/// files, and moving is what the comparison is looking for.
pub const SCRUB: u32 = 1;

/// A release's version: `YYYY.MM.DD.N`.
///
/// It sorts by component, reads as the day it was made, and `N` separates two
/// runs on one day. `N` counts versions of **this dataset** on this day and
/// not runs of the engine, so a release that wrote nothing still gets one:
/// having made no change is a fact about a version.
pub fn next(today: nils_registry::day::Day, previous: Option<&str>) -> String {
    let stem = format!("{:04}.{:02}.{:02}", today.year(), today.month(), day(today));
    let n = match previous {
        Some(p) if p.starts_with(&stem) => {
            p.rsplit_once('.')
                .and_then(|(_, n)| n.parse::<u32>().ok())
                .unwrap_or(0)
                + 1
        }
        _ => 1,
    };
    format!("{stem}.{n}")
}

fn day(d: nils_registry::day::Day) -> u32 {
    let first = nils_registry::day::Day::new(d.year(), d.month(), 1).expect("the first is a day");
    (first.days_to(d) + 1) as u32
}

/// What became of one stack between two versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// Same bytes, same place. Nothing is done at all, which is the point.
    Unchanged,
    /// Same bytes, somewhere else. **Renamed and not rewritten**, which is
    /// most of the saving: a QC decision that corrects a body part changes the
    /// name of a few thousand files and the content of none.
    Moved,
    /// The bytes differ, so it is written again.
    Rewritten,
    /// It was not in the last version.
    Added,
    /// It was, and is not now.
    Removed,
}

impl Change {
    pub fn name(self) -> &'static str {
        match self {
            Change::Unchanged => "unchanged",
            Change::Moved => "moved",
            Change::Rewritten => "rewritten",
            Change::Added => "added",
            Change::Removed => "removed",
        }
    }

    /// Whether anything happens on disk.
    pub fn is_work(self) -> bool {
        self != Change::Unchanged
    }
}

/// What a version knows about one stack it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Was {
    /// Everything that decides the file's bytes. **Not** where it goes:
    /// keeping the place out of the digest is what lets a move be seen as a
    /// move rather than as a rewrite.
    pub content: String,
    /// The directory it went in, relative to the release's root.
    pub dir: String,
}

/// Everything that decides one stack's bytes, as one digest.
///
/// The place is deliberately absent. A name is a rendering of the decided axes
/// and none of them touches a byte of the file, so a stack whose body part was
/// corrected has the same content in a different directory, and the work is a
/// rename.
pub fn content_of(
    policy: &str,
    categories: &str,
    private: &str,
    pack: &str,
    instances: &[(i64, i64, i64)],
) -> String {
    let mut h = blake2::Blake2s256::new();
    h.update(SCRUB.to_be_bytes());
    for part in [policy, categories, private, pack] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part.as_bytes());
    }
    // The instances, and enough of each to notice that its source changed.
    // Size and modification time are what the digest of Wave 1 decides a file
    // is unchanged by, so a release that trusted less would be trusting more
    // than the digest does.
    let mut sorted = instances.to_vec();
    sorted.sort_unstable();
    for (id, size, mtime) in sorted {
        h.update(id.to_be_bytes());
        h.update(size.to_be_bytes());
        h.update(mtime.to_be_bytes());
    }
    hex::encode(h.finalize())
}

/// What became of a stack, given what the last version knew and what this one
/// worked out.
pub fn compare(was: Option<&Was>, content: &str, dir: &str) -> Change {
    match was {
        None => Change::Added,
        Some(w) if w.content != content => Change::Rewritten,
        Some(w) if w.dir != dir => Change::Moved,
        Some(_) => Change::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_registry::day::Day;

    fn d(s: &str) -> Day {
        Day::parse(s).unwrap()
    }

    #[test]
    fn a_version_reads_as_the_day_it_was_made() {
        assert_eq!(next(d("20260905"), None), "2026.09.05.1");
        assert_eq!(next(d("20260101"), None), "2026.01.01.1");
    }

    #[test]
    fn a_second_run_on_one_day_is_the_next_version_of_that_day() {
        assert_eq!(next(d("20260905"), Some("2026.09.05.1")), "2026.09.05.2");
        assert_eq!(next(d("20260905"), Some("2026.09.05.9")), "2026.09.05.10");
    }

    #[test]
    fn a_run_on_another_day_starts_that_day_at_one() {
        assert_eq!(next(d("20260906"), Some("2026.09.05.4")), "2026.09.06.1");
    }

    #[test]
    fn a_version_that_wrote_nothing_is_still_a_version() {
        // Having made no change is a fact about a version, and a dataset whose
        // version did not move cannot say it was checked.
        assert_eq!(next(d("20260905"), Some("2026.09.05.1")), "2026.09.05.2");
    }

    #[test]
    fn the_content_digest_does_not_depend_on_where_the_file_goes() {
        // The whole reason a move can be a move. A name is a rendering of the
        // decided axes and none of them touches a byte.
        let a = content_of("p", "c", "v", "mri@1", &[(1, 100, 5)]);
        let b = content_of("p", "c", "v", "mri@1", &[(1, 100, 5)]);
        assert_eq!(a, b);
        assert_eq!(
            compare(
                Some(&Was {
                    content: a.clone(),
                    dir: "sub-x/ses-1/anat/Ax_T1w".into()
                }),
                &a,
                "sub-x/ses-1/anat/SC_Ax_T1w"
            ),
            Change::Moved
        );
    }

    #[test]
    fn a_change_to_any_input_of_the_bytes_is_a_rewrite() {
        let base = content_of("p", "c", "v", "mri@1", &[(1, 100, 5)]);
        for other in [
            content_of("shifted", "c", "v", "mri@1", &[(1, 100, 5)]),
            content_of("p", "fewer", "v", "mri@1", &[(1, 100, 5)]),
            content_of("p", "c", "another list", "mri@1", &[(1, 100, 5)]),
            content_of("p", "c", "v", "mri@2", &[(1, 100, 5)]),
            content_of("p", "c", "v", "mri@1", &[(1, 101, 5)]),
            content_of("p", "c", "v", "mri@1", &[(1, 100, 6)]),
            content_of("p", "c", "v", "mri@1", &[(1, 100, 5), (2, 100, 5)]),
        ] {
            assert_ne!(base, other);
            assert_eq!(
                compare(
                    Some(&Was {
                        content: base.clone(),
                        dir: "d".into()
                    }),
                    &other,
                    "d"
                ),
                Change::Rewritten
            );
        }
    }

    #[test]
    fn the_order_the_instances_arrive_in_is_not_an_input() {
        assert_eq!(
            content_of("p", "c", "v", "m", &[(1, 1, 1), (2, 2, 2)]),
            content_of("p", "c", "v", "m", &[(2, 2, 2), (1, 1, 1)])
        );
    }

    #[test]
    fn nothing_and_something_are_added_and_removed() {
        assert_eq!(compare(None, "x", "d"), Change::Added);
        assert!(Change::Removed.is_work());
        assert!(!Change::Unchanged.is_work());
    }

    #[test]
    fn the_same_inputs_under_a_different_engine_are_a_rewrite() {
        // What the constant is for. Without it a fix to the date arithmetic
        // would compare unchanged inputs, find them unchanged, and leave a
        // tree that is a mixture of two engines with nothing saying so.
        let mut h = blake2::Blake2s256::new();
        h.update((SCRUB + 1).to_be_bytes());
        for part in ["p", "c", "v", "m"] {
            h.update((part.len() as u64).to_be_bytes());
            h.update(part.as_bytes());
        }
        h.update(1i64.to_be_bytes());
        h.update(1i64.to_be_bytes());
        h.update(1i64.to_be_bytes());
        let later = hex::encode(h.finalize());
        assert_ne!(content_of("p", "c", "v", "m", &[(1, 1, 1)]), later);
    }
}
