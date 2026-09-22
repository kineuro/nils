// SPDX-License-Identifier: AGPL-3.0-only

//! What a release does with the dates
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.3).
//!
//! **The date is the date** (record 38 S3). A release writes every date as
//! the archive holds it: the release is pseudonymous, not anonymous, and the
//! date is the key the clinical layer joins on. The shift and the year-only
//! policies are gone, with the offset they drew. Where a date must not show
//! in a path, the session scheme labels by months since baseline (`M00`,
//! `M06`) instead, which is the scheme's business and not the file's.
//!
//! **The registry is never rewritten** either way: what a release writes is
//! a property of the release, not of the archive.

use nils_registry::day::Day;

/// Age in whole years at a day, from a birth date.
///
/// Computed **before** the birth date is removed, because the birth date is
/// not written at all. v0 writes no age and removes the birth date, so an age
/// that was derivable from the archive is not derivable from its output; this
/// is what puts it back.
pub fn age_years(born: Day, at: Day) -> Option<i64> {
    let years = at.year() as i64 - born.year() as i64;
    if !(0..=150).contains(&years) {
        return None;
    }
    // Not yet had the birthday this year.
    let had = (at.month(), at.day()) >= (born.month(), born.day());
    Some(if had { years } else { years - 1 })
}

/// How DICOM writes an age: three digits and a unit.
pub fn age_string(years: i64) -> String {
    format!("{years:03}Y")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Day {
        Day::parse(s).unwrap()
    }

    #[test]
    fn an_age_is_whole_years_and_knows_about_birthdays() {
        assert_eq!(age_years(d("19800615"), d("20220614")), Some(41));
        assert_eq!(age_years(d("19800615"), d("20220615")), Some(42));
        assert_eq!(age_years(d("19800615"), d("20220616")), Some(42));
        assert_eq!(age_string(42), "042Y");
        assert_eq!(age_string(7), "007Y");
    }

    #[test]
    fn an_age_that_is_not_one_is_not_written() {
        // A birth date after the study, or a hundred and fifty years before
        // it, is a placeholder rather than a person.
        assert_eq!(age_years(d("20230101"), d("20220101")), None);
        assert_eq!(age_years(d("18000101"), d("20220101")), None);
    }
}
