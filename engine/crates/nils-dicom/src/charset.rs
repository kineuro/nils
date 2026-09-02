// SPDX-License-Identifier: AGPL-3.0-only

//! The character set of a file, and its repair
//! (`docs/specs/wave1-parse-and-digest.md`, §6.1).
//!
//! `dicom-rs` applies SpecificCharacterSet (0008,0005) to the text VRs (LO, LT,
//! PN, SH, ST, UC, UT) while it parses, and when it does not recognize the code
//! it keeps the default repertoire, which it decodes as ISO-8859-1: a lossless
//! map of every byte to one character. So a misspelled code (`ISO IR 100`, which
//! the Go spike tripped over 1,771 times in the nmosd corpus) costs nothing but
//! a second look: the code is normalized after the parse, and when the
//! normalized code is known and is not ISO-8859-1 itself, the text values are
//! re-decoded from the bytes they preserve. A code that is still unknown after
//! normalization is a `charset_unknown` diagnostic and the values stay as
//! ISO-8859-1 read them, which is the `charset_fallback` knob's default.
//!
//! A codec that meets bytes it cannot decode writes them as `\ooo` octal
//! escapes; NILS replaces those with U+FFFD and counts `charset_lossy`, so a
//! stored value never pretends to be what it is not.

use std::borrow::Cow;

use dicom_core::VR;
use dicom_encoding::text::{SpecificCharacterSet, TextCodec};

/// The character set of one file, as declared and as understood.
#[derive(Debug, Clone)]
pub struct Charset {
    /// (0008,0005) as written: trimmed, values joined by `\`; `None` when the
    /// element is absent or empty.
    pub declared: Option<String>,
    resolution: Resolution,
}

#[derive(Debug, Clone)]
enum Resolution {
    /// Absent, or a code `dicom-rs` applied while parsing: the values are what
    /// they should be.
    Applied { may_escape: bool },
    /// The parser did not know the code but its normalized form is known and
    /// is not ISO-8859-1: text values are re-decoded.
    Redecode(SpecificCharacterSet),
    /// Unknown after normalization: values stay as ISO-8859-1 read them.
    Unknown,
}

/// A text value after the character set is applied.
#[derive(Debug)]
pub struct Text<'a> {
    pub value: Cow<'a, str>,
    /// True when a byte could not be decoded and became U+FFFD.
    pub lossy: bool,
}

impl Charset {
    /// Interpret the declared code. `declared` is the value of (0008,0005) as
    /// the parser stored it (its parts joined by `\`), or `None`.
    pub fn resolve(declared: Option<&str>) -> Charset {
        let declared = declared.map(str::trim).filter(|s| !s.is_empty());
        let Some(raw) = declared else {
            return Charset {
                declared: None,
                resolution: Resolution::Applied { may_escape: false },
            };
        };
        // dicom-rs looks at the first value only, trimmed at the end.
        let first = raw.split('\\').next().unwrap_or("");
        let resolution = match SpecificCharacterSet::from_code(first) {
            Some(cs) => Resolution::Applied {
                may_escape: !is_latin1(&cs),
            },
            None => {
                let candidate = raw
                    .split('\\')
                    .map(normalize_code)
                    .find(|c| !c.is_empty())
                    .unwrap_or_default();
                match SpecificCharacterSet::from_code(&candidate) {
                    Some(cs) if is_latin1(&cs) => Resolution::Applied { may_escape: false },
                    Some(cs) => Resolution::Redecode(cs),
                    None => Resolution::Unknown,
                }
            }
        };
        Charset {
            declared: Some(raw.to_string()),
            resolution,
        }
    }

    /// True when the declared code could not be understood.
    pub fn is_unknown(&self) -> bool {
        matches!(self.resolution, Resolution::Unknown)
    }

    /// True when the values will be re-decoded from their preserved bytes.
    pub fn redecodes(&self) -> bool {
        matches!(self.resolution, Resolution::Redecode(_))
    }

    /// The code as normalized, when it was understood.
    pub fn normalized(&self) -> Option<Cow<'static, str>> {
        match &self.resolution {
            Resolution::Redecode(cs) => Some(cs.name()),
            Resolution::Applied { .. } => self.declared.as_deref().and_then(|d| {
                SpecificCharacterSet::from_code(d.split('\\').next().unwrap_or(""))
                    .map(|cs| cs.name())
                    .or(Some(Cow::Borrowed("ISO_IR 100")))
            }),
            Resolution::Unknown => None,
        }
    }

    /// Apply the character set to one text value of the given VR.
    pub fn text<'a>(&self, value: &'a str, vr: VR) -> Text<'a> {
        if !is_text_vr(vr) {
            return Text {
                value: Cow::Borrowed(value),
                lossy: false,
            };
        }
        match &self.resolution {
            Resolution::Applied { may_escape: false } | Resolution::Unknown => Text {
                value: Cow::Borrowed(value),
                lossy: false,
            },
            Resolution::Applied { may_escape: true } => replace_escapes(value),
            Resolution::Redecode(cs) => {
                let bytes: Option<Vec<u8>> =
                    value.chars().map(|c| u8::try_from(c as u32).ok()).collect();
                match bytes.as_deref().map(|b| cs.decode(b)) {
                    Some(Ok(decoded)) => {
                        let t = replace_escapes(&decoded);
                        Text {
                            value: Cow::Owned(t.value.into_owned()),
                            lossy: t.lossy,
                        }
                    }
                    _ => Text {
                        value: Cow::Borrowed(value),
                        lossy: true,
                    },
                }
            }
        }
    }
}

fn is_latin1(cs: &SpecificCharacterSet) -> bool {
    *cs == SpecificCharacterSet::ISO_IR_6 || *cs == SpecificCharacterSet::ISO_IR_100
}

/// The VRs the parser decodes with the declared character set.
pub fn is_text_vr(vr: VR) -> bool {
    matches!(
        vr,
        VR::LO | VR::LT | VR::PN | VR::SH | VR::ST | VR::UC | VR::UT
    )
}

/// Normalize one character set code to the spelling of PS3.3 C.12.1.1.2:
/// `ISO_IR 100`, `ISO 2022 IR 100`, `GB18030`. Case, and the separators
/// between `ISO`, `IR` and the number, are forgiven.
pub fn normalize_code(code: &str) -> String {
    let upper = code.trim().to_ascii_uppercase();
    let tokens: Vec<&str> = upper
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|t| !t.is_empty())
        .collect();
    // Split a glued `IR100` into `IR` and `100`.
    let mut parts: Vec<String> = Vec::new();
    for t in tokens {
        if let Some(rest) = t.strip_prefix("IR")
            && !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
        {
            parts.push("IR".to_string());
            parts.push(rest.to_string());
        } else {
            parts.push(t.to_string());
        }
    }
    let p: Vec<&str> = parts.iter().map(String::as_str).collect();
    match p.as_slice() {
        ["ISO", "IR", n] if is_number(n) => format!("ISO_IR {n}"),
        ["ISO", "2022", "IR", n] if is_number(n) => format!("ISO 2022 IR {n}"),
        ["IR", n] if is_number(n) => format!("ISO_IR {n}"),
        ["ISO", "8859", n] | ["ISO8859", n] if is_number(n) => match *n {
            "1" => "ISO_IR 100".to_string(),
            "2" => "ISO_IR 101".to_string(),
            "3" => "ISO_IR 109".to_string(),
            "4" => "ISO_IR 110".to_string(),
            "5" => "ISO_IR 144".to_string(),
            "6" => "ISO_IR 127".to_string(),
            "7" => "ISO_IR 126".to_string(),
            "8" => "ISO_IR 138".to_string(),
            _ => upper,
        },
        ["LATIN1"] | ["LATIN", "1"] | ["ISO", "LATIN", "1"] => "ISO_IR 100".to_string(),
        ["UTF", "8"] | ["UTF8"] => "ISO_IR 192".to_string(),
        _ => p.join(" "),
    }
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Replace the parser's `\ooo` octal escapes with U+FFFD.
fn replace_escapes(value: &str) -> Text<'_> {
    if !value.contains('\\') {
        return Text {
            value: Cow::Borrowed(value),
            lossy: false,
        };
    }
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut lossy = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && (b'0'..=b'3').contains(&bytes[i + 1])
            && (b'0'..=b'7').contains(&bytes[i + 2])
            && (b'0'..=b'7').contains(&bytes[i + 3])
        {
            out.push('\u{FFFD}');
            lossy = true;
            i += 4;
            continue;
        }
        // Copy one character (the escape check above only matched ASCII).
        let ch = value[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    if lossy {
        Text {
            value: Cow::Owned(out),
            lossy: true,
        }
    } else {
        Text {
            value: Cow::Borrowed(value),
            lossy: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_the_spellings_seen_in_the_wild() {
        assert_eq!(normalize_code("ISO IR 100"), "ISO_IR 100");
        assert_eq!(normalize_code("iso_ir 100"), "ISO_IR 100");
        assert_eq!(normalize_code("ISO-IR-100"), "ISO_IR 100");
        assert_eq!(normalize_code("ISO_IR100 "), "ISO_IR 100");
        assert_eq!(normalize_code("ISO 2022 IR 100"), "ISO 2022 IR 100");
        assert_eq!(normalize_code("ISO_2022_IR_87"), "ISO 2022 IR 87");
        assert_eq!(normalize_code("ISO_IR 192"), "ISO_IR 192");
        assert_eq!(normalize_code("utf-8"), "ISO_IR 192");
        assert_eq!(normalize_code("ISO 8859-1"), "ISO_IR 100");
        assert_eq!(normalize_code("GB18030"), "GB18030");
        assert_eq!(normalize_code("Windows-1252"), "WINDOWS 1252");
    }

    #[test]
    fn absent_and_known_codes_are_applied() {
        let c = Charset::resolve(None);
        assert!(!c.is_unknown() && !c.redecodes());
        assert_eq!(c.declared, None);
        let c = Charset::resolve(Some("ISO_IR 100"));
        assert!(!c.is_unknown() && !c.redecodes());
        assert_eq!(c.normalized().as_deref(), Some("ISO_IR 100"));
        let c = Charset::resolve(Some("ISO_IR 192"));
        assert!(!c.is_unknown() && !c.redecodes());
    }

    #[test]
    fn a_misspelled_latin1_needs_no_redecode() {
        let c = Charset::resolve(Some("ISO IR 100"));
        assert!(!c.is_unknown());
        assert!(!c.redecodes());
        assert_eq!(c.declared.as_deref(), Some("ISO IR 100"));
        assert_eq!(c.normalized().as_deref(), Some("ISO_IR 100"));
        assert_eq!(c.text("Åke", VR::PN).value, "Åke");
    }

    #[test]
    fn a_misspelled_utf8_is_redecoded() {
        let c = Charset::resolve(Some("ISO IR 192"));
        assert!(c.redecodes());
        // "Åke" in UTF-8 as ISO-8859-1 read it
        let latin1_view: String = "Åke".bytes().map(|b| b as char).collect();
        let t = c.text(&latin1_view, VR::PN);
        assert_eq!(t.value, "Åke");
        assert!(!t.lossy);
        // CS values were never decoded with the declared set
        assert_eq!(c.text(&latin1_view, VR::CS).value, latin1_view);
    }

    #[test]
    fn undecodable_bytes_become_replacement_characters() {
        let c = Charset::resolve(Some("ISO IR 192"));
        let bad: String = [0xC3u8, 0x28, 0x41].iter().map(|&b| b as char).collect();
        let t = c.text(&bad, VR::LO);
        assert!(t.lossy);
        assert!(t.value.contains('\u{FFFD}'));
        assert!(t.value.ends_with("(A") || t.value.ends_with('A'));
    }

    #[test]
    fn unknown_codes_stay_as_read() {
        let c = Charset::resolve(Some("KLINGON"));
        assert!(c.is_unknown());
        assert_eq!(c.normalized(), None);
        let t = c.text("caf\u{e9}", VR::LO);
        assert_eq!(t.value, "caf\u{e9}");
        assert!(!t.lossy);
    }

    #[test]
    fn multi_valued_declarations_take_the_first_known_code() {
        let c = Charset::resolve(Some("\\ISO 2022 IR 87"));
        assert!(c.redecodes());
        assert_eq!(c.normalized().as_deref(), Some("ISO_IR 87"));
    }

    #[test]
    fn escapes_are_replaced_only_when_they_look_like_the_trap() {
        let t = replace_escapes("a\\303\\251b");
        assert_eq!(t.value, "a\u{FFFD}\u{FFFD}b");
        assert!(t.lossy);
        let t = replace_escapes("C:\\path\\9");
        assert_eq!(t.value, "C:\\path\\9");
        assert!(!t.lossy);
    }
}
