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

/// What a file NILS set aside held, in words a reader of a digest knows
/// (record 37, S9). About one file in a hundred of a study directory is an
/// object NILS quarantines, and `unsupported_sop_class` with a UID beside it
/// tells nobody that the radiologist's annotations were in a presentation
/// state or the measurements in a report.
pub fn set_aside_kind(class: QuarantineClass, detail: Option<&str>) -> &'static str {
    match class {
        QuarantineClass::NotDicom => "not DICOM",
        QuarantineClass::Unreadable => "unreadable",
        QuarantineClass::ParseError => "a broken header",
        QuarantineClass::MissingUid => "no identifier",
        QuarantineClass::MissingModality => "no modality",
        QuarantineClass::UnsupportedModality => "another modality",
        QuarantineClass::UnsupportedSopClass => sop_class_kind(detail.unwrap_or_default()),
    }
}

/// The family a SOP class belongs to, for a reader who wants to know what the
/// archive held rather than which UID it wore.
pub fn sop_class_kind(uid: &str) -> &'static str {
    let root = "1.2.840.10008.5.1.4.1.1.";
    let Some(rest) = uid.strip_prefix(root) else {
        return match uid.starts_with("1.2.840.10008") {
            true => "another standard class",
            false => "a private class",
        };
    };
    match rest {
        "7" | "7.1" | "7.2" | "7.3" | "7.4" => "secondary capture",
        "11.1" | "11.2" | "11.3" | "11.4" | "11.5" | "11.6" | "11.7" => "presentation state",
        "88.59" => "key object selection",
        r if r.starts_with("88.") => "structured report",
        "66" => "raw data",
        "66.1" | "66.3" => "registration",
        "66.2" => "fiducials",
        "66.4" | "66.5" => "segmentation",
        r if r.starts_with("104.") => "encapsulated document",
        r if r.starts_with("9.") => "waveform",
        r if r.starts_with("481.") => "radiotherapy",
        "30" | "30.1" => "parametric map",
        _ => "another image class",
    }
}

/// The standard's name for a SOP class NILS does not accept, when it knows
/// it. The accepted nine are named by [`crate::extract::sop_class_name`].
pub fn refused_sop_class_name(uid: &str) -> Option<&'static str> {
    Some(match uid {
        "1.2.840.10008.5.1.4.1.1.7" => "Secondary Capture",
        "1.2.840.10008.5.1.4.1.1.7.1" => "Multi-frame Single Bit Secondary Capture",
        "1.2.840.10008.5.1.4.1.1.7.2" => "Multi-frame Grayscale Byte Secondary Capture",
        "1.2.840.10008.5.1.4.1.1.7.3" => "Multi-frame Grayscale Word Secondary Capture",
        "1.2.840.10008.5.1.4.1.1.7.4" => "Multi-frame True Colour Secondary Capture",
        "1.2.840.10008.5.1.4.1.1.11.1" => "Grayscale Softcopy Presentation State",
        "1.2.840.10008.5.1.4.1.1.11.2" => "Colour Softcopy Presentation State",
        "1.2.840.10008.5.1.4.1.1.11.3" => "Pseudo-Colour Softcopy Presentation State",
        "1.2.840.10008.5.1.4.1.1.11.4" => "Blending Softcopy Presentation State",
        "1.2.840.10008.5.1.4.1.1.30" => "Parametric Map",
        "1.2.840.10008.5.1.4.1.1.66" => "Raw Data",
        "1.2.840.10008.5.1.4.1.1.66.1" => "Spatial Registration",
        "1.2.840.10008.5.1.4.1.1.66.2" => "Spatial Fiducials",
        "1.2.840.10008.5.1.4.1.1.66.3" => "Deformable Spatial Registration",
        "1.2.840.10008.5.1.4.1.1.66.4" => "Segmentation",
        "1.2.840.10008.5.1.4.1.1.66.5" => "Surface Segmentation",
        "1.2.840.10008.5.1.4.1.1.88.11" => "Basic Text SR",
        "1.2.840.10008.5.1.4.1.1.88.22" => "Enhanced SR",
        "1.2.840.10008.5.1.4.1.1.88.33" => "Comprehensive SR",
        "1.2.840.10008.5.1.4.1.1.88.34" => "Comprehensive 3D SR",
        "1.2.840.10008.5.1.4.1.1.88.59" => "Key Object Selection Document",
        "1.2.840.10008.5.1.4.1.1.104.1" => "Encapsulated PDF",
        "1.2.840.10008.5.1.4.1.1.104.2" => "Encapsulated CDA",
        "1.2.840.10008.5.1.4.1.1.104.3" => "Encapsulated STL",
        _ => return None,
    })
}

/// How a SOP class is named in a report: the standard's name where NILS has
/// it, accepted or not, and the UID where it has not.
pub fn sop_class_label(uid: &str) -> String {
    crate::extract::sop_class_name(uid)
        .or_else(|| refused_sop_class_name(uid))
        .unwrap_or(uid)
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_aside_file_is_named_by_kind() {
        // the objects the survey found in a study directory (record 37, S9)
        let kinds = |uid: &str| set_aside_kind(QuarantineClass::UnsupportedSopClass, Some(uid));
        assert_eq!(kinds("1.2.840.10008.5.1.4.1.1.7"), "secondary capture");
        assert_eq!(kinds("1.2.840.10008.5.1.4.1.1.7.2"), "secondary capture");
        assert_eq!(kinds("1.2.840.10008.5.1.4.1.1.11.1"), "presentation state");
        assert_eq!(kinds("1.2.840.10008.5.1.4.1.1.88.22"), "structured report");
        assert_eq!(kinds("1.2.840.10008.5.1.4.1.1.88.11"), "structured report");
        assert_eq!(
            kinds("1.2.840.10008.5.1.4.1.1.88.59"),
            "key object selection"
        );
        assert_eq!(kinds("1.2.840.10008.5.1.4.1.1.66.1"), "registration");
        assert_eq!(kinds("1.2.840.10008.5.1.4.1.1.66"), "raw data");
        assert_eq!(
            kinds("1.2.840.10008.5.1.4.1.1.104.1"),
            "encapsulated document"
        );
        assert_eq!(kinds("1.2.840.10008.5.1.4.1.1.6.1"), "another image class");
        assert_eq!(kinds("1.2.840.10008.1.9"), "another standard class");
        // a vendor's own class, which one site's archive holds twelve of
        assert_eq!(kinds("1.3.46.670589.11.0.0.12.2"), "a private class");
        assert_eq!(kinds(""), "a private class");
        // the classes the reader decides, and the knobs
        assert_eq!(set_aside_kind(QuarantineClass::NotDicom, None), "not DICOM");
        assert_eq!(
            set_aside_kind(QuarantineClass::MissingUid, Some("SOPInstanceUID")),
            "no identifier"
        );
        assert_eq!(
            set_aside_kind(QuarantineClass::UnsupportedModality, Some("US")),
            "another modality"
        );
    }

    #[test]
    fn a_class_is_labelled_by_name_where_nils_knows_one() {
        assert_eq!(
            sop_class_label("1.2.840.10008.5.1.4.1.1.104.1"),
            "Encapsulated PDF"
        );
        assert_eq!(
            sop_class_label("1.2.840.10008.5.1.4.1.1.4"),
            "MR Image Storage"
        );
        assert_eq!(sop_class_label("1.2.3.4"), "1.2.3.4");
    }
}
