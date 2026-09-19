// SPDX-License-Identifier: AGPL-3.0-only

//! Whether a stack's pixels carry text
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.4).
//!
//! **The engine does not look at pixels** (§13). What it does is read what the
//! file says about them, and where the file says nothing, say how many it could
//! not judge rather than pretending it judged them.
//!
//! Three answers, and the third is the point. A stack the file says is burned
//! in is not written, and it raises a review item. A stack the file says is
//! clean is written. A stack whose `BurnedInAnnotation` is absent is neither:
//! it is written and **counted**, and the release says how many stacks it
//! could not judge, because "no tag" is not "no text" and a number a release
//! has to print is how an archive full of unjudgeable stacks gets confronted.
//!
//! Holding on an absent tag is what a site asks for with `--on-unknown hold`,
//! not what it gets by default. The tag is absent on most of the series of a
//! real archive, so a default that held on it held three quarters of what was
//! selected, wrote nothing at all for most of the subjects, and filed a
//! question per held stack on every release: a check nobody can run is not a
//! check, and a release nobody can make is not a policy.
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
///
/// Either way the stack is counted and the count is reported and recorded, so
/// the row says both how many could not be judged and what was done with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnUnknown {
    /// Write it and count it. The default (§8.4): what the file will not say
    /// is a number the release has to print, not a stack it keeps.
    #[default]
    Write,
    /// Hold it and raise a review item, one per stack and once. The strict
    /// setting, for a site that will not let an unjudged stack leave until a
    /// person has looked at it.
    Hold,
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
        // The third answer stays a third answer: what the release does with it
        // is a setting, and what it never does is call it clean.
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
    fn counting_what_cannot_be_judged_is_the_default_and_holding_is_the_setting() {
        // §8.4: where the tag is absent the release says how many stacks it
        // could not judge. A site that wants the stack held as well asks.
        assert_eq!(OnUnknown::default(), OnUnknown::Write);
        assert_eq!(OnUnknown::default().name(), "write");
        assert_eq!(OnUnknown::parse("hold"), Some(OnUnknown::Hold));
        assert_eq!(OnUnknown::parse("write"), Some(OnUnknown::Write));
        assert_eq!(OnUnknown::parse("nonsense"), None);
    }
}
