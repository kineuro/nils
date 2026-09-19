// SPDX-License-Identifier: AGPL-3.0-only

//! Whether two stacks are one acquisition done twice (record 37, S2).
//!
//! `run-` is the standard's word for a repeat: the same acquisition, made
//! again. Until this module existed the release wrote it wherever two stacks
//! wanted one filename, and the 2026-09-19 naming study measured what that
//! claim was worth: of 702 named stacks 403 took a `run-` index, and 270 of
//! them, 67 per cent, sat in a name that covered more than one acquisition.
//! Archive-wide only 18 per cent of colliding names are repeats. A `run-2`
//! that is really a different echo time is a claim no downstream tool can
//! audit, because a validator passes it and a reader believes it.
//!
//! So the counter is replaced by a test, and the test is declared here rather
//! than hidden in the caller, so that a person can argue with it and two
//! releases agree. Two stacks are **one acquisition twice** when every one of
//! these holds:
//!
//! | what | why it is in the test |
//! |---|---|
//! | they are stacks of two different series | one series split into two stacks is one acquisition in parts, never a repeat of itself; the split happened because the engine already saw a difference |
//! | every axis the pack decided agrees | the axes are what the engine claims the stack *is*, and after S5 they carry the identities `ImageType` states: the echo-combined image, the composed one, the Dixon part, the map, and what a file says is wrong with itself |
//! | the coverage does not differ ([`coverage::differs`]) | S1. Slice count alone separates 3,610 colliding names and 9,905 stacks in the archive, the commonest residual there is |
//! | the receive coil agrees | S3. 2,412 stacks are separated by nothing else, and the digest already splits a series on it |
//! | the protocol name agrees, less the scanner's own counter | the study's rule: a re-run step comes back as `... 2`, and that is the repeat it was meant to describe |
//! | TE, TR, TI, the flip angle and the b value agree within [`SAME_TIME_FRACTION`] | timing and contrast differ in 169 of the 202 residual pairs, and lead the archive-scale table |
//! | the slice thickness and the spacing agree within [`SAME_LENGTH_FRACTION`] | the rest of the geometry the coverage does not reach |
//! | the averages, the echo train length, the acquisition matrix, the image size, the scan options, the scanning sequence and the sequence variant agree exactly | integers and vocabularies, where a tolerance would mean nothing |
//!
//! **Which way each half errs.** Pairwise the test is generous, deliberately:
//! a value only one side recorded is not a difference, because an absence is
//! not a measurement, and the tolerances are the study's own. Generous here
//! means a pair is called a repeat when it might not be one, and the study's
//! sensitivity table says its false-run rate is a floor for that reason.
//!
//! Over a whole group of colliding stacks the caller is strict instead: a
//! group is one acquisition only when **every pair** in it is, and a group
//! that mixes is refused whole rather than split into a part that keeps the
//! name and a part that does not. Sameness within tolerance is not
//! transitive, and where a name covers more than one acquisition no subset of
//! it has a better claim to the name than any other.
//!
//! One thing the study compared and this does not: the raw `ImageType`
//! string. S5 turned the tokens there that change what an image is into pack
//! axes, which the second row of the table already compares; what is left in
//! that string is the reconstruction and filter vocabulary record 37's first
//! finding classes as `rec` or noise (`NORM`, `FIL`, `DIS2D`, `MFSPLIT`, the
//! shim codes), and refusing a name over those would refuse real repeats.

use std::collections::BTreeMap;

use nils_classify::coverage::{self, Coverage};

/// Timings and angles within this fraction of the larger of them are the same
/// parameter. The study's tolerance, over TE, TR, TI, the flip angle and the
/// b value.
pub const SAME_TIME_FRACTION: f64 = 0.02;

/// Thicknesses and spacings within this fraction are the same geometry. Wider
/// than the timings because a prescription is written in millimetres and a
/// scanner rounds what it writes back.
pub const SAME_LENGTH_FRACTION: f64 = 0.05;

/// What the test reads of one stack.
///
/// Everything here is a column of `stack_fingerprint` or a decided axis, so
/// the test asks the registry and never a file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Acquisition {
    /// The series the stack came out of. Two stacks of one series are one
    /// acquisition in parts.
    pub series: i64,
    /// Every axis the pack decided, by name, the values of a multi-valued one
    /// joined as the classifier joins them.
    pub axes: BTreeMap<String, String>,
    pub coverage: Coverage,
    pub coil: Option<String>,
    /// The protocol name, folded and lower-cased.
    pub protocol: Option<String>,
    pub echo_time: Option<f64>,
    pub repetition_time: Option<f64>,
    pub inversion_time: Option<f64>,
    pub flip_angle: Option<f64>,
    pub b_value: Option<f64>,
    pub averages: Option<f64>,
    pub slice_thickness: Option<f64>,
    pub slice_spacing: Option<f64>,
    pub echo_train_length: Option<i64>,
    pub matrix: Option<String>,
    pub rows: Option<i64>,
    pub columns: Option<i64>,
    pub scan_options: Option<String>,
    pub scanning_sequence: Option<String>,
    pub sequence_variant: Option<String>,
}

/// What separates two stacks, in words a review item can carry. Empty means
/// they are one acquisition done twice.
pub fn differences(a: &Acquisition, b: &Acquisition) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if a.series == b.series {
        out.push("they are two stacks of one series, which a split made".to_string());
    }
    for axis in a.axes.keys().chain(b.axes.keys()) {
        if a.axes.get(axis) != b.axes.get(axis) {
            out.push(format!("the {axis} axis"));
        }
    }
    if coverage::differs(&a.coverage, &b.coverage) {
        out.push("what it covers".to_string());
    }
    if word_differs(a.coil.as_deref(), b.coil.as_deref()) {
        out.push("the receive coil".to_string());
    }
    if let (Some(x), Some(y)) = (a.protocol.as_deref(), b.protocol.as_deref())
        && step(x) != step(y)
    {
        out.push("the protocol".to_string());
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
    ] {
        if number_differs(x, y, tolerance) {
            out.push(what.to_string());
        }
    }
    if int_differs(a.echo_train_length, b.echo_train_length) {
        out.push("the echo train length".to_string());
    }
    if int_differs(a.rows, b.rows) || int_differs(a.columns, b.columns) {
        out.push("the image size".to_string());
    }
    for (what, x, y) in [
        ("the acquisition matrix", &a.matrix, &b.matrix),
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
    ] {
        if word_differs(x.as_deref(), y.as_deref()) {
            out.push(what.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// What separates the members of a whole colliding group, as the union of
/// every pair's answer. Empty means every pair is the same acquisition, which
/// is the only case `run-` describes.
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

/// A protocol name with the scanner's own re-run counter taken off it.
///
/// A step run a second time comes back as `T1 MPRAGE 2`, and the study's rule
/// is to ignore that digit. It ignores only a **whole trailing token**: a
/// digit welded to a letter, as in `T1` or `p2`, is part of the protocol's
/// name and not a counter, and dropping every digit would read `T1 MPRAGE`
/// and `T2 MPRAGE` as one protocol. That is stricter than the study, which
/// dropped all of them, and strict is the safe direction here: what it costs
/// is a refusal and a question, not a wrong filename.
pub fn step(text: &str) -> String {
    let mut out = fold(text);
    loop {
        let body = out.trim_end_matches(|c: char| c.is_ascii_digit());
        if body.len() == out.len() {
            break;
        }
        let head = body.trim_end_matches([' ', '_', '-', '.']);
        if head.len() == body.len() || head.is_empty() {
            break;
        }
        out = head.to_string();
    }
    out
}

/// Text as it is compared: whitespace collapsed, case dropped.
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
    fn the_counter_a_scanner_puts_on_a_rerun_step_is_not_a_difference() {
        let rerun = Acquisition {
            protocol: Some("T1 MPRAGE 2".to_string()),
            ..again()
        };
        assert_eq!(differences(&mprage(), &rerun), Vec::<String>::new());
        // ... and the same with the separators a scanner actually writes
        for spelling in ["T1 MPRAGE_2", "T1 MPRAGE-3", "t1 mprage  2"] {
            let rerun = Acquisition {
                protocol: Some(spelling.to_string()),
                ..again()
            };
            assert_eq!(
                differences(&mprage(), &rerun),
                Vec::<String>::new(),
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_digit_welded_to_a_letter_is_part_of_the_protocols_name() {
        // Dropping every digit, which is what the study did, reads these as
        // one protocol. They are two.
        assert_eq!(step("T1 MPRAGE"), "t1 mprage");
        assert_ne!(step("T1 MPRAGE"), step("T2 MPRAGE"));
        assert_eq!(step("t1_mprage_sag_p2_2"), "t1_mprage_sag_p2");
        // and a name that is only a number keeps it rather than becoming
        // nothing at all
        assert_eq!(step("2"), "2");
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
