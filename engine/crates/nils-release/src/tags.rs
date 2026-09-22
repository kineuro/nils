// SPDX-License-Identifier: AGPL-3.0-only

//! Which elements a release removes, as declared categories
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.4).
//!
//! Carried from v0's `anonymize/tags.py`, tag for tag, with one change of
//! shape. v0 has five categories and the fifth is `Time_And_Date_Information`,
//! which **removes** the series, acquisition and content dates and every time.
//! Here the dates are not a category: every date in the file leaves as it is
//! (§8.3, record 38 S3), because the date is the clinical join key and the
//! intervals between dates are the science, and a release is pseudonymous,
//! not anonymous. What is left of that category is the **times**, which are
//! identifying at a granularity nobody needs: a scan at 03:14 on a known day
//! narrows a population a long way.
//!
//! A release records which categories it applied, because "de-identified" is
//! not a property a file can carry without saying under what rule. v0's table
//! is a menu: a deployment picks from it on the command line and nothing in
//! the output says which pick was made.
//!
//! Carrying v0 tag for tag also carried what v0 never had. Record 35 finding
//! 1: the accession number and the device serial number were in none of the
//! categories, so a release wrote both through unchanged and its change list
//! said nothing about either. An accession number is the hospital's own
//! identifier for that examination, and whoever holds it and can reach the
//! hospital's systems undoes everything else the release did. The sixth
//! category, `ids`, is the home those elements never had: it is measured
//! against PS3.15 Annex E's basic profile and holds the direct identifiers
//! v0's five never named. The four v0 categories are still carried tag for
//! tag, because the pseudonymiser declares them by count on a door of its own
//! (record 28) and re-cutting them there is not this record's work.

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
    /// The times of day. The dates stay (§8.3) and are not a removal.
    Times,
    /// The identifiers themselves, which v0's five never named (record 35):
    /// the accession number and its issuer, the study id, the admission and
    /// service episode ids and their issuers, the placer and filler order
    /// numbers, the machine that made the images and the stations and
    /// locations it was run from, the free text where a person types a name
    /// or an accession, and the few names, issuers and places the patient and
    /// provider categories missed. 47 elements.
    Ids,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Patient => "patient",
            Category::Trial => "trial",
            Category::Provider => "provider",
            Category::Institution => "institution",
            Category::Times => "times",
            Category::Ids => "ids",
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
            Category::Ids,
        ]
    }

    pub fn tags(self) -> &'static [(u16, u16)] {
        match self {
            Category::Patient => PATIENT,
            Category::Trial => TRIAL,
            Category::Provider => PROVIDER,
            Category::Institution => INSTITUTION,
            Category::Times => TIMES,
            Category::Ids => IDS,
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

/// v0's `Time_And_Date_Information` less its dates, which stay (§8.3).
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

/// The direct identifiers v0's five categories never named, against PS3.15
/// Annex E's basic profile (record 35, finding 1).
///
/// In five groups, and every one of them removed rather than emptied: a
/// zero-length element is what the standard asks of a recipient that has to
/// keep the element present, and a research tree has no such recipient.
///
/// The examination and the order: the accession number and the issuer that
/// says whose numbering it is, the study id, the admission and service
/// episode ids with their issuers, and the placer and filler order numbers,
/// which are the request's own numbers in the hospital's systems.
///
/// The machine: its serial number, its own UID, the gantry, the unique device
/// identifier and the sequence that carries it, and the stations, AE titles
/// and locations the step was scheduled and performed at. A serial number is
/// one scanner in one department, which is a site, a room and a small set of
/// people.
///
/// The free text: the identifying, study, acquisition, image and visit
/// comments and the description of the constraint on the patient's data.
/// These are where a name, an accession number or a telephone number is
/// typed, and nothing classifies on them: the evidence a pack reads was read
/// at digest and is in the registry long before a release writes.
///
/// The people and places the patient and provider categories missed: the
/// issuer qualifiers of the patient id, the birth name, the photograph, the
/// responsible organisation, the alias, the ward the patient was on, the
/// institution they reside in, the intended recipients of the results, who
/// entered the order, from where and on which telephone, and the bare person
/// name of the content items.
///
/// The retired results and interpretation elements, which old archives still
/// carry: the issuers of the results and interpretation ids, the transcriber,
/// the author, the physician who approved it, and the name and address it was
/// distributed to.
const IDS: &[(u16, u16)] = &[
    (0x0008, 0x0050),
    (0x0008, 0x0051),
    (0x0008, 0x4000),
    (0x0010, 0x0024),
    (0x0010, 0x1005),
    (0x0010, 0x1100),
    (0x0010, 0x2299),
    (0x0018, 0x1000),
    (0x0018, 0x1002),
    (0x0018, 0x1008),
    (0x0018, 0x1009),
    (0x0018, 0x100A),
    (0x0018, 0x4000),
    (0x0020, 0x0010),
    (0x0020, 0x4000),
    (0x0032, 0x4000),
    (0x0038, 0x0004),
    (0x0038, 0x0010),
    (0x0038, 0x0011),
    (0x0038, 0x0014),
    (0x0038, 0x0060),
    (0x0038, 0x0061),
    (0x0038, 0x0300),
    (0x0038, 0x0400),
    (0x0038, 0x4000),
    (0x0040, 0x0001),
    (0x0040, 0x0010),
    (0x0040, 0x0011),
    (0x0040, 0x0241),
    (0x0040, 0x0242),
    (0x0040, 0x0243),
    (0x0040, 0x1005),
    (0x0040, 0x1010),
    (0x0040, 0x2008),
    (0x0040, 0x2009),
    (0x0040, 0x2010),
    (0x0040, 0x2016),
    (0x0040, 0x2017),
    (0x0040, 0x3001),
    (0x0040, 0xA123),
    (0x4008, 0x0042),
    (0x4008, 0x0102),
    (0x4008, 0x010C),
    (0x4008, 0x0114),
    (0x4008, 0x0119),
    (0x4008, 0x011A),
    (0x4008, 0x0202),
];

/// What may never survive a release, whatever else it did (record 35, S1).
///
/// The direct identifiers, across every category that holds one: the person,
/// the people around them, the site, the machine, the trial subject, and the
/// numbers the hospital's systems put on the examination. Nothing UID-valued
/// is here, because §8.2 governs every UID and two policies on one element is
/// how one of them is forgotten; nothing date-valued, because §8.3 governs
/// every date; and no sequence, because the list is checked against files a
/// test writes and a sequence carries no value to write.
///
/// **Written out rather than derived from the categories.** A list built from
/// what the code removes can only ever agree with the code, and the finding
/// this list exists for was exactly that: a release did what its categories
/// said, and its categories were missing an element nobody had checked for.
pub const NEVER_LEAVES: &[(u16, u16)] = &[
    (0x0008, 0x0050),
    (0x0008, 0x0080),
    (0x0008, 0x0081),
    (0x0008, 0x0090),
    (0x0008, 0x1010),
    (0x0008, 0x1050),
    (0x0008, 0x1070),
    (0x0010, 0x0010),
    (0x0010, 0x0030),
    (0x0010, 0x1000),
    (0x0010, 0x1001),
    (0x0010, 0x1005),
    (0x0010, 0x1040),
    (0x0010, 0x2154),
    (0x0010, 0x4000),
    (0x0012, 0x0040),
    (0x0018, 0x1000),
    (0x0020, 0x0010),
    (0x0032, 0x1032),
    (0x0038, 0x0010),
    (0x0038, 0x0300),
    (0x0038, 0x0400),
    (0x0040, 0x0242),
    (0x0040, 0x2008),
    (0x0040, 0x2010),
    (0x0040, 0x2016),
    (0x0040, 0x2017),
    (0x0040, 0xA123),
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
        // And the sixth is not v0's: it is what v0 never had.
        assert_eq!(IDS.len(), 47);
    }

    #[test]
    fn what_may_never_survive_is_in_a_category() {
        // Record 35, finding 1. The list is written out and this is what ties
        // it to the code: an element nobody put in a category is named here,
        // by its number, rather than written through in silence.
        for (g, e) in NEVER_LEAVES {
            let held: Vec<&str> = Category::every()
                .into_iter()
                .filter(|c| c.tags().contains(&(*g, *e)))
                .map(|c| c.name())
                .collect();
            assert_eq!(
                held.len(),
                1,
                "({g:04X},{e:04X}) is in {held:?}, and it belongs in exactly one category"
            );
        }
        let mut listed: Vec<(u16, u16)> = NEVER_LEAVES.to_vec();
        let n = listed.len();
        listed.sort_unstable();
        listed.dedup();
        assert_eq!(listed.len(), n, "named twice");
        assert_eq!(
            listed, NEVER_LEAVES,
            "in tag order, so a reader can scan it"
        );
    }

    #[test]
    fn nothing_that_may_never_survive_is_a_date_a_uid_or_the_code() {
        // §8.2 governs every UID and §8.3 every date: an element under one of
        // those policies and on this list would be two policies on one
        // element, which is how one of them is forgotten. The patient id is
        // replaced by the code the registry chose and so is never absent.
        for (g, e) in NEVER_LEAVES {
            assert_ne!(
                (*g, *e),
                (0x0010, 0x0020),
                "the code is written, not removed"
            );
            assert!(!MANDATORY.contains(&(*g, *e)));
        }
        // The one date on the list is the birth date, which its category
        // removes outright; the age is computed from it first.
        assert!(NEVER_LEAVES.contains(&(0x0010, 0x0030)));
        assert!(PATIENT.contains(&(0x0010, 0x0030)));
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
    fn no_category_removes_a_date() {
        // §8.3 and record 38 S3: every date leaves as it is. A category that
        // removed one would throw away the clinical join key and the interval
        // between two visits, which is the science.
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
        assert_eq!(Category::every().len(), 6);
        // And the sum is the count, which is also what says no element is in
        // two categories: `tags_of` deduplicates, so a tag in two of them
        // would come out one short.
        assert_eq!(
            tags_of(&Category::every()).len(),
            34 + 23 + 38 + 5 + 10 + 47
        );
    }
}
