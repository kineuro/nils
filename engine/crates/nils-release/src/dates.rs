// SPDX-License-Identifier: AGPL-3.0-only

//! What a release does with the dates
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.3).
//!
//! **The registry is never rewritten.** A policy is a property of a release,
//! not of the archive, so two releases of one selection may differ and neither
//! damages what was read.
//!
//! v0 has no date policy at all: its scrubber records `StudyDate` as
//! "retained" and moves on, with a comment saying it is the session key the
//! later stages build sessions from. That is true of v0, where a session is a
//! date; it is not true here, where a session is derived from a scheme and
//! carries its own label (§5).

use nils_registry::day::Day;

/// What happens to every date a release writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Written as they are. The right answer where the recipient is entitled
    /// to the dates, which is most of what this group does today.
    #[default]
    Keep,
    /// Moved by one offset per subject, drawn once and held in the linkage
    /// store, so that **every interval survives**: the gap between two visits,
    /// the age at scan, the time from an event. The clinical layer joins as
    /// before because it is shifted by the same offset.
    Shift,
    /// Only the year survives, as the first of January.
    Year,
}

impl Policy {
    pub fn name(self) -> &'static str {
        match self {
            Policy::Keep => "keep",
            Policy::Shift => "shift",
            Policy::Year => "year",
        }
    }

    pub fn parse(text: &str) -> Option<Policy> {
        match text {
            "keep" => Some(Policy::Keep),
            "shift" => Some(Policy::Shift),
            "year" => Some(Policy::Year),
            _ => None,
        }
    }

    /// Whether this policy changes a date at all.
    ///
    /// The one thing §4.3 turns on: a policy that moves dates and leaves UIDs
    /// alone has moved nothing, because the date leaves in the UID.
    pub fn moves_dates(self) -> bool {
        self != Policy::Keep
    }
}

/// How far a subject's dates move. Zero under every policy but `shift`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Offset(pub i64);

/// The widest a shift may be, either way.
///
/// Half a year is the usual choice and is what the spec fixes. Wider hides
/// less than it looks: what an offset protects is the absolute date, and the
/// intervals, which are what a reader could re-identify from, survive any
/// width by construction.
pub const MAX_SHIFT: i64 = 180;

/// Draw a subject's offset, deterministically from the key and the subject.
///
/// Drawn rather than stored would be enough on its own, but it **is** stored
/// (`date_shift` in the linkage store), because the offset is the thing that
/// undoes the policy and it belongs with the identifiers rather than beside
/// the images. Deriving it as well means a lost row is recoverable and a
/// tampered one is detectable.
pub fn draw(key: &[u8], subject_id: i64) -> Offset {
    let sub = nils_registry::pseudonym::subkey(key, b"nils/release/date-shift");
    let mut mac = <blake2::Blake2bMac<blake2::digest::consts::U8> as blake2::digest::KeyInit>::new_from_slice(&sub)
        .expect("a 32 byte key is a valid blake2b key");
    blake2::digest::Update::update(&mut mac, &subject_id.to_be_bytes());
    let digest = blake2::digest::FixedOutput::finalize_fixed(mac);
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest);
    // Uniform on the closed range, which is 361 values and not 360: a shift of
    // zero is as legitimate as any other and leaving it out would make the
    // distribution the one thing an attacker knows.
    let span = (MAX_SHIFT * 2 + 1) as u64;
    let n = (u64::from_be_bytes(bytes) % span) as i64;
    Offset(n - MAX_SHIFT)
}

/// Apply the policy to one day.
///
/// `None` means the release writes no date at all, which nothing here returns
/// today: every policy leaves something, and a release that wants a date gone
/// removes the element rather than blanking it.
pub fn apply(policy: Policy, offset: Offset, day: Day) -> Day {
    match policy {
        Policy::Keep => day,
        Policy::Shift => Day::from_days(day.to_days() + offset.0),
        // The first of January, which is what a year-only date is. It reads as
        // a real date and is not one, and the dataset description says so:
        // there is no way to write "some time in 2022" in a DA.
        Policy::Year => Day::new(day.year(), 1, 1).expect("the first of January is a day"),
    }
}

/// Age in whole years at a day, from a birth date.
///
/// Computed **before** the policy is applied, because a shift moves the study
/// and the birth date by the same amount only if both are shifted, and the
/// birth date is not written at all. v0 writes no age and removes the birth
/// date, so an age that was derivable from the archive is not derivable from
/// its output; this is what puts it back.
pub fn age_years(born: Day, at: Day) -> Option<i64> {
    let years = at.year() as i64 - born.year() as i64;
    if !(0..=150).contains(&years) {
        return None;
    }
    // Not yet had the birthday this year.
    let had = (at.month(), day_of(at)) >= (born.month(), day_of(born));
    Some(if had { years } else { years - 1 })
}

fn day_of(d: Day) -> u32 {
    // `Day` does not expose the day of the month; the round trip is exact.
    let first = Day::new(d.year(), d.month(), 1).expect("the first is a day");
    (first.days_to(d) + 1) as u32
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
    fn keeping_a_date_keeps_it() {
        assert_eq!(
            apply(Policy::Keep, Offset(37), d("20220115")),
            d("20220115")
        );
    }

    #[test]
    fn a_shift_moves_every_date_of_a_subject_by_the_same_amount() {
        // Which is the whole point: the gap between two visits survives, so
        // the clinical layer joins as before.
        let o = Offset(-42);
        let first = apply(Policy::Shift, o, d("20220115"));
        let second = apply(Policy::Shift, o, d("20220715"));
        assert_eq!(first, d("20211204"));
        assert_eq!(
            d("20220115").days_to(d("20220715")),
            first.days_to(second),
            "the interval is what survives"
        );
    }

    #[test]
    fn a_year_only_date_is_the_first_of_january() {
        assert_eq!(apply(Policy::Year, Offset(0), d("20220715")), d("20220101"));
        assert_eq!(apply(Policy::Year, Offset(0), d("20221231")), d("20220101"));
    }

    #[test]
    fn an_offset_is_the_same_for_a_subject_every_time_and_within_the_range() {
        let key = b"a key of some length";
        assert_eq!(draw(key, 7), draw(key, 7));
        for subject in 1..500 {
            let o = draw(key, subject).0;
            assert!((-MAX_SHIFT..=MAX_SHIFT).contains(&o), "{o}");
        }
    }

    #[test]
    fn two_subjects_move_by_different_amounts() {
        // Not guaranteed for any given pair, so this checks the spread rather
        // than a pair: an offset shared by everybody would leave every
        // absolute date recoverable from one known one.
        let key = b"a key of some length";
        let mut seen = std::collections::HashSet::new();
        for subject in 1..200 {
            seen.insert(draw(key, subject).0);
        }
        assert!(seen.len() > 100, "{} distinct offsets in 199", seen.len());
    }

    #[test]
    fn a_different_key_draws_a_different_offset() {
        assert_ne!(
            draw(b"one key of some length", 1),
            draw(b"two key of some length", 1)
        );
    }

    #[test]
    fn zero_is_a_legitimate_offset() {
        // Excluding it would make the distribution the one thing an attacker
        // knows for certain.
        let key = b"a key of some length";
        assert!((1..5000).any(|s| draw(key, s).0 == 0));
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

    #[test]
    fn only_keep_leaves_the_dates_alone() {
        assert!(!Policy::Keep.moves_dates());
        assert!(Policy::Shift.moves_dates());
        assert!(Policy::Year.moves_dates());
    }
}
