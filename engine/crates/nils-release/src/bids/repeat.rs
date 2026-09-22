// SPDX-License-Identifier: AGPL-3.0-only

//! Whether two stacks are one acquisition made again (record 37, S2, and
//! record 38).
//!
//! `run-` is the standard's word for a repeat: the same acquisition, made
//! again. Until record 37 the release wrote it wherever two stacks wanted one
//! filename, and the 2026-09-19 naming study measured what that claim was
//! worth: of 702 named stacks 403 took a `run-` index, and 270 of them, 67 per
//! cent, sat in a name that covered more than one acquisition. A `run-2` that
//! is really a different echo time is a claim no downstream tool can audit,
//! because a validator passes it and a reader believes it.
//!
//! So the counter is replaced by a test, and the test is declared here rather
//! than hidden in the caller, so that a person can argue with it and two
//! releases agree. Record 38 settled what it is, in Nima's words: a rescan
//! happens inside one session, and the two stacks are identical in every
//! tag, physical parameter, classification axis and both texts, and differ
//! only in the time. The caller already holds one subject, session and
//! datatype and one BIDS name; two stacks there are **one acquisition made
//! again** when every one of these holds:
//!
//! | what | why it is in the test |
//! |---|---|
//! | they are stacks of two different series | one series split into two stacks is one acquisition in parts, never a repeat of itself |
//! | each was acquired at its own moment | two series with one acquisition time are one acquisition written twice, a re-send or a re-export, and not a rescan: 386 such pairs in one research cohort (`studies/2026-09-22-what-a-rescan-is`, section 4) |
//! | every axis the pack decided agrees | the axes are what the engine claims the stack *is*, and they carry the identities `ImageType` states |
//! | the whole `ImageType` agrees, as a set of tokens | "everything" means everything: a token no axis reads still says the two images were made differently |
//! | the protocol name, the series description, the sequence name, the body part examined and the contrast administration agree, folded for case and whitespace only | inside one session a rescan is the same protocol run again and its texts are identical; a pair that differs only there is asked about, not numbered. No counter is taken off: `T1 MPRAGE 2` is a different text |
//! | the coverage does not differ ([`coverage::differs`]) and the stack sits in the same place ([`coverage::moved`]) | slice count alone separates 3,610 colliding names in the archive; the centre, along the slice normal and in three dimensions where the images say where they sit, is what tells two stations of one prescription apart, sagittal ones included |
//! | the slice orientation agrees, as a label and as cosines within [`SAME_COSINE`] | a stack planned at another angle covers other ground |
//! | the modality, the scanner and its field strength agree | one acquisition is made on one machine |
//! | the receive coil agrees | the digest already splits a series on it |
//! | TE, TR, TI, the flip angle, the b value, the pixel bandwidth and the field strength agree within [`SAME_TIME_FRACTION`] | timing and contrast differ in 169 of the 202 residual pairs of record 37's study |
//! | the slice thickness, the spacing and the pixel spacing agree within [`SAME_LENGTH_FRACTION`] | the rest of the geometry the coverage does not reach |
//! | the diffusion gradient directions, their number, every b value and the phase encoding direction agree | readout-segmented diffusion stored one series per direction is alike in everything else, and was the largest group of false rescans in the cohort measured |
//! | the temporal position and the number of temporal positions agree | a dynamic stored one series per time point is alike in everything else too |
//! | the averages, the echo train length, the echo numbers, the acquisition matrix, the image size, the number of images, the 2D or 3D acquisition type, the scan options, the scanning sequence, the sequence variant and how the series was split agree exactly | integers and vocabularies, where a tolerance would mean nothing |
//!
//! **Which way each half errs.** Pairwise the test is strict about what was
//! measured and silent about what was not: a value only one side recorded is
//! not a difference, because an absence is not a measurement.
//!
//! Over a whole group of colliding stacks the caller is strict as well: a
//! group is one acquisition only when **every pair** in it is, and a group
//! that mixes is refused whole rather than split into a part that keeps the
//! name and a part that does not. Sameness within tolerance is not
//! transitive, and where a name covers more than one acquisition no subset of
//! it has a better claim to the name than any other.
//!
//! **No text makes a name.** The texts are compared here and nowhere else in
//! naming: a pair they separate is refused a shared name and asked about,
//! and the text never reaches a filename, a report or a review item.

use std::collections::{BTreeMap, BTreeSet};

use nils_classify::coverage::{self, Coverage};

/// Timings and angles within this fraction of the larger of them are the same
/// parameter. The study's tolerance, over TE, TR, TI, the flip angle, the b
/// value, the pixel bandwidth and the field strength.
pub const SAME_TIME_FRACTION: f64 = 0.02;

/// Thicknesses and spacings within this fraction are the same geometry. Wider
/// than the timings because a prescription is written in millimetres and a
/// scanner rounds what it writes back.
pub const SAME_LENGTH_FRACTION: f64 = 0.05;

/// Two direction cosines this close are one orientation: about half a degree,
/// which is what a scanner's rounding of a written cosine moves, and less
/// than any angle a person plans a stack at.
pub const SAME_COSINE: f64 = 0.01;

/// The words a review item carries for two stacks made at one moment. Not a
/// field that differs, so it is said as what happened.
pub const SAME_MOMENT: &str = "acquired at the same moment";

/// What the test reads of one stack.
///
/// Everything here is a column of `stack_fingerprint` or a decided axis, so
/// the test asks the registry and never a file. The texts are the folded and
/// lower-cased `_ci` spellings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Acquisition {
    /// The series the stack came out of. Two stacks of one series are one
    /// acquisition in parts.
    pub series: i64,
    /// Every axis the pack decided, by name, the values of a multi-valued one
    /// joined as the classifier joins them.
    pub axes: BTreeMap<String, String>,
    pub coverage: Coverage,
    /// The earliest acquisition date and time of the stack's images.
    pub acquired_date: Option<String>,
    pub acquired_time: Option<String>,
    pub protocol: Option<String>,
    pub description: Option<String>,
    pub sequence_name: Option<String>,
    pub body_part: Option<String>,
    pub contrast: Option<String>,
    /// `ImageType` as read, compared as a set of tokens.
    pub image_type: Option<String>,
    pub modality: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub station: Option<String>,
    pub field_strength: Option<f64>,
    pub coil: Option<String>,
    /// The orientation label the digest gave the stack, and its cosines.
    pub orientation: Option<String>,
    pub cosines: Option<String>,
    pub echo_time: Option<f64>,
    pub repetition_time: Option<f64>,
    pub inversion_time: Option<f64>,
    pub flip_angle: Option<f64>,
    pub b_value: Option<f64>,
    pub b_values: Option<String>,
    pub pixel_bandwidth: Option<f64>,
    pub averages: Option<f64>,
    pub slice_thickness: Option<f64>,
    pub slice_spacing: Option<f64>,
    pub pixel_spacing_row: Option<f64>,
    pub pixel_spacing_col: Option<f64>,
    pub echo_train_length: Option<i64>,
    pub echo_numbers: Option<String>,
    pub matrix: Option<String>,
    pub rows: Option<i64>,
    pub columns: Option<i64>,
    pub images: Option<i64>,
    pub acquisition_type: Option<String>,
    pub scan_options: Option<String>,
    pub scanning_sequence: Option<String>,
    pub sequence_variant: Option<String>,
    pub directions: Option<i64>,
    pub gradients: Option<String>,
    pub pe_direction: Option<String>,
    pub temporal_position: Option<i64>,
    pub temporal_positions: Option<i64>,
    pub split_reason: Option<String>,
    pub stacks_in_series: Option<i64>,
}

/// What separates two stacks, in words a review item can carry. Empty means
/// they are one acquisition made again.
pub fn differences(a: &Acquisition, b: &Acquisition) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if a.series == b.series {
        out.push("they are two stacks of one series, which a split made".to_string());
    }
    if same_moment(a, b) {
        out.push(SAME_MOMENT.to_string());
    }
    for axis in a.axes.keys().chain(b.axes.keys()) {
        if a.axes.get(axis) != b.axes.get(axis) {
            out.push(format!("the {axis} axis"));
        }
    }
    if tokens_differ(a.image_type.as_deref(), b.image_type.as_deref()) {
        out.push("the image type".to_string());
    }
    if coverage::differs(&a.coverage, &b.coverage) {
        out.push("what it covers".to_string());
    }
    // The two are held to one orientation below, so either's cosines say
    // which way is along the slices and which across them.
    let planes = a.cosines.as_deref().and_then(read_cosines);
    if coverage::moved(&a.coverage, &b.coverage, planes.as_deref()) {
        out.push("where it sits".to_string());
    }
    if cosines_differ(a.cosines.as_deref(), b.cosines.as_deref()) {
        out.push("the slice orientation".to_string());
    }
    for (what, x, y) in [
        ("the protocol name", &a.protocol, &b.protocol),
        ("the series description", &a.description, &b.description),
        ("the sequence name", &a.sequence_name, &b.sequence_name),
        ("the body part examined", &a.body_part, &b.body_part),
        ("the contrast administration", &a.contrast, &b.contrast),
        ("the modality", &a.modality, &b.modality),
        ("the scanner", &a.manufacturer, &b.manufacturer),
        ("the scanner", &a.model, &b.model),
        ("the scanner", &a.station, &b.station),
        ("the receive coil", &a.coil, &b.coil),
        ("the slice orientation", &a.orientation, &b.orientation),
        ("the acquisition matrix", &a.matrix, &b.matrix),
        ("the echo numbers", &a.echo_numbers, &b.echo_numbers),
        (
            "the acquisition type",
            &a.acquisition_type,
            &b.acquisition_type,
        ),
        ("the scan options", &a.scan_options, &b.scan_options),
        (
            "the scanning sequence",
            &a.scanning_sequence,
            &b.scanning_sequence,
        ),
        (
            "the sequence variant",
            &a.sequence_variant,
            &b.sequence_variant,
        ),
        ("the b values", &a.b_values, &b.b_values),
        (
            "the diffusion gradient directions",
            &a.gradients,
            &b.gradients,
        ),
        (
            "the phase encoding direction",
            &a.pe_direction,
            &b.pe_direction,
        ),
        ("how its series was split", &a.split_reason, &b.split_reason),
    ] {
        if word_differs(x.as_deref(), y.as_deref()) {
            out.push(what.to_string());
        }
    }
    for (what, x, y, tolerance) in [
        (
            "the echo time",
            a.echo_time,
            b.echo_time,
            SAME_TIME_FRACTION,
        ),
        (
            "the repetition time",
            a.repetition_time,
            b.repetition_time,
            SAME_TIME_FRACTION,
        ),
        (
            "the inversion time",
            a.inversion_time,
            b.inversion_time,
            SAME_TIME_FRACTION,
        ),
        (
            "the flip angle",
            a.flip_angle,
            b.flip_angle,
            SAME_TIME_FRACTION,
        ),
        ("the b value", a.b_value, b.b_value, SAME_TIME_FRACTION),
        (
            "the pixel bandwidth",
            a.pixel_bandwidth,
            b.pixel_bandwidth,
            SAME_TIME_FRACTION,
        ),
        (
            "the field strength",
            a.field_strength,
            b.field_strength,
            SAME_TIME_FRACTION,
        ),
        // A count of averages, so any difference at all is one.
        ("the averages", a.averages, b.averages, 0.0),
        (
            "the slice thickness",
            a.slice_thickness,
            b.slice_thickness,
            SAME_LENGTH_FRACTION,
        ),
        (
            "the spacing between slices",
            a.slice_spacing,
            b.slice_spacing,
            SAME_LENGTH_FRACTION,
        ),
        (
            "the pixel spacing",
            a.pixel_spacing_row,
            b.pixel_spacing_row,
            SAME_LENGTH_FRACTION,
        ),
        (
            "the pixel spacing",
            a.pixel_spacing_col,
            b.pixel_spacing_col,
            SAME_LENGTH_FRACTION,
        ),
    ] {
        if number_differs(x, y, tolerance) {
            out.push(what.to_string());
        }
    }
    for (what, x, y) in [
        (
            "the echo train length",
            a.echo_train_length,
            b.echo_train_length,
        ),
        ("the image size", a.rows, b.rows),
        ("the image size", a.columns, b.columns),
        ("the number of images", a.images, b.images),
        (
            "the number of diffusion directions",
            a.directions,
            b.directions,
        ),
        (
            "the temporal position",
            a.temporal_position,
            b.temporal_position,
        ),
        (
            "the number of temporal positions",
            a.temporal_positions,
            b.temporal_positions,
        ),
        (
            "how its series was split",
            a.stacks_in_series,
            b.stacks_in_series,
        ),
    ] {
        if int_differs(x, y) {
            out.push(what.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// What separates the members of a whole colliding group, as the union of
/// every pair's answer. Empty means every pair is the same acquisition made
/// again, which is the only case `run-` describes.
///
/// Strict on purpose: one odd member refuses the group rather than taking the
/// name off the odd one and leaving it on the rest. Two stacks may each be
/// within tolerance of a third and not of each other, so no subset has a
/// better claim to the name, and a tree that says `run-1`, `run-2` beside a
/// stack in `sourcedata/` for the same protocol would be saying two things.
pub fn one_acquisition(group: &[&Acquisition]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, a) in group.iter().enumerate() {
        for b in &group[i + 1..] {
            out.extend(differences(a, b));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The sentence a refused group carries: what happened, then what differs.
pub fn why(differs: &[String]) -> String {
    let moment = differs.iter().any(|d| d == SAME_MOMENT);
    let mut fields: Vec<&str> = differs
        .iter()
        .map(String::as_str)
        .filter(|d| *d != SAME_MOMENT)
        .collect();
    let fields = match fields.pop() {
        None => None,
        Some(last) if fields.is_empty() => Some(format!("{last} differs")),
        Some(last) => Some(format!("{} and {last} differ", fields.join(", "))),
    };
    let said = match (moment, fields) {
        (true, None) => format!("they were {SAME_MOMENT}"),
        (true, Some(f)) => format!("they were {SAME_MOMENT}, and {f}"),
        (false, Some(f)) => f,
        (false, None) => "nothing measured differs".to_string(),
    };
    format!("they are not repeats of one another: {said}")
}

/// Whether two stacks were acquired at one moment: both times recorded and
/// equal, on a day that does not differ. A time only one side recorded says
/// nothing, as everywhere here.
fn same_moment(a: &Acquisition, b: &Acquisition) -> bool {
    let (Some(x), Some(y)) = (&a.acquired_time, &b.acquired_time) else {
        return false;
    };
    let days_differ = matches!((&a.acquired_date, &b.acquired_date), (Some(p), Some(q)) if p != q);
    x == y && !days_differ
}

/// Text as it is compared: whitespace collapsed, case dropped, and nothing
/// else. No token is dropped and no counter is taken off, so a text that
/// differs by a digit differs.
fn fold(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether two numbers are further apart than the tolerance, **where both
/// were measured**. A number only one side recorded is not a difference: the
/// answer is what can be said, and nothing can be said about a value nobody
/// wrote down. The same rule [`coverage::differs`] takes.
fn number_differs(a: Option<f64>, b: Option<f64>, fraction: f64) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => (x - y).abs() > fraction * x.abs().max(y.abs()),
        _ => false,
    }
}

fn int_differs(a: Option<i64>, b: Option<i64>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x != y)
}

fn word_differs(a: Option<&str>, b: Option<&str>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if fold(x) != fold(y))
}

/// `ImageType` as the set of its tokens, upper case: the order a vendor writes
/// the tail in and its capitalisation are spellings, a token is a fact.
fn tokens_differ(a: Option<&str>, b: Option<&str>) -> bool {
    let set = |t: &str| -> BTreeSet<String> {
        t.split('\\')
            .map(|p| p.trim().to_uppercase())
            .filter(|p| !p.is_empty())
            .collect()
    };
    matches!((a, b), (Some(x), Some(y)) if set(x) != set(y))
}

/// `ImageOrientationPatient` as six numbers, or nothing.
fn read_cosines(t: &str) -> Option<Vec<f64>> {
    let v: Vec<f64> = t
        .split('\\')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<Vec<f64>>>()?;
    (v.len() == 6).then_some(v)
}

/// Six direction cosines further apart than [`SAME_COSINE`] in any one of
/// them. Cosines that do not read as six numbers are not a measurement.
fn cosines_differ(a: Option<&str>, b: Option<&str>) -> bool {
    match (a.and_then(read_cosines), b.and_then(read_cosines)) {
        (Some(x), Some(y)) => x.iter().zip(&y).any(|(p, q)| (p - q).abs() > SAME_COSINE),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One protocol step, measured twice. Every field below is a fact the
    /// studies found on real stacks; none of it is read from an archive.
    fn mprage() -> Acquisition {
        Acquisition {
            series: 1,
            axes: [
                ("base".to_string(), "T1w".to_string()),
                ("technique".to_string(), "MPRAGE".to_string()),
                ("body_part".to_string(), "brain".to_string()),
            ]
            .into(),
            coverage: coverage::of(&[0.0, 5.0, 10.0, 15.0]),
            coil: Some("HeadNeck_20".to_string()),
            protocol: Some("T1 MPRAGE".to_string()),
            echo_time: Some(3.0),
            repetition_time: Some(2000.0),
            flip_angle: Some(9.0),
            slice_thickness: Some(5.0),
            rows: Some(256),
            columns: Some(256),
            scanning_sequence: Some("GR\\IR".to_string()),
            ..Acquisition::default()
        }
    }

    fn again() -> Acquisition {
        Acquisition {
            series: 2,
            ..mprage()
        }
    }

    #[test]
    fn one_protocol_measured_twice_is_a_repeat() {
        // The case `run-` exists for, and the one a site genuinely scanning
        // twice depends on. Nothing differs but the series.
        assert_eq!(differences(&mprage(), &again()), Vec::<String>::new());
        assert_eq!(
            one_acquisition(&[&mprage(), &again()]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_text_that_differs_is_not_a_rescan_whatever_it_differs_by() {
        // Record 38: inside one session a rescan is the same protocol run
        // again, and its texts are identical. A counter is a difference too.
        for spelling in ["T1 MPRAGE 2", "T1 MPRAGE_2", "T1 MPRAGE ISO"] {
            let other = Acquisition {
                protocol: Some(spelling.to_string()),
                ..again()
            };
            assert_eq!(
                differences(&mprage(), &other),
                ["the protocol name"],
                "{spelling}"
            );
        }
        let described = Acquisition {
            description: Some("t1_mprage_sag_repeat".to_string()),
            ..again()
        };
        let original = Acquisition {
            description: Some("t1_mprage_sag".to_string()),
            ..mprage()
        };
        assert_eq!(
            differences(&original, &described),
            ["the series description"]
        );
        // Case and whitespace are spellings, and only those.
        let spelled = Acquisition {
            protocol: Some("t1  mprage".to_string()),
            ..again()
        };
        assert_eq!(differences(&mprage(), &spelled), Vec::<String>::new());
    }

    #[test]
    fn two_stacks_made_at_one_moment_are_not_a_rescan() {
        // One acquisition written twice, a re-send or a re-export.
        let first = Acquisition {
            acquired_date: Some("2022-01-15".to_string()),
            acquired_time: Some("10:01:02.000000".to_string()),
            ..mprage()
        };
        let copy = Acquisition {
            series: 2,
            ..first.clone()
        };
        assert_eq!(differences(&first, &copy), [SAME_MOMENT]);
        // Its own moment is what a rescan has.
        let later = Acquisition {
            acquired_time: Some("10:09:40.000000".to_string()),
            ..copy.clone()
        };
        assert_eq!(differences(&first, &later), Vec::<String>::new());
        // One hour on two days is two moments.
        let next_day = Acquisition {
            acquired_date: Some("2022-01-16".to_string()),
            ..copy
        };
        assert_eq!(differences(&first, &next_day), Vec::<String>::new());
        // And a time only one side recorded says nothing.
        assert_eq!(differences(&first, &again()), Vec::<String>::new());
    }

    #[test]
    fn two_stations_of_one_prescription_are_two_acquisitions() {
        // Record 38 S2: every parameter, the count and the extent agree; the
        // second station sits 200 mm down the table.
        let lower = Acquisition {
            coverage: coverage::of(&[-200.0, -195.0, -190.0, -185.0]),
            ..again()
        };
        assert_eq!(differences(&mprage(), &lower), ["where it sits"]);
    }

    #[test]
    fn diffusion_stored_one_direction_per_series_is_not_a_rescan() {
        // The largest false rescans measured: readout-segmented diffusion,
        // alike in everything but the direction each series played.
        let x = Acquisition {
            gradients: Some("1.0000,0.0000,0.0000".to_string()),
            directions: Some(1),
            ..mprage()
        };
        let y = Acquisition {
            series: 2,
            gradients: Some("0.0000,1.0000,0.0000".to_string()),
            ..x.clone()
        };
        assert_eq!(differences(&x, &y), ["the diffusion gradient directions"]);
        let more = Acquisition {
            series: 2,
            directions: Some(30),
            ..x.clone()
        };
        assert_eq!(
            differences(&x, &more),
            ["the number of diffusion directions"]
        );
    }

    #[test]
    fn a_dynamic_stored_one_series_per_time_point_is_not_a_rescan() {
        let first = Acquisition {
            temporal_position: Some(1),
            temporal_positions: Some(20),
            ..mprage()
        };
        let second = Acquisition {
            series: 2,
            temporal_position: Some(2),
            ..first.clone()
        };
        assert_eq!(differences(&first, &second), ["the temporal position"]);
    }

    #[test]
    fn the_whole_image_type_is_compared_as_a_set_of_tokens() {
        let a = Acquisition {
            image_type: Some("ORIGINAL\\PRIMARY\\M\\ND\\NORM".to_string()),
            ..mprage()
        };
        let reordered = Acquisition {
            series: 2,
            image_type: Some("original\\PRIMARY\\NORM\\ND\\M".to_string()),
            ..a.clone()
        };
        assert_eq!(differences(&a, &reordered), Vec::<String>::new());
        let filtered = Acquisition {
            series: 2,
            image_type: Some("ORIGINAL\\PRIMARY\\M\\ND".to_string()),
            ..a.clone()
        };
        assert_eq!(differences(&a, &filtered), ["the image type"]);
    }

    #[test]
    fn a_stack_planned_at_another_angle_is_not_a_rescan() {
        let a = Acquisition {
            cosines: Some("1\\0\\0\\0\\1\\0".to_string()),
            ..mprage()
        };
        let rounded = Acquisition {
            series: 2,
            cosines: Some("0.99999\\0.001\\0\\0\\1\\0".to_string()),
            ..a.clone()
        };
        assert_eq!(differences(&a, &rounded), Vec::<String>::new());
        let tilted = Acquisition {
            series: 2,
            cosines: Some("1\\0\\0\\0\\0.9848\\0.1736".to_string()),
            ..a.clone()
        };
        assert_eq!(differences(&a, &tilted), ["the slice orientation"]);
    }

    #[test]
    fn the_sentence_says_what_happened_and_then_what_differs() {
        assert_eq!(
            why(&[SAME_MOMENT.to_string()]),
            "they are not repeats of one another: they were acquired at the same moment"
        );
        assert_eq!(
            why(&["the echo time".to_string(), SAME_MOMENT.to_string()]),
            "they are not repeats of one another: they were acquired at the same moment, \
             and the echo time differs"
        );
        assert_eq!(
            why(&["the echo time".to_string(), "where it sits".to_string()]),
            "they are not repeats of one another: the echo time and where it sits differ"
        );
    }

    #[test]
    fn two_stacks_of_one_series_are_not_repeats_of_each_other() {
        // A split made them, and a split happens because the engine already
        // saw a difference. Parts of one acquisition, never one acquisition
        // twice.
        let other = Acquisition {
            series: 1,
            ..again()
        };
        assert!(
            differences(&mprage(), &other)
                .iter()
                .any(|d| d.contains("one series")),
            "{:?}",
            differences(&mprage(), &other)
        );
    }

    #[test]
    fn a_slice_more_is_not_a_repeat() {
        // Record 37 finding 2, and S1's whole reason: the commonest single
        // difference in the archive, which no name says.
        let longer = Acquisition {
            coverage: coverage::of(&[0.0, 5.0, 10.0, 15.0, 20.0]),
            ..again()
        };
        assert_eq!(differences(&mprage(), &longer), ["what it covers"]);
    }

    #[test]
    fn two_coils_are_not_one_acquisition_and_an_unrecorded_coil_is_not_a_coil() {
        // S3: the digest splits a series on the coil and then kept it
        // nowhere a name could reach.
        let other = Acquisition {
            coil: Some("Body_18".to_string()),
            ..again()
        };
        assert_eq!(differences(&mprage(), &other), ["the receive coil"]);
        let silent = Acquisition {
            coil: None,
            ..again()
        };
        assert_eq!(differences(&mprage(), &silent), Vec::<String>::new());
    }

    #[test]
    fn an_identity_the_image_type_states_is_a_difference() {
        // S5: `MEAN` beside its per-echo siblings, and a file that says its
        // input was unavailable. Both are axes now, so both are here.
        let mean = Acquisition {
            axes: [
                ("base".to_string(), "T1w".to_string()),
                ("technique".to_string(), "MPRAGE".to_string()),
                ("body_part".to_string(), "brain".to_string()),
                ("construct".to_string(), "Mean".to_string()),
            ]
            .into(),
            ..again()
        };
        assert_eq!(differences(&mprage(), &mean), ["the construct axis"]);
        let unavailable = Acquisition {
            axes: {
                let mut a = mprage().axes;
                a.insert("quality".to_string(), "InputUnavailable".to_string());
                a
            },
            ..again()
        };
        assert_eq!(differences(&mprage(), &unavailable), ["the quality axis"]);
    }

    #[test]
    fn a_timing_inside_the_tolerance_is_the_same_parameter_and_beyond_it_is_not() {
        let rounded = Acquisition {
            echo_time: Some(3.05),
            repetition_time: Some(2020.0),
            ..again()
        };
        assert_eq!(differences(&mprage(), &rounded), Vec::<String>::new());
        let other = Acquisition {
            echo_time: Some(9.0),
            ..again()
        };
        assert_eq!(differences(&mprage(), &other), ["the echo time"]);
    }

    #[test]
    fn a_count_has_no_tolerance() {
        let twice = Acquisition {
            averages: Some(2.0),
            ..again()
        };
        let once = Acquisition {
            averages: Some(1.0),
            ..mprage()
        };
        assert_eq!(differences(&once, &twice), ["the averages"]);
        let turbo = Acquisition {
            echo_train_length: Some(15),
            ..again()
        };
        let longer = Acquisition {
            echo_train_length: Some(16),
            ..mprage()
        };
        assert_eq!(differences(&turbo, &longer), ["the echo train length"]);
    }

    #[test]
    fn an_absence_is_not_a_difference() {
        // Generous on purpose: a value only one side wrote down is not a
        // measurement of two things.
        let silent = Acquisition {
            echo_time: None,
            slice_thickness: None,
            protocol: None,
            scanning_sequence: None,
            coverage: Coverage::default(),
            ..again()
        };
        assert_eq!(differences(&mprage(), &silent), Vec::<String>::new());
    }

    #[test]
    fn a_group_is_one_acquisition_only_when_every_pair_is() {
        // Sameness within a tolerance is not transitive: each of these is
        // within two per cent of the next and the ends are not. The group is
        // refused, which is the strict direction, because no subset of it has
        // a better claim to the name than any other.
        let a = mprage();
        let b = Acquisition {
            series: 2,
            echo_time: Some(3.05),
            ..mprage()
        };
        let c = Acquisition {
            series: 3,
            echo_time: Some(3.11),
            ..mprage()
        };
        assert!(differences(&a, &b).is_empty());
        assert!(differences(&b, &c).is_empty());
        assert_eq!(differences(&a, &c), ["the echo time"]);
        assert_eq!(one_acquisition(&[&a, &b, &c]), ["the echo time"]);
    }

    #[test]
    fn what_differs_is_said_once_however_many_pairs_say_it() {
        let a = mprage();
        let b = Acquisition {
            series: 2,
            echo_time: Some(90.0),
            ..mprage()
        };
        let c = Acquisition {
            series: 3,
            echo_time: Some(120.0),
            ..mprage()
        };
        assert_eq!(one_acquisition(&[&a, &b, &c]), ["the echo time"]);
    }
}
