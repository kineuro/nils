// SPDX-License-Identifier: AGPL-3.0-only

//! A calendar day, and the two questions asked of one: how many days between,
//! and how many months.
//!
//! The registry stores a date as `YYYY-MM-DD` text, and two things need to do
//! arithmetic on it: the date vote of Wave 3 §4, which reads a timestamp out of
//! a UID, and the session scheme of §5, which measures a visit from an anchor.
//! Both are a dozen lines of civil-calendar arithmetic, and one copy of it is
//! better than two that drift.

use std::fmt;

/// A day that exists. There is no way to make one that does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day {
    year: i32,
    month: u32,
    day: u32,
}

impl Day {
    /// A day, or none when the numbers are not one.
    pub fn new(year: i32, month: u32, day: u32) -> Option<Day> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in(year, month) {
            return None;
        }
        Some(Day { year, month, day })
    }

    /// `YYYYMMDD` or `YYYY-MM-DD`. Anything else is not a day.
    pub fn parse(text: &str) -> Option<Day> {
        let digits: String = text.trim().chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() != 8 {
            return None;
        }
        Day::new(
            digits[0..4].parse().ok()?,
            digits[4..6].parse().ok()?,
            digits[6..8].parse().ok()?,
        )
    }

    /// Unix epoch seconds as the day they fall on.
    pub fn from_unix(secs: i64) -> Option<Day> {
        if secs <= 0 {
            return None;
        }
        Some(Day::from_days(secs.div_euclid(86_400)))
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u32 {
        self.month
    }

    /// `YYYYMMDD`, which is how DICOM writes a date and how a session labelled
    /// by its date is named.
    pub fn compact(self) -> String {
        format!("{:04}{:02}{:02}", self.year, self.month, self.day)
    }

    /// `n` whole months later, or earlier when `n` is negative, clamped to the
    /// end of the target month so the 31st of January plus one month is the
    /// 28th or 29th of February.
    pub fn plus_months(self, n: i64) -> Day {
        add_months(self, n)
    }

    /// Days since the epoch, which is what makes subtraction easy.
    pub fn to_days(self) -> i64 {
        // Howard Hinnant's days_from_civil: March-based years, so a leap day is
        // the last day of a year and needs no special case.
        let y = if self.month <= 2 {
            self.year - 1
        } else {
            self.year
        } as i64;
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = self.month as i64;
        let d = self.day as i64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// The inverse: civil_from_days.
    pub fn from_days(days: i64) -> Day {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        Day {
            year: (if m <= 2 { y + 1 } else { y }) as i32,
            month: m,
            day: d,
        }
    }

    /// Whole days from `self` to `other`, negative when `other` is earlier.
    pub fn days_to(self, other: Day) -> i64 {
        other.to_days() - self.to_days()
    }

    /// Months from `self` to `other`, signed and fractional.
    ///
    /// Whole calendar months first, then the remaining days over the mean
    /// Gregorian month. Signed because a session before its anchor is a real
    /// thing, and folding it onto a later label with an absolute value
    /// mislabels it silently. Fractional because a tolerance is a fraction: at
    /// tolerance 1.5, month 7.4 still reaches a nominal 6, and rounding here
    /// first would make 1.5 behave exactly like 1.
    pub fn months_to(self, other: Day) -> f64 {
        let (a, b, sign) = if other >= self {
            (self, other, 1.0)
        } else {
            (other, self, -1.0)
        };
        let mut months = (b.year - a.year) as i64 * 12 + b.month as i64 - a.month as i64;
        // The last month is only whole once the day of the month is reached.
        let mut anchor = add_months(a, months);
        if anchor > b {
            months -= 1;
            anchor = add_months(a, months);
        }
        let rest = anchor.days_to(b) as f64 / MEAN_MONTH;
        sign * (months as f64 + rest)
    }
}

/// The mean Gregorian month, which is what v0 measures a part-month with and
/// what its labels were computed against.
pub const MEAN_MONTH: f64 = 30.44;

/// `self` plus `n` whole months, clamped to the end of the target month, so
/// the 31st of January plus one month is the 28th or 29th of February.
fn add_months(day: Day, n: i64) -> Day {
    let total = day.year as i64 * 12 + (day.month as i64 - 1) + n;
    let year = total.div_euclid(12) as i32;
    let month = (total.rem_euclid(12) + 1) as u32;
    let d = day.day.min(days_in(year, month));
    Day {
        year,
        month,
        day: d,
    }
}

fn days_in(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

impl fmt::Display for Day {
    /// `YYYY-MM-DD`, which is how the registry stores one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Day {
        Day::parse(s).unwrap_or_else(|| panic!("{s} is a day"))
    }

    #[test]
    fn a_day_is_a_day_or_it_is_nothing() {
        assert_eq!(d("20220115").to_string(), "2022-01-15");
        assert_eq!(d("2022-01-15").to_string(), "2022-01-15");
        assert!(Day::parse("20221345").is_none(), "month thirteen");
        assert!(Day::parse("20220230").is_none(), "february thirtieth");
        assert!(
            Day::parse("00000000").is_none(),
            "the way of writing nothing"
        );
        assert!(Day::parse("2022").is_none());
        assert_eq!(d("20200229").to_string(), "2020-02-29", "a leap day");
        assert!(Day::parse("20210229").is_none(), "and only in a leap year");
    }

    #[test]
    fn the_day_number_round_trips() {
        for s in ["19700101", "20000229", "20220115", "19851231", "21000301"] {
            let day = d(s);
            assert_eq!(Day::from_days(day.to_days()), day, "{s}");
        }
        assert_eq!(d("19700101").to_days(), 0, "the epoch is day zero");
        assert_eq!(d("2022-01-15").compact(), "20220115");
    }

    #[test]
    fn a_timestamp_is_the_day_it_falls_on() {
        // what a GE scanner leaves in a SOP UID
        assert_eq!(
            Day::from_unix(1_572_249_167).unwrap().to_string(),
            "2019-10-28"
        );
        assert!(Day::from_unix(0).is_none());
    }

    #[test]
    fn days_between_are_signed() {
        assert_eq!(d("20221001").days_to(d("20221004")), 3);
        assert_eq!(d("20221004").days_to(d("20221001")), -3);
        assert_eq!(d("20220228").days_to(d("20220301")), 1);
        assert_eq!(d("20200228").days_to(d("20200301")), 2, "a leap year");
    }

    #[test]
    fn months_between_are_signed_and_fractional() {
        assert_eq!(d("20220101").months_to(d("20220701")), 6.0);
        assert_eq!(d("20220701").months_to(d("20220101")), -6.0);
        // six months and two days
        let m = d("20220101").months_to(d("20220703"));
        assert!((m - (6.0 + 2.0 / MEAN_MONTH)).abs() < 1e-9, "{m}");
        // the end of a month plus a month is the end of the next
        assert_eq!(d("20220131").months_to(d("20220228")), 1.0);
        // and nine months lands on nine, not on eight and a bit
        assert_eq!(d("20220101").months_to(d("20221001")), 9.0);
        // and the inverse: six months back from July is January
        assert_eq!(d("20220701").plus_months(-6), d("20220101"));
        assert_eq!(d("20220131").plus_months(1), d("20220228"), "clamped");
    }
}
