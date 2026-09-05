// SPDX-License-Identifier: AGPL-3.0-only

//! What a diffusion stack turns out to be
//! (`docs/specs/wave3-anonymize-and-bids.md`, §6).
//!
//! Four values v0 derives: the shell, every shell, the anatomical
//! phase-encoding direction, and how many gradient directions were played.
//! Everything they are worked out from is something the scanner wrote about
//! this stack, so none of it waits for a classification.
//!
//! v0 runs this after classifying and only on stacks it routed to `dwi`, which
//! means a diffusion stack it failed to route never gets its b values read, and
//! then has less to be classified by. Doing it at the fingerprint breaks that
//! circle. What replaces the routing gate is an **evidence** gate: the loose
//! signals, which are the direction-count and phase-direction patterns in
//! prose, are only consulted once something specific has established that this
//! is diffusion at all. A b value in a tag, a gradient orientation, a
//! directionality, or a b value spelled out in the text all count as specific;
//! `_32_` in a protocol name does not.
//!
//! Three of v0's answers here are wrong for one reason each, and all three are
//! recorded in the tests:
//!
//! 1. v0 joins `mri_series_details`, which is keyed by series, to `instance`,
//!    so each of its "per instance" loops walks one row repeated once per
//!    image. Its list of b values can hold at most one measured value.
//! 2. The same join makes its set of gradient vectors hold at most one, so
//!    `count_gradient_directions` returns 1 and, being non-empty, never falls
//!    through to the text. Every diffusion stack with a stored gradient is
//!    recorded as having one direction.
//! 3. Its phase-encoding computation reads a column filled from the keyword
//!    `PhaseEncodingDirection`, which is not a DICOM element, so the column is
//!    always null and the code takes its `"COL"` default. Every Siemens
//!    direction v0 computed used the column cosine whatever the file said.
//!
//! v1 keeps the seven varying values per image, which is the fix for the first
//! two, and Wave 1 already reads `InPlanePhaseEncodingDirection`, which is the
//! fix for the third.

use std::collections::BTreeSet;

/// Above this a b value is a sentinel or a misread, not an acquisition.
const B_MAX: i64 = 10_000;

/// Philips writes this for a volume that is not diffusion weighted.
const PHILIPS_SENTINEL: f64 = 1e37;

/// One image's diffusion values, as the registry stores them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Image {
    /// `DiffusionBValue`, which may carry several values separated by `\`.
    pub b_value: Option<String>,
    pub gradient: Option<String>,
    pub directionality: Option<String>,
    pub siemens_b_value: Option<i64>,
    pub siemens_directionality: Option<String>,
    pub ge_b_value: Option<i64>,
    pub philips_b_value: Option<f64>,
}

impl Image {
    /// Whether this image says anything about diffusion at all.
    fn says_anything(&self) -> bool {
        self.b_value.is_some()
            || self.gradient.is_some()
            || self.directionality.is_some()
            || self.siemens_b_value.is_some()
            || self.siemens_directionality.is_some()
            || self.ge_b_value.is_some()
            || self.philips_b_value.is_some()
    }

    /// Whether this image is one of the unweighted volumes, which carries no
    /// gradient direction to count.
    fn unweighted(&self) -> bool {
        let d = self
            .siemens_directionality
            .as_deref()
            .or(self.directionality.as_deref())
            .unwrap_or("")
            .to_ascii_uppercase();
        d.contains("NONE") || d.contains("ISOTROPIC")
    }
}

/// What the stack's own row says, beyond its images.
#[derive(Debug, Clone, Default)]
pub struct Stack<'a> {
    /// `ImageOrientationPatient`, six cosines separated by `\`.
    pub orientation: Option<&'a str>,
    /// `InPlanePhaseEncodingDirection`: `ROW` or `COL`.
    pub in_plane: Option<&'a str>,
    /// The Siemens CSA `PhaseEncodingDirectionPositive`, per series.
    pub pe_positive: Option<i64>,
    /// GE's direction count, per series.
    pub ge_directions: Option<i64>,
    /// Every text field of the stack, folded and joined.
    pub text: Option<&'a str>,
}

/// The four values, with what answered each.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Derived {
    /// The shell: the largest b value that is not zero, or zero when every
    /// value is. What names a diffusion image.
    pub b_value: Option<f64>,
    /// Every distinct b value, ascending, comma separated.
    pub b_values: Option<String>,
    /// `private`, `standard`, `text`, or several of them joined by a comma.
    pub b_value_source: Option<String>,
    /// `AP`, `PA`, `RL`, `LR`, `IS` or `SI`.
    pub pe_direction: Option<String>,
    /// `geometry` when computed from the cosines, `text` when read from prose.
    pub pe_direction_source: Option<String>,
    pub directions: Option<i64>,
    /// `vendor_count`, `gradients` or `text`.
    pub directions_source: Option<String>,
}

/// Everything §6 derives about one diffusion stack.
pub fn derive(images: &[Image], stack: &Stack) -> Derived {
    let (b_values, b_source) = b_values(images, stack.text);
    // A stack is diffusion if something specific says so. The prose patterns
    // below are loose enough to fire on a protocol name that never went near a
    // gradient, so they are only asked once that is settled.
    let is_diffusion = images.iter().any(Image::says_anything)
        || stack.ge_directions.is_some()
        || !b_values.is_empty();
    if !is_diffusion {
        return Derived::default();
    }

    let (pe, pe_source) = pe_direction(images, stack);
    let (n, n_source) = directions(images, stack);
    Derived {
        b_value: (!b_values.is_empty()).then(|| {
            b_values
                .iter()
                .copied()
                .filter(|v| *v > 0)
                .max()
                .unwrap_or(0) as f64
        }),
        b_values: (!b_values.is_empty()).then(|| {
            b_values
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        }),
        b_value_source: (!b_values.is_empty()).then(|| b_source.join(",")),
        pe_direction: pe.map(str::to_string),
        pe_direction_source: pe.map(|_| pe_source.to_string()),
        directions: n,
        directions_source: n.map(|_| n_source.to_string()),
    }
}

/// Every b value of the stack, and which kinds of evidence contributed.
///
/// The private and the standard element are the same measurement written
/// twice, so both are read and the answers are unioned. v0 stops at the first
/// that answers, which loses a shell whenever one vendor tag omits it. Both are
/// per image, which is the point: a two-shell acquisition is two values in the
/// same stack.
///
/// The text is different in kind and is only consulted when the tags say
/// nothing or say only zero. v0's reason for that last clause is a good one and
/// is kept: a derived image such as a Trace or an ADC map carries b=0 from the
/// scanner even though the shell it came from was not zero, and the sequence
/// name is where the real value survives.
fn b_values(images: &[Image], text: Option<&str>) -> (BTreeSet<i64>, Vec<&'static str>) {
    let mut found = BTreeSet::new();
    let mut sources = Vec::new();

    // The private elements are found by their creator block (Wave 1 §6.2), so
    // a value that is there identifies its own vendor. v0 gates these on the
    // manufacturer string instead, and reads nothing at all when that string
    // is missing or spelled a way it does not recognise.
    for i in images {
        let mut any = false;
        for v in [i.siemens_b_value, i.ge_b_value] {
            any |= v.is_some_and(|v| plausible(v as f64).map(|b| found.insert(b)).is_some());
        }
        if let Some(v) = i.philips_b_value
            && v <= PHILIPS_SENTINEL
            && let Some(b) = plausible(v)
        {
            found.insert(b);
            any = true;
        }
        if any && !sources.contains(&"private") {
            sources.push("private");
        }
    }
    for i in images {
        let Some(raw) = &i.b_value else { continue };
        let mut any = false;
        for part in raw.split('\\') {
            let Ok(v) = part.trim().parse::<f64>() else {
                continue;
            };
            if v <= PHILIPS_SENTINEL
                && let Some(b) = plausible(v)
            {
                found.insert(b);
                any = true;
            }
        }
        if any && !sources.contains(&"standard") {
            sources.push("standard");
        }
    }

    if found.is_empty() || found.iter().copied().eq([0]) {
        let from_text = in_text(text.unwrap_or(""));
        if !from_text.is_empty() {
            sources.push("text");
            found.extend(from_text);
        }
    }
    (found, sources)
}

/// A b value as a whole number, when it is one an acquisition could have used.
fn plausible(v: f64) -> Option<i64> {
    if !v.is_finite() {
        return None;
    }
    let r = v.round() as i64;
    (0..=B_MAX).contains(&r).then_some(r)
}

/// The b values written into a name.
///
/// A scanner spells these a dozen ways: `b1000`, `b=1000`, `b 1000`, `1000b`,
/// `iso_b1000`, Siemens's `*ep_b1000t` and `*ep_b1000#32`, and the range form
/// `B0-500-1000`. This walks the text rather than matching a pattern, because
/// the shapes need to look behind and ahead of the number at once and Rust's
/// regular expressions have no way to say that.
fn in_text(text: &str) -> BTreeSet<i64> {
    let b: Vec<char> = text.chars().collect();
    let mut found = BTreeSet::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != 'b' && b[i] != 'B' {
            i += 1;
            continue;
        }
        // `b` has to start a token, or follow `iso`, which is how Philips
        // writes an isotropic image: `isob1000`.
        let starts = i == 0
            || matches!(b[i - 1], '_' | ' ' | '=' | '-' | ':' | '/' | '*' | '\t')
            || (i >= 3
                && b[i - 3..i]
                    .iter()
                    .collect::<String>()
                    .eq_ignore_ascii_case("iso"));
        if !starts {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        // An optional separator between the letter and the number.
        if j < b.len() && matches!(b[j], ' ' | '=' | '-') {
            j += 1;
        }
        let (first, mut j2) = number(&b, j, 4);
        let Some(first) = first else {
            i += 1;
            continue;
        };
        let mut run = vec![first];
        // A range: `b0-500-1000`, or Siemens's `*ep_b0_1000`. `#` is not a
        // separator here: it introduces the direction index, not a shell.
        while j2 < b.len() && matches!(b[j2], '-' | '/' | '_') {
            let (next, j3) = number(&b, j2 + 1, 4);
            let Some(next) = next else { break };
            run.push(next);
            j2 = j3;
        }
        // Siemens marks a Trace with a trailing `t`, and appends `#N` for the
        // direction index. Either may follow the number; nothing else may,
        // because `b1000mm` is a length and not a shell.
        let mut end = j2;
        if end < b.len() && (b[end] == 't' || b[end] == 'T') {
            end += 1;
        } else if end < b.len() && b[end] == '#' {
            let (_, after) = number(&b, end + 1, 4);
            end = after;
        }
        if end >= b.len() || !b[end].is_ascii_alphanumeric() {
            for v in run {
                if (0..=B_MAX).contains(&v) {
                    found.insert(v);
                }
            }
        }
        i = j2.max(i + 1);
    }
    found
}

/// The number at `from`, up to `most` digits, and where it ends.
fn number(b: &[char], from: usize, most: usize) -> (Option<i64>, usize) {
    let mut end = from;
    while end < b.len() && b[end].is_ascii_digit() && end - from < most {
        end += 1;
    }
    if end == from {
        return (None, from);
    }
    let text: String = b[from..end].iter().collect();
    (text.parse().ok(), end)
}

/// The anatomical direction the phase encoding ran in.
fn pe_direction(images: &[Image], stack: &Stack) -> (Option<&'static str>, &'static str) {
    if let Some(positive) = stack.pe_positive
        && let Some(iop) = stack.orientation
        && images.iter().any(Image::says_anything)
        && let Some(d) = from_geometry(iop, stack.in_plane, positive)
    {
        return (Some(d), "geometry");
    }
    (in_text_direction(stack.text.unwrap_or("")), "text")
}

/// The direction, from the slice's own cosines.
///
/// The phase encoding runs along one of the two in-plane axes, and which one is
/// what `InPlanePhaseEncodingDirection` says. Its cosine points somewhere in
/// patient space; the axis it points most along is the anatomical axis, its
/// sign says which way, and the Siemens CSA flag says whether the encoding ran
/// with that direction or against it.
///
/// v0 reads `InPlanePhaseEncodingDirection` from a column that is always null
/// and falls back to `COL`, so it always takes the column cosine. Here a stack
/// that does not say gets no answer from geometry rather than a guessed one:
/// the text is asked instead, and a wrong direction is worse than none, since
/// BIDS writes it into the file name as `dir-`.
fn from_geometry(iop: &str, in_plane: Option<&str>, positive: i64) -> Option<&'static str> {
    let cosines: Vec<f64> = iop
        .replace(',', "\\")
        .split('\\')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    if cosines.len() < 6 {
        return None;
    }
    let plane = in_plane?.trim().to_ascii_uppercase();
    let phase = match plane.as_str() {
        "COL" => &cosines[3..6],
        "ROW" => &cosines[0..3],
        _ => return None,
    };
    let axis = (0..3).max_by(|a, b| phase[*a].abs().total_cmp(&phase[*b].abs()))?;
    if phase[axis].abs() < 1e-6 {
        return None;
    }
    let sign = if phase[axis] >= 0.0 { 1 } else { -1 };
    let effective = if positive == 1 { sign } else { -sign };
    Some(match (axis, effective) {
        (0, 1) => "RL",
        (0, _) => "LR",
        (1, 1) => "AP",
        (1, _) => "PA",
        (2, 1) => "IS",
        (_, _) => "SI",
    })
}

/// The direction written into a name: `..._AP`, `DWI-PA`, `ep2d_diff_RL_...`.
fn in_text_direction(text: &str) -> Option<&'static str> {
    let upper = text.to_ascii_uppercase();
    let b: Vec<char> = upper.chars().collect();
    for i in 0..b.len().saturating_sub(1) {
        let before = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if !before {
            continue;
        }
        let after = i + 2 >= b.len() || !b[i + 2].is_ascii_alphanumeric();
        if !after {
            continue;
        }
        let pair: String = b[i..i + 2].iter().collect();
        if let Some(d) = ["AP", "PA", "RL", "LR"].iter().find(|d| ***d == *pair) {
            return Some(d);
        }
    }
    None
}

/// How many gradient directions were played.
fn directions(images: &[Image], stack: &Stack) -> (Option<i64>, &'static str) {
    // GE counts them itself, in a private element found by its creator block.
    if let Some(n) = stack.ge_directions
        && (1..=1024).contains(&n)
    {
        return (Some(n), "vendor_count");
    }
    // Otherwise count the distinct gradients, which is what the per-image
    // values are for. The unweighted volumes carry no direction and are left
    // out, which is what makes this the number of directions rather than the
    // number of volumes.
    let mut vectors: BTreeSet<Vec<i64>> = BTreeSet::new();
    for i in images {
        if i.unweighted() {
            continue;
        }
        let Some(raw) = &i.gradient else { continue };
        let parts: Vec<f64> = raw
            .split('\\')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        if parts.len() != 3 || parts.iter().all(|p| p.abs() <= 0.001) {
            continue;
        }
        // Rounded to four places before comparing, so that two images of one
        // direction are one direction and not two.
        vectors.insert(
            parts
                .iter()
                .map(|p| (p * 10_000.0).round() as i64)
                .collect(),
        );
    }
    if !vectors.is_empty() {
        return (Some(vectors.len() as i64), "gradients");
    }
    (in_text_directions(stack.text.unwrap_or("")), "text")
}

/// A direction count written into a name: `32 directions`, `dir32`, and the
/// Swedish `32 riktningar` the local protocols use.
fn in_text_directions(text: &str) -> Option<i64> {
    let lower = text.to_ascii_lowercase();
    let b: Vec<char> = lower.chars().collect();
    // `<number> dir|riktningar`
    for i in 0..b.len() {
        if !b[i].is_ascii_digit() || (i > 0 && b[i - 1].is_ascii_digit()) {
            continue;
        }
        let (Some(v), mut j) = number(&b, i, 3) else {
            continue;
        };
        while j < b.len() && b[j] == ' ' {
            j += 1;
        }
        let rest: String = b[j..b.len().min(j + 12)].iter().collect();
        if (rest.starts_with("dir") || rest.starts_with("rikt")) && (2..=256).contains(&v) {
            return Some(v);
        }
    }
    // `dir32`, `dir_32`, `dir 32`
    if let Some(at) = lower.find("dir") {
        let mut j = at + 3;
        while j < b.len() && matches!(b[j], ' ' | '_') {
            j += 1;
        }
        if let (Some(v), _) = number(&b, j, 3)
            && (2..=256).contains(&v)
        {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(v: &str) -> Image {
        Image {
            b_value: Some(v.into()),
            ..Image::default()
        }
    }

    fn grad(g: &str, directionality: &str) -> Image {
        Image {
            gradient: Some(g.into()),
            directionality: Some(directionality.into()),
            ..Image::default()
        }
    }

    fn text(t: &str) -> Stack<'_> {
        Stack {
            text: Some(t),
            ..Stack::default()
        }
    }

    #[test]
    fn a_two_shell_acquisition_is_two_shells() {
        // The whole reason the values moved to the image. v0 joins a
        // per-series table to instance, so its list holds one value however
        // many shells were played.
        let d = derive(&[b("0"), b("0"), b("1000"), b("1000")], &text("dwi"));
        assert_eq!(d.b_values.as_deref(), Some("0,1000"));
        assert_eq!(d.b_value, Some(1000.0), "the shell is the largest non-zero");
        assert_eq!(d.b_value_source.as_deref(), Some("standard"));

        let d = derive(&[b("0"), b("1000"), b("2000")], &text("dwi"));
        assert_eq!(d.b_values.as_deref(), Some("0,1000,2000"));
        assert_eq!(d.b_value, Some(2000.0));
    }

    #[test]
    fn a_private_and_a_standard_tag_are_one_measurement_written_twice() {
        let images = vec![
            Image {
                siemens_b_value: Some(0),
                b_value: Some("0".into()),
                ..Image::default()
            },
            Image {
                siemens_b_value: Some(1000),
                b_value: Some("1000".into()),
                ..Image::default()
            },
        ];
        let d = derive(&images, &text("ep2d_diff"));
        assert_eq!(d.b_values.as_deref(), Some("0,1000"));
        assert_eq!(
            d.b_value_source.as_deref(),
            Some("private,standard"),
            "both are named, because both were read"
        );
    }

    #[test]
    fn a_philips_sentinel_is_not_a_b_value() {
        let images = vec![
            Image {
                philips_b_value: Some(1000.0),
                ..Image::default()
            },
            Image {
                philips_b_value: Some(1.7e38),
                ..Image::default()
            },
        ];
        let d = derive(&images, &text("dwi"));
        assert_eq!(d.b_values.as_deref(), Some("1000"));
    }

    #[test]
    fn a_derived_image_keeps_the_shell_it_came_from() {
        // v0's argument, kept: a Trace carries b=0 from the scanner even
        // though the shell it was made from was not zero, and the sequence
        // name is where that survives.
        let d = derive(&[b("0")], &text("ax dwi trace *re_b1000t"));
        assert_eq!(d.b_values.as_deref(), Some("0,1000"));
        assert_eq!(d.b_value, Some(1000.0));
        assert_eq!(d.b_value_source.as_deref(), Some("standard,text"));
    }

    #[test]
    fn a_tag_that_answers_stops_the_text_being_asked() {
        // The name says b1000 and the tags say b800. The tags are the
        // measurement, so the name is not consulted at all.
        let d = derive(&[b("800")], &text("ax dwi b1000"));
        assert_eq!(d.b_values.as_deref(), Some("800"));
    }

    #[test]
    fn a_b_value_is_read_the_dozen_ways_a_scanner_writes_one() {
        let one = |t: &str| {
            derive(&[Image::default()], &text(t))
                .b_values
                .unwrap_or_default()
        };
        // nothing measured, so these have to establish diffusion themselves
        assert_eq!(one("ax dwi b1000"), "1000");
        assert_eq!(one("ax dwi b=1000"), "1000");
        assert_eq!(one("ax dwi b-1000"), "1000");
        assert_eq!(one("ax dwi b 1000"), "1000");
        assert_eq!(one("ax_dwi_b1000"), "1000");
        assert_eq!(one("iso_b1000"), "1000");
        assert_eq!(one("isob1000"), "1000", "Philips writes it closed up");
        assert_eq!(one("*ep_b1000t"), "1000", "Siemens marks a Trace with t");
        assert_eq!(
            one("*ep_b1000#32"),
            "1000",
            "and appends the direction index"
        );
        assert_eq!(one("B0-500-1000"), "0,500,1000", "a range");
        assert_eq!(
            one("*ep_b0_1000"),
            "0,1000",
            "Siemens writes a range with _"
        );
        assert_eq!(one("dwi b0 b1000"), "0,1000", "two of them in one name");
    }

    #[test]
    fn a_number_beside_a_b_that_is_not_a_shell_is_not_read() {
        let one = |t: &str| {
            derive(&[Image::default()], &text(t))
                .b_values
                .unwrap_or_default()
        };
        assert_eq!(one("t1 mprage"), "", "no b anywhere");
        assert_eq!(one("slab1000"), "", "the b does not start a token");
        assert_eq!(one("b1000mm"), "", "a length, not a shell");
        assert_eq!(one("b99999"), "", "above any acquisition");
    }

    #[test]
    fn a_stack_that_says_nothing_about_diffusion_is_left_alone() {
        // The gate that replaces v0's routing. `_32_` is a real Siemens
        // direction-count shape and would fire on this name, which never went
        // near a gradient.
        let d = derive(&[Image::default()], &text("t1_mprage_32_sag"));
        assert_eq!(d, Derived::default());
        assert_eq!(d.directions, None);
    }

    #[test]
    fn one_gradient_in_a_tag_is_enough_to_ask_the_rest() {
        let images = vec![grad("0.1\\0.2\\0.9", "DIRECTIONAL")];
        let d = derive(&images, &text("ep2d_diff_32_dir"));
        assert_eq!(d.directions, Some(1), "one distinct vector was stored");
        assert_eq!(d.directions_source.as_deref(), Some("gradients"));
    }

    #[test]
    fn the_directions_are_the_distinct_gradients() {
        // v0 can only ever see one, so it reports 1 for every diffusion stack
        // that has a gradient at all, and being non-empty it never falls
        // through to the text either.
        let images = vec![
            grad("0\\0\\0", "NONE"),
            grad("1\\0\\0", "DIRECTIONAL"),
            grad("0\\1\\0", "DIRECTIONAL"),
            grad("0\\0\\1", "DIRECTIONAL"),
            grad("0\\0\\1", "DIRECTIONAL"),
        ];
        let d = derive(&images, &text("ep2d_diff"));
        assert_eq!(
            d.directions,
            Some(3),
            "three distinct, the repeat counted once"
        );
        assert_eq!(d.directions_source.as_deref(), Some("gradients"));
    }

    #[test]
    fn an_unweighted_volume_carries_no_direction() {
        let images = vec![
            grad("0.577\\0.577\\0.577", "ISOTROPIC"),
            grad("1\\0\\0", "DIRECTIONAL"),
        ];
        assert_eq!(derive(&images, &text("dwi")).directions, Some(1));
    }

    #[test]
    fn a_vendor_that_counts_them_itself_is_believed_first() {
        let stack = Stack {
            ge_directions: Some(25),
            text: Some("dwi 32 directions"),
            ..Stack::default()
        };
        let images = vec![grad("1\\0\\0", "DIRECTIONAL")];
        let d = derive(&images, &stack);
        assert_eq!(d.directions, Some(25));
        assert_eq!(d.directions_source.as_deref(), Some("vendor_count"));
    }

    #[test]
    fn a_count_in_a_name_answers_when_no_gradient_was_stored() {
        let images = vec![Image {
            siemens_b_value: Some(1000),
            ..Image::default()
        }];
        let n = |t: &str| derive(&images, &text(t)).directions;
        assert_eq!(n("ep2d_diff 32 directions"), Some(32));
        assert_eq!(n("ep2d_diff 32dir"), Some(32));
        assert_eq!(n("ep2d_diff dir32"), Some(32));
        assert_eq!(n("ep2d_diff dir_32"), Some(32));
        assert_eq!(
            n("dti 6 riktningar"),
            Some(6),
            "the local protocols say this"
        );
        assert_eq!(n("ep2d_diff"), None, "and nothing is not a count");
    }

    #[test]
    fn the_phase_direction_comes_from_the_slice_cosines() {
        // An axial slice: row along +x, column along +y. Phase encoding along
        // the column is the anterior-posterior axis.
        let images = vec![grad("1\\0\\0", "DIRECTIONAL")];
        let axial = |in_plane, positive| Stack {
            orientation: Some("1\\0\\0\\0\\1\\0"),
            in_plane: Some(in_plane),
            pe_positive: Some(positive),
            text: None,
            ge_directions: None,
        };
        let d = derive(&images, &axial("COL", 1));
        assert_eq!(d.pe_direction.as_deref(), Some("AP"));
        assert_eq!(d.pe_direction_source.as_deref(), Some("geometry"));
        assert_eq!(
            derive(&images, &axial("COL", 0)).pe_direction.as_deref(),
            Some("PA")
        );
        // and along the row it is the left-right axis
        assert_eq!(
            derive(&images, &axial("ROW", 1)).pe_direction.as_deref(),
            Some("RL")
        );
        assert_eq!(
            derive(&images, &axial("ROW", 0)).pe_direction.as_deref(),
            Some("LR")
        );
    }

    #[test]
    fn a_stack_that_does_not_say_which_axis_is_not_guessed_at() {
        // v0 defaults to COL, and because the column it reads is always null
        // that default is what every Siemens direction it ever computed used.
        // A wrong direction is worse than none: BIDS writes it into the name.
        let images = vec![grad("1\\0\\0", "DIRECTIONAL")];
        let d = derive(
            &images,
            &Stack {
                orientation: Some("1\\0\\0\\0\\1\\0"),
                in_plane: None,
                pe_positive: Some(1),
                ..Stack::default()
            },
        );
        assert_eq!(d.pe_direction, None);
    }

    #[test]
    fn a_direction_in_a_name_answers_when_the_geometry_cannot() {
        let images = vec![grad("1\\0\\0", "DIRECTIONAL")];
        let d = |t| derive(&images, &text(t)).pe_direction;
        assert_eq!(d("ep2d_diff_b1000_AP").as_deref(), Some("AP"));
        assert_eq!(d("DWI-PA").as_deref(), Some("PA"));
        assert_eq!(d("ep2d diff RL 32dir").as_deref(), Some("RL"));
        assert_eq!(d("ax dwi").as_deref(), None);
        assert_eq!(d("ap_dwi").as_deref(), Some("AP"), "at the start of a name");
        assert_eq!(
            d("ax_grappa_dwi").as_deref(),
            None,
            "a pair inside a word is not a direction"
        );
    }

    #[test]
    fn a_malformed_orientation_falls_through_rather_than_lying() {
        let images = vec![grad("1\\0\\0", "DIRECTIONAL")];
        for iop in ["1\\0\\0", "", "a\\b\\c\\d\\e\\f"] {
            let d = derive(
                &images,
                &Stack {
                    orientation: Some(iop),
                    in_plane: Some("COL"),
                    pe_positive: Some(1),
                    text: Some("dwi_PA"),
                    ge_directions: None,
                },
            );
            assert_eq!(d.pe_direction.as_deref(), Some("PA"), "{iop}");
            assert_eq!(d.pe_direction_source.as_deref(), Some("text"));
        }
    }
}
