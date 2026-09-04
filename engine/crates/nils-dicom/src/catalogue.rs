// SPDX-License-Identifier: AGPL-3.0-only

//! The field catalogue (`docs/specs/wave1-parse-and-digest.md`, §6.2): one row
//! per column the digest writes, with its level, its source, its converter, its
//! sensitivity class and a note. Wave 1's rule is v0's field set, v0's
//! fallbacks, v0's converters, then additions; the notes say where v1 reads
//! something v0 could not.
//!
//! `docs/reference/catalogue.md` is rendered from this table by
//! `cargo run -p nils-dicom --example catalogue`, and a test checks that the
//! file on disk is that rendering.

use std::fmt;

use dicom_core::Tag;
use dicom_dictionary_std::tags;

use crate::private::Dwi;
use crate::value::Converter;

/// The table a column belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    Subject,
    Study,
    Series,
    SeriesMr,
    SeriesCt,
    SeriesPet,
    Stack,
    Instance,
}

impl Level {
    pub const ALL: [Level; 8] = [
        Level::Subject,
        Level::Study,
        Level::Series,
        Level::SeriesMr,
        Level::SeriesCt,
        Level::SeriesPet,
        Level::Stack,
        Level::Instance,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Level::Subject => "subject",
            Level::Study => "study",
            Level::Series => "series",
            Level::SeriesMr => "series_mr",
            Level::SeriesCt => "series_ct",
            Level::SeriesPet => "series_pet",
            Level::Stack => "stack",
            Level::Instance => "instance",
        }
    }

    /// The modality a level is read for, when it is read for one only.
    pub fn modality(self) -> Option<&'static str> {
        match self {
            Level::SeriesMr => Some("MR"),
            Level::SeriesCt => Some("CT"),
            Level::SeriesPet => Some("PT"),
            _ => None,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The sensitivity class (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sensitivity {
    Identifying,
    QuasiIdentifying,
    Clinical,
    Technical,
}

impl Sensitivity {
    pub fn name(self) -> &'static str {
        match self {
            Sensitivity::Identifying => "identifying",
            Sensitivity::QuasiIdentifying => "quasi-identifying",
            Sensitivity::Clinical => "clinical",
            Sensitivity::Technical => "technical",
        }
    }
}

impl fmt::Display for Sensitivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A field of the file meta group that a column falls back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meta {
    TransferSyntax,
    MediaStorageSopClass,
    MediaStorageSopInstance,
    ImplementationClass,
    ImplementationVersion,
}

impl Meta {
    pub fn keyword(self) -> &'static str {
        match self {
            Meta::TransferSyntax => "TransferSyntaxUID",
            Meta::MediaStorageSopClass => "MediaStorageSOPClassUID",
            Meta::MediaStorageSopInstance => "MediaStorageSOPInstanceUID",
            Meta::ImplementationClass => "ImplementationClassUID",
            Meta::ImplementationVersion => "ImplementationVersionName",
        }
    }
}

/// One step of a fallback chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The element at the top level of the data set.
    Top(Tag),
    /// The first item of a top-level sequence, then its element.
    Item(Tag, Tag),
    /// v0's `_from_enhanced_fg`: SharedFunctionalGroupsSequence, then the first
    /// item of PerFrameFunctionalGroupsSequence; in each, the first item of the
    /// named functional group sequence, then its element.
    Fg(Tag, Tag),
    /// v0's `_from_enhanced_private`: the first item of
    /// PerFrameFunctionalGroupsSequence, then the first item of the Philips
    /// (2005,140F) or the Siemens (0021,1201) private sequence, then its element.
    Private(Tag),
}

/// What a column is computed from, when it is not an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    /// The normalized modality (§5.3): Modality, or a single-valued
    /// ModalitiesInStudy; `PET` becomes `PT`.
    Modality,
    /// SpecificCharacterSet as written.
    Charset,
    /// One of the six diffusion privates.
    Dwi(Dwi),
}

/// Where a column's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// One element at the top level.
    Tag(Tag),
    /// The element, or the file meta's field when the element is absent.
    TagOrMeta(Tag, Meta),
    /// Steps tried in order; the first present and non-empty wins.
    Chain(&'static [Step]),
    /// Computed.
    Special(Special),
    /// No standard element: v0 named a keyword that does not exist and the
    /// column was always null. Kept so that the tables match.
    None,
}

/// One row of the catalogue.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub column: &'static str,
    pub level: Level,
    pub source: Source,
    pub converter: Converter,
    pub class: Sensitivity,
    pub note: &'static str,
}

use Converter::{Date, Double, Int, Json, Text, Time};
use Level::{Instance, Series, SeriesCt, SeriesMr, SeriesPet, Stack, Study, Subject};
use Sensitivity::{QuasiIdentifying as Quasi, Technical as Tech};
use Source::{Chain, Tag as T, TagOrMeta};
use Step::{Fg, Item, Private, Top};

const fn f(
    column: &'static str,
    level: Level,
    source: Source,
    converter: Converter,
    class: Sensitivity,
    note: &'static str,
) -> Field {
    Field {
        column,
        level,
        source,
        converter,
        class,
        note,
    }
}

const SHARED: Tag = tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE;
const PER_FRAME: Tag = tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE;
/// The private per-frame sequences of v0's `_from_enhanced_private`.
pub const PRIVATE_PER_FRAME: [Tag; 2] = [Tag(0x2005, 0x140F), Tag(0x0021, 0x1201)];
/// The roots of v0's `_from_enhanced_fg`, in order.
pub const FG_ROOTS: [Tag; 2] = [SHARED, PER_FRAME];
const RADIOPHARM: Tag = tags::RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE;

const ECHO_TIME_CHAIN: &[Step] = &[
    Top(tags::ECHO_TIME),
    Fg(tags::MR_ECHO_SEQUENCE, tags::EFFECTIVE_ECHO_TIME),
    Private(tags::ECHO_TIME),
];
const ECHO_TRAIN_CHAIN: &[Step] = &[
    Top(tags::ECHO_TRAIN_LENGTH),
    Fg(
        tags::MR_TIMING_AND_RELATED_PARAMETERS_SEQUENCE,
        tags::ECHO_TRAIN_LENGTH,
    ),
    Private(tags::ECHO_TRAIN_LENGTH),
];
const REPETITION_CHAIN: &[Step] = &[
    Top(tags::REPETITION_TIME),
    Fg(
        tags::MR_TIMING_AND_RELATED_PARAMETERS_SEQUENCE,
        tags::REPETITION_TIME,
    ),
    Private(tags::REPETITION_TIME),
];
const FLIP_ANGLE_CHAIN: &[Step] = &[
    Top(tags::FLIP_ANGLE),
    Fg(
        tags::MR_TIMING_AND_RELATED_PARAMETERS_SEQUENCE,
        tags::FLIP_ANGLE,
    ),
    Private(tags::FLIP_ANGLE),
];
const RECEIVE_COIL_CHAIN: &[Step] = &[
    Top(tags::RECEIVE_COIL_NAME),
    Fg(tags::MR_RECEIVE_COIL_SEQUENCE, tags::RECEIVE_COIL_NAME),
];
const ORIENTATION_CHAIN: &[Step] = &[
    Top(tags::IMAGE_ORIENTATION_PATIENT),
    Fg(
        tags::PLANE_ORIENTATION_SEQUENCE,
        tags::IMAGE_ORIENTATION_PATIENT,
    ),
];

/// Series-level columns that carry the first instance's value by design and
/// differ between the instances of a series (v0 kept them on the series row):
/// the file meta's SOP instance and the slice position. The writer leaves them
/// out of the `field_disagreement` check (§9.1), which would otherwise count
/// every slice of every series.
pub const VARIES_PER_INSTANCE: [&str; 2] =
    ["media_storage_sop_instance_uid", "image_position_patient"];

const NOTE_FG: &str = "Enhanced MR fallback: the functional groups, shared then per-frame (v0)";
const NOTE_PRIVATE: &str = "the private per-frame sequences are the Philips (2005,140F) and the Siemens (0021,1201) one, read without a creator check (v0)";
const NOTE_META: &str = "the file meta when the element is absent (v0)";
const NOTE_NONE: &str = "v0 named a keyword that is no DICOM element; always null, kept for parity";
const NOTE_RADIOPHARM: &str = "addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only";

/// The catalogue: 171 rows.
#[allow(
    deprecated,
    reason = "StudyComments and CountsIncluded are retired tags that v0 read"
)]
pub static CATALOGUE: &[Field] = &[
    // subject (2)
    f(
        "birth_date",
        Subject,
        T(tags::PATIENT_BIRTH_DATE),
        Date,
        Quasi,
        "addition: v0 filled it through its importer",
    ),
    f(
        "sex",
        Subject,
        T(tags::PATIENT_SEX),
        Text,
        Quasi,
        "addition: v0 filled it through its importer",
    ),
    // study (12)
    f("study_date", Study, T(tags::STUDY_DATE), Date, Quasi, ""),
    f("study_time", Study, T(tags::STUDY_TIME), Time, Quasi, ""),
    f(
        "pps_start_date",
        Study,
        T(tags::PERFORMED_PROCEDURE_STEP_START_DATE),
        Date,
        Quasi,
        "addition: a date the vote reads when StudyDate is gone (Wave 3 §4.2)",
    ),
    f(
        "pps_end_date",
        Study,
        T(tags::PERFORMED_PROCEDURE_STEP_END_DATE),
        Date,
        Quasi,
        "addition: the same",
    ),
    f(
        "issue_date",
        Study,
        T(tags::ISSUE_DATE_OF_IMAGING_SERVICE_REQUEST),
        Date,
        Quasi,
        "addition: the same, and weaker",
    ),
    f(
        "study_description",
        Study,
        T(tags::STUDY_DESCRIPTION),
        Text,
        Quasi,
        "",
    ),
    f(
        "study_comments",
        Study,
        T(tags::STUDY_COMMENTS),
        Text,
        Quasi,
        "",
    ),
    f(
        "modalities_in_study",
        Study,
        T(tags::MODALITIES_IN_STUDY),
        Text,
        Tech,
        "v0's study.modality",
    ),
    f("manufacturer", Study, T(tags::MANUFACTURER), Text, Tech, ""),
    f(
        "manufacturer_model_name",
        Study,
        T(tags::MANUFACTURER_MODEL_NAME),
        Text,
        Tech,
        "",
    ),
    f(
        "station_name",
        Study,
        T(tags::STATION_NAME),
        Text,
        Quasi,
        "",
    ),
    f(
        "institution_name",
        Study,
        T(tags::INSTITUTION_NAME),
        Text,
        Quasi,
        "",
    ),
    // series (30)
    f(
        "modality",
        Series,
        Source::Special(Special::Modality),
        Text,
        Tech,
        "Modality, else a single-valued ModalitiesInStudy; PET becomes PT",
    ),
    f(
        "frame_of_reference_uid",
        Series,
        T(tags::FRAME_OF_REFERENCE_UID),
        Text,
        Tech,
        "",
    ),
    f(
        "implementation_class_uid",
        Series,
        TagOrMeta(tags::IMPLEMENTATION_CLASS_UID, Meta::ImplementationClass),
        Text,
        Tech,
        NOTE_META,
    ),
    f(
        "media_storage_sop_instance_uid",
        Series,
        TagOrMeta(
            tags::MEDIA_STORAGE_SOP_INSTANCE_UID,
            Meta::MediaStorageSopInstance,
        ),
        Text,
        Tech,
        NOTE_META,
    ),
    f(
        "sop_class_uid",
        Series,
        TagOrMeta(tags::SOP_CLASS_UID, Meta::MediaStorageSopClass),
        Text,
        Tech,
        NOTE_META,
    ),
    f(
        "implementation_version_name",
        Series,
        TagOrMeta(
            tags::IMPLEMENTATION_VERSION_NAME,
            Meta::ImplementationVersion,
        ),
        Text,
        Tech,
        NOTE_META,
    ),
    f(
        "sequence_name",
        Series,
        T(tags::SEQUENCE_NAME),
        Text,
        Quasi,
        "",
    ),
    f(
        "protocol_name",
        Series,
        T(tags::PROTOCOL_NAME),
        Text,
        Quasi,
        "",
    ),
    f("series_date", Series, T(tags::SERIES_DATE), Date, Quasi, ""),
    f("series_time", Series, T(tags::SERIES_TIME), Time, Quasi, ""),
    f(
        "series_description",
        Series,
        T(tags::SERIES_DESCRIPTION),
        Text,
        Quasi,
        "",
    ),
    f(
        "body_part_examined",
        Series,
        T(tags::BODY_PART_EXAMINED),
        Text,
        Tech,
        "",
    ),
    f(
        "scanning_sequence",
        Series,
        Chain(&[
            Top(tags::SCANNING_SEQUENCE),
            Private(tags::SCANNING_SEQUENCE),
        ]),
        Text,
        Tech,
        NOTE_PRIVATE,
    ),
    f(
        "sequence_variant",
        Series,
        Chain(&[Top(tags::SEQUENCE_VARIANT), Private(tags::SEQUENCE_VARIANT)]),
        Text,
        Tech,
        NOTE_PRIVATE,
    ),
    f(
        "scan_options",
        Series,
        T(tags::SCAN_OPTIONS),
        Text,
        Tech,
        "",
    ),
    f(
        "series_comments",
        Series,
        Source::None,
        Text,
        Quasi,
        NOTE_NONE,
    ),
    f("image_type", Series, T(tags::IMAGE_TYPE), Text, Tech, ""),
    f(
        "slice_thickness",
        Series,
        Chain(&[
            Top(tags::SLICE_THICKNESS),
            Fg(tags::PIXEL_MEASURES_SEQUENCE, tags::SLICE_THICKNESS),
        ]),
        Double,
        Tech,
        NOTE_FG,
    ),
    f(
        "spacing_between_slices",
        Series,
        Chain(&[
            Top(tags::SPACING_BETWEEN_SLICES),
            Fg(tags::PIXEL_MEASURES_SEQUENCE, tags::SPACING_BETWEEN_SLICES),
        ]),
        Double,
        Tech,
        NOTE_FG,
    ),
    f(
        "images_in_acquisition",
        Series,
        T(tags::IMAGES_IN_ACQUISITION),
        Text,
        Tech,
        "text, as v0 stored it",
    ),
    f(
        "image_orientation_patient",
        Series,
        Chain(ORIENTATION_CHAIN),
        Text,
        Tech,
        NOTE_FG,
    ),
    f(
        "image_position_patient",
        Series,
        T(tags::IMAGE_POSITION_PATIENT),
        Text,
        Tech,
        "",
    ),
    f(
        "patient_position",
        Series,
        T(tags::PATIENT_POSITION),
        Text,
        Tech,
        "",
    ),
    f(
        "contrast_bolus_agent",
        Series,
        T(tags::CONTRAST_BOLUS_AGENT),
        Text,
        Tech,
        "",
    ),
    f(
        "contrast_bolus_route",
        Series,
        T(tags::CONTRAST_BOLUS_ROUTE),
        Text,
        Tech,
        "",
    ),
    f(
        "contrast_bolus_total_dose",
        Series,
        T(tags::CONTRAST_BOLUS_TOTAL_DOSE),
        Double,
        Tech,
        "",
    ),
    f(
        "contrast_bolus_start_time",
        Series,
        T(tags::CONTRAST_BOLUS_START_TIME),
        Time,
        Quasi,
        "",
    ),
    f(
        "contrast_bolus_volume",
        Series,
        T(tags::CONTRAST_BOLUS_VOLUME),
        Double,
        Tech,
        "",
    ),
    f(
        "contrast_flow_rate",
        Series,
        T(tags::CONTRAST_FLOW_RATE),
        Double,
        Tech,
        "",
    ),
    f(
        "contrast_flow_duration",
        Series,
        T(tags::CONTRAST_FLOW_DURATION),
        Double,
        Tech,
        "",
    ),
    // instance (26)
    f(
        "instance_number",
        Instance,
        T(tags::INSTANCE_NUMBER),
        Int,
        Tech,
        "",
    ),
    f(
        "acquisition_number",
        Instance,
        T(tags::ACQUISITION_NUMBER),
        Int,
        Tech,
        "",
    ),
    f(
        "acquisition_date",
        Instance,
        T(tags::ACQUISITION_DATE),
        Date,
        Quasi,
        "",
    ),
    f(
        "acquisition_time",
        Instance,
        T(tags::ACQUISITION_TIME),
        Time,
        Quasi,
        "",
    ),
    f(
        "content_date",
        Instance,
        T(tags::CONTENT_DATE),
        Date,
        Quasi,
        "",
    ),
    f(
        "instance_creation_date",
        Instance,
        T(tags::INSTANCE_CREATION_DATE),
        Date,
        Quasi,
        "addition: a date the vote reads, and one an anonymiser often rewrites \
         to a first of January (Wave 3 §4.2)",
    ),
    f(
        "presentation_creation_date",
        Instance,
        T(tags::PRESENTATION_CREATION_DATE),
        Date,
        Quasi,
        "addition: the same, and weaker",
    ),
    f(
        "content_time",
        Instance,
        T(tags::CONTENT_TIME),
        Time,
        Quasi,
        "",
    ),
    f(
        "slice_location",
        Instance,
        T(tags::SLICE_LOCATION),
        Double,
        Tech,
        "",
    ),
    f(
        "pixel_spacing",
        Instance,
        Chain(&[
            Top(tags::PIXEL_SPACING),
            Fg(tags::PIXEL_MEASURES_SEQUENCE, tags::PIXEL_SPACING),
        ]),
        Text,
        Tech,
        NOTE_FG,
    ),
    f("rows", Instance, T(tags::ROWS), Int, Tech, ""),
    f("columns", Instance, T(tags::COLUMNS), Int, Tech, ""),
    f(
        "bits_allocated",
        Instance,
        T(tags::BITS_ALLOCATED),
        Int,
        Tech,
        "",
    ),
    f("bits_stored", Instance, T(tags::BITS_STORED), Int, Tech, ""),
    f("high_bit", Instance, T(tags::HIGH_BIT), Int, Tech, ""),
    f(
        "pixel_representation",
        Instance,
        T(tags::PIXEL_REPRESENTATION),
        Int,
        Tech,
        "",
    ),
    f(
        "window_center",
        Instance,
        T(tags::WINDOW_CENTER),
        Text,
        Tech,
        "",
    ),
    f(
        "window_width",
        Instance,
        T(tags::WINDOW_WIDTH),
        Text,
        Tech,
        "",
    ),
    f(
        "rescale_intercept",
        Instance,
        T(tags::RESCALE_INTERCEPT),
        Double,
        Tech,
        "",
    ),
    f(
        "rescale_slope",
        Instance,
        T(tags::RESCALE_SLOPE),
        Double,
        Tech,
        "",
    ),
    f(
        "number_of_frames",
        Instance,
        T(tags::NUMBER_OF_FRAMES),
        Int,
        Tech,
        "",
    ),
    f(
        "lossy_image_compression",
        Instance,
        T(tags::LOSSY_IMAGE_COMPRESSION),
        Text,
        Tech,
        "",
    ),
    f(
        "derivation_description",
        Instance,
        T(tags::DERIVATION_DESCRIPTION),
        Text,
        Quasi,
        "",
    ),
    f(
        "image_comments",
        Instance,
        T(tags::IMAGE_COMMENTS),
        Text,
        Quasi,
        "",
    ),
    f(
        "transfer_syntax_uid",
        Instance,
        TagOrMeta(tags::TRANSFER_SYNTAX_UID, Meta::TransferSyntax),
        Text,
        Tech,
        "the file meta; for a bare data set the syntax it was read with (v0 stored null there)",
    ),
    f(
        "charset",
        Instance,
        Source::Special(Special::Charset),
        Text,
        Tech,
        "addition: SpecificCharacterSet as written",
    ),
    // stack (14)
    f(
        "inversion_time",
        Stack,
        T(tags::INVERSION_TIME),
        Double,
        Tech,
        "",
    ),
    f(
        "echo_time",
        Stack,
        Chain(ECHO_TIME_CHAIN),
        Double,
        Tech,
        "MREchoSequence.EffectiveEchoTime, then the private sequences (v0)",
    ),
    f("echo_numbers", Stack, T(tags::ECHO_NUMBERS), Text, Tech, ""),
    f(
        "echo_train_length",
        Stack,
        Chain(ECHO_TRAIN_CHAIN),
        Int,
        Tech,
        "MRTimingAndRelatedParametersSequence, then the private sequences (v0)",
    ),
    f(
        "repetition_time",
        Stack,
        Chain(REPETITION_CHAIN),
        Double,
        Tech,
        "MRTimingAndRelatedParametersSequence, then the private sequences (v0)",
    ),
    f(
        "flip_angle",
        Stack,
        Chain(FLIP_ANGLE_CHAIN),
        Double,
        Tech,
        "MRTimingAndRelatedParametersSequence, then the private sequences (v0)",
    ),
    f(
        "receive_coil_name",
        Stack,
        Chain(RECEIVE_COIL_CHAIN),
        Text,
        Tech,
        "MRReceiveCoilSequence (v0)",
    ),
    f(
        "image_orientation_patient",
        Stack,
        Chain(ORIENTATION_CHAIN),
        Text,
        Tech,
        NOTE_FG,
    ),
    f("image_type", Stack, T(tags::IMAGE_TYPE), Text, Tech, ""),
    f(
        "xray_exposure",
        Stack,
        T(tags::EXPOSURE),
        Double,
        Tech,
        "Exposure",
    ),
    f("kvp", Stack, T(tags::KVP), Double, Tech, ""),
    f(
        "tube_current",
        Stack,
        T(tags::X_RAY_TUBE_CURRENT),
        Double,
        Tech,
        "XRayTubeCurrent",
    ),
    f(
        "pet_bed_index",
        Stack,
        T(tags::NUMBER_OF_SLICES),
        Int,
        Tech,
        "NumberOfSlices, v0's name",
    ),
    f(
        "pet_frame_type",
        Stack,
        T(tags::SERIES_TYPE),
        Text,
        Tech,
        "SeriesType, v0's name",
    ),
    // series_mr (33 + 6)
    f(
        "mr_acquisition_type",
        SeriesMr,
        T(tags::MR_ACQUISITION_TYPE),
        Text,
        Tech,
        "",
    ),
    f("angio_flag", SeriesMr, T(tags::ANGIO_FLAG), Text, Tech, ""),
    f(
        "repetition_time",
        SeriesMr,
        Chain(REPETITION_CHAIN),
        Double,
        Tech,
        "as on the stack",
    ),
    f(
        "echo_time",
        SeriesMr,
        Chain(ECHO_TIME_CHAIN),
        Double,
        Tech,
        "as on the stack",
    ),
    f(
        "inversion_time",
        SeriesMr,
        T(tags::INVERSION_TIME),
        Double,
        Tech,
        "",
    ),
    f(
        "inversion_times",
        SeriesMr,
        T(tags::INVERSION_TIMES),
        Text,
        Tech,
        "",
    ),
    f(
        "flip_angle",
        SeriesMr,
        Chain(FLIP_ANGLE_CHAIN),
        Double,
        Tech,
        "as on the stack",
    ),
    f(
        "phase_contrast",
        SeriesMr,
        T(tags::PHASE_CONTRAST),
        Text,
        Tech,
        "",
    ),
    f(
        "number_of_averages",
        SeriesMr,
        Chain(&[
            Top(tags::NUMBER_OF_AVERAGES),
            Fg(tags::MR_AVERAGES_SEQUENCE, tags::NUMBER_OF_AVERAGES),
        ]),
        Double,
        Tech,
        NOTE_FG,
    ),
    f(
        "imaging_frequency",
        SeriesMr,
        T(tags::IMAGING_FREQUENCY),
        Double,
        Tech,
        "",
    ),
    f(
        "imaged_nucleus",
        SeriesMr,
        T(tags::IMAGED_NUCLEUS),
        Text,
        Tech,
        "",
    ),
    f(
        "echo_numbers",
        SeriesMr,
        T(tags::ECHO_NUMBERS),
        Text,
        Tech,
        "",
    ),
    f(
        "magnetic_field_strength",
        SeriesMr,
        T(tags::MAGNETIC_FIELD_STRENGTH),
        Double,
        Tech,
        "",
    ),
    f(
        "number_of_phase_encoding_steps",
        SeriesMr,
        T(tags::NUMBER_OF_PHASE_ENCODING_STEPS),
        Text,
        Tech,
        "text, as v0 stored it",
    ),
    f(
        "echo_train_length",
        SeriesMr,
        Chain(ECHO_TRAIN_CHAIN),
        Int,
        Tech,
        "as on the stack",
    ),
    f(
        "percent_sampling",
        SeriesMr,
        Chain(&[
            Top(tags::PERCENT_SAMPLING),
            Fg(tags::MRFOV_GEOMETRY_SEQUENCE, tags::PERCENT_SAMPLING),
        ]),
        Double,
        Tech,
        NOTE_FG,
    ),
    f(
        "percent_phase_field_of_view",
        SeriesMr,
        Chain(&[
            Top(tags::PERCENT_PHASE_FIELD_OF_VIEW),
            Fg(
                tags::MRFOV_GEOMETRY_SEQUENCE,
                tags::PERCENT_PHASE_FIELD_OF_VIEW,
            ),
        ]),
        Double,
        Tech,
        NOTE_FG,
    ),
    f(
        "pixel_bandwidth",
        SeriesMr,
        Chain(&[
            Top(tags::PIXEL_BANDWIDTH),
            Fg(tags::MR_IMAGING_MODIFIER_SEQUENCE, tags::PIXEL_BANDWIDTH),
        ]),
        Text,
        Tech,
        NOTE_FG,
    ),
    f(
        "receive_coil_name",
        SeriesMr,
        Chain(RECEIVE_COIL_CHAIN),
        Text,
        Tech,
        "as on the stack",
    ),
    f(
        "transmit_coil_name",
        SeriesMr,
        Chain(&[
            Top(tags::TRANSMIT_COIL_NAME),
            Fg(tags::MR_TRANSMIT_COIL_SEQUENCE, tags::TRANSMIT_COIL_NAME),
        ]),
        Text,
        Tech,
        NOTE_FG,
    ),
    f(
        "acquisition_matrix",
        SeriesMr,
        T(tags::ACQUISITION_MATRIX),
        Text,
        Tech,
        "",
    ),
    f(
        "phase_encoding_direction",
        SeriesMr,
        T(tags::IN_PLANE_PHASE_ENCODING_DIRECTION),
        Text,
        Tech,
        "addition: InPlanePhaseEncodingDirection; v0's keyword PhaseEncodingDirection is no element and the column was always null",
    ),
    f("sar", SeriesMr, T(tags::SAR), Double, Tech, ""),
    f(
        "dbdt",
        SeriesMr,
        T(tags::D_BDT),
        Text,
        Tech,
        "dBdt, text as v0 stored it",
    ),
    f("b1rms", SeriesMr, T(tags::B1RMS), Double, Tech, ""),
    f(
        "temporal_position_identifier",
        SeriesMr,
        T(tags::TEMPORAL_POSITION_IDENTIFIER),
        Int,
        Tech,
        "",
    ),
    f(
        "number_of_temporal_positions",
        SeriesMr,
        T(tags::NUMBER_OF_TEMPORAL_POSITIONS),
        Int,
        Tech,
        "",
    ),
    f(
        "temporal_resolution",
        SeriesMr,
        T(tags::TEMPORAL_RESOLUTION),
        Text,
        Tech,
        "text, as v0 stored it",
    ),
    f(
        "diffusion_b_value",
        SeriesMr,
        Chain(&[
            Top(tags::DIFFUSION_B_VALUE),
            Fg(tags::MR_DIFFUSION_SEQUENCE, tags::DIFFUSION_B_VALUE),
        ]),
        Text,
        Tech,
        NOTE_FG,
    ),
    f(
        "diffusion_gradient_orientation",
        SeriesMr,
        T(tags::DIFFUSION_GRADIENT_ORIENTATION),
        Text,
        Tech,
        "",
    ),
    f(
        "diffusion_directionality",
        SeriesMr,
        Chain(&[
            Top(tags::DIFFUSION_DIRECTIONALITY),
            Fg(tags::MR_DIFFUSION_SEQUENCE, tags::DIFFUSION_DIRECTIONALITY),
        ]),
        Text,
        Tech,
        NOTE_FG,
    ),
    f(
        "parallel_acquisition_technique",
        SeriesMr,
        Chain(&[
            Top(tags::PARALLEL_ACQUISITION_TECHNIQUE),
            Fg(
                tags::MR_MODIFIER_SEQUENCE,
                tags::PARALLEL_ACQUISITION_TECHNIQUE,
            ),
        ]),
        Text,
        Tech,
        NOTE_FG,
    ),
    f(
        "parallel_reduction_factor_in_plane",
        SeriesMr,
        Chain(&[
            Top(tags::PARALLEL_REDUCTION_FACTOR_IN_PLANE),
            Fg(
                tags::MR_MODIFIER_SEQUENCE,
                tags::PARALLEL_REDUCTION_FACTOR_IN_PLANE,
            ),
        ]),
        Text,
        Tech,
        NOTE_FG,
    ),
    f(
        "dwi_siemens_b_value",
        SeriesMr,
        Source::Special(Special::Dwi(Dwi::SiemensBValue)),
        Int,
        Tech,
        "private, by creator block; bytes of an implicit VR file read as IS",
    ),
    f(
        "dwi_siemens_directionality",
        SeriesMr,
        Source::Special(Special::Dwi(Dwi::SiemensDirectionality)),
        Text,
        Tech,
        "private, by creator block",
    ),
    f(
        "dwi_siemens_pe_dir_positive",
        SeriesMr,
        Source::Special(Special::Dwi(Dwi::SiemensPeDirPositive)),
        Int,
        Tech,
        "CSA image header, SV10 only (v0)",
    ),
    f(
        "dwi_ge_b_value",
        SeriesMr,
        Source::Special(Special::Dwi(Dwi::GeBValue)),
        Int,
        Tech,
        "the first of the four values",
    ),
    f(
        "dwi_ge_n_directions",
        SeriesMr,
        Source::Special(Special::Dwi(Dwi::GeNDirections)),
        Int,
        Tech,
        "private, by creator block; bytes read as SS",
    ),
    f(
        "dwi_philips_b_value",
        SeriesMr,
        Source::Special(Special::Dwi(Dwi::PhilipsBValue)),
        Double,
        Tech,
        "the sentinel above 1e37 is null (v0); bytes read as FL",
    ),
    // series_ct (24)
    f("kvp", SeriesCt, T(tags::KVP), Double, Tech, ""),
    f(
        "data_collection_diameter",
        SeriesCt,
        T(tags::DATA_COLLECTION_DIAMETER),
        Double,
        Tech,
        "",
    ),
    f(
        "reconstruction_diameter",
        SeriesCt,
        T(tags::RECONSTRUCTION_DIAMETER),
        Double,
        Tech,
        "",
    ),
    f(
        "gantry_detector_tilt",
        SeriesCt,
        T(tags::GANTRY_DETECTOR_TILT),
        Double,
        Tech,
        "",
    ),
    f(
        "table_height",
        SeriesCt,
        T(tags::TABLE_HEIGHT),
        Double,
        Tech,
        "",
    ),
    f(
        "rotation_direction",
        SeriesCt,
        T(tags::ROTATION_DIRECTION),
        Text,
        Tech,
        "",
    ),
    f(
        "exposure_time",
        SeriesCt,
        T(tags::EXPOSURE_TIME),
        Double,
        Tech,
        "",
    ),
    f(
        "x_ray_tube_current",
        SeriesCt,
        T(tags::X_RAY_TUBE_CURRENT),
        Double,
        Tech,
        "",
    ),
    f("exposure", SeriesCt, T(tags::EXPOSURE), Double, Tech, ""),
    f(
        "filter_type",
        SeriesCt,
        T(tags::FILTER_TYPE),
        Text,
        Tech,
        "",
    ),
    f(
        "generator_power",
        SeriesCt,
        T(tags::GENERATOR_POWER),
        Double,
        Tech,
        "",
    ),
    f(
        "focal_spots",
        SeriesCt,
        T(tags::FOCAL_SPOTS),
        Text,
        Tech,
        "",
    ),
    f(
        "convolution_kernel",
        SeriesCt,
        T(tags::CONVOLUTION_KERNEL),
        Text,
        Tech,
        "",
    ),
    f(
        "revolution_time",
        SeriesCt,
        T(tags::REVOLUTION_TIME),
        Double,
        Tech,
        "",
    ),
    f(
        "single_collimation_width",
        SeriesCt,
        T(tags::SINGLE_COLLIMATION_WIDTH),
        Double,
        Tech,
        "",
    ),
    f(
        "total_collimation_width",
        SeriesCt,
        T(tags::TOTAL_COLLIMATION_WIDTH),
        Double,
        Tech,
        "",
    ),
    f(
        "table_speed",
        SeriesCt,
        T(tags::TABLE_SPEED),
        Double,
        Tech,
        "",
    ),
    f(
        "table_feed_per_rotation",
        SeriesCt,
        T(tags::TABLE_FEED_PER_ROTATION),
        Double,
        Tech,
        "",
    ),
    f(
        "spiral_pitch_factor",
        SeriesCt,
        T(tags::SPIRAL_PITCH_FACTOR),
        Double,
        Tech,
        "",
    ),
    f(
        "exposure_modulation_type",
        SeriesCt,
        T(tags::EXPOSURE_MODULATION_TYPE),
        Text,
        Tech,
        "",
    ),
    f(
        "ctdi_vol",
        SeriesCt,
        T(tags::CTD_IVOL),
        Double,
        Tech,
        "CTDIvol",
    ),
    f(
        "ctdi_phantom_type_code_sequence",
        SeriesCt,
        T(tags::CTDI_PHANTOM_TYPE_CODE_SEQUENCE),
        Json,
        Tech,
        "DICOM JSON of the items",
    ),
    f(
        "calcium_scoring_mass_factor_device",
        SeriesCt,
        T(tags::CALCIUM_SCORING_MASS_FACTOR_DEVICE),
        Double,
        Tech,
        "",
    ),
    f(
        "calcium_scoring_mass_factor_patient",
        SeriesCt,
        T(tags::CALCIUM_SCORING_MASS_FACTOR_PATIENT),
        Double,
        Tech,
        "",
    ),
    // series_pet (29)
    f(
        "radiopharmaceutical",
        SeriesPet,
        Chain(&[
            Top(tags::RADIOPHARMACEUTICAL),
            Item(RADIOPHARM, tags::RADIOPHARMACEUTICAL),
        ]),
        Text,
        Tech,
        NOTE_RADIOPHARM,
    ),
    f(
        "radionuclide_total_dose",
        SeriesPet,
        Chain(&[
            Top(tags::RADIONUCLIDE_TOTAL_DOSE),
            Item(RADIOPHARM, tags::RADIONUCLIDE_TOTAL_DOSE),
        ]),
        Double,
        Tech,
        NOTE_RADIOPHARM,
    ),
    f(
        "radionuclide_half_life",
        SeriesPet,
        Chain(&[
            Top(tags::RADIONUCLIDE_HALF_LIFE),
            Item(RADIOPHARM, tags::RADIONUCLIDE_HALF_LIFE),
        ]),
        Double,
        Tech,
        NOTE_RADIOPHARM,
    ),
    f(
        "radionuclide_positron_fraction",
        SeriesPet,
        Chain(&[
            Top(tags::RADIONUCLIDE_POSITRON_FRACTION),
            Item(RADIOPHARM, tags::RADIONUCLIDE_POSITRON_FRACTION),
        ]),
        Double,
        Tech,
        NOTE_RADIOPHARM,
    ),
    f(
        "radiopharmaceutical_start_time",
        SeriesPet,
        Chain(&[
            Top(tags::RADIOPHARMACEUTICAL_START_TIME),
            Item(RADIOPHARM, tags::RADIOPHARMACEUTICAL_START_TIME),
        ]),
        Time,
        Quasi,
        NOTE_RADIOPHARM,
    ),
    f(
        "radiopharmaceutical_stop_time",
        SeriesPet,
        Chain(&[
            Top(tags::RADIOPHARMACEUTICAL_STOP_TIME),
            Item(RADIOPHARM, tags::RADIOPHARMACEUTICAL_STOP_TIME),
        ]),
        Time,
        Quasi,
        NOTE_RADIOPHARM,
    ),
    f(
        "radiopharmaceutical_volume",
        SeriesPet,
        Chain(&[
            Top(tags::RADIOPHARMACEUTICAL_VOLUME),
            Item(RADIOPHARM, tags::RADIOPHARMACEUTICAL_VOLUME),
        ]),
        Double,
        Tech,
        NOTE_RADIOPHARM,
    ),
    f(
        "radiopharmaceutical_route",
        SeriesPet,
        Chain(&[
            Top(tags::RADIOPHARMACEUTICAL_ROUTE),
            Item(RADIOPHARM, tags::RADIOPHARMACEUTICAL_ROUTE),
        ]),
        Text,
        Tech,
        NOTE_RADIOPHARM,
    ),
    f(
        "decay_correction",
        SeriesPet,
        T(tags::DECAY_CORRECTION),
        Text,
        Tech,
        "",
    ),
    f(
        "decay_factor",
        SeriesPet,
        T(tags::DECAY_FACTOR),
        Double,
        Tech,
        "",
    ),
    f(
        "reconstruction_method",
        SeriesPet,
        T(tags::RECONSTRUCTION_METHOD),
        Text,
        Tech,
        "",
    ),
    f(
        "scatter_correction_method",
        SeriesPet,
        T(tags::SCATTER_CORRECTION_METHOD),
        Text,
        Tech,
        "",
    ),
    f(
        "attenuation_correction_method",
        SeriesPet,
        T(tags::ATTENUATION_CORRECTION_METHOD),
        Text,
        Tech,
        "",
    ),
    f(
        "randoms_correction_method",
        SeriesPet,
        T(tags::RANDOMS_CORRECTION_METHOD),
        Text,
        Tech,
        "",
    ),
    f(
        "dose_calibration_factor",
        SeriesPet,
        T(tags::DOSE_CALIBRATION_FACTOR),
        Double,
        Tech,
        "",
    ),
    f(
        "activity_concentration_scale",
        SeriesPet,
        Source::None,
        Double,
        Tech,
        NOTE_NONE,
    ),
    f("suv_type", SeriesPet, T(tags::SUV_TYPE), Text, Tech, ""),
    f("suvbw", SeriesPet, Source::None, Double, Tech, NOTE_NONE),
    f("suvlbm", SeriesPet, Source::None, Double, Tech, NOTE_NONE),
    f("suvbsa", SeriesPet, Source::None, Double, Tech, NOTE_NONE),
    f(
        "counts_source",
        SeriesPet,
        T(tags::COUNTS_SOURCE),
        Text,
        Tech,
        "",
    ),
    f("units", SeriesPet, T(tags::UNITS), Text, Tech, ""),
    f(
        "frame_reference_time",
        SeriesPet,
        T(tags::FRAME_REFERENCE_TIME),
        Double,
        Tech,
        "",
    ),
    f(
        "actual_frame_duration",
        SeriesPet,
        T(tags::ACTUAL_FRAME_DURATION),
        Double,
        Tech,
        "",
    ),
    f(
        "patient_gantry_relationship_code",
        SeriesPet,
        T(tags::PATIENT_GANTRY_RELATIONSHIP_CODE_SEQUENCE),
        Json,
        Tech,
        "DICOM JSON of the items",
    ),
    f(
        "slice_progression_direction",
        SeriesPet,
        T(tags::SLICE_PROGRESSION_DIRECTION),
        Text,
        Tech,
        "",
    ),
    f(
        "series_type",
        SeriesPet,
        T(tags::SERIES_TYPE),
        Text,
        Tech,
        "",
    ),
    f(
        "units_type",
        SeriesPet,
        T(tags::UNITS),
        Text,
        Tech,
        "Units once more, the way v0 has it",
    ),
    f(
        "counts_included",
        SeriesPet,
        T(tags::COUNTS_INCLUDED),
        Text,
        Tech,
        "",
    ),
];

/// The rows of one level, in catalogue order.
pub fn fields_of(level: Level) -> impl Iterator<Item = (usize, &'static Field)> {
    CATALOGUE
        .iter()
        .enumerate()
        .filter(move |(_, f)| f.level == level)
}

/// The index of a column at a level.
pub fn index_of(level: Level, column: &str) -> Option<usize> {
    CATALOGUE
        .iter()
        .position(|f| f.level == level && f.column == column)
}

/// Whether a series-level column reads what a stack column reads (§8): the
/// series row and its detail row carry the first instance's value of it, as
/// in v0, and the stacks carry every value the series has, so instances that
/// differ on it are the series' stacks, not a `field_disagreement`.
pub fn stack_defining(field: &Field) -> bool {
    matches!(
        field.level,
        Level::Series | Level::SeriesMr | Level::SeriesCt | Level::SeriesPet
    ) && fields_of(Level::Stack).any(|(_, s)| s.source == field.source)
}

fn tag_text(tag: Tag) -> String {
    format!("({:04X},{:04X})", tag.group(), tag.element())
}

fn keyword(tag: Tag) -> String {
    use dicom_core::dictionary::DataDictionary;
    dicom_dictionary_std::StandardDataDictionary
        .by_tag(tag)
        .map(|e| e.alias.to_string())
        .unwrap_or_else(|| tag_text(tag))
}

fn step_text(step: &Step) -> String {
    match step {
        Top(tag) => keyword(*tag),
        Item(seq, tag) => format!("{}[0].{}", keyword(*seq), keyword(*tag)),
        Fg(seq, tag) => format!("fg {}.{}", keyword(*seq), keyword(*tag)),
        Private(tag) => format!("private per-frame .{}", keyword(*tag)),
    }
}

impl Source {
    /// The source as the documentation prints it.
    pub fn text(&self) -> String {
        match self {
            Source::Tag(tag) => format!("{} {}", keyword(*tag), tag_text(*tag)),
            Source::TagOrMeta(tag, meta) => {
                format!(
                    "{} {}, else meta {}",
                    keyword(*tag),
                    tag_text(*tag),
                    meta.keyword()
                )
            }
            Source::Chain(steps) => steps
                .iter()
                .map(step_text)
                .collect::<Vec<_>>()
                .join(", then "),
            Source::Special(Special::Modality) => "Modality, else ModalitiesInStudy".to_string(),
            Source::Special(Special::Charset) => "SpecificCharacterSet (0008,0005)".to_string(),
            Source::Special(Special::Dwi(d)) => d.tag_text().to_string(),
            Source::None => "none".to_string(),
        }
    }
}

/// Render the catalogue as the Markdown reference page.
pub fn render_markdown() -> String {
    let mut out = String::new();
    out.push_str("# The field catalogue\n\n");
    out.push_str("Generated from `engine/crates/nils-dicom/src/catalogue.rs` by `cargo run -p nils-dicom --example catalogue`; do not edit by hand. One row per column the digest writes (`docs/specs/wave1-parse-and-digest.md`, §6.2): the source, the converter (§6.3), the sensitivity class (§4.3) and a note. A chain is tried in order and the first present, non-empty element wins; an empty element does not stop a chain. `fg X.Y` is the Enhanced MR fallback: SharedFunctionalGroupsSequence, then the first item of PerFrameFunctionalGroupsSequence, in each the first item of X and its Y.\n\n");
    for level in Level::ALL {
        let rows: Vec<&Field> = CATALOGUE.iter().filter(|f| f.level == level).collect();
        let modality = level
            .modality()
            .map(|m| format!(", {m} only"))
            .unwrap_or_default();
        out.push_str(&format!("## {} ({}{})\n\n", level, rows.len(), modality));
        out.push_str("| column | source | converter | class | note |\n|---|---|---|---|---|\n");
        for f in rows {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} |\n",
                f.column,
                f.source.text().replace('|', "\\|"),
                f.converter,
                f.class,
                f.note.replace('|', "\\|")
            ));
        }
        out.push('\n');
    }
    out.push_str(&format!("{} columns.\n", CATALOGUE.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_per_instance_columns_are_series_columns() {
        for column in VARIES_PER_INSTANCE {
            assert!(index_of(Series, column).is_some(), "{column}");
        }
    }

    #[test]
    fn the_stack_defining_series_columns_are_the_thirteen() {
        let defining: Vec<String> = CATALOGUE
            .iter()
            .filter(|f| stack_defining(f))
            .map(|f| format!("{}.{}", f.level, f.column))
            .collect();
        assert_eq!(
            defining,
            [
                "series.image_type",
                "series.image_orientation_patient",
                "series_mr.repetition_time",
                "series_mr.echo_time",
                "series_mr.inversion_time",
                "series_mr.flip_angle",
                "series_mr.echo_numbers",
                "series_mr.echo_train_length",
                "series_mr.receive_coil_name",
                "series_ct.kvp",
                "series_ct.x_ray_tube_current",
                "series_ct.exposure",
                "series_pet.series_type",
            ]
        );
        // a stack column is never its own reason, nor is an instance column
        assert!(fields_of(Stack).all(|(_, f)| !stack_defining(f)));
        assert!(fields_of(Instance).all(|(_, f)| !stack_defining(f)));
    }

    #[test]
    fn counts_per_level_are_the_spec_s() {
        let count = |l: Level| CATALOGUE.iter().filter(|f| f.level == l).count();
        assert_eq!(count(Subject), 2);
        assert_eq!(count(Study), 12);
        assert_eq!(count(Series), 30);
        assert_eq!(count(Instance), 26);
        assert_eq!(count(Stack), 14);
        assert_eq!(count(SeriesMr), 39);
        assert_eq!(count(SeriesCt), 24);
        assert_eq!(count(SeriesPet), 29);
        assert_eq!(CATALOGUE.len(), 176);
    }

    #[test]
    fn columns_are_unique_per_level_and_snake_case() {
        let mut seen = HashSet::new();
        for f in CATALOGUE {
            assert!(seen.insert((f.level, f.column)), "{} {}", f.level, f.column);
            assert!(
                f.column
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{}",
                f.column
            );
        }
    }

    #[test]
    fn nothing_identifying_is_stored() {
        for f in CATALOGUE {
            assert_ne!(f.class, Sensitivity::Identifying, "{}", f.column);
            if let Source::Tag(t) | Source::TagOrMeta(t, _) = f.source {
                assert_ne!(t, tags::PATIENT_NAME);
                assert_ne!(t, tags::PATIENT_ID);
            }
        }
    }

    #[test]
    fn dates_and_times_are_quasi_identifying() {
        for f in CATALOGUE {
            if matches!(f.converter, Date | Time) {
                assert_eq!(f.class, Quasi, "{}", f.column);
            }
        }
    }

    #[test]
    fn chains_start_at_the_top_level_and_json_reads_sequences() {
        for f in CATALOGUE {
            if let Chain(steps) = f.source {
                assert!(matches!(steps[0], Top(_)), "{}", f.column);
                assert!(steps.len() >= 2, "{}", f.column);
            }
            if f.converter == Json {
                assert!(matches!(f.source, Source::Tag(_)), "{}", f.column);
            }
        }
    }

    #[test]
    fn stack_fields_repeat_on_series_mr_with_the_same_source() {
        for column in [
            "repetition_time",
            "echo_time",
            "flip_angle",
            "echo_train_length",
            "receive_coil_name",
        ] {
            let s = &CATALOGUE[index_of(Stack, column).unwrap()];
            let m = &CATALOGUE[index_of(SeriesMr, column).unwrap()];
            assert_eq!(s.source, m.source, "{column}");
            assert_eq!(s.converter, m.converter, "{column}");
        }
    }

    #[test]
    fn the_reference_page_is_this_table() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/reference/catalogue.md");
        let on_disk = std::fs::read_to_string(&path).expect("docs/reference/catalogue.md exists");
        assert!(
            on_disk == render_markdown(),
            "docs/reference/catalogue.md is stale; run `cargo run -p nils-dicom --example catalogue -- --write`"
        );
    }

    #[test]
    fn sources_render() {
        assert_eq!(
            Source::Tag(tags::STUDY_DATE).text(),
            "StudyDate (0008,0020)"
        );
        assert!(Chain(ECHO_TIME_CHAIN).text().starts_with(
            "EchoTime, then fg MREchoSequence.EffectiveEchoTime, then private per-frame .EchoTime"
        ));
        let md = render_markdown();
        assert!(md.contains("## series_mr (39, MR only)"));
        assert!(md.contains("176 columns."));
    }
}
