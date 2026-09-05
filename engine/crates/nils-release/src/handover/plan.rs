// SPDX-License-Identifier: AGPL-3.0-only

//! What goes in which archive
//! (`docs/specs/wave3-anonymize-and-bids.md`, §11).
//!
//! The unit is a **subject**, because a recipient who has half a person has
//! nothing, and because a subject is what a tree's top level is made of. A
//! subject's raw images, its `sourcedata/` and its `derivatives/` go in one
//! archive together whatever depth they sit at.
//!
//! Two departures from v0's `compress/`.
//!
//! **The plan comes from the registry and not from a directory scan.** v0 walks
//! the tree and stats every file to learn its size; v1 already recorded the
//! path and the size of every file it wrote (§8.6's manifest), so the plan is a
//! query. That is faster, and it is what lets a handover say afterwards whether
//! the tree it packed is the tree the release wrote.
//!
//! **The dataset's own files are packed.** v0 scans only top-level
//! *directories*, so `dataset_description.json`, `participants.tsv` and the
//! `README` are silently left behind, and a handover of a BIDS tree arrives as
//! something that is not a dataset. They go in the first archive.

use std::collections::BTreeMap;

/// How the chunks are filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strategy {
    /// In the order the subjects come, closing a chunk when the next would
    /// overflow it. The archives then follow the tree, which is what somebody
    /// reading a shelf of discs wants.
    #[default]
    Ordered,
    /// First fit decreasing: the largest first, into the first chunk with room.
    /// Fewer archives, in no order anybody can predict.
    Packed,
}

impl Strategy {
    pub fn name(self) -> &'static str {
        match self {
            Strategy::Ordered => "ordered",
            Strategy::Packed => "packed",
        }
    }

    pub fn parse(text: &str) -> Option<Strategy> {
        match text {
            "ordered" => Some(Strategy::Ordered),
            "packed" | "ffd" => Some(Strategy::Packed),
            _ => None,
        }
    }
}

/// One member of the plan: everything of one subject, or the dataset's own
/// files, which belong to no subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The subject's code, or empty for the dataset's own files.
    pub subject: String,
    /// The paths, relative to the release root, in a fixed order.
    pub paths: Vec<String>,
    pub bytes: i64,
}

/// One archive to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub ordinal: usize,
    pub members: Vec<Member>,
    pub bytes: i64,
}

impl Chunk {
    pub fn files(&self) -> usize {
        self.members.iter().map(|m| m.paths.len()).sum()
    }

    /// What `7z` is given: the top of each member rather than every file, so a
    /// chunk of a real dataset names hundreds of directories instead of
    /// millions of paths.
    pub fn entries(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for m in &self.members {
            match m.subject.is_empty() {
                true => out.extend(m.paths.iter().cloned()),
                false => {
                    for top in tops(&m.paths) {
                        if !out.contains(&top) {
                            out.push(top);
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// What the archive is called.
    ///
    /// v0's shape, because people have shelves of them: the ordinal, then the
    /// first and last subject it holds, so a person looking for one knows which
    /// disc to reach for without opening any of them.
    pub fn name(&self, dataset: &str) -> String {
        let mut codes: Vec<&str> = self
            .members
            .iter()
            .map(|m| m.subject.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        codes.sort();
        let stem = safe(dataset);
        match (codes.first(), codes.last()) {
            (Some(first), Some(last)) if first == last => {
                format!("{stem}_{:04}_sub-{first}.7z", self.ordinal)
            }
            (Some(first), Some(last)) => {
                format!("{stem}_{:04}_sub-{first}-to-{last}.7z", self.ordinal)
            }
            _ => format!("{stem}_{:04}.7z", self.ordinal),
        }
    }
}

/// The directories a member's files hang from, at the depth the subject sits
/// at: `sub-x`, `sourcedata/sub-x`, `derivatives/nils/sub-x`.
fn tops(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        let parts: Vec<&str> = p.split('/').collect();
        let at = parts.iter().position(|s| s.starts_with("sub-"));
        let top = match at {
            Some(i) => parts[..=i].join("/"),
            None => p.clone(),
        };
        if !out.contains(&top) {
            out.push(top);
        }
    }
    out
}

/// A dataset name a filesystem takes.
fn safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() || c == '-' {
            true => c,
            false => '-',
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    match trimmed.is_empty() {
        true => "dataset".to_string(),
        false => trimmed,
    }
}

/// Group a release's files by subject.
///
/// `paths` is every file of the version, with its size. The subject is the
/// `sub-<code>` segment wherever it appears, so a subject's raw images, its
/// `sourcedata/` and its `derivatives/` stay together.
pub fn members(paths: &[(String, i64)]) -> Vec<Member> {
    let mut by_subject: BTreeMap<String, Member> = BTreeMap::new();
    for (path, bytes) in paths {
        let subject = path
            .split('/')
            .find_map(|s| s.strip_prefix("sub-"))
            .unwrap_or("")
            .to_string();
        let m = by_subject.entry(subject.clone()).or_insert(Member {
            subject,
            paths: Vec::new(),
            bytes: 0,
        });
        m.paths.push(path.clone());
        m.bytes += bytes;
    }
    let mut out: Vec<Member> = by_subject.into_values().collect();
    for m in &mut out {
        m.paths.sort();
    }
    // The dataset's own files first, so they are in the first archive whichever
    // strategy fills the rest.
    out.sort_by(|a, b| {
        a.subject
            .is_empty()
            .cmp(&b.subject.is_empty())
            .reverse()
            .then(a.subject.cmp(&b.subject))
    });
    out
}

/// Fill the chunks.
///
/// A member larger than the cap gets an archive of its own rather than being
/// split: a subject is the unit, and half a person is nothing.
pub fn chunks(members: &[Member], cap: i64, strategy: Strategy) -> Vec<Chunk> {
    let mut order: Vec<&Member> = members.iter().collect();
    if strategy == Strategy::Packed {
        // The dataset's own files stay first; the rest go largest first.
        order.sort_by(|a, b| {
            a.subject
                .is_empty()
                .cmp(&b.subject.is_empty())
                .reverse()
                .then(b.bytes.cmp(&a.bytes))
                .then(a.subject.cmp(&b.subject))
        });
    }
    let mut bins: Vec<Chunk> = Vec::new();
    for m in order {
        let fits = match strategy {
            // Only the last, so the archives follow the tree.
            Strategy::Ordered => bins.last_mut().filter(|c| c.bytes + m.bytes <= cap),
            Strategy::Packed => bins.iter_mut().find(|c| c.bytes + m.bytes <= cap),
        };
        match fits {
            Some(c) => {
                c.bytes += m.bytes;
                c.members.push(m.clone());
            }
            None => bins.push(Chunk {
                ordinal: bins.len() + 1,
                members: vec![m.clone()],
                bytes: m.bytes,
            }),
        }
    }
    for (i, c) in bins.iter_mut().enumerate() {
        c.ordinal = i + 1;
    }
    bins
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Vec<(String, i64)> {
        vec![
            ("dataset_description.json".to_string(), 400),
            ("participants.tsv".to_string(), 100),
            ("sub-a/ses-1/anat/sub-a_ses-1_T1w.nii.gz".to_string(), 30),
            ("sub-a/ses-1/anat/sub-a_ses-1_T1w.json".to_string(), 2),
            ("sourcedata/sub-a/ses-1/loc/00000001.dcm".to_string(), 8),
            ("sub-b/ses-1/anat/sub-b_ses-1_T1w.nii.gz".to_string(), 50),
            ("derivatives/nils/sub-b/ses-1/anat/x.nii.gz".to_string(), 5),
        ]
    }

    #[test]
    fn a_subject_is_the_unit_wherever_its_files_sit() {
        // Raw, sourcedata and derivatives together, because a recipient who has
        // half a person has nothing.
        let m = members(&tree());
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].subject, "", "the dataset's own files first");
        assert_eq!(m[1].subject, "a");
        assert_eq!(m[1].bytes, 40, "raw and sourcedata together");
        assert_eq!(m[2].subject, "b");
        assert_eq!(m[2].bytes, 55, "raw and derivatives together");
    }

    #[test]
    fn the_datasets_own_files_are_packed() {
        // v0 scans only top-level directories, so a handover of a BIDS tree
        // arrives as something that is not a dataset.
        let c = chunks(&members(&tree()), 1000, Strategy::Ordered);
        assert_eq!(c.len(), 1);
        let entries = c[0].entries();
        assert!(
            entries.contains(&"dataset_description.json".to_string()),
            "{entries:?}"
        );
        assert!(entries.contains(&"participants.tsv".to_string()));
    }

    #[test]
    fn an_archive_names_the_directories_and_not_every_file() {
        // A chunk of a real dataset holds millions of paths and hundreds of
        // directories.
        let c = chunks(&members(&tree()), 1000, Strategy::Ordered);
        let entries = c[0].entries();
        assert!(entries.contains(&"sub-a".to_string()), "{entries:?}");
        assert!(entries.contains(&"sourcedata/sub-a".to_string()));
        assert!(entries.contains(&"derivatives/nils/sub-b".to_string()));
        assert!(!entries.iter().any(|e| e.ends_with(".dcm")));
    }

    #[test]
    fn ordered_follows_the_tree_and_packed_fills_the_bins() {
        let m = members(&tree());
        // A cap that takes the dataset files and one subject.
        let ordered = chunks(&m, 545, Strategy::Ordered);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].members[1].subject, "a", "in the tree's order");
        // Packed tries the largest first and puts it where it fits, which here
        // is a second chunk, and then fills the first with what is left. Fewer
        // archives, in no order anybody can predict, which is the trade.
        let packed = chunks(&m, 545, Strategy::Packed);
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].members[1].subject, "a");
        assert_eq!(packed[1].members[0].subject, "b");
    }

    #[test]
    fn a_member_bigger_than_the_cap_gets_an_archive_of_its_own() {
        // Rather than being split: a subject is the unit.
        let m = members(&tree());
        let c = chunks(&m, 10, Strategy::Ordered);
        assert_eq!(c.len(), 3);
        assert!(c.iter().all(|c| c.members.len() == 1));
        assert_eq!(c[1].bytes, 40, "over the cap and whole");
    }

    #[test]
    fn an_archive_says_which_people_are_in_it() {
        // So a person looking for one knows which disc to reach for without
        // opening any of them, which is v0's shape and worth keeping.
        let c = chunks(&members(&tree()), 1000, Strategy::Ordered);
        assert_eq!(c[0].name("a cohort"), "a-cohort_0001_sub-a-to-b.7z");
        let single = Chunk {
            ordinal: 4,
            members: vec![Member {
                subject: "abc".into(),
                paths: vec!["sub-abc/x".into()],
                bytes: 1,
            }],
            bytes: 1,
        };
        assert_eq!(single.name("c"), "c_0004_sub-abc.7z");
    }

    #[test]
    fn a_strategy_is_a_word_the_run_records() {
        assert_eq!(Strategy::parse("packed"), Some(Strategy::Packed));
        assert_eq!(Strategy::parse("ffd"), Some(Strategy::Packed), "v0's name");
        assert_eq!(Strategy::parse("nonsense"), None);
        assert_eq!(Strategy::default().name(), "ordered");
    }
}
