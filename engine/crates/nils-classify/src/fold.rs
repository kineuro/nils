// SPDX-License-Identifier: AGPL-3.0-only

//! Folding text (`docs/specs/wave2-fingerprint-and-classify.md`, §4.2).
//!
//! Folding is what is true of text: Unicode NFKC, whitespace collapsed to
//! single spaces, the ends trimmed. **Case is kept.** A pack's first
//! normalizer step is a case-sensitive removal of a literal phrase, so a
//! lower-cased store would silently stop it from ever firing.
//!
//! What is true of MRI is not here. `ir` does not become `inversion-recovery`,
//! `*` does not become `star`, no token is dropped: that is §6.4, it belongs
//! to a pack, and it runs when a pack is loaded.

use unicode_normalization::UnicodeNormalization;

/// Fold one field. `None` and a value that folds to nothing are both `None`,
/// so an empty string never reaches a rule as a value.
pub fn fold(s: Option<&str>) -> Option<String> {
    let s = s?;
    // The common case by far: ASCII with single spaces and no ends to trim.
    // Checking costs a pass and saves an allocation plus the normalizer.
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.nfkc() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// The join v0 builds its text blob from: the present parts, in order,
/// separated by one space (`sort/fingerprint.py`, `_build_text_blob`).
pub fn join(parts: &[Option<&str>]) -> Option<String> {
    let mut out = String::new();
    for p in parts.iter().flatten() {
        if p.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(p);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// v0's contrast blob: the administration fields, each labelled, joined
/// (`sort/fingerprint.py`, `_build_contrast_blob`). Numbers are written the
/// way the registry stores them, so that the pack sees one spelling.
pub fn contrast(
    agent: Option<&str>,
    route: Option<&str>,
    dose: Option<f64>,
    start_time: Option<&str>,
    volume: Option<f64>,
    rate: Option<f64>,
    duration: Option<f64>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = agent.filter(|a| !a.is_empty()) {
        parts.push(a.to_string());
    }
    if let Some(r) = route.filter(|r| !r.is_empty()) {
        parts.push(r.to_string());
    }
    if let Some(d) = dose {
        parts.push(format!("dose {}", number(d)));
    }
    if let Some(v) = volume {
        parts.push(format!("volume {}", number(v)));
    }
    if let Some(r) = rate {
        parts.push(format!("rate {}", number(r)));
    }
    if let Some(d) = duration {
        parts.push(format!("duration {}", number(d)));
    }
    if let Some(t) = start_time.filter(|t| !t.is_empty()) {
        parts.push(format!("start {t}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// A double as the shortest text that reads back as the same number, so that
/// 15.0 and 15 are one token and not two.
fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_whitespace_and_keeps_case() {
        assert_eq!(
            fold(Some("  T2  FLAIR\tax \n")).as_deref(),
            Some("T2 FLAIR ax")
        );
        assert_eq!(fold(Some("")), None);
        assert_eq!(fold(Some("   ")), None);
        assert_eq!(fold(None), None);
    }

    #[test]
    fn keeps_the_nordic_letters_and_the_case_a_pack_needs() {
        // A pack's first step removes this phrase literally, uppercase and all.
        let s = fold(Some("RÖR  PÅ DXSIN SE PÄRM"));
        assert_eq!(s.as_deref(), Some("RÖR PÅ DXSIN SE PÄRM"));
    }

    #[test]
    fn nfkc_composes_a_decomposed_letter() {
        // "RO\u{308}R" is the same word as "RÖR" and v0 would not have matched it.
        let decomposed = "RO\u{308}R";
        assert_ne!(decomposed, "RÖR");
        assert_eq!(fold(Some(decomposed)).as_deref(), Some("RÖR"));
    }

    #[test]
    fn nfkc_folds_a_compatibility_character() {
        // A superscript two is a two; v0's filter would have dropped it.
        assert_eq!(fold(Some("T2\u{b2}")).as_deref(), Some("T22"));
    }

    #[test]
    fn joins_the_present_parts_in_order() {
        let parts = [Some("ax t2"), None, Some(""), Some("tse")];
        assert_eq!(join(&parts).as_deref(), Some("ax t2 tse"));
        assert_eq!(join(&[None, Some("")]), None);
    }

    #[test]
    fn writes_a_whole_dose_without_a_point() {
        let s = contrast(
            Some("Dotarem"),
            Some("IV"),
            Some(15.0),
            None,
            Some(7.5),
            None,
            None,
        );
        assert_eq!(s.as_deref(), Some("Dotarem IV dose 15 volume 7.5"));
        assert_eq!(contrast(None, None, None, None, None, None, None), None);
    }
}
