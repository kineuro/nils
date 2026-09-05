// SPDX-License-Identifier: AGPL-3.0-only

//! Which elements a release removes, as declared categories
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.4).
//!
//! Carried from v0's `anonymize/tags.py`, tag for tag, with one change of
//! shape. v0 has five categories and the fifth is `Time_And_Date_Information`,
//! which **removes** the series, acquisition and content dates and every time.
//! Here the dates are not a category: they are the policy of §8.3, which
//! applies to every date in the file, because the intervals between them are
//! the science and removing them throws that away to hide something a shift
//! already hides. What is left of that category is the **times**, which are
//! identifying at a granularity nobody needs: a scan at 03:14 on a known day
//! narrows a population a long way.
//!
//! A release records which categories it applied, because "de-identified" is
//! not a property a file can carry without saying under what rule. v0's table
//! is a menu: a deployment picks from it on the command line and nothing in
//! the output says which pick was made.

use dicom_core::Tag;

/// A named set of elements a release may remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// Who the patient is: the name, the birth date, the address, the
    /// insurance, the comments. 34 elements, and the one every release wants.
    Patient,
    /// Which trial, arm, site and protocol the subject was on, which names the
    /// study and often the site. 23 elements.
    Trial,
    /// Who performed, referred, read and reported: 38 elements. Names of
    /// people who are not the subject are still names.
    Provider,
    /// Where it was done: the institution, its address and department.
    Institution,
    /// The times of day. The dates are §8.3's policy and not a removal.
    Times,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Patient => "patient",
            Category::Trial => "trial",
            Category::Provider => "provider",
            Category::Institution => "institution",
            Category::Times => "times",
        }
    }

    pub fn parse(text: &str) -> Option<Category> {
        Category::every().into_iter().find(|c| c.name() == text)
    }

    /// Every category, which is also the default: a release removes all of
    /// them unless it says otherwise, because the safe set is the one nobody
    /// had to think about.
    pub fn every() -> Vec<Category> {
        vec![
            Category::Patient,
            Category::Trial,
            Category::Provider,
            Category::Institution,
            Category::Times,
        ]
    }

    pub fn tags(self) -> &'static [(u16, u16)] {
        match self {
            Category::Patient => PATIENT,
            Category::Trial => TRIAL,
            Category::Provider => PROVIDER,
            Category::Institution => INSTITUTION,
            Category::Times => TIMES,
        }
    }
}

/// The elements of the named categories, as tags.
pub fn tags_of(categories: &[Category]) -> Vec<Tag> {
    let mut out: Vec<Tag> = categories
        .iter()
        .flat_map(|c| c.tags().iter().map(|(g, e)| Tag(*g, *e)))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// What is never removed, whatever a category says.
///
/// v0's `MANDATORY_TAGS`, which are the two that make a file a file: without
/// its SOP class and instance UID it is not a DICOM object and no tool will
/// read it. They are remapped by §8.2 rather than kept, but they are never
/// gone.
pub const MANDATORY: &[(u16, u16)] = &[(0x0008, 0x0016), (0x0008, 0x0018)];

const PATIENT: &[(u16, u16)] = &[
    (0x0010, 0x0010),
    (0x0010, 0x0021),
    (0x0010, 0x0030),
    (0x0010, 0x0032),
    (0x0010, 0x0040),
    (0x0010, 0x0050),
    (0x0010, 0x0101),
    (0x0010, 0x0102),
    (0x0010, 0x1000),
    (0x0010, 0x1001),
    (0x0010, 0x1002),
    (0x0010, 0x1010),
    (0x0010, 0x1020),
    (0x0010, 0x1030),
    (0x0010, 0x1040),
    (0x0010, 0x1060),
    (0x0010, 0x1080),
    (0x0010, 0x1081),
    (0x0010, 0x1090),
    (0x0010, 0x2000),
    (0x0010, 0x2110),
    (0x0010, 0x2150),
    (0x0010, 0x2152),
    (0x0010, 0x2154),
    (0x0010, 0x2160),
    (0x0010, 0x2180),
    (0x0010, 0x21A0),
    (0x0010, 0x21B0),
    (0x0010, 0x21C0),
    (0x0010, 0x21D0),
    (0x0010, 0x21F0),
    (0x0010, 0x2297),
    (0x0010, 0x2298),
    (0x0010, 0x4000),
];

const TRIAL: &[(u16, u16)] = &[
    (0x0012, 0x0010),
    (0x0012, 0x0020),
    (0x0012, 0x0021),
    (0x0012, 0x0030),
    (0x0012, 0x0031),
    (0x0012, 0x0040),
    (0x0012, 0x0042),
    (0x0012, 0x0050),
    (0x0012, 0x0051),
    (0x0012, 0x0060),
    (0x0012, 0x0071),
    (0x0012, 0x0072),
    (0x0012, 0x0081),
    (0x0012, 0x0082),
    (0x0012, 0x0083),
    (0x0012, 0x0084),
    (0x0012, 0x0085),
    (0x0012, 0x0086),
    (0x0012, 0x0087),
    (0x0012, 0x0088),
    (0x0012, 0x0089),
    (0x0012, 0x0090),
    (0x0012, 0x0091),
];

const PROVIDER: &[(u16, u16)] = &[
    (0x0008, 0x0090),
    (0x0008, 0x0092),
    (0x0008, 0x0094),
    (0x0008, 0x0096),
    (0x0008, 0x1048),
    (0x0008, 0x1049),
    (0x0008, 0x1050),
    (0x0008, 0x1052),
    (0x0008, 0x1060),
    (0x0008, 0x1062),
    (0x0008, 0x106E),
    (0x0008, 0x1070),
    (0x0008, 0x1072),
    (0x0008, 0x1080),
    (0x0008, 0x2111),
    (0x0032, 0x1032),
    (0x0032, 0x1033),
    (0x0032, 0x1060),
    (0x0040, 0x0006),
    (0x0040, 0x0007),
    (0x0040, 0x0009),
    (0x0040, 0x000B),
    (0x0040, 0x0253),
    (0x0040, 0x0254),
    (0x0040, 0x0260),
    (0x0040, 0x0275),
    (0x0040, 0x1001),
    (0x0040, 0x1002),
    (0x0040, 0x1102),
    (0x0040, 0x1103),
    (0x0040, 0x1104),
    (0x0040, 0x1400),
    (0x0040, 0xA073),
    (0x0040, 0xA075),
    (0x0040, 0xA730),
    (0x0070, 0x0084),
    (0x0070, 0x0086),
    (0x0400, 0x0561),
];

const INSTITUTION: &[(u16, u16)] = &[
    (0x0008, 0x0080),
    (0x0008, 0x0081),
    (0x0008, 0x1010),
    (0x0008, 0x1040),
    (0x0008, 0x1041),
];

/// v0's `Time_And_Date_Information` less its dates, which §8.3 governs.
const TIMES: &[(u16, u16)] = &[
    (0x0008, 0x0013),
    (0x0008, 0x0030),
    (0x0008, 0x0031),
    (0x0008, 0x0032),
    (0x0008, 0x0033),
    (0x0032, 0x1051),
    (0x0040, 0x0245),
    (0x0040, 0x0251),
    (0x0040, 0x2005),
    (0x0040, 0xA032),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_categories_are_v0_s_counts() {
        assert_eq!(PATIENT.len(), 34);
        assert_eq!(TRIAL.len(), 23);
        assert_eq!(PROVIDER.len(), 38);
        assert_eq!(INSTITUTION.len(), 5);
    }

    #[test]
    fn no_category_holds_a_tag_twice_and_none_is_mandatory() {
        for c in Category::every() {
            let mut seen: Vec<(u16, u16)> = c.tags().to_vec();
            let n = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), n, "{}", c.name());
            for m in MANDATORY {
                assert!(!c.tags().contains(m), "{} holds {m:?}", c.name());
            }
        }
    }

    #[test]
    fn a_category_holding_a_date_would_be_the_policy_overruling_itself() {
        // §8.3 governs every date in the file, and a category that removed one
        // would take it out from under the policy: the interval between two
        // visits is the science, and a removal throws it away to hide
        // something a shift already hides.
        let dates = [
            (0x0008u16, 0x0020u16),
            (0x0008, 0x0021),
            (0x0008, 0x0022),
            (0x0008, 0x0023),
            (0x0008, 0x0012),
        ];
        for c in Category::every() {
            for d in &dates {
                assert!(!c.tags().contains(d), "{} holds the date {d:?}", c.name());
            }
        }
    }

    #[test]
    fn every_category_is_named_and_reads_back() {
        for c in Category::every() {
            assert_eq!(Category::parse(c.name()), Some(c));
        }
        assert_eq!(Category::parse("nonsense"), None);
    }

    #[test]
    fn the_default_is_all_of_them() {
        // The safe set is the one nobody had to think about.
        assert_eq!(Category::every().len(), 5);
        assert_eq!(tags_of(&Category::every()).len(), 34 + 23 + 38 + 5 + 10);
    }
}
