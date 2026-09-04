// SPDX-License-Identifier: AGPL-3.0-only

//! Normalization (`docs/specs/wave1-parse-and-digest.md`, §6.3): one converter
//! per logical type, and the normal form written down because the gate compares
//! against it.
//!
//! The parser keeps DA, TM, DS and IS as the strings they were on disk (the
//! `Preserved` value strategy), so a malformed value never fails a file: it
//! fails its own column, as null plus a `value_invalid` diagnostic.

use std::fmt;

use dicom_core::VR;
use dicom_core::{DicomValue, PrimitiveValue};
use dicom_object::InMemDicomObject;
use dicom_object::mem::InMemElement;
use serde::Serialize;

use crate::charset::Charset;

/// A converted value, ready for its column.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    Text(String),
    Int(i64),
    Double(f64),
    /// `YYYY-MM-DD`.
    Date(String),
    /// `HH:MM:SS` or `HH:MM:SS.ffffff`.
    Time(String),
    /// The DICOM JSON model of the element, serialized.
    Json(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Text(s) | Value::Date(s) | Value::Time(s) | Value::Json(s) => f.write_str(s),
            Value::Int(i) => write!(f, "{i}"),
            Value::Double(d) => write!(f, "{d}"),
        }
    }
}

/// The converters of the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Converter {
    Text,
    Int,
    Double,
    Date,
    Time,
    Json,
}

impl Converter {
    pub fn name(self) -> &'static str {
        match self {
            Converter::Text => "text",
            Converter::Int => "int",
            Converter::Double => "double",
            Converter::Date => "date",
            Converter::Time => "time",
            Converter::Json => "json",
        }
    }
}

impl fmt::Display for Converter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The result of converting one element.
#[derive(Debug, Default, PartialEq)]
pub struct Conversion {
    /// The value, or null.
    pub value: Option<Value>,
    /// The raw text of a value the converter refused (for the diagnostic's
    /// shape), when it refused one.
    pub invalid: Option<String>,
    /// True when a byte of the value could not be decoded.
    pub lossy: bool,
}

impl Conversion {
    fn null() -> Self {
        Conversion::default()
    }

    fn of(value: Value) -> Self {
        Conversion {
            value: Some(value),
            ..Default::default()
        }
    }

    fn invalid(raw: impl Into<String>) -> Self {
        Conversion {
            invalid: Some(raw.into()),
            ..Default::default()
        }
    }
}

/// Convert one element with the given converter under the file's charset.
pub fn convert(converter: Converter, elem: &InMemElement, charset: &Charset) -> Conversion {
    match elem.value() {
        DicomValue::Primitive(p) => convert_primitive(converter, p, elem.vr(), charset),
        DicomValue::Sequence(seq) => match converter {
            Converter::Json => convert_items(seq.items()),
            _ => {
                if seq.items().is_empty() {
                    Conversion::null()
                } else {
                    Conversion::invalid(format!("<sequence of {} items>", seq.items().len()))
                }
            }
        },
        DicomValue::PixelSequence(_) => Conversion::invalid("<pixel sequence>"),
    }
}

/// Convert a primitive value.
pub fn convert_primitive(
    converter: Converter,
    p: &PrimitiveValue,
    vr: VR,
    charset: &Charset,
) -> Conversion {
    if matches!(p, PrimitiveValue::Empty) || p.multiplicity() == 0 {
        return Conversion::null();
    }
    match converter {
        Converter::Text => text(p, vr, charset),
        Converter::Int => int(p),
        Converter::Double => double(p),
        Converter::Date => date(p),
        Converter::Time => time(p),
        Converter::Json => Conversion::invalid("<not a sequence>"),
    }
}

/// The parts of a string value, trailing spaces and NULs trimmed, the charset
/// applied. `None` for a value that holds bytes.
fn string_parts(p: &PrimitiveValue, vr: VR, charset: &Charset) -> Option<(Vec<String>, bool)> {
    let mut lossy = false;
    let parts: Vec<String> = match p {
        PrimitiveValue::Strs(parts) => parts
            .iter()
            .map(|s| {
                let t = charset.text(s, vr);
                lossy |= t.lossy;
                trim_end(&t.value).to_string()
            })
            .collect(),
        PrimitiveValue::Str(s) => {
            let t = charset.text(s, vr);
            lossy |= t.lossy;
            vec![trim_end(&t.value).to_string()]
        }
        PrimitiveValue::U8(_) => return None,
        other => other.to_str().split('\\').map(str::to_string).collect(),
    };
    Some((parts, lossy))
}

fn trim_end(s: &str) -> &str {
    s.trim_end_matches([' ', '\0'])
}

fn text(p: &PrimitiveValue, vr: VR, charset: &Charset) -> Conversion {
    match string_parts(p, vr, charset) {
        Some((parts, lossy)) => {
            let joined = parts.join("\\");
            let mut c = if joined.is_empty() {
                Conversion::null()
            } else {
                Conversion::of(Value::Text(joined))
            };
            c.lossy = lossy;
            c
        }
        None => Conversion::invalid(format!("<{} bytes>", p.calculate_byte_len())),
    }
}

/// The single string of a value that must have exactly one, or the raw text of
/// the whole value when it has more.
fn single(p: &PrimitiveValue) -> Result<Option<String>, String> {
    match p {
        PrimitiveValue::Strs(parts) => {
            let trimmed: Vec<&str> = parts.iter().map(|s| s.trim_matches([' ', '\0'])).collect();
            let non_empty: Vec<&str> = trimmed.iter().copied().filter(|s| !s.is_empty()).collect();
            match non_empty.as_slice() {
                [] => Ok(None),
                [one] => Ok(Some((*one).to_string())),
                _ => Err(trimmed.join("\\")),
            }
        }
        PrimitiveValue::Str(s) => {
            let t = s.trim_matches([' ', '\0']);
            Ok((!t.is_empty()).then(|| t.to_string()))
        }
        PrimitiveValue::U8(_) => Err(format!("<{} bytes>", p.calculate_byte_len())),
        other => {
            if other.multiplicity() == 1 {
                Ok(Some(other.to_str().into_owned()))
            } else {
                Err(other.to_str().into_owned())
            }
        }
    }
}

fn int(p: &PrimitiveValue) -> Conversion {
    match p {
        PrimitiveValue::U16(v) if v.len() == 1 => return Conversion::of(Value::Int(v[0].into())),
        PrimitiveValue::I16(v) if v.len() == 1 => return Conversion::of(Value::Int(v[0].into())),
        PrimitiveValue::U32(v) if v.len() == 1 => return Conversion::of(Value::Int(v[0].into())),
        PrimitiveValue::I32(v) if v.len() == 1 => return Conversion::of(Value::Int(v[0].into())),
        PrimitiveValue::I64(v) if v.len() == 1 => return Conversion::of(Value::Int(v[0])),
        PrimitiveValue::U64(v) if v.len() == 1 => {
            return match i64::try_from(v[0]) {
                Ok(i) => Conversion::of(Value::Int(i)),
                Err(_) => Conversion::invalid(v[0].to_string()),
            };
        }
        _ => {}
    }
    match single(p) {
        Ok(None) => Conversion::null(),
        Ok(Some(s)) => match parse_int(&s) {
            Some(i) => Conversion::of(Value::Int(i)),
            None => Conversion::invalid(s),
        },
        Err(raw) => Conversion::invalid(raw),
    }
}

/// An integer from a decimal string: an optional sign, digits, and at most a
/// fraction of zeros (`12`, `+12`, `12.0`).
pub fn parse_int(s: &str) -> Option<i64> {
    let s = s.trim();
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    if let Some(f) = frac
        && !f.bytes().all(|b| b == b'0')
    {
        return None;
    }
    let digits = int_part.strip_prefix(['+', '-']).unwrap_or(int_part);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    int_part.parse().ok()
}

fn double(p: &PrimitiveValue) -> Conversion {
    match p {
        PrimitiveValue::F32(v) if v.len() == 1 => {
            return Conversion::of(Value::Double(v[0].into()));
        }
        PrimitiveValue::F64(v) if v.len() == 1 => return Conversion::of(Value::Double(v[0])),
        PrimitiveValue::U16(v) if v.len() == 1 => {
            return Conversion::of(Value::Double(v[0].into()));
        }
        PrimitiveValue::I16(v) if v.len() == 1 => {
            return Conversion::of(Value::Double(v[0].into()));
        }
        PrimitiveValue::U32(v) if v.len() == 1 => {
            return Conversion::of(Value::Double(v[0].into()));
        }
        PrimitiveValue::I32(v) if v.len() == 1 => {
            return Conversion::of(Value::Double(v[0].into()));
        }
        _ => {}
    }
    match single(p) {
        Ok(None) => Conversion::null(),
        Ok(Some(s)) => match parse_double(&s) {
            Some(d) => Conversion::of(Value::Double(d)),
            None => Conversion::invalid(s),
        },
        Err(raw) => Conversion::invalid(raw),
    }
}

/// A double from a decimal string the way DS writes it; `inf` and `nan` are
/// refused.
pub fn parse_double(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b))
    {
        return None;
    }
    s.parse::<f64>().ok().filter(|d| d.is_finite())
}

fn date(p: &PrimitiveValue) -> Conversion {
    match single(p) {
        Ok(None) => Conversion::null(),
        Ok(Some(s)) => match normalize_date(&s) {
            Some(d) => Conversion::of(Value::Date(d)),
            None => Conversion::invalid(s),
        },
        Err(raw) => Conversion::invalid(raw),
    }
}

/// Whether `y-m-d` is a day that exists. Eight digits are not a date: a
/// scanner writes `00000000` to mean nothing, and month thirteen means the
/// value is junk rather than a day (Wave 3 §4.2).
fn is_a_day(y: i32, m: u32, d: u32) -> bool {
    if !(1..=12).contains(&m) || d == 0 {
        return false;
    }
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    };
    d <= days
}

fn day_of(y: &str, m: &str, d: &str) -> bool {
    match (y.parse::<i32>(), m.parse::<u32>(), d.parse::<u32>()) {
        (Ok(y), Ok(m), Ok(d)) => is_a_day(y, m, d),
        _ => false,
    }
}

/// `YYYYMMDD` to `YYYY-MM-DD`; an ISO value passes; anything else is refused,
/// and so is anything that is not a day.
pub fn normalize_date(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() == 8 && b.iter().all(u8::is_ascii_digit) {
        if !day_of(&s[..4], &s[4..6], &s[6..8]) {
            return None;
        }
        return Some(format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..8]));
    }
    if b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..].iter().all(u8::is_ascii_digit)
    {
        if !day_of(&s[..4], &s[5..7], &s[8..]) {
            return None;
        }
        return Some(s.to_string());
    }
    None
}

fn time(p: &PrimitiveValue) -> Conversion {
    match single(p) {
        Ok(None) => Conversion::null(),
        Ok(Some(s)) => match normalize_time(&s) {
            Some(t) => Conversion::of(Value::Time(t)),
            None => Conversion::invalid(s),
        },
        Err(raw) => Conversion::invalid(raw),
    }
}

/// `HHMMSS[.f]` to `HH:MM:SS.ffffff`, the fraction padded to six digits when
/// present; `HH:MM:SS[.f]` passes the same way; anything else is refused.
pub fn normalize_time(s: &str) -> Option<String> {
    let s = s.trim();
    let (hms, frac) = match s.split_once('.') {
        Some((h, f)) => (h, Some(f)),
        None => (s, None),
    };
    let b = hms.as_bytes();
    let (hh, mm, ss) = if b.len() == 6 && b.iter().all(u8::is_ascii_digit) {
        (&hms[..2], &hms[2..4], &hms[4..6])
    } else if b.len() == 8
        && b[2] == b':'
        && b[5] == b':'
        && b[..2].iter().all(u8::is_ascii_digit)
        && b[3..5].iter().all(u8::is_ascii_digit)
        && b[6..].iter().all(u8::is_ascii_digit)
    {
        (&hms[..2], &hms[3..5], &hms[6..8])
    } else {
        return None;
    };
    match frac {
        None => Some(format!("{hh}:{mm}:{ss}")),
        Some(f) => {
            if f.is_empty() || f.len() > 6 || !f.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            Some(format!("{hh}:{mm}:{ss}.{f:0<6}"))
        }
    }
}

/// The DICOM JSON model of a sequence's items, as an array.
fn convert_items(items: &[InMemDicomObject]) -> Conversion {
    if items.is_empty() {
        return Conversion::null();
    }
    match dicom_json::to_string(items) {
        Ok(json) => Conversion::of(Value::Json(json)),
        Err(e) => Conversion::invalid(format!("<json: {e}>")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::smallvec::smallvec;

    fn strs(parts: &[&str]) -> PrimitiveValue {
        PrimitiveValue::Strs(parts.iter().map(|s| s.to_string()).collect())
    }

    fn cs() -> Charset {
        Charset::resolve(None)
    }

    #[test]
    fn text_joins_and_trims() {
        let c = convert_primitive(
            Converter::Text,
            &strs(&["ORIGINAL ", "PRIMARY\0"]),
            VR::CS,
            &cs(),
        );
        assert_eq!(c.value, Some(Value::Text("ORIGINAL\\PRIMARY".into())));
        let c = convert_primitive(Converter::Text, &strs(&["  lead"]), VR::LO, &cs());
        assert_eq!(c.value, Some(Value::Text("  lead".into())));
        let c = convert_primitive(Converter::Text, &strs(&[""]), VR::LO, &cs());
        assert_eq!(c, Conversion::null());
        let c = convert_primitive(
            Converter::Text,
            &PrimitiveValue::U16(smallvec![1, 2]),
            VR::US,
            &cs(),
        );
        assert_eq!(c.value, Some(Value::Text("1\\2".into())));
    }

    #[test]
    fn bytes_are_invalid_text() {
        let c = convert_primitive(
            Converter::Text,
            &PrimitiveValue::U8(smallvec![1, 2, 3]),
            VR::OB,
            &cs(),
        );
        assert_eq!(c.value, None);
        assert_eq!(c.invalid.as_deref(), Some("<3 bytes>"));
    }

    #[test]
    fn ints() {
        assert_eq!(parse_int("12"), Some(12));
        assert_eq!(parse_int(" +12 "), Some(12));
        assert_eq!(parse_int("-3"), Some(-3));
        assert_eq!(parse_int("12.0"), Some(12));
        assert_eq!(parse_int("12.5"), None);
        assert_eq!(parse_int("1e3"), None);
        assert_eq!(parse_int("abc"), None);
        assert_eq!(parse_int(""), None);
        let c = convert_primitive(Converter::Int, &strs(&["7 "]), VR::IS, &cs());
        assert_eq!(c.value, Some(Value::Int(7)));
        let c = convert_primitive(Converter::Int, &strs(&["1", "2"]), VR::IS, &cs());
        assert_eq!(c.value, None);
        assert_eq!(c.invalid.as_deref(), Some("1\\2"));
        let c = convert_primitive(
            Converter::Int,
            &PrimitiveValue::U16(smallvec![512]),
            VR::US,
            &cs(),
        );
        assert_eq!(c.value, Some(Value::Int(512)));
        let c = convert_primitive(
            Converter::Int,
            &PrimitiveValue::U16(smallvec![1, 2]),
            VR::US,
            &cs(),
        );
        assert_eq!(c.invalid.as_deref(), Some("1\\2"));
        let c = convert_primitive(Converter::Int, &strs(&["", ""]), VR::IS, &cs());
        assert_eq!(c, Conversion::null());
    }

    #[test]
    fn doubles() {
        assert_eq!(parse_double("1.5"), Some(1.5));
        assert_eq!(parse_double("1.5E+02"), Some(150.0));
        assert_eq!(parse_double("-0"), Some(-0.0));
        assert_eq!(parse_double("inf"), None);
        assert_eq!(parse_double("1,5"), None);
        let c = convert_primitive(Converter::Double, &strs(&["2.5", "3"]), VR::DS, &cs());
        assert_eq!(c.value, None);
        assert_eq!(c.invalid.as_deref(), Some("2.5\\3"));
        let c = convert_primitive(
            Converter::Double,
            &PrimitiveValue::F64(smallvec![0.25]),
            VR::FD,
            &cs(),
        );
        assert_eq!(c.value, Some(Value::Double(0.25)));
        let c = convert_primitive(
            Converter::Double,
            &PrimitiveValue::F32(smallvec![0.5]),
            VR::FL,
            &cs(),
        );
        assert_eq!(c.value, Some(Value::Double(0.5)));
    }

    #[test]
    fn dates() {
        assert_eq!(normalize_date("20240131").as_deref(), Some("2024-01-31"));
        assert_eq!(normalize_date("2024-01-31 ").as_deref(), Some("2024-01-31"));
        assert_eq!(normalize_date("202401"), None);
        assert_eq!(normalize_date("2024.01.31"), None);
        assert_eq!(normalize_date(""), None);
    }

    #[test]
    fn times() {
        assert_eq!(normalize_time("091530").as_deref(), Some("09:15:30"));
        assert_eq!(
            normalize_time("091530.5").as_deref(),
            Some("09:15:30.500000")
        );
        assert_eq!(
            normalize_time("091530.123456").as_deref(),
            Some("09:15:30.123456")
        );
        assert_eq!(normalize_time("09:15:30").as_deref(), Some("09:15:30"));
        assert_eq!(
            normalize_time("09:15:30.25").as_deref(),
            Some("09:15:30.250000")
        );
        assert_eq!(normalize_time("0915"), None);
        assert_eq!(normalize_time("091530."), None);
        assert_eq!(normalize_time("091530.1234567"), None);
        assert_eq!(normalize_time("noon"), None);
    }
}
