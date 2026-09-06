// SPDX-License-Identifier: AGPL-3.0-only

//! What private elements an archive actually carries
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.4 and §15's sixth question).
//!
//! A private element means whatever its vendor decided, and nothing in the file
//! says what: some carry a diffusion direction, a slice-timing table or a
//! magnetisation-transfer flag, and some carry the operator's name. So a
//! release drops them all and an allowlist brings back the ones a pack names.
//!
//! Choosing that list from a chair is guesswork. This is how it is chosen from
//! the archive instead: walk it, and count what is there.
//!
//! **It reports shapes and never values.** A creator's name, a block, an
//! element, a VR, how many files carry it, how long the value is and whether it
//! is printable. That is enough to decide whether an element is worth keeping
//! and to recognise the ones that obviously are not, and it is safe to carry
//! out of a private host, which a survey that quoted values would not be.

use std::collections::BTreeMap;

use dicom_core::header::Header as _;
use dicom_object::InMemDicomObject;

/// One private element, as the archive has it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// Files that carry it.
    pub files: u64,
    /// The value representations it appears under, in order of first sight. A
    /// element that is `LO` in one vendor's files and `OB` in another's is two
    /// things with one address, which is worth seeing.
    pub vrs: Vec<String>,
    /// The shortest and longest value, in bytes.
    pub shortest: usize,
    pub longest: usize,
    /// Files whose value is entirely printable, which is what a header, a
    /// number and a name have in common and a binary blob does not.
    pub printable: u64,
    /// Distinct values, counted up to a cap: an element with one value across
    /// an archive is a constant and one with thousands is per acquisition,
    /// and telling them apart is most of what makes an element worth keeping.
    pub distinct: usize,
    seen: std::collections::HashSet<u64>,
}

/// What a survey found, by creator and then by element.
#[derive(Debug, Clone, Default)]
pub struct Survey {
    pub files: u64,
    /// Files that carried at least one private element.
    pub with_private: u64,
    /// `(creator, group, element offset)` to what was seen.
    pub elements: BTreeMap<(String, u16, u8), Seen>,
    /// Private elements in a block no creator reserved, which cannot be
    /// addressed by name and so can never be kept.
    pub orphans: u64,
}

/// How many distinct values are counted before an element is called varied.
const CAP: usize = 64;

impl Survey {
    /// Read one object into the survey.
    pub fn add(&mut self, object: &InMemDicomObject) {
        self.files += 1;
        let mut creators: BTreeMap<(u16, u16), String> = BTreeMap::new();
        for e in object.iter() {
            let tag = e.tag();
            if tag.group() % 2 == 0 || tag.group() <= 0x0008 {
                continue;
            }
            if (0x0010..=0x00FF).contains(&tag.element())
                && let Ok(name) = e.value().to_str()
            {
                creators.insert(
                    (tag.group(), tag.element()),
                    name.trim_matches([' ', '\0']).to_string(),
                );
            }
        }

        let mut any = false;
        for e in object.iter() {
            let tag = e.tag();
            let (group, element) = (tag.group(), tag.element());
            if group % 2 == 0 || group <= 0x0008 || (0x0010..=0x00FF).contains(&element) {
                continue;
            }
            any = true;
            let slot = element >> 8;
            let Some(creator) = creators.get(&(group, slot)) else {
                self.orphans += 1;
                continue;
            };
            let bytes = e.value().to_bytes().map(|b| b.to_vec()).unwrap_or_default();
            let entry = self
                .elements
                .entry((creator.clone(), group, (element & 0x00FF) as u8))
                .or_default();
            entry.files += 1;
            let vr = format!("{:?}", e.header().vr());
            if !entry.vrs.contains(&vr) {
                entry.vrs.push(vr);
            }
            if entry.files == 1 || bytes.len() < entry.shortest {
                entry.shortest = bytes.len();
            }
            entry.longest = entry.longest.max(bytes.len());
            if bytes
                .iter()
                .all(|b| b.is_ascii_graphic() || *b == b' ' || *b == 0)
            {
                entry.printable += 1;
            }
            // A hash rather than the value, because a survey that held values
            // would be a copy of the archive's private data.
            if entry.seen.len() < CAP {
                let mut h: u64 = 0xcbf29ce484222325;
                for b in &bytes {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                entry.seen.insert(h);
                entry.distinct = entry.seen.len();
            }
        }
        if any {
            self.with_private += 1;
        }
    }

    /// Fold another survey in, so a walk can count in parallel.
    pub fn merge(&mut self, other: Survey) {
        self.files += other.files;
        self.with_private += other.with_private;
        self.orphans += other.orphans;
        for (key, seen) in other.elements {
            let mine = self.elements.entry(key).or_default();
            let first = mine.files == 0;
            mine.files += seen.files;
            mine.printable += seen.printable;
            for vr in seen.vrs {
                if !mine.vrs.contains(&vr) {
                    mine.vrs.push(vr);
                }
            }
            mine.shortest = match first {
                true => seen.shortest,
                false => mine.shortest.min(seen.shortest),
            };
            mine.longest = mine.longest.max(seen.longest);
            for h in seen.seen {
                if mine.seen.len() < CAP {
                    mine.seen.insert(h);
                }
            }
            mine.distinct = mine.seen.len();
        }
    }

    /// The rows a report prints, commonest first.
    pub fn rows(&self) -> Vec<Row> {
        let mut out: Vec<Row> = self
            .elements
            .iter()
            .map(|((creator, group, element), seen)| Row {
                creator: creator.clone(),
                group: *group,
                element: *element,
                files: seen.files,
                vrs: seen.vrs.join("/"),
                shortest: seen.shortest,
                longest: seen.longest,
                printable: seen.printable == seen.files,
                distinct: seen.distinct,
                varied: seen.distinct >= CAP,
            })
            .collect();
        // By creator, and the creators by how much of the archive they are in,
        // so a vendor's elements are read together and the vendor that matters
        // most is first. A sort by count alone splits one creator across the
        // report, which is how the first run of this printed `SIEMENS CSA
        // HEADER` twice.
        let mut totals: BTreeMap<String, u64> = BTreeMap::new();
        for r in &out {
            *totals.entry(r.creator.clone()).or_insert(0) += r.files;
        }
        out.sort_by(|a, b| {
            totals
                .get(&b.creator)
                .cmp(&totals.get(&a.creator))
                .then(a.creator.cmp(&b.creator))
                .then(b.files.cmp(&a.files))
                .then(a.group.cmp(&b.group))
                .then(a.element.cmp(&b.element))
        });
        out
    }
}

/// One line of a survey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub creator: String,
    pub group: u16,
    pub element: u8,
    pub files: u64,
    pub vrs: String,
    pub shortest: usize,
    pub longest: usize,
    /// Every value printable, which a header, a number and a name have in
    /// common and a binary blob does not.
    pub printable: bool,
    pub distinct: usize,
    /// It reached the cap, so it varies per acquisition rather than being a
    /// constant of the scanner or the site.
    pub varied: bool,
}

impl Row {
    /// How the allowlist would address it, which is what a pack entry needs.
    pub fn address(&self) -> String {
        format!(
            "({:04X},xx{:02X}) {}",
            self.group, self.element, self.creator
        )
    }
}

/// Read one file's header into a survey.
///
/// Header only: a survey never needs the pixels, and an archive's pixels are
/// most of its bytes.
pub fn read_into(path: &std::path::Path, survey: &mut Survey) {
    if let Ok(object) = dicom_object::OpenFileOptions::new()
        .read_until(dicom_dictionary_std::tags::PIXEL_DATA)
        .open_file(path)
    {
        survey.add(&object);
    }
}

/// Every file under `root`, to a limit, in a fixed order.
///
/// Its own walk rather than the digest's: a survey reads a few thousand files
/// and wants them in an order that does not depend on how a filesystem lists,
/// so that two surveys of one archive read the same files and can be compared.
pub fn files_under(root: &std::path::Path, limit: usize) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        here.sort();
        for path in here {
            match path.is_dir() {
                true => queue.push(path),
                false => {
                    out.push(path);
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
    }
    out
}

/// Survey a tree, in parallel.
pub fn walk(root: &std::path::Path, limit: usize, workers: usize) -> Survey {
    let paths = files_under(root, limit);
    let workers = workers.max(1);
    let chunk = paths.len().div_ceil(workers).max(1);
    let parts: Vec<Survey> = std::thread::scope(|s| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|batch| {
                s.spawn(move || {
                    let mut mine = Survey::default();
                    for path in batch {
                        read_into(path, &mut mine);
                    }
                    mine
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    let mut out = Survey::default();
    for part in parts {
        out.merge(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;
    use dicom_core::{DataElement, PrimitiveValue, Tag, VR};

    fn object(pairs: &[(u16, u16, VR, &str)]) -> InMemDicomObject {
        let mut ds = InMemDicomObject::new_empty();
        for (g, e, vr, v) in pairs {
            ds.put(DataElement::new(Tag(*g, *e), *vr, PrimitiveValue::from(*v)));
        }
        ds
    }

    #[test]
    fn an_element_is_counted_under_the_creator_that_reserved_it() {
        // Not under its position: the same vendor lands at a different offset
        // from file to file, so a survey keyed on position counts one thing
        // twice and two things once.
        let mut s = Survey::default();
        s.add(&object(&[
            (0x0019, 0x0010, VR::LO, "SIEMENS MR HEADER"),
            (0x0019, 0x100C, VR::IS, "1000"),
        ]));
        s.add(&object(&[
            (0x0019, 0x0011, VR::LO, "SIEMENS MR HEADER"),
            (0x0019, 0x110C, VR::IS, "2000"),
        ]));
        let rows = s.rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].files, 2);
        assert_eq!(rows[0].address(), "(0019,xx0C) SIEMENS MR HEADER");
        assert_eq!(rows[0].distinct, 2, "and it varies");
    }

    #[test]
    fn a_constant_and_a_varying_element_are_told_apart() {
        // Most of what makes an element worth keeping: one value across an
        // archive is a property of the scanner, and one per file is a property
        // of the acquisition.
        let mut s = Survey::default();
        for i in 0..10 {
            s.add(&object(&[
                (0x0019, 0x0010, VR::LO, "A VENDOR"),
                (0x0019, 0x1001, VR::LO, "always the same"),
                (0x0019, 0x1002, VR::IS, &format!("{i}")),
            ]));
        }
        let rows = s.rows();
        let constant = rows.iter().find(|r| r.element == 0x01).unwrap();
        let varying = rows.iter().find(|r| r.element == 0x02).unwrap();
        assert_eq!(constant.distinct, 1);
        assert_eq!(varying.distinct, 10);
    }

    #[test]
    fn a_survey_reports_shapes_and_never_values() {
        // It has to be safe to carry out of a private host, which a survey
        // that quoted values would not be.
        let mut s = Survey::default();
        s.add(&object(&[
            (0x0029, 0x0010, VR::LO, "SIEMENS CSA HEADER"),
            (0x0029, 0x1010, VR::OB, "the operator was Anna"),
        ]));
        let rendered = format!("{:?}", s.rows());
        assert!(rendered.contains("SIEMENS CSA HEADER"), "{rendered}");
        assert!(!rendered.contains("Anna"), "{rendered}");
        assert_eq!(s.rows()[0].longest, 21, "its length, which is a shape");
    }

    #[test]
    fn an_element_no_creator_reserved_is_counted_and_never_named() {
        // It cannot be addressed by name, so it can never be kept, and a
        // report that pretended otherwise would be offering the impossible.
        let mut s = Survey::default();
        s.add(&object(&[(0x0019, 0x1099, VR::LO, "orphaned")]));
        assert_eq!(s.orphans, 1);
        assert!(s.rows().is_empty());
    }

    #[test]
    fn two_surveys_fold_into_one() {
        let mut a = Survey::default();
        a.add(&object(&[
            (0x0019, 0x0010, VR::LO, "A VENDOR"),
            (0x0019, 0x1001, VR::LO, "one"),
        ]));
        let mut b = Survey::default();
        b.add(&object(&[
            (0x0019, 0x0010, VR::LO, "A VENDOR"),
            (0x0019, 0x1001, VR::LO, "two"),
        ]));
        a.merge(b);
        assert_eq!(a.files, 2);
        assert_eq!(a.rows()[0].files, 2);
        assert_eq!(a.rows()[0].distinct, 2);
    }

    #[test]
    fn a_file_with_no_private_element_is_counted_as_one() {
        let mut s = Survey::default();
        s.add(&object(&[(0x0008, 0x0060, VR::CS, "MR")]));
        let _ = synth::minimal_mr("1", "2", "3");
        assert_eq!(s.files, 1);
        assert_eq!(s.with_private, 0);
    }
}
