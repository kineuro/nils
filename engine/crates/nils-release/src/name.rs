// SPDX-License-Identifier: AGPL-3.0-only

//! The `descriptive` layout's grammar
//! (`docs/specs/wave3-anonymize-and-bids.md`, §9.1).
//!
//! v0's, carried because it is good and because people have years of files
//! named this way:
//!
//! ```text
//! [BodyPart_]{Orient}_{base}_{acq}_{mods}_{technique}_{accel}_{construct}
//!     [_CE][_b{N}][_{PE}][_{n}dir][_e{k}|_ti{k}]
//! ```
//!
//! It names **every** stack in the archive, including the 56.9 percent BIDS
//! cannot place: BIDS has no suffix for a localizer, a reformat, a projection,
//! an SWI image or a synthetic contrast, which is not a defect in BIDS because
//! those are not acquisitions.
//!
//! Three things differ from v0, each from a fault v0's own bug report names.
//!
//! 1. The echo suffix comes from the **measured echo number** rather than from
//!    the stack's index within its series. Siemens exports every echo as its
//!    own series, so v0's condition (`is_multi_stack_series`) is false for all
//!    of them, every echo of a session builds an identical name, and they fall
//!    through to a generic `_1`, `_2` counter that does not even correspond
//!    between magnitude and phase.
//! 2. Disambiguation is computed over the session **as the registry holds
//!    it**, not over the selection, which is v0's fourth naming bug.
//! 3. A character a filesystem cannot take is mapped by a rule declared here
//!    rather than left to the converter. v0 hands `T2*w` to dcm2niix, which
//!    strips the star, so the archive is full of `Ax_T2_w_2D_MEGRE` where
//!    `Ax_T2starw_2D_MEGRE` was meant.

use std::collections::BTreeMap;

/// What a stack is called, before anything about its siblings is known.
#[derive(Debug, Clone, Default)]
pub struct Fields<'a> {
    pub body_part: Option<&'a str>,
    pub spinal_cord: bool,
    pub orientation: Option<&'a str>,
    pub base: Option<&'a str>,
    /// `2D` or `3D`.
    pub acquisition_type: Option<&'a str>,
    /// Comma-joined, as the axis stores it.
    pub modifier: Option<&'a str>,
    pub technique: Option<&'a str>,
    pub acceleration: Option<&'a str>,
    pub construct: Option<&'a str>,
    pub post_contrast: bool,
    /// v0's `directory_type`, which decides whether the diffusion suffix
    /// applies.
    pub datatype: Option<&'a str>,
    pub dwi_b_value: Option<f64>,
    pub dwi_pe_direction: Option<&'a str>,
    pub dwi_directions: Option<i64>,
}

/// v0's abbreviations, which is what is already on disk.
fn orient(name: &str) -> String {
    match name {
        "Axial" => "Ax".to_string(),
        "Coronal" => "Cor".to_string(),
        "Sagittal" => "Sag".to_string(),
        other => other.chars().take(3).collect(),
    }
}

/// v0's body-part prefixes. A body part outside the map adds no prefix **and
/// suppresses the `SC` fallback**, which is v0's rule and is kept: a stack that
/// says "brain" is not a spinal cord however a flag was set.
fn body_part(name: &str) -> Option<&'static str> {
    match name {
        "spine" => Some("SC"),
        "neck" => Some("Neck"),
        "brain-neck" => Some("BrainNeck"),
        _ => None,
    }
}

/// Constructs whose `b=0` is the scanner stamping a derived image rather than
/// a shell that was played. v0's list, and its argument: a genuine b=0
/// acquisition carries no construct.
const DERIVED_DWI: &[&str] = &["trace", "adc", "fa", "colfa", "isodwi"];

/// One stack's name, from its own fields alone.
pub fn describe(f: &Fields, include_acceleration: bool, include_contrast: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(prefix) = f.body_part.and_then(body_part) {
        parts.push(prefix.to_string());
    } else if f.spinal_cord && f.body_part.is_none_or(str::is_empty) {
        parts.push("SC".to_string());
    }
    let mut push = |v: Option<&str>| {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            parts.push(v.replace(',', "-"));
        }
    };
    push(f.orientation.map(orient).as_deref());
    push(f.base);
    push(f.acquisition_type);
    push(f.modifier);
    push(f.technique);
    if include_acceleration {
        push(f.acceleration);
    }
    push(f.construct);

    let mut name = if parts.is_empty() {
        "unknown".to_string()
    } else {
        parts.join("_")
    };
    if include_contrast && f.post_contrast {
        name.push_str("_CE");
    }
    if f.datatype == Some("dwi")
        && let Some(suffix) = diffusion(f)
    {
        name.push('_');
        name.push_str(&suffix);
    }
    filename_safe(&name)
}

fn diffusion(f: &Fields) -> Option<String> {
    let constructs: Vec<String> = f
        .construct
        .unwrap_or("")
        .split(',')
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty())
        .collect();
    let derived = constructs.iter().any(|c| DERIVED_DWI.contains(&c.as_str()));
    let mut tokens: Vec<String> = Vec::new();
    if let Some(b) = f.dwi_b_value
        && b.is_finite()
        && !(derived && b == 0.0)
    {
        tokens.push(format!("b{}", b.trunc() as i64));
    }
    if let Some(pe) = f.dwi_pe_direction.filter(|p| !p.is_empty()) {
        tokens.push(pe.to_string());
    }
    if let Some(n) = f.dwi_directions.filter(|n| *n > 0) {
        tokens.push(format!("{n}dir"));
    }
    (!tokens.is_empty()).then(|| tokens.join("_"))
}

/// Map what a filesystem or a downstream tool cannot take.
///
/// One value in the archive's whole vocabulary has a hostile character, and it
/// is the one that matters: `T2*w`. v0 hands it to dcm2niix, which strips the
/// star and writes `T2_w`, so the axis name is corrupted in every file of a
/// multi-echo GRE. The star becomes `star`, which is also the word BIDS uses,
/// and everything else hostile becomes a hyphen rather than being dropped: a
/// dropped character silently joins two tokens into one.
pub fn filename_safe(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '*' => out.push_str("star"),
            '/' | '\\' | ':' | '?' | '"' | '<' | '>' | '|' => out.push('-'),
            c if c.is_control() || c == ' ' || c == '\t' => out.push('-'),
            c => out.push(c),
        }
    }
    // A run of separators reads as a slot that is missing rather than as one
    // that was mapped, so it is collapsed; and a name may not begin or end
    // with one, nor end with a dot, which Windows drops.
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches(['-', '_', '.', ' ']).to_string()
}

/// One stack, as the layout needs it once its name is built.
#[derive(Debug, Clone)]
pub struct Named {
    pub stack: i64,
    /// The name its own fields give it.
    pub name: String,
    /// The folder it lands in.
    pub folder: String,
    /// `EchoNumbers` as the scanner wrote it, which is what identifies an
    /// echo whether the vendor splits echoes within a series or across them.
    pub echo: Option<i64>,
    /// The inversion time, for a series split by it.
    pub inversion_time: Option<f64>,
    pub series: i64,
    /// Whether this stack's series holds more than one, which is a fact about
    /// the series and not about the selection.
    pub siblings: i64,
    /// Why the series split, when it did: `multi_echo`, `multi_ti`, ...
    pub split: Option<String>,
    /// Where it sits within its series, for a last-resort ordering.
    pub index: i64,
}

/// Give every stack of one bucket a name nothing else in it has.
///
/// The bucket is one subject's one session and one folder, **as the registry
/// holds it**: every stack that will land in that directory, whether or not
/// this release selected it. v0 computes the same thing over the already
/// filtered list, so exporting one echo of a two-echo series drops the echo
/// suffix and the file is named as though it were the only one.
///
/// Three passes, weakest last:
///
/// 1. an echo or inversion suffix, where the stacks differ in one;
/// 2. still colliding, a `_1`, `_2` counter in a fixed order;
/// 3. nothing else, because a name that needs more than this is a name the
///    grammar cannot make, and quietly numbering it hides that.
pub fn disambiguate(bucket: &mut [Named]) {
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, s) in bucket.iter().enumerate() {
        by_name.entry(s.name.clone()).or_default().push(i);
    }

    for (_, members) in by_name {
        if members.len() < 2 {
            // One stack of this name, and no suffix unless its own series says
            // it is one of several: v0 emits the echo suffix for a multi-stack
            // series whether or not anything collides, and those names are on
            // disk.
            for i in members {
                if let Some(suffix) = own_suffix(&bucket[i]) {
                    bucket[i].name = format!("{}_{suffix}", bucket[i].name);
                }
            }
            continue;
        }

        // The measured echo number first, which is what v0's own bug report
        // says the suffix should have come from all along.
        let echoes: Vec<Option<i64>> = members.iter().map(|i| bucket[*i].echo).collect();
        if echoes.iter().all(Option::is_some) && distinct(&echoes) {
            for i in &members {
                let echo = bucket[*i].echo.expect("all some");
                bucket[*i].name = format!("{}_e{echo}", bucket[*i].name);
            }
            continue;
        }
        // Then the inversion time, ordered, which has no measured index.
        let times: Vec<Option<i64>> = members
            .iter()
            .map(|i| bucket[*i].inversion_time.map(|t| (t * 1000.0) as i64))
            .collect();
        if times.iter().all(Option::is_some) && distinct(&times) {
            let mut order: Vec<usize> = members.clone();
            order.sort_by_key(|i| bucket[*i].inversion_time.map(|t| (t * 1000.0) as i64));
            for (n, i) in order.into_iter().enumerate() {
                bucket[i].name = format!("{}_ti{}", bucket[i].name, n + 1);
            }
            continue;
        }

        // And last a counter, in an order that does not depend on which rows
        // came back first: the series, then the stack within it.
        let mut order: Vec<usize> = members.clone();
        order.sort_by_key(|i| (bucket[*i].series, bucket[*i].index, bucket[*i].stack));
        for (n, i) in order.into_iter().enumerate() {
            bucket[i].name = format!("{}_{}", bucket[i].name, n + 1);
        }
    }
}

/// The suffix a stack gets from its own series alone, which is v0's rule.
fn own_suffix(s: &Named) -> Option<String> {
    if s.siblings <= 1 {
        return None;
    }
    match s.split.as_deref() {
        Some("multi_echo") => Some(format!("e{}", s.echo.unwrap_or(s.index + 1))),
        Some("multi_ti") => Some(format!("ti{}", s.index + 1)),
        _ => None,
    }
}

fn distinct<T: Ord + Clone>(values: &[T]) -> bool {
    let mut v = values.to_vec();
    v.sort();
    let n = v.len();
    v.dedup();
    v.len() == n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields<'a>() -> Fields<'a> {
        Fields {
            orientation: Some("Axial"),
            base: Some("T1w"),
            acquisition_type: Some("3D"),
            technique: Some("MPRAGE"),
            ..Fields::default()
        }
    }

    fn named(stack: i64, name: &str, echo: Option<i64>) -> Named {
        Named {
            stack,
            name: name.to_string(),
            folder: "anat".into(),
            echo,
            inversion_time: None,
            series: stack,
            siblings: 1,
            split: None,
            index: 0,
        }
    }

    #[test]
    fn the_grammar_is_v0_s() {
        assert_eq!(describe(&fields(), true, true), "Ax_T1w_3D_MPRAGE");
        let f = Fields {
            body_part: Some("spine"),
            modifier: Some("FatSat,FLAIR"),
            acceleration: Some("GRAPPA"),
            construct: Some("Magnitude"),
            post_contrast: true,
            ..fields()
        };
        assert_eq!(
            describe(&f, true, true),
            "SC_Ax_T1w_3D_FatSat-FLAIR_MPRAGE_GRAPPA_Magnitude_CE"
        );
    }

    #[test]
    fn a_slot_with_nothing_in_it_is_left_out() {
        let f = Fields {
            base: Some("T1w"),
            ..Fields::default()
        };
        assert_eq!(describe(&f, true, true), "T1w");
        assert_eq!(describe(&Fields::default(), true, true), "unknown");
    }

    #[test]
    fn the_acceleration_and_the_contrast_can_be_left_out() {
        // v0's two callers differ in exactly this: the QC holds pre and post
        // together in one bundle, so it passes false.
        let f = Fields {
            acceleration: Some("GRAPPA"),
            post_contrast: true,
            ..fields()
        };
        assert_eq!(describe(&f, false, false), "Ax_T1w_3D_MPRAGE");
    }

    #[test]
    fn a_body_part_outside_the_map_suppresses_the_spinal_cord_fallback() {
        // v0's rule: a stack that says brain is not a spinal cord however a
        // flag was set.
        let f = Fields {
            body_part: Some("brain"),
            spinal_cord: true,
            ..fields()
        };
        assert_eq!(describe(&f, true, true), "Ax_T1w_3D_MPRAGE");
        let f = Fields {
            body_part: None,
            spinal_cord: true,
            ..fields()
        };
        assert_eq!(describe(&f, true, true), "SC_Ax_T1w_3D_MPRAGE");
    }

    #[test]
    fn a_star_becomes_the_word_rather_than_being_stripped() {
        // v0 hands `T2*w` to dcm2niix, which strips the star, so the archive
        // is full of `Ax_T2_w_2D_MEGRE` where `Ax_T2starw_2D_MEGRE` was meant.
        let f = Fields {
            base: Some("T2*w"),
            acquisition_type: Some("2D"),
            technique: Some("MEGRE"),
            ..fields()
        };
        assert_eq!(describe(&f, true, true), "Ax_T2starw_2D_MEGRE");
    }

    #[test]
    fn everything_else_hostile_becomes_a_hyphen_and_not_nothing() {
        // A dropped character silently joins two tokens into one.
        assert_eq!(filename_safe("a/b"), "a-b");
        assert_eq!(filename_safe("a:b?c|d"), "a-b-c-d");
        assert_eq!(filename_safe("a  b"), "a-b");
        assert_eq!(filename_safe("_a_"), "a");
        assert_eq!(filename_safe("a."), "a");
        assert_eq!(filename_safe("a__b"), "a_b");
    }

    #[test]
    fn a_diffusion_name_carries_its_shell_direction_and_count() {
        let f = Fields {
            base: Some("DWI"),
            datatype: Some("dwi"),
            technique: Some("DWI-EPI"),
            acquisition_type: Some("2D"),
            dwi_b_value: Some(1000.0),
            dwi_pe_direction: Some("AP"),
            dwi_directions: Some(32),
            ..fields()
        };
        assert_eq!(describe(&f, true, true), "Ax_DWI_2D_DWI-EPI_b1000_AP_32dir");
    }

    #[test]
    fn a_derived_diffusion_image_does_not_claim_a_b_zero_shell() {
        // v0's argument, kept: the scanner stamps b=0 on a Trace or an ADC as
        // an artefact, and a genuine b=0 acquisition carries no construct.
        let f = Fields {
            base: Some("DWI"),
            datatype: Some("dwi"),
            construct: Some("ADC"),
            dwi_b_value: Some(0.0),
            ..Fields::default()
        };
        assert_eq!(describe(&f, true, true), "DWI_ADC");
        let real = Fields {
            base: Some("DWI"),
            datatype: Some("dwi"),
            dwi_b_value: Some(0.0),
            ..Fields::default()
        };
        assert_eq!(describe(&real, true, true), "DWI_b0");
    }

    #[test]
    fn a_diffusion_suffix_only_applies_to_diffusion() {
        let f = Fields {
            dwi_b_value: Some(1000.0),
            ..fields()
        };
        assert_eq!(describe(&f, true, true), "Ax_T1w_3D_MPRAGE");
    }

    #[test]
    fn echoes_split_across_series_carry_their_echo_number() {
        // The vendor Siemens is: every echo its own series, so v0's condition
        // is false for all of them, every echo builds an identical name, and
        // they fall through to a counter that does not correspond between
        // magnitude and phase.
        let mut bucket = vec![
            named(1, "Ax_T2starw_2D_MEGRE", Some(1)),
            named(2, "Ax_T2starw_2D_MEGRE", Some(2)),
            named(3, "Ax_T2starw_2D_MEGRE", Some(3)),
        ];
        disambiguate(&mut bucket);
        assert_eq!(bucket[0].name, "Ax_T2starw_2D_MEGRE_e1");
        assert_eq!(bucket[1].name, "Ax_T2starw_2D_MEGRE_e2");
        assert_eq!(bucket[2].name, "Ax_T2starw_2D_MEGRE_e3");
    }

    #[test]
    fn magnitude_and_phase_agree_about_which_echo_is_which() {
        // v0's counter numbers by arrival, so magnitude_1 and phase_1 need not
        // be the same echo. An echo number cannot disagree with itself.
        let mut bucket = vec![
            Named {
                name: "Ax_T2starw_Magnitude".into(),
                ..named(1, "", Some(2))
            },
            Named {
                name: "Ax_T2starw_Magnitude".into(),
                ..named(2, "", Some(1))
            },
            Named {
                name: "Ax_T2starw_Phase".into(),
                ..named(3, "", Some(2))
            },
            Named {
                name: "Ax_T2starw_Phase".into(),
                ..named(4, "", Some(1))
            },
        ];
        disambiguate(&mut bucket);
        assert_eq!(bucket[0].name, "Ax_T2starw_Magnitude_e2");
        assert_eq!(bucket[1].name, "Ax_T2starw_Magnitude_e1");
        assert_eq!(bucket[2].name, "Ax_T2starw_Phase_e2");
        assert_eq!(bucket[3].name, "Ax_T2starw_Phase_e1");
    }

    #[test]
    fn one_echo_of_a_two_echo_series_keeps_its_suffix() {
        // v0's fourth naming bug: it counts stacks per series over the already
        // filtered list, so exporting one echo of two drops the suffix and the
        // file is named as though it were the only one. `siblings` is a fact
        // about the series and not about the selection.
        let mut bucket = vec![Named {
            siblings: 2,
            split: Some("multi_echo".into()),
            ..named(1, "Ax_T2starw_2D_MEGRE", Some(2))
        }];
        disambiguate(&mut bucket);
        assert_eq!(bucket[0].name, "Ax_T2starw_2D_MEGRE_e2");
    }

    #[test]
    fn a_lone_stack_of_a_lone_series_gets_no_suffix() {
        let mut bucket = vec![named(1, "Ax_T1w_3D_MPRAGE", Some(1))];
        disambiguate(&mut bucket);
        assert_eq!(bucket[0].name, "Ax_T1w_3D_MPRAGE");
    }

    #[test]
    fn an_inversion_series_is_numbered_in_time_order() {
        let mut bucket = vec![
            Named {
                inversion_time: Some(900.0),
                ..named(1, "Ax_T1w_3D_MP2RAGE", None)
            },
            Named {
                inversion_time: Some(2750.0),
                ..named(2, "Ax_T1w_3D_MP2RAGE", None)
            },
        ];
        disambiguate(&mut bucket);
        assert_eq!(bucket[0].name, "Ax_T1w_3D_MP2RAGE_ti1");
        assert_eq!(bucket[1].name, "Ax_T1w_3D_MP2RAGE_ti2");
    }

    #[test]
    fn what_nothing_tells_apart_is_counted_in_a_fixed_order() {
        // And in an order that does not depend on which rows came back first,
        // so the same session names the same way twice.
        let mut forward = vec![
            named(1, "Ax_T1w_3D_MPRAGE", None),
            named(2, "Ax_T1w_3D_MPRAGE", None),
        ];
        let mut backward = vec![
            named(2, "Ax_T1w_3D_MPRAGE", None),
            named(1, "Ax_T1w_3D_MPRAGE", None),
        ];
        disambiguate(&mut forward);
        disambiguate(&mut backward);
        assert_eq!(forward[0].name, "Ax_T1w_3D_MPRAGE_1");
        assert_eq!(forward[1].name, "Ax_T1w_3D_MPRAGE_2");
        assert_eq!(backward[0].name, "Ax_T1w_3D_MPRAGE_2");
        assert_eq!(backward[1].name, "Ax_T1w_3D_MPRAGE_1");
    }
}
