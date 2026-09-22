// SPDX-License-Identifier: AGPL-3.0-only

//! What a stack covers (record 37, S1).
//!
//! The fingerprint has always carried what one slice looks like: the rows,
//! the columns, the two pixel spacings, the thickness and the spacing between
//! slices. It has never carried how many slices there are, nor how far they
//! reach, and that is the commonest single difference between two stacks of
//! one protocol on a real archive: measured over 437,069 classified stacks,
//! the slice count alone separates more pairs than every timing parameter
//! together.
//!
//! The count is of *positions*, not of images. A stack holding twenty
//! dynamics of thirty slices holds six hundred images and covers thirty
//! slices, and `n_instances` already says the first of those.
//!
//! A fact meant to distinguish has to be stable, or every pair looks
//! different and the failure is only the mirror of the one it was meant to
//! fix. Two tolerances hold it still:
//!
//! 1. Inside a stack, positions within [`SAME_POSITION_MM`] of one another
//!    are one slice. `SliceLocation` is a decimal string a scanner writes,
//!    and the same prescribed slice comes back as `12.5` from one file and
//!    `12.499999` from the next. The positions are clustered from the lowest
//!    up rather than rounded to a grid, so two values a hair apart cannot
//!    fall either side of a boundary.
//! 2. Between two stacks, [`differs`] answers only what it can measure: the
//!    counts must be equal, and the extents are compared with the wider of
//!    [`SAME_SPAN_MM`] and [`SAME_SPAN_FRACTION`] of the larger. Where either
//!    side never measured a coverage, nothing differs, because an absence is
//!    not a difference.
//!
//! **Where, as well as how much** (record 38, S2). A count and an extent say
//! how much ground a stack covers and not which ground: two stations of a
//! spine, or of a body, are one prescription placed twice, and they share
//! every parameter, the count and the extent included. So a coverage also
//! carries its centre, the midpoint of the same positions, and [`moved`] says
//! whether two stacks sit in measurably different places along the slice
//! normal. Its tolerance is the wider of [`SAME_SPAN_MM`] and half a slice
//! step ([`moved`] says why), and an absence is again not a difference.

/// Two slice positions no further apart than this are the same slice.
pub const SAME_POSITION_MM: f64 = 0.01;

/// Two extents no further apart than this cover the same ground, ...
pub const SAME_SPAN_MM: f64 = 1.0;

/// ... as do two that are within this fraction of the larger of them.
pub const SAME_SPAN_FRACTION: f64 = 0.01;

/// Which evidence answered the coverage. Written on every row, so that a
/// stack whose files never said where their slices were reads as unmeasured
/// rather than as a stack of no slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// `SliceLocation`, on the stack's own images.
    SliceLocation,
    /// No image of the stack carried a position.
    #[default]
    Unmeasured,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::SliceLocation => "slice_location",
            Source::Unmeasured => "none",
        }
    }

    /// The name read back, for a reader of the row rather than its writer. A
    /// word this never wrote reads as unmeasured, which is what a row from an
    /// older derivation is.
    pub fn parse(text: &str) -> Source {
        match text {
            "slice_location" => Source::SliceLocation,
            _ => Source::Unmeasured,
        }
    }
}

/// A stack's coverage along the slice axis.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Coverage {
    /// How many distinct slice positions the stack holds.
    pub n_slices: Option<i64>,
    /// The distance from the first slice to the last, in millimetres. Zero
    /// for a stack of one slice, which is a fact and not a hole.
    pub span_mm: Option<f64>,
    /// The midpoint of the first slice and the last, in the same frame as
    /// the positions: where along the slice normal the stack sits (record 38,
    /// S2). Only a comparison between two stacks of one session means
    /// anything; the number alone is a coordinate of one scanner's table.
    pub centre_mm: Option<f64>,
    pub source: Source,
}

/// The coverage of the distinct positions of one stack's images, in any
/// order. A position that is not a number is not a position.
pub fn of(positions: &[f64]) -> Coverage {
    let mut values: Vec<f64> = positions
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    if values.is_empty() {
        return Coverage::default();
    }
    values.sort_by(f64::total_cmp);
    let mut n = 1;
    let mut last = values[0];
    for v in &values[1..] {
        if *v - last > SAME_POSITION_MM {
            n += 1;
            last = *v;
        }
    }
    let (low, high) = (values[0], values[values.len() - 1]);
    let hundredths = |v: f64| (v * 100.0).round() / 100.0;
    Coverage {
        n_slices: Some(n),
        span_mm: Some(hundredths(high - low)),
        centre_mm: Some(hundredths((low + high) / 2.0)),
        source: Source::SliceLocation,
    }
}

/// Whether two stacks cover measurably different ground. An unmeasured
/// coverage on either side differs from nothing: the answer is what can be
/// said, and nothing can be said about a stack whose files never placed their
/// slices.
pub fn differs(a: &Coverage, b: &Coverage) -> bool {
    let (Some(na), Some(nb)) = (a.n_slices, b.n_slices) else {
        return false;
    };
    if na != nb {
        return true;
    }
    match (a.span_mm, b.span_mm) {
        (Some(sa), Some(sb)) => {
            let tolerance = SAME_SPAN_MM.max(sa.abs().max(sb.abs()) * SAME_SPAN_FRACTION);
            (sa - sb).abs() > tolerance
        }
        _ => false,
    }
}

/// Whether two stacks sit in measurably different places along the slice
/// normal: their centres are further apart than the wider of
/// [`SAME_SPAN_MM`] and half the larger of their two slice steps (record 38,
/// S2). A centre either side never measured moves nothing.
///
/// **Why half a slice step.** Two stacks whose centres are closer than that
/// sample the same positions: every slice of one lies nearer a slice of the
/// other than any other slice does, so they cover one place, and a re-planned
/// rescan that lands a millimetre off its first attempt is still a rescan.
/// Beyond it the two sample different ground, and a second station of a
/// spine is tens of slices away rather than a fraction of one. The step is
/// the stack's own, the extent over the gaps between its slices, so the
/// tolerance follows the geometry the scanner used rather than a constant
/// that is too tight for a 5 mm brain stack and too loose for a 0.8 mm one.
/// A stack of one slice has no step and is held to the millimetre.
pub fn moved(a: &Coverage, b: &Coverage) -> bool {
    let (Some(ca), Some(cb)) = (a.centre_mm, b.centre_mm) else {
        return false;
    };
    let tolerance = SAME_SPAN_MM.max(step(a).max(step(b)) / 2.0);
    (ca - cb).abs() > tolerance
}

/// The distance between neighbouring slices, measured as the extent over
/// the gaps. Zero where there is no gap to measure.
fn step(c: &Coverage) -> f64 {
    match (c.n_slices, c.span_mm) {
        (Some(n), Some(span)) if n > 1 => span.abs() / (n - 1) as f64,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_are_counted_by_position_and_not_by_image() {
        // three prescribed slices, each imaged twice: a dynamic series
        let c = of(&[1.0, 2.0, 3.0, 1.0, 2.0, 3.0]);
        assert_eq!(c.n_slices, Some(3));
        assert_eq!(c.span_mm, Some(2.0));
        assert_eq!(c.source.name(), "slice_location");
    }

    #[test]
    fn positions_a_hair_apart_are_one_slice() {
        let c = of(&[12.5, 12.499999, 12.500001, 15.0]);
        assert_eq!(c.n_slices, Some(2), "the jitter is not a slice");
        assert_eq!(c.span_mm, Some(2.5));
    }

    #[test]
    fn a_stack_with_no_positions_says_so_rather_than_counting_none() {
        let c = of(&[]);
        assert_eq!(c.n_slices, None);
        assert_eq!(c.span_mm, None);
        assert_eq!(c.source.name(), "none");
    }

    #[test]
    fn one_slice_covers_nothing_and_says_so() {
        let c = of(&[-7.25]);
        assert_eq!(c.n_slices, Some(1));
        assert_eq!(c.span_mm, Some(0.0));
    }

    #[test]
    fn a_negative_and_a_positive_side_measure_the_whole_extent() {
        let c = of(&[-30.0, 0.0, 30.0]);
        assert_eq!(c.n_slices, Some(3));
        assert_eq!(c.span_mm, Some(60.0));
    }

    #[test]
    fn a_slice_more_is_a_difference() {
        let a = of(&[0.0, 5.0, 10.0]);
        let b = of(&[0.0, 5.0, 10.0, 15.0]);
        assert!(differs(&a, &b));
    }

    #[test]
    fn the_same_prescription_twice_does_not_differ() {
        // the second acquisition sits a millimetre lower and its positions
        // carry the scanner's own rounding
        let a = of(&[0.0, 5.0, 10.0, 15.0, 20.0]);
        let b = of(&[-1.0, 4.000001, 9.0, 14.0, 18.9]);
        assert!(!differs(&a, &b), "{a:?} against {b:?}");
    }

    #[test]
    fn a_long_stack_is_compared_in_proportion() {
        let a = Coverage {
            n_slices: Some(200),
            span_mm: Some(400.0),
            centre_mm: Some(0.0),
            source: Source::SliceLocation,
        };
        let within = Coverage {
            span_mm: Some(403.0),
            ..a
        };
        let beyond = Coverage {
            span_mm: Some(420.0),
            ..a
        };
        assert!(!differs(&a, &within), "three parts in four hundred");
        assert!(differs(&a, &beyond));
    }

    #[test]
    fn an_unmeasured_coverage_differs_from_nothing() {
        let known = of(&[0.0, 5.0]);
        let unknown = of(&[]);
        assert!(!differs(&known, &unknown));
        assert!(!differs(&unknown, &known));
        assert!(!differs(&unknown, &unknown));
    }

    #[test]
    fn the_centre_is_the_midpoint_of_the_positions() {
        let c = of(&[-30.0, 0.0, 30.0, 60.0]);
        assert_eq!(c.centre_mm, Some(15.0));
        assert_eq!(of(&[-7.25]).centre_mm, Some(-7.25));
        assert_eq!(of(&[]).centre_mm, None);
    }

    #[test]
    fn two_stations_of_one_prescription_are_two_places() {
        // Record 38 S2: the count and the extent agree, so `differs` cannot
        // tell them apart, and the centre is 200 mm down the table.
        let upper = of(&[0.0, 5.0, 10.0, 15.0, 20.0]);
        let lower = of(&[-200.0, -195.0, -190.0, -185.0, -180.0]);
        assert!(!differs(&upper, &lower));
        assert!(moved(&upper, &lower));
    }

    #[test]
    fn a_rescan_less_than_half_a_slice_off_is_the_same_place() {
        // A 5 mm step: a re-planned rescan 2 mm off still samples the same
        // positions, and one 3 mm off samples between them.
        let first = of(&[0.0, 5.0, 10.0, 15.0, 20.0]);
        let near = of(&[2.0, 7.0, 12.0, 17.0, 22.0]);
        let between = of(&[3.0, 8.0, 13.0, 18.0, 23.0]);
        assert!(!moved(&first, &near));
        assert!(moved(&first, &between));
        // and a fine stack is held to the millimetre, not to half its step
        let fine = of(&[0.0, 0.8, 1.6, 2.4]);
        let off = Coverage {
            centre_mm: fine.centre_mm.map(|c| c + 1.5),
            ..fine
        };
        assert!(moved(&fine, &off));
        let within = Coverage {
            centre_mm: fine.centre_mm.map(|c| c + 0.9),
            ..fine
        };
        assert!(!moved(&fine, &within));
    }

    #[test]
    fn an_unmeasured_centre_moves_nothing() {
        let known = of(&[0.0, 5.0]);
        let unknown = of(&[]);
        assert!(!moved(&known, &unknown));
        assert!(!moved(&unknown, &known));
    }
}
