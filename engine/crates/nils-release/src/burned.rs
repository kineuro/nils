// SPDX-License-Identifier: AGPL-3.0-only

//! Whether a stack's pixels carry text
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.4).
//!
//! **The engine does not look at pixels** (§13). What it does is read what the
//! file says about them, and where the file says nothing, say how many it could
//! not judge rather than pretending it judged them.
//!
//! Three answers, and the third is the point. A stack the file says is burned
//! in is not written. A stack the file says is clean is written. A stack whose
//! `BurnedInAnnotation` is absent is neither: it raises a review item and is
//! held, because "no tag" is not "no text", and an archive where 90 percent of
//! the stacks are unjudgeable is a fact a release should have to confront
//! rather than one it can average away.
//!
//! v0 has no such check at any level.

/// What the file says about its own pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// `BurnedInAnnotation` is `NO`, or the stack is one the pack ruled is not
    /// a picture of a screen.
    Clean,
    /// `BurnedInAnnotation` is `YES`, or the image type carries a token that
    /// means somebody photographed a screen.
    Burned,
    /// The tag is absent and nothing else decided. Not a synonym for clean.
    Unknown,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Clean => "clean",
            Verdict::Burned => "burned_in",
            Verdict::Unknown => "unjudged",
        }
    }
}

/// What a release does with a stack it cannot judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnUnknown {
    /// Hold it and raise a review item. The default, because a release is a
    /// thing that leaves.
    #[default]
    Hold,
    /// Write it. A real answer for an archive a person has already looked at,
    /// and one the release records so the tree says which was chosen.
    Write,
}

impl OnUnknown {
    pub fn name(self) -> &'static str {
        match self {
            OnUnknown::Hold => "hold",
            OnUnknown::Write => "write",
        }
    }

    pub fn parse(text: &str) -> Option<OnUnknown> {
        match text {
            "hold" => Some(OnUnknown::Hold),
            "write" => Some(OnUnknown::Write),
            _ => None,
        }
    }
}

/// The tokens of `ImageType` that mean a picture of a screen rather than of a
/// person. The same three the fingerprint's image role reads (§6).
const NOT_AN_IMAGE: &[&str] = &["SCREENSHOT", "PASTED", "ERROR"];

/// What the file says, from `BurnedInAnnotation` and `ImageType`.
///
/// `burned_in` is the value as read, and `image_role` is what the fingerprint
/// worked out (§6). The role is consulted first because it is the stronger
/// statement: a stack whose image type says `SCREENSHOT` is a photograph of a
/// screen whatever a `BurnedInAnnotation` of `NO` claims, and firmware that
/// writes one frequently writes the other by rote.
pub fn judge(
    burned_in: Option<&str>,
    image_role: Option<&str>,
    image_type: Option<&str>,
) -> Verdict {
    if image_role == Some("not_an_image") {
        return Verdict::Burned;
    }
    let it = image_type.unwrap_or("").to_ascii_uppercase();
    if it.split('\\').any(|t| NOT_AN_IMAGE.contains(&t.trim())) {
        return Verdict::Burned;
    }
    match burned_in.map(|v| v.trim().to_ascii_uppercase()) {
        Some(v) if v == "YES" => Verdict::Burned,
        Some(v) if v == "NO" => Verdict::Clean,
        _ => Verdict::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_saying_yes_is_believed() {
        assert_eq!(judge(Some("YES"), None, None), Verdict::Burned);
        assert_eq!(judge(Some(" yes "), None, None), Verdict::Burned);
    }

    #[test]
    fn the_file_saying_no_is_believed_too() {
        assert_eq!(
            judge(Some("NO"), Some("original_primary"), None),
            Verdict::Clean
        );
    }

    #[test]
    fn no_tag_is_not_the_same_as_no_text() {
        // The point of the third answer. An archive where most stacks are
        // unjudgeable is a fact a release should confront rather than average
        // away, and "absent" read as "clean" is how a screenshot leaves.
        assert_eq!(judge(None, None, None), Verdict::Unknown);
        assert_eq!(judge(Some(""), None, None), Verdict::Unknown);
        assert_eq!(judge(Some("MAYBE"), None, None), Verdict::Unknown);
    }

    #[test]
    fn an_image_type_that_says_screenshot_outranks_a_tag_that_says_no() {
        // Firmware that writes one frequently writes the other by rote, and a
        // photograph of a screen is a photograph of a screen.
        assert_eq!(
            judge(Some("NO"), None, Some("ORIGINAL\\SECONDARY\\SCREENSHOT")),
            Verdict::Burned
        );
        for token in ["SCREENSHOT", "PASTED", "ERROR"] {
            assert_eq!(
                judge(
                    Some("NO"),
                    None,
                    Some(&format!("ORIGINAL\\SECONDARY\\{token}"))
                ),
                Verdict::Burned,
                "{token}"
            );
        }
    }

    #[test]
    fn the_fingerprint_s_own_reading_is_the_first_thing_asked() {
        // §6 worked it out once, from the same three tokens, and three things
        // read it. Asking it here rather than parsing again is what keeps them
        // from disagreeing.
        assert_eq!(
            judge(Some("NO"), Some("not_an_image"), None),
            Verdict::Burned
        );
        assert_eq!(
            judge(Some("NO"), Some("original_primary"), None),
            Verdict::Clean
        );
    }

    #[test]
    fn a_token_inside_a_word_is_not_a_token() {
        assert_eq!(
            judge(Some("NO"), None, Some("ORIGINAL\\PRIMARY\\NOERROR")),
            Verdict::Clean
        );
    }

    #[test]
    fn holding_is_the_default_because_a_release_is_a_thing_that_leaves() {
        assert_eq!(OnUnknown::default(), OnUnknown::Hold);
        assert_eq!(OnUnknown::parse("write"), Some(OnUnknown::Write));
        assert_eq!(OnUnknown::parse("nonsense"), None);
    }
}
