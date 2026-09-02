// SPDX-License-Identifier: AGPL-3.0-only

//! The quarantine classes (`docs/specs/wave1-parse-and-digest.md`, §5.3): a file
//! that is not ingested gets exactly one of them, and the batch's report counts
//! each.

use std::fmt;

/// Why a file was not ingested. The names are the values of
/// `source_file.reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum QuarantineClass {
    /// No `DICM` marker and no readable bare data set that yields a
    /// SOPInstanceUID.
    NotDicom,
    /// An I/O error opening or reading.
    Unreadable,
    /// The reader failed inside the header; `detail` carries the reader's kind
    /// and error chain.
    ParseError,
    /// No StudyInstanceUID, SeriesInstanceUID, SOPInstanceUID or SOPClassUID;
    /// `detail` names the first one missing.
    MissingUid,
    /// A SOP class outside the batch's `sop_classes` knob; `detail` is the UID.
    UnsupportedSopClass,
    /// No Modality and no single-valued ModalitiesInStudy to fall back on.
    MissingModality,
    /// A modality outside the batch's `modalities` knob; `detail` is the value.
    UnsupportedModality,
}

impl QuarantineClass {
    /// Every class, in the order the report prints them.
    pub const ALL: [QuarantineClass; 7] = [
        QuarantineClass::NotDicom,
        QuarantineClass::Unreadable,
        QuarantineClass::ParseError,
        QuarantineClass::MissingUid,
        QuarantineClass::UnsupportedSopClass,
        QuarantineClass::MissingModality,
        QuarantineClass::UnsupportedModality,
    ];

    /// The name as written in `source_file.reason` and the report.
    pub fn name(self) -> &'static str {
        match self {
            QuarantineClass::NotDicom => "not_dicom",
            QuarantineClass::Unreadable => "unreadable",
            QuarantineClass::ParseError => "parse_error",
            QuarantineClass::MissingUid => "missing_uid",
            QuarantineClass::UnsupportedSopClass => "unsupported_sop_class",
            QuarantineClass::MissingModality => "missing_modality",
            QuarantineClass::UnsupportedModality => "unsupported_modality",
        }
    }

    /// True for the classes the reader decides before any policy applies: the
    /// spike's harness counted exactly these.
    pub fn is_reader_class(self) -> bool {
        matches!(
            self,
            QuarantineClass::NotDicom
                | QuarantineClass::Unreadable
                | QuarantineClass::ParseError
                | QuarantineClass::MissingUid
        )
    }
}

impl fmt::Display for QuarantineClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A refusal: the class and the detail that goes with it into
/// `source_file.detail`. The detail never carries a value from the file other
/// than a UID, a modality code, a tag keyword or the reader's error text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub class: QuarantineClass,
    pub detail: Option<String>,
}

impl Refusal {
    pub fn new(class: QuarantineClass, detail: impl Into<Option<String>>) -> Self {
        Refusal {
            class,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "{}: {d}", self.class),
            None => f.write_str(self.class.name()),
        }
    }
}
