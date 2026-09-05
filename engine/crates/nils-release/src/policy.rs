// SPDX-License-Identifier: AGPL-3.0-only

//! What a release declares it will do, and the combinations it refuses
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8 and §4.3).
//!
//! A policy is written down and recorded on the run, because **"de-identified"
//! is not a property a file can carry without saying under what rule**. v0's
//! category table is a menu: a deployment picks from it, the pick is a
//! command-line argument, and nothing in the output says which pick was made.

use crate::dates;
use crate::uid;

/// What a release does with UIDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Uids {
    /// Keyed and deterministic, so the same UID gives the same new one for
    /// ever and two releases of overlapping selections agree.
    #[default]
    Remap,
    /// As they are. A real policy for a recipient who has to match the release
    /// against a PACS, and constrained by §4.3.
    Preserve,
}

impl Uids {
    pub fn name(self) -> &'static str {
        match self {
            Uids::Remap => "remap",
            Uids::Preserve => "preserve",
        }
    }

    pub fn parse(text: &str) -> Option<Uids> {
        match text {
            "remap" => Some(Uids::Remap),
            "preserve" => Some(Uids::Preserve),
            _ => None,
        }
    }
}

/// Everything a release declares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Policy {
    pub dates: dates::Policy,
    pub uids: Uids,
    pub root: uid::Root,
}

/// A combination the engine will not write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

impl Policy {
    /// §4.3, which is the finding that most changes the release.
    ///
    /// A UID commonly embeds `YYYYMMDD`, and it is the last-resort source the
    /// date vote of §4 reads. So the two policies are one policy: a release
    /// that shifts or truncates dates and preserves UIDs has shifted nothing,
    /// because the true date leaves in the UID.
    ///
    /// **Refused rather than warned about.** A warning on a run that produced
    /// a tree is read after the tree exists, and by then the dataset has left.
    pub fn check(&self) -> Result<(), Refused> {
        if self.dates.moves_dates() && self.uids == Uids::Preserve {
            return Err(Refused(format!(
                "dates {} and UIDs preserved is not a policy: a UID commonly carries the \
                 acquisition date, so the date would leave in the UID and the policy would be \
                 decorative (§4.3). Remap the UIDs, or keep the dates.",
                self.dates.name()
            )));
        }
        Ok(())
    }

    /// How the run and the dataset description say what was done.
    pub fn describe(&self) -> String {
        let mut out = format!("dates {}, uids {}", self.dates.name(), self.uids.name());
        if self.uids == Uids::Remap {
            out.push_str(&format!(" under {}", self.root.as_str()));
        }
        out
    }

    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "dates": self.dates.name(),
            "uids": self.uids.name(),
            "uid_root": (self.uids == Uids::Remap).then(|| self.root.as_str()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeping_dates_and_preserving_uids_is_a_policy() {
        let p = Policy {
            dates: dates::Policy::Keep,
            uids: Uids::Preserve,
            ..Policy::default()
        };
        assert!(p.check().is_ok());
    }

    #[test]
    fn moving_dates_and_preserving_uids_is_refused_and_not_warned_about() {
        // The true date would leave in the UID, so the policy would be
        // decorative. A warning is read after the tree exists.
        for dates in [dates::Policy::Shift, dates::Policy::Year] {
            let p = Policy {
                dates,
                uids: Uids::Preserve,
                ..Policy::default()
            };
            let e = p.check().unwrap_err().to_string();
            assert!(e.contains("§4.3"), "{e}");
            assert!(e.contains("decorative"), "{e}");
        }
    }

    #[test]
    fn moving_dates_and_remapping_uids_is_the_combination_that_works() {
        for dates in [dates::Policy::Shift, dates::Policy::Year] {
            let p = Policy {
                dates,
                uids: Uids::Remap,
                ..Policy::default()
            };
            assert!(p.check().is_ok());
        }
    }

    #[test]
    fn a_policy_says_what_it_did_including_the_arc_it_hung_uids_from() {
        let p = Policy::default();
        assert_eq!(p.describe(), "dates keep, uids remap under 2.25");
        let kept = Policy {
            uids: Uids::Preserve,
            ..Policy::default()
        };
        assert_eq!(kept.describe(), "dates keep, uids preserve");
        assert_eq!(kept.as_json()["uid_root"], serde_json::Value::Null);
    }
}
