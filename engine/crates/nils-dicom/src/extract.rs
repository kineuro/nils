// SPDX-License-Identifier: AGPL-3.0-only

//! From one file to the catalogue's values (`docs/specs/wave1-parse-and-digest.md`,
//! §5.3 and §6): read the header, decide whether the file is one NILS keeps,
//! resolve every catalogue row through its source and converter, and collect
//! the diagnostics on the way.
//!
//! The acceptance policy is v0's, in v0's order: a file with no
//! StudyInstanceUID, SeriesInstanceUID, SOPInstanceUID or SOPClassUID is
//! `missing_uid`; a SOP class outside the nine v0 accepted is
//! `unsupported_sop_class`; a modality that cannot be read is
//! `missing_modality`; one outside MR, CT and PT is `unsupported_modality`.

use std::path::Path;

use dicom_core::VR;
use dicom_core::{DataDictionary, Tag};
use dicom_core::{DicomValue, PrimitiveValue};
use dicom_dictionary_std::{StandardDataDictionary, tags};
use dicom_object::InMemDicomObject;
use dicom_object::mem::InMemElement;

use crate::catalogue::{
    CATALOGUE, FG_ROOTS, Field, Level, Meta, PRIVATE_PER_FRAME, Source, Special, Step,
};
use crate::charset::Charset;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::read::{Form, Header, ReadFailure, read};
use crate::refusal::{QuarantineClass, Refusal};
use crate::value::{Conversion, Converter, Value, convert};

/// The nine SOP classes v0 accepted, and NILS keeps (§5.3).
pub const SOP_CLASSES: [&str; 9] = [
    "1.2.840.10008.5.1.4.1.1.2",
    "1.2.840.10008.5.1.4.1.1.2.1",
    "1.2.840.10008.5.1.4.1.1.2.2",
    "1.2.840.10008.5.1.4.1.1.4",
    "1.2.840.10008.5.1.4.1.1.4.1",
    "1.2.840.10008.5.1.4.1.1.4.2",
    "1.2.840.10008.5.1.4.1.1.4.4",
    "1.2.840.10008.5.1.4.1.1.128",
    "1.2.840.10008.5.1.4.1.1.128.1",
];

/// The modalities NILS keeps, after normalization.
pub const MODALITIES: [&str; 3] = ["MR", "CT", "PT"];

/// The name of a SOP class, for reports.
pub fn sop_class_name(uid: &str) -> Option<&'static str> {
    Some(match uid {
        "1.2.840.10008.5.1.4.1.1.2" => "CT Image Storage",
        "1.2.840.10008.5.1.4.1.1.2.1" => "Enhanced CT Image Storage",
        "1.2.840.10008.5.1.4.1.1.2.2" => "Legacy Converted Enhanced CT Image Storage",
        "1.2.840.10008.5.1.4.1.1.4" => "MR Image Storage",
        "1.2.840.10008.5.1.4.1.1.4.1" => "Enhanced MR Image Storage",
        "1.2.840.10008.5.1.4.1.1.4.2" => "MR Spectroscopy Storage",
        "1.2.840.10008.5.1.4.1.1.4.4" => "Legacy Converted Enhanced MR Image Storage",
        "1.2.840.10008.5.1.4.1.1.128" => "Positron Emission Tomography Image Storage",
        "1.2.840.10008.5.1.4.1.1.128.1" => "Legacy Converted Enhanced PET Image Storage",
        _ => return None,
    })
}

/// The two identifying elements, read for the subject key and never stored.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Identity {
    pub patient_id: Option<String>,
    pub patient_name: Option<String>,
}

/// One accepted file: its keys, its catalogue values and its diagnostics.
#[derive(Debug)]
pub struct Extracted {
    pub form: Form,
    pub transfer_syntax: String,
    pub study_uid: String,
    pub series_uid: String,
    pub sop_uid: String,
    pub sop_class: String,
    /// Normalized: trimmed, upper case, `PET` read as `PT`.
    pub modality: String,
    pub charset: Charset,
    /// One slot per catalogue row, in catalogue order; a modality level that is
    /// not this file's stays null.
    pub values: Vec<Option<Value>>,
    pub identity: Identity,
    pub diagnostics: Vec<Diagnostic>,
}

impl Extracted {
    /// The value of a column, by level and name.
    pub fn value(&self, level: Level, column: &str) -> Option<&Value> {
        crate::catalogue::index_of(level, column).and_then(|i| self.values[i].as_ref())
    }

    /// The rows of one level with their values.
    pub fn row(&self, level: Level) -> impl Iterator<Item = (&'static Field, Option<&Value>)> {
        crate::catalogue::fields_of(level).map(move |(i, f)| (f, self.values[i].as_ref()))
    }
}

/// Read and extract the file at `path`.
pub fn extract(path: &Path) -> Result<Extracted, Refusal> {
    let header = read(path).map_err(refusal_of)?;
    extract_header(header)
}

/// The quarantine class of a read failure.
pub fn refusal_of(failure: ReadFailure) -> Refusal {
    match failure {
        ReadFailure::Unreadable(text) => Refusal::new(QuarantineClass::Unreadable, Some(text)),
        ReadFailure::NotDicom => Refusal::new(QuarantineClass::NotDicom, None),
        ReadFailure::Parse { .. } => Refusal::new(QuarantineClass::ParseError, failure.detail()),
    }
}

/// Extract from a header already read.
pub fn extract_header(header: Header) -> Result<Extracted, Refusal> {
    let Header {
        form,
        meta,
        dataset,
    } = header;
    let transfer_syntax = match (&meta, form) {
        (Some(m), _) => m.transfer_syntax().to_string(),
        (None, Form::BareExplicit) => crate::read::EXPLICIT_VR_LE.to_string(),
        (None, _) => crate::read::IMPLICIT_VR_LE.to_string(),
    };

    let mut diagnostics = Vec::new();
    let declared = declared_charset(&dataset);
    let charset = Charset::resolve(declared.as_deref());
    if charset.is_unknown()
        && let Some(code) = &declared
    {
        diagnostics.push(Diagnostic::new(
            DiagnosticKind::CharsetUnknown,
            code.clone(),
        ));
    }

    // v0's acceptance, in v0's order.
    let uid = |tag: Tag| text_of(&dataset, tag, &charset);
    let study_uid = uid(tags::STUDY_INSTANCE_UID).ok_or_else(|| missing_uid("StudyInstanceUID"))?;
    let series_uid =
        uid(tags::SERIES_INSTANCE_UID).ok_or_else(|| missing_uid("SeriesInstanceUID"))?;
    let sop_uid = uid(tags::SOP_INSTANCE_UID).ok_or_else(|| missing_uid("SOPInstanceUID"))?;
    let sop_class = uid(tags::SOP_CLASS_UID)
        .or_else(|| {
            meta.as_ref()
                .map(|m| m.media_storage_sop_class_uid().to_string())
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| missing_uid("SOPClassUID"))?;
    if !SOP_CLASSES.contains(&sop_class.as_str()) {
        return Err(Refusal::new(
            QuarantineClass::UnsupportedSopClass,
            Some(sop_class),
        ));
    }
    let modality = modality_of(&dataset, &charset)?;

    let identity = Identity {
        patient_id: text_of(&dataset, tags::PATIENT_ID, &charset),
        patient_name: text_of(&dataset, tags::PATIENT_NAME, &charset),
    };

    let mut values = Vec::with_capacity(CATALOGUE.len());
    for field in CATALOGUE {
        if let Some(only) = field.level.modality()
            && only != modality
        {
            values.push(None);
            continue;
        }
        let conversion = match field.source {
            Source::Tag(tag) => dataset
                .get(tag)
                .map(|e| convert(field.converter, e, &charset))
                .unwrap_or_default(),
            Source::TagOrMeta(tag, which) => {
                let c = dataset
                    .get(tag)
                    .map(|e| convert(field.converter, e, &charset))
                    .unwrap_or_default();
                if c.value.is_some() || c.invalid.is_some() {
                    c
                } else {
                    let fallback = match which {
                        Meta::TransferSyntax => Some(transfer_syntax.clone()),
                        _ => meta.as_ref().and_then(|m| meta_field(m, which)),
                    };
                    Conversion {
                        value: fallback.map(Value::Text),
                        ..Default::default()
                    }
                }
            }
            Source::Chain(steps) => {
                let mut found = Conversion::default();
                for step in steps {
                    if let Some(e) = resolve(&dataset, step) {
                        let c = convert(field.converter, e, &charset);
                        if c.value.is_some() || c.invalid.is_some() {
                            found = c;
                            break;
                        }
                    }
                }
                found
            }
            Source::Special(Special::Modality) => Conversion {
                value: Some(Value::Text(modality.clone())),
                ..Default::default()
            },
            Source::Special(Special::Charset) => Conversion {
                value: declared.clone().map(Value::Text),
                ..Default::default()
            },
            Source::Special(Special::Dwi(d)) => d.read(&dataset, &charset),
            Source::None => Conversion::default(),
        };
        if let Some(raw) = &conversion.invalid {
            diagnostics.push(
                Diagnostic::new(
                    DiagnosticKind::ValueInvalid,
                    format!("{}.{}", field.level, field.column),
                )
                .with_shape(raw),
            );
        }
        if conversion.lossy {
            let mut d = Diagnostic::new(
                DiagnosticKind::CharsetLossy,
                format!("{}.{}", field.level, field.column),
            );
            if let Some(code) = charset.normalized() {
                d.shape = Some(code.into_owned());
            }
            diagnostics.push(d);
        }
        values.push(conversion.value);
    }

    Ok(Extracted {
        form,
        transfer_syntax,
        study_uid,
        series_uid,
        sop_uid,
        sop_class,
        modality,
        charset,
        values,
        identity,
        diagnostics,
    })
}

fn missing_uid(which: &str) -> Refusal {
    Refusal::new(QuarantineClass::MissingUid, Some(which.to_string()))
}

/// SpecificCharacterSet as written: the parts joined by backslash, trailing
/// padding trimmed, or none when the element is absent or empty.
fn declared_charset(dataset: &InMemDicomObject) -> Option<String> {
    let e = dataset.get(tags::SPECIFIC_CHARACTER_SET)?;
    let DicomValue::Primitive(p) = e.value() else {
        return None;
    };
    let parts: Vec<String> = match p {
        PrimitiveValue::Strs(parts) => parts
            .iter()
            .map(|s| s.trim_end_matches([' ', '\0']).to_string())
            .collect(),
        PrimitiveValue::Str(s) => vec![s.trim_end_matches([' ', '\0']).to_string()],
        PrimitiveValue::U8(bytes) => bytes
            .split(|&b| b == b'\\')
            .map(|b| {
                b.iter()
                    .map(|&b| b as char)
                    .collect::<String>()
                    .trim_end_matches([' ', '\0'])
                    .to_string()
            })
            .collect(),
        _ => return None,
    };
    let joined = parts.join("\\");
    (!joined.trim_matches(['\\', ' ']).is_empty()).then_some(joined)
}

/// The text of a top-level element, or none when absent, empty or not text.
fn text_of(dataset: &InMemDicomObject, tag: Tag, charset: &Charset) -> Option<String> {
    let e = dataset.get(tag)?;
    match convert(Converter::Text, e, charset).value {
        Some(Value::Text(s)) => Some(s),
        _ => None,
    }
}

/// The modality (§5.3): Modality trimmed and upper-cased, else a single-valued
/// ModalitiesInStudy; `PET` becomes `PT`.
fn modality_of(dataset: &InMemDicomObject, charset: &Charset) -> Result<String, Refusal> {
    let raw = text_of(dataset, tags::MODALITY, charset)
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty());
    let raw = match raw {
        Some(m) => m,
        None => {
            let mis = text_of(dataset, tags::MODALITIES_IN_STUDY, charset);
            let single = mis.as_deref().map(|s| {
                let parts: Vec<&str> = s
                    .split('\\')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .collect();
                (parts.len() == 1).then(|| parts[0].to_ascii_uppercase())
            });
            match single {
                Some(Some(m)) => m,
                Some(None) => {
                    return Err(Refusal::new(
                        QuarantineClass::MissingModality,
                        Some(format!("ModalitiesInStudy {}", mis.unwrap_or_default())),
                    ));
                }
                None => return Err(Refusal::new(QuarantineClass::MissingModality, None)),
            }
        }
    };
    let modality = if raw == "PET" { "PT".to_string() } else { raw };
    if MODALITIES.contains(&modality.as_str()) {
        Ok(modality)
    } else {
        Err(Refusal::new(
            QuarantineClass::UnsupportedModality,
            Some(modality),
        ))
    }
}

fn meta_field(meta: &dicom_object::FileMetaTable, which: Meta) -> Option<String> {
    let s = match which {
        Meta::TransferSyntax => meta.transfer_syntax(),
        Meta::MediaStorageSopClass => meta.media_storage_sop_class_uid(),
        Meta::MediaStorageSopInstance => meta.media_storage_sop_instance_uid(),
        Meta::ImplementationClass => {
            let uid = meta.implementation_class_uid();
            // dicom-rs fills in its own UID when the file has none.
            if uid == dicom_object::IMPLEMENTATION_CLASS_UID {
                return None;
            }
            uid
        }
        Meta::ImplementationVersion => {
            if meta.implementation_class_uid() == dicom_object::IMPLEMENTATION_CLASS_UID {
                return None;
            }
            meta.implementation_version_name()?
        }
    };
    (!s.is_empty()).then(|| s.to_string())
}

/// The first item of a sequence element, when it has one.
fn first_item(e: &InMemElement) -> Option<&InMemDicomObject> {
    match e.value() {
        DicomValue::Sequence(seq) => seq.items().first(),
        _ => None,
    }
}

/// The element a chain step names, when the path to it exists.
fn resolve<'a>(dataset: &'a InMemDicomObject, step: &Step) -> Option<&'a InMemElement> {
    match *step {
        Step::Top(tag) => dataset.get(tag),
        Step::Item(seq, tag) => dataset.get(seq).and_then(first_item)?.get(tag),
        Step::Fg(seq, tag) => FG_ROOTS.iter().find_map(|root| {
            dataset
                .get(*root)
                .and_then(first_item)?
                .get(seq)
                .and_then(first_item)?
                .get(tag)
                .filter(|e| !is_empty(e))
        }),
        Step::Private(tag) => {
            let frame = dataset
                .get(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
                .and_then(first_item)?;
            PRIVATE_PER_FRAME.iter().find_map(|p| {
                frame
                    .get(*p)
                    .and_then(first_item)?
                    .get(tag)
                    .filter(|e| !is_empty(e))
            })
        }
    }
}

fn is_empty(e: &InMemElement) -> bool {
    match e.value() {
        DicomValue::Primitive(p) => matches!(p, PrimitiveValue::Empty) || p.multiplicity() == 0,
        DicomValue::Sequence(seq) => seq.items().is_empty(),
        DicomValue::PixelSequence(_) => false,
    }
}

/// The keyword of a tag, for messages.
pub fn keyword_of(tag: Tag) -> String {
    StandardDataDictionary
        .by_tag(tag)
        .map(|e| e.alias.to_string())
        .unwrap_or_else(|| format!("({:04X},{:04X})", tag.group(), tag.element()))
}

/// The VR the dictionary gives a tag, for tests and synthetic files.
pub fn vr_of(tag: Tag) -> Option<VR> {
    StandardDataDictionary.by_tag(tag).map(|e| e.vr.relaxed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{self, Elem};
    use dicom_object::InMemDicomObject;

    fn header(elems: Vec<Elem>) -> Header {
        let bytes = synth::part10(&synth::MetaFields::mr("1.2.3.4"), &elems, true);
        let dir = synth::TempDir::new("extract");
        let path = dir.path().join("a.dcm");
        std::fs::write(&path, bytes).unwrap();
        read(&path).unwrap()
    }

    fn mr(elems: Vec<Elem>) -> Vec<Elem> {
        let mut all = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        all.extend(elems);
        all
    }

    #[test]
    fn extracts_uids_modality_and_values() {
        let x = extract_header(header(mr(vec![
            synth::text(tags::STUDY_DATE, VR::DA, "20240102"),
            synth::text(tags::ECHO_TIME, VR::DS, "12.5 "),
            synth::text(tags::INSTANCE_NUMBER, VR::IS, "7"),
            synth::us(tags::ROWS, 256),
        ])))
        .unwrap();
        assert_eq!(x.study_uid, "1.2.3");
        assert_eq!(x.series_uid, "1.2.3.1");
        assert_eq!(x.sop_uid, "1.2.3.4");
        assert_eq!(x.sop_class, "1.2.840.10008.5.1.4.1.1.4");
        assert_eq!(x.modality, "MR");
        assert_eq!(
            x.value(Level::Study, "study_date"),
            Some(&Value::Date("2024-01-02".into()))
        );
        assert_eq!(
            x.value(Level::Stack, "echo_time"),
            Some(&Value::Double(12.5))
        );
        assert_eq!(
            x.value(Level::SeriesMr, "echo_time"),
            Some(&Value::Double(12.5))
        );
        assert_eq!(
            x.value(Level::Instance, "instance_number"),
            Some(&Value::Int(7))
        );
        assert_eq!(x.value(Level::Instance, "rows"), Some(&Value::Int(256)));
        assert_eq!(
            x.value(Level::Series, "modality"),
            Some(&Value::Text("MR".into()))
        );
        assert_eq!(
            x.value(Level::Instance, "transfer_syntax_uid"),
            Some(&Value::Text("1.2.840.10008.1.2.1".into()))
        );
        assert_eq!(x.value(Level::SeriesCt, "kvp"), None);
        assert!(x.diagnostics.is_empty());
        assert_eq!(x.values.len(), CATALOGUE.len());
    }

    #[test]
    fn missing_uids_and_classes_are_refused_in_order() {
        let mut elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.retain(|e| e.tag != tags::SERIES_INSTANCE_UID);
        let r = extract_header(header(elems)).unwrap_err();
        assert_eq!(r.class, QuarantineClass::MissingUid);
        assert_eq!(r.detail.as_deref(), Some("SeriesInstanceUID"));

        let mut elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.retain(|e| e.tag != tags::SOP_CLASS_UID);
        // the meta's MediaStorageSOPClassUID stands in
        let x = extract_header(header(elems)).unwrap();
        assert_eq!(x.sop_class, "1.2.840.10008.5.1.4.1.1.4");

        let mut elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.retain(|e| e.tag != tags::SOP_CLASS_UID);
        elems.push(synth::text(
            tags::SOP_CLASS_UID,
            VR::UI,
            "1.2.840.10008.5.1.4.1.1.7",
        ));
        let r = extract_header(header(elems)).unwrap_err();
        assert_eq!(r.class, QuarantineClass::UnsupportedSopClass);
        assert_eq!(r.detail.as_deref(), Some("1.2.840.10008.5.1.4.1.1.7"));
    }

    #[test]
    fn modality_is_normalized_or_refused() {
        let mut elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.retain(|e| e.tag != tags::MODALITY);
        elems.push(synth::text(tags::MODALITY, VR::CS, " pet"));
        assert_eq!(extract_header(header(elems)).unwrap().modality, "PT");

        let mut elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.retain(|e| e.tag != tags::MODALITY);
        elems.push(synth::text(tags::MODALITIES_IN_STUDY, VR::CS, "CT"));
        assert_eq!(extract_header(header(elems)).unwrap().modality, "CT");

        let mut elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.retain(|e| e.tag != tags::MODALITY);
        elems.push(synth::text(tags::MODALITIES_IN_STUDY, VR::CS, "CT\\MR"));
        let r = extract_header(header(elems)).unwrap_err();
        assert_eq!(r.class, QuarantineClass::MissingModality);

        let mut elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.retain(|e| e.tag != tags::MODALITY);
        let r = extract_header(header(elems)).unwrap_err();
        assert_eq!(r.class, QuarantineClass::MissingModality);
        assert_eq!(r.detail, None);

        let mut elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.retain(|e| e.tag != tags::MODALITY);
        elems.push(synth::text(tags::MODALITY, VR::CS, "US"));
        let r = extract_header(header(elems)).unwrap_err();
        assert_eq!(r.class, QuarantineClass::UnsupportedModality);
        assert_eq!(r.detail.as_deref(), Some("US"));
    }

    #[test]
    fn enhanced_fallbacks_shared_then_per_frame_then_private() {
        let timing = |tr: f64| {
            synth::seq(
                tags::MR_TIMING_AND_RELATED_PARAMETERS_SEQUENCE,
                vec![vec![synth::num(tags::REPETITION_TIME, VR::FD, tr)]],
            )
        };
        // an empty top-level element does not stop the chain
        let x = extract_header(header(mr(vec![
            synth::text(tags::REPETITION_TIME, VR::DS, ""),
            synth::seq(
                tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
                vec![vec![timing(2000.0)]],
            ),
            synth::seq(
                tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
                vec![vec![timing(3000.0)]],
            ),
        ])))
        .unwrap();
        assert_eq!(
            x.value(Level::Stack, "repetition_time"),
            Some(&Value::Double(2000.0))
        );

        // shared has the group but no value: per-frame wins
        let x = extract_header(header(mr(vec![
            synth::seq(
                tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
                vec![vec![synth::seq(
                    tags::MR_TIMING_AND_RELATED_PARAMETERS_SEQUENCE,
                    vec![vec![]],
                )]],
            ),
            synth::seq(
                tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
                vec![vec![timing(3000.0)]],
            ),
        ])))
        .unwrap();
        assert_eq!(
            x.value(Level::Stack, "repetition_time"),
            Some(&Value::Double(3000.0))
        );

        // neither: the Philips private per-frame sequence
        let x = extract_header(header(mr(vec![synth::seq(
            tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
            vec![vec![synth::seq(
                Tag(0x2005, 0x140F),
                vec![vec![
                    synth::text(tags::REPETITION_TIME, VR::DS, "4000"),
                    synth::text(tags::SCANNING_SEQUENCE, VR::CS, "GR"),
                ]],
            )]],
        )])))
        .unwrap();
        assert_eq!(
            x.value(Level::Stack, "repetition_time"),
            Some(&Value::Double(4000.0))
        );
        assert_eq!(
            x.value(Level::Series, "scanning_sequence"),
            Some(&Value::Text("GR".into()))
        );

        // the top level wins when present
        let x = extract_header(header(mr(vec![
            synth::text(tags::REPETITION_TIME, VR::DS, "1000"),
            synth::seq(
                tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
                vec![vec![timing(2000.0)]],
            ),
        ])))
        .unwrap();
        assert_eq!(
            x.value(Level::Stack, "repetition_time"),
            Some(&Value::Double(1000.0))
        );
    }

    #[test]
    fn pet_radiopharmaceutical_falls_into_the_sequence() {
        let mut elems = synth::minimal_pet("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.push(synth::seq(
            tags::RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE,
            vec![vec![
                synth::text(tags::RADIOPHARMACEUTICAL, VR::LO, "FDG"),
                synth::text(tags::RADIONUCLIDE_TOTAL_DOSE, VR::DS, "250000000"),
                synth::text(tags::RADIOPHARMACEUTICAL_START_TIME, VR::TM, "101500"),
            ]],
        ));
        let x = extract_header(header(elems)).unwrap();
        assert_eq!(x.modality, "PT");
        assert_eq!(
            x.value(Level::SeriesPet, "radiopharmaceutical"),
            Some(&Value::Text("FDG".into()))
        );
        assert_eq!(
            x.value(Level::SeriesPet, "radionuclide_total_dose"),
            Some(&Value::Double(250000000.0))
        );
        assert_eq!(
            x.value(Level::SeriesPet, "radiopharmaceutical_start_time"),
            Some(&Value::Time("10:15:00".into()))
        );
        assert_eq!(x.value(Level::SeriesMr, "echo_time"), None);
        assert_eq!(x.value(Level::SeriesPet, "suvbw"), None);
    }

    #[test]
    fn invalid_values_become_null_with_a_diagnostic() {
        let x = extract_header(header(mr(vec![
            synth::text(tags::INSTANCE_NUMBER, VR::IS, "7a"),
            synth::text(tags::ECHO_TIME, VR::DS, "1\\2"),
        ])))
        .unwrap();
        assert_eq!(x.value(Level::Instance, "instance_number"), None);
        assert_eq!(x.value(Level::Stack, "echo_time"), None);
        let subjects: Vec<&str> = x.diagnostics.iter().map(|d| d.subject.as_str()).collect();
        assert!(subjects.contains(&"instance.instance_number"));
        assert!(subjects.contains(&"stack.echo_time"));
        assert!(subjects.contains(&"series_mr.echo_time"));
        assert!(
            x.diagnostics
                .iter()
                .all(|d| d.kind == DiagnosticKind::ValueInvalid)
        );
        let d = x
            .diagnostics
            .iter()
            .find(|d| d.subject == "instance.instance_number")
            .unwrap();
        assert_eq!(d.shape.as_deref(), Some("9a"));
    }

    #[test]
    fn charsets_are_recorded_repaired_and_flagged() {
        let x = extract_header(header(mr(vec![
            synth::text(tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO_IR 100"),
            synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 \u{e5}\u{e4}\u{f6}"),
        ])))
        .unwrap();
        assert_eq!(
            x.value(Level::Instance, "charset"),
            Some(&Value::Text("ISO_IR 100".into()))
        );
        assert_eq!(
            x.value(Level::Series, "series_description"),
            Some(&Value::Text("t1 \u{e5}\u{e4}\u{f6}".into()))
        );
        assert!(x.diagnostics.is_empty());

        // a misspelled UTF-8 declaration: re-decoded, no diagnostic
        let x = extract_header(header(mr(vec![
            synth::text(tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO IR 192"),
            synth::bytes(
                tags::SERIES_DESCRIPTION,
                VR::LO,
                "t1 \u{e5}\u{e4}\u{f6}".as_bytes().to_vec(),
            ),
        ])))
        .unwrap();
        assert_eq!(
            x.value(Level::Series, "series_description"),
            Some(&Value::Text("t1 \u{e5}\u{e4}\u{f6}".into()))
        );
        assert!(x.diagnostics.is_empty(), "{:?}", x.diagnostics);

        // an unknown code: kept as written, one diagnostic
        let x = extract_header(header(mr(vec![synth::text(
            tags::SPECIFIC_CHARACTER_SET,
            VR::CS,
            "KLINGON",
        )])))
        .unwrap();
        assert_eq!(
            x.value(Level::Instance, "charset"),
            Some(&Value::Text("KLINGON".into()))
        );
        assert_eq!(x.diagnostics.len(), 1);
        assert_eq!(x.diagnostics[0].kind, DiagnosticKind::CharsetUnknown);
        assert_eq!(x.diagnostics[0].subject, "KLINGON");
    }

    #[test]
    fn identity_is_read_and_kept_apart() {
        let x = extract_header(header(mr(vec![
            synth::text(tags::PATIENT_ID, VR::LO, "P001 "),
            synth::text(tags::PATIENT_NAME, VR::PN, "Doe^Jane"),
        ])))
        .unwrap();
        assert_eq!(x.identity.patient_id.as_deref(), Some("P001"));
        assert_eq!(x.identity.patient_name.as_deref(), Some("Doe^Jane"));
        assert!(
            x.values
                .iter()
                .flatten()
                .all(|v| !v.to_string().contains("Doe"))
        );
    }

    #[test]
    fn privates_by_creator_block() {
        let x = extract_header(header(mr(vec![
            synth::text(Tag(0x0019, 0x0011), VR::LO, "SIEMENS MR HEADER"),
            synth::text(Tag(0x0019, 0x110C), VR::IS, "1000"),
            synth::text(Tag(0x0019, 0x110D), VR::CS, "DIRECTIONAL"),
            synth::text(Tag(0x0043, 0x0010), VR::LO, "GEMS_PARM_01"),
            synth::text(Tag(0x0043, 0x1039), VR::IS, "1000\\8\\0\\0"),
            synth::bytes(Tag(0x0043, 0x1030), VR::SS, 6i16.to_le_bytes().to_vec()),
            synth::text(Tag(0x2001, 0x0010), VR::LO, "Philips Imaging DD 001"),
            synth::bytes(Tag(0x2001, 0x1003), VR::FL, 800f32.to_le_bytes().to_vec()),
        ])))
        .unwrap();
        assert_eq!(
            x.value(Level::SeriesMr, "dwi_siemens_b_value"),
            Some(&Value::Int(1000))
        );
        assert_eq!(
            x.value(Level::SeriesMr, "dwi_siemens_directionality"),
            Some(&Value::Text("DIRECTIONAL".into()))
        );
        assert_eq!(
            x.value(Level::SeriesMr, "dwi_ge_b_value"),
            Some(&Value::Int(1000))
        );
        assert_eq!(
            x.value(Level::SeriesMr, "dwi_ge_n_directions"),
            Some(&Value::Int(6))
        );
        assert_eq!(
            x.value(Level::SeriesMr, "dwi_philips_b_value"),
            Some(&Value::Double(800.0))
        );
    }

    #[test]
    fn bare_dataset_reports_the_syntax_it_was_read_with() {
        let elems = synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        let dir = synth::TempDir::new("extract-bare");
        let path = dir.path().join("bare");
        std::fs::write(&path, synth::bare(&elems, false)).unwrap();
        let x = extract(&path).unwrap();
        assert_eq!(x.form, Form::BareImplicit);
        assert_eq!(
            x.value(Level::Instance, "transfer_syntax_uid"),
            Some(&Value::Text("1.2.840.10008.1.2".into()))
        );
        assert_eq!(x.value(Level::Series, "implementation_class_uid"), None);
        assert_eq!(
            x.value(Level::Series, "media_storage_sop_instance_uid"),
            None
        );
    }

    #[test]
    fn empty_object_helpers() {
        let obj = InMemDicomObject::new_empty();
        assert_eq!(declared_charset(&obj), None);
        assert_eq!(keyword_of(tags::STUDY_DATE), "StudyDate");
        assert_eq!(keyword_of(Tag(0x0019, 0x100C)), "(0019,100C)");
    }
}
