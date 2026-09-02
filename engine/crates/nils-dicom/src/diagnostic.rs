// SPDX-License-Identifier: AGPL-3.0-only

//! Diagnostics (`docs/specs/wave1-parse-and-digest.md`, §11): what is odd but
//! not refused. A diagnostic is counted per batch and kind, with a sample of at
//! most ten *shapes*; a shape keeps the form of a value and drops its content,
//! so no sample carries a name, a date or a comment.

use std::fmt;

/// The kinds of Wave 1. The reader raises the first four; the rest belong to the
/// identity, signature and writer stages of later slices and are declared here
/// so that the report has one list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DiagnosticKind {
    WalkError,
    CharsetUnknown,
    CharsetLossy,
    ValueInvalid,
    FieldDisagreement,
    IdentityUnparsed,
    IdentityFallback,
    SubjectFieldDisagreement,
    FileChanged,
    OrientationOblique,
    SeriesMultiStudy,
}

impl DiagnosticKind {
    /// Every kind, in the order the report prints them.
    pub const ALL: [DiagnosticKind; 11] = [
        DiagnosticKind::WalkError,
        DiagnosticKind::CharsetUnknown,
        DiagnosticKind::CharsetLossy,
        DiagnosticKind::ValueInvalid,
        DiagnosticKind::FieldDisagreement,
        DiagnosticKind::IdentityUnparsed,
        DiagnosticKind::IdentityFallback,
        DiagnosticKind::SubjectFieldDisagreement,
        DiagnosticKind::FileChanged,
        DiagnosticKind::OrientationOblique,
        DiagnosticKind::SeriesMultiStudy,
    ];

    /// The name as written in `diagnostic.kind` and the report.
    pub fn name(self) -> &'static str {
        match self {
            DiagnosticKind::WalkError => "walk_error",
            DiagnosticKind::CharsetUnknown => "charset_unknown",
            DiagnosticKind::CharsetLossy => "charset_lossy",
            DiagnosticKind::ValueInvalid => "value_invalid",
            DiagnosticKind::FieldDisagreement => "field_disagreement",
            DiagnosticKind::IdentityUnparsed => "identity_unparsed",
            DiagnosticKind::IdentityFallback => "identity_fallback",
            DiagnosticKind::SubjectFieldDisagreement => "subject_field_disagreement",
            DiagnosticKind::FileChanged => "file_changed",
            DiagnosticKind::OrientationOblique => "orientation_oblique",
            DiagnosticKind::SeriesMultiStudy => "series_multi_study",
        }
    }
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One diagnostic raised while reading one file. `subject` names what it is
/// about (a column, a tag keyword, a character set code); `shape` is the shape
/// of the offending value, when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub subject: String,
    pub shape: Option<String>,
}

impl Diagnostic {
    pub fn new(kind: DiagnosticKind, subject: impl Into<String>) -> Self {
        Diagnostic {
            kind,
            subject: subject.into(),
            shape: None,
        }
    }

    pub fn with_shape(mut self, value: &str) -> Self {
        self.shape = Some(shape(value));
        self
    }

    /// The sample as the report prints it: the subject, then the shape.
    pub fn sample(&self) -> String {
        match &self.shape {
            Some(s) => format!("{}={s}", self.subject),
            None => self.subject.clone(),
        }
    }
}

/// The length a sample shape is capped at.
pub const SHAPE_MAX: usize = 40;

/// The shape of a value: digits become `9`, lower-case letters `a`, upper-case
/// letters `A`, other characters stay (so `\`, `.`, `:` and `-` show the form),
/// capped at [`SHAPE_MAX`] characters with a trailing `…`.
pub fn shape(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(SHAPE_MAX + 3));
    for (i, c) in value.chars().enumerate() {
        if i == SHAPE_MAX {
            out.push('…');
            break;
        }
        out.push(match c {
            '0'..='9' => '9',
            c if c.is_lowercase() => 'a',
            c if c.is_uppercase() => 'A',
            c if c.is_alphabetic() => 'a',
            c => c,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_hides_content_and_keeps_form() {
        assert_eq!(shape("20240131"), "99999999");
        assert_eq!(shape("Doe^John"), "Aaa^Aaaa");
        assert_eq!(shape("1.5\\2.25"), "9.9\\9.99");
        assert_eq!(shape("ISO IR 100"), "AAA AA 999");
        assert_eq!(shape("Åke"), "Aaa");
    }

    #[test]
    fn shape_is_capped() {
        let long = "x".repeat(100);
        let s = shape(&long);
        assert_eq!(s.chars().count(), SHAPE_MAX + 1);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn names_are_snake_case_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in DiagnosticKind::ALL {
            assert!(seen.insert(k.name()));
            assert!(
                k.name()
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_')
            );
        }
    }
}
