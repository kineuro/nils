// SPDX-License-Identifier: AGPL-3.0-only

//! The Swedish personal identity number in one written form, so that one
//! person derives one code however a source wrote the number.
//!
//! An identifier is a personnummer when the id type it is filed under is
//! named [`ID_TYPE`]. Nothing else is: a value of another type is hashed as
//! it was read, whatever it looks like, since a ten or twelve digit value
//! with a valid check digit is not rare among other identifiers and a guess
//! would give a person two codes. A value filed under [`ID_TYPE`] is
//! normalised before its code or its lookup is derived:
//!
//! - spaces are dropped, and the one separator between the date and the
//!   serial (`-` or `+`) is dropped;
//! - a ten digit number gets its century: the latest year ending in its two
//!   digits that is not after this year, a hundred years earlier when that
//!   date is still to come, and a hundred years earlier again when the
//!   separator is `+`, which a number carries from the year its holder
//!   turns a hundred;
//! - a coordination number keeps its day plus sixty as written.
//!
//! What comes out is twelve digits, `YYYYMMDDNNNC`. A value that is not a
//! personnummer (another shape, a date that is no date, a wrong check digit)
//! is refused rather than hashed as read, so that the code it would have
//! given is never mistaken for the person's.

use std::fmt;

use crate::time::now_iso;

/// The id type whose values are personnummer.
pub const ID_TYPE: &str = "personnummer";

/// Whether identifiers of `id_type` are normalised as personnummer.
pub fn is_type(id_type: &str) -> bool {
    id_type == ID_TYPE
}

/// Why a value is not a personnummer. Never carries the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    /// Not ten or twelve digits with at most one separator.
    Shape,
    /// The digits name no date, coordination day included.
    Date,
    /// The check digit is not the one the other nine give.
    Checksum,
}

impl Invalid {
    pub fn name(self) -> &'static str {
        match self {
            Invalid::Shape => "shape",
            Invalid::Date => "date",
            Invalid::Checksum => "checksum",
        }
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Invalid::Shape => "not ten or twelve digits with at most one separator",
            Invalid::Date => "the digits name no date",
            Invalid::Checksum => "the check digit is wrong",
        })
    }
}

impl std::error::Error for Invalid {}

/// A day as (year, month, day).
pub type Day = (i32, u32, u32);

/// Today in UTC.
pub fn today() -> Day {
    let now = now_iso();
    let num = |a: usize, b: usize| now[a..b].parse::<u32>().unwrap_or(1);
    (num(0, 4) as i32, num(5, 7), num(8, 10))
}

/// `value` as twelve digits, the century of a ten digit number taken
/// against today.
pub fn normalise(value: &str) -> Result<String, Invalid> {
    normal(value, today)
}

/// `value` as twelve digits, the century of a ten digit number taken
/// against `today`.
pub fn normalise_on(value: &str, today: Day) -> Result<String, Invalid> {
    normal(value, || today)
}

/// The clock is read only for a ten digit number.
fn normal(value: &str, today: impl FnOnce() -> Day) -> Result<String, Invalid> {
    let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    // a personnummer is ASCII; anything else is no shape of one, and slicing
    // it by bytes below would cut a character in two (the review of Wave
    // 7a's merge, 2026-10-10: "12345ä789" panicked a digest's reader)
    if !compact.is_ascii() {
        return Err(Invalid::Shape);
    }
    let bytes = compact.as_bytes();
    let (date, plus, serial) = match bytes.len() {
        10 | 12 => (
            &compact[..bytes.len() - 4],
            false,
            &compact[bytes.len() - 4..],
        ),
        11 | 13 => {
            let sep = bytes[bytes.len() - 5];
            if sep != b'-' && sep != b'+' {
                return Err(Invalid::Shape);
            }
            (
                &compact[..bytes.len() - 5],
                sep == b'+',
                &compact[bytes.len() - 4..],
            )
        }
        _ => return Err(Invalid::Shape),
    };
    if !date.bytes().all(|b| b.is_ascii_digit()) || !serial.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Invalid::Shape);
    }
    let month: u32 = date[date.len() - 4..date.len() - 2]
        .parse()
        .map_err(|_| Invalid::Shape)?;
    let written_day: u32 = date[date.len() - 2..].parse().map_err(|_| Invalid::Shape)?;
    let day = if written_day > 60 {
        written_day - 60
    } else {
        written_day
    };
    let year: i32 = if date.len() == 8 {
        date[..4].parse().map_err(|_| Invalid::Shape)?
    } else {
        let yy: i32 = date[..2].parse().map_err(|_| Invalid::Shape)?;
        let (ty, tm, td) = today();
        let latest = ty - (ty - yy).rem_euclid(100);
        if plus {
            // the holder turns a hundred this year or turned it before
            latest - 100
        } else if latest == ty && (month, day) > (tm, td) {
            // nobody holds a number for a day still to come
            latest - 100
        } else {
            latest
        }
    };
    if !(1..=12).contains(&month) || day == 0 || day > days_in(year, month) {
        return Err(Invalid::Date);
    }
    // the check digit is over the ten digit form, the century left out
    let ten = format!("{}{serial}", &date[date.len() - 6..]);
    if !luhn(&ten) {
        return Err(Invalid::Checksum);
    }
    Ok(format!("{year:04}{month:02}{written_day:02}{serial}"))
}

/// `value` normalised when `id_type` is [`ID_TYPE`], else as read.
pub fn for_type(id_type: &str, value: &str) -> Result<String, Invalid> {
    if is_type(id_type) {
        normalise(value)
    } else {
        Ok(value.to_string())
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

/// The Luhn check over ten digits: weights 2 and 1 from the left.
fn luhn(ten: &str) -> bool {
    let mut sum = 0u32;
    for (i, b) in ten.bytes().enumerate() {
        let d = u32::from(b - b'0');
        let w = if i % 2 == 0 { d * 2 } else { d };
        sum += w / 10 + w % 10;
    }
    sum.is_multiple_of(10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pseudonym::{self, Scheme};

    // The numbers here are the tax agency's published test numbers, which it
    // never gives to anyone, except the coordination number, which is made
    // up for a person born in 1890 (no coordination number was ever given
    // for that year) with a valid check digit. None is anyone's.
    const TODAY: Day = (2026, 10, 6);
    const ADULT: &str = "198501012382";
    const CHILD: &str = "201501012395";
    const CENTENARIAN: &str = "192501019216";
    const THIS_YEAR: &str = "202601012384";
    const TURNS_100: &str = "192612049029";
    const LEAP: &str = "200002292399";
    const COORDINATION: &str = "189001619809";

    /// The ten digit form with a separator.
    fn ten(twelve: &str, sep: char) -> String {
        format!("{}{sep}{}", &twelve[2..8], &twelve[8..])
    }

    #[test]
    fn the_check_digit_agrees_with_the_published_numbers() {
        for pn in [ADULT, CHILD, CENTENARIAN, THIS_YEAR, TURNS_100, LEAP] {
            assert!(luhn(&pn[2..]), "{pn}");
        }
        assert!(!luhn("8501012383"));
    }

    #[test]
    fn a_value_that_is_not_ascii_is_no_shape_and_never_a_panic() {
        // the review of Wave 7a's merge (2026-10-10): a multibyte character
        // across the byte the date ends at panicked the parser's slicing
        for value in [
            "12345ä789",
            "123456789ä",
            "1234567890ö1",
            "ååååååå-åååå",
            "١٢٣٤٥٦٧٨٩٠",
        ] {
            assert_eq!(normalise_on(value, TODAY), Err(Invalid::Shape), "{value}");
        }
    }

    #[test]
    fn twelve_digits_stay_as_they_are() {
        for written in [
            ADULT.to_string(),
            format!("{}-{}", &ADULT[..8], &ADULT[8..]),
            format!(" {} {} ", &ADULT[..8], &ADULT[8..]),
            format!("{}+{}", &ADULT[..8], &ADULT[8..]),
        ] {
            assert_eq!(normalise_on(&written, TODAY).unwrap(), ADULT, "{written}");
        }
    }

    #[test]
    fn ten_digits_get_their_century() {
        assert_eq!(normalise_on(&ten(ADULT, '-'), TODAY).unwrap(), ADULT);
        assert_eq!(normalise_on(&ADULT[2..], TODAY).unwrap(), ADULT);
        assert_eq!(normalise_on(&ten(CHILD, '-'), TODAY).unwrap(), CHILD);
        // born earlier this year: this year
        assert_eq!(
            normalise_on(&ten(THIS_YEAR, '-'), TODAY).unwrap(),
            THIS_YEAR
        );
        // a day later this year, written with a hyphen, is a century ago
        assert_eq!(
            normalise_on(&ten(TURNS_100, '-'), TODAY).unwrap(),
            TURNS_100
        );
        assert_eq!(normalise_on(&ten(LEAP, '-'), TODAY).unwrap(), LEAP);
    }

    #[test]
    fn a_plus_means_a_hundred_years_or_more() {
        assert_eq!(
            normalise_on(&ten(CENTENARIAN, '+'), TODAY).unwrap(),
            CENTENARIAN
        );
        // the year the holder turns a hundred, the day still to come
        assert_eq!(
            normalise_on(&ten(TURNS_100, '+'), TODAY).unwrap(),
            TURNS_100
        );
        // without the plus the same digits are a child of this century
        assert_eq!(
            normalise_on(&ten(CENTENARIAN, '-'), TODAY).unwrap(),
            format!("20{}", &CENTENARIAN[2..])
        );
    }

    #[test]
    fn a_coordination_number_keeps_its_day_plus_sixty() {
        assert_eq!(normalise_on(COORDINATION, TODAY).unwrap(), COORDINATION);
        assert_eq!(
            normalise_on(
                &format!("{}-{}", &COORDINATION[..8], &COORDINATION[8..]),
                TODAY
            )
            .unwrap(),
            COORDINATION
        );
        // ten digits, born more than a hundred years ago: a plus
        assert_eq!(
            normalise_on(&ten(COORDINATION, '+'), TODAY).unwrap(),
            COORDINATION
        );
        // day 91 is the 31st, which February has not got
        assert_eq!(normalise_on("189002919800", TODAY), Err(Invalid::Date));
    }

    #[test]
    fn what_is_not_a_personnummer_is_refused() {
        assert_eq!(normalise_on("198501012383", TODAY), Err(Invalid::Checksum));
        assert_eq!(normalise_on("850101-2383", TODAY), Err(Invalid::Checksum));
        assert_eq!(normalise_on("", TODAY), Err(Invalid::Shape));
        assert_eq!(normalise_on("TRIAL-0042", TODAY), Err(Invalid::Shape));
        assert_eq!(normalise_on("12345", TODAY), Err(Invalid::Shape));
        assert_eq!(normalise_on("85010123821", TODAY), Err(Invalid::Shape));
        assert_eq!(normalise_on("850101:2382", TODAY), Err(Invalid::Shape));
        assert_eq!(normalise_on("85010--2382", TODAY), Err(Invalid::Shape));
        assert_eq!(normalise_on("19850101-238X", TODAY), Err(Invalid::Shape));
        // month 13, day 0
        assert_eq!(normalise_on("198513012382", TODAY), Err(Invalid::Date));
        assert_eq!(normalise_on("198501002382", TODAY), Err(Invalid::Date));
        // the 29th of February of a year that has none (1900 had none)
        assert_eq!(normalise_on("190002292399", TODAY), Err(Invalid::Date));
        assert_eq!(normalise_on("200102292399", TODAY), Err(Invalid::Date));
    }

    #[test]
    fn only_the_personnummer_type_is_normalised() {
        let dashed = ten(ADULT, '-');
        assert_eq!(for_type("patient-id", &dashed).unwrap(), dashed);
        assert_eq!(for_type("patient-id", "anything").unwrap(), "anything");
        assert_eq!(normalise(&dashed).unwrap(), ADULT);
        assert_eq!(for_type(ID_TYPE, &dashed).unwrap(), ADULT);
        assert_eq!(for_type(ID_TYPE, "anything"), Err(Invalid::Shape));
    }

    /// The subject code generator as written in Python,
    /// `hashlib.blake2b(pn.encode(), key=key.encode(), digest_size=8).hexdigest()`,
    /// of the twelve digits, under a made-up test key. Each vector was
    /// computed with Python's hashlib; every written form of the number must
    /// give it through the generator's scheme.
    #[test]
    fn every_written_form_derives_the_generators_code_of_the_twelve_digits() {
        const KEY: &[u8] = b"test-reg-key-not-real";
        let vectors = [
            (ADULT, "c6d36050d4d0a55b"),
            (CHILD, "97567e4f9035c39b"),
            (CENTENARIAN, "d8604fc9f03635d3"),
            (THIS_YEAR, "ec727c220caa82d8"),
            (TURNS_100, "f977bf3917175e95"),
            (LEAP, "0bdc645f3f1978f4"),
            (COORDINATION, "652381cebe1443e3"),
        ];
        for (pn, expected) in vectors {
            let sep = if pn.starts_with("20") || pn == ADULT {
                '-'
            } else {
                '+'
            };
            let forms = [
                pn.to_string(),
                format!("{}-{}", &pn[..8], &pn[8..]),
                format!("{} {}", &pn[..8], &pn[8..]),
                ten(pn, sep),
            ];
            for written in forms {
                let twelve = normalise_on(&written, TODAY).unwrap();
                let c = pseudonym::code(Scheme::SUBJECT_CODE_GENERATOR, KEY, &twelve, 12);
                assert_eq!(c.code, expected, "{written}");
            }
        }
    }
}
