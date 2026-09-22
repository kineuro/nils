// SPDX-License-Identifier: AGPL-3.0-only

//! What a release declares it will do
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8).
//!
//! A policy is written down and recorded on the run, because **"de-identified"
//! is not a property a file can carry without saying under what rule**. v0's
//! category table is a menu: a deployment picks from it, the pick is a
//! command-line argument, and nothing in the output says which pick was made.

use crate::uid;

/// What a release does with UIDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Uids {
    /// Keyed and deterministic, so the same UID gives the same new one for
    /// ever and two releases of overlapping selections agree.
    #[default]
    Remap,
    /// As they are. A real policy for a recipient who has to match the release
    /// against a PACS; since every release keeps the dates (record 38 S3), a
    /// date a UID carries is no leak.
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

/// What a release writes for the dates: the dates, always (record 38 S3).
///
/// Not a choice any more. The name stays on the row, in the dataset
/// description and in the content digest, so a reader from before reads the
/// truth and a tree released before record 38 re-runs unchanged.
pub const DATES: &str = "keep";

/// Everything a release declares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Policy {
    pub uids: Uids,
    pub root: uid::Root,
}

/// Where a run's policy comes from (record 26 §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// `--uids` was given (or `--dates keep`, which a caller from before
    /// record 38 may still send): the run's policy applies to every file,
    /// whatever its dataset says, and the row says so.
    Flags,
    /// Neither was given: each dataset's `on_release` applies to its own
    /// files, and the run's defaults to a file under no dataset.
    #[default]
    Datasets,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Flags => "flags",
            Source::Datasets => "datasets",
        }
    }
}

impl Policy {
    /// A dataset's leaving policy as its place declares it,
    /// `handling.on_release`, under the run's UID root; the defaults where
    /// nothing is declared. The dates are not read: every release keeps them
    /// (record 38 S3).
    pub fn of_handling(handling: &serde_json::Value, root: &uid::Root) -> Policy {
        let release = &handling["on_release"];
        Policy {
            uids: release["uids"]
                .as_str()
                .and_then(Uids::parse)
                .unwrap_or_default(),
            root: root.clone(),
        }
    }
}

impl Policy {
    /// How the run and the dataset description say what was done.
    pub fn describe(&self) -> String {
        let mut out = format!("dates {DATES}, uids {}", self.uids.name());
        if self.uids == Uids::Remap {
            out.push_str(&format!(" under {}", self.root.as_str()));
        }
        out
    }

    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "dates": DATES,
            "uids": self.uids.name(),
            "uid_root": (self.uids == Uids::Remap).then(|| self.root.as_str()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dataset_declares_its_leaving_policy_and_the_defaults_stand_in() {
        let declared = serde_json::json!({"on_release": {"uids": "preserve"}});
        let p = Policy::of_handling(&declared, &uid::Root::default());
        assert_eq!(p.uids, Uids::Preserve);
        let nothing = Policy::of_handling(&serde_json::Value::Null, &uid::Root::default());
        assert_eq!(nothing, Policy::default());
        assert_eq!(Source::default(), Source::Datasets);
        assert_eq!(Source::Flags.name(), "flags");
    }

    #[test]
    fn a_dataset_that_declared_a_moving_date_policy_before_is_read_and_keeps_its_dates() {
        // Record 38 S3: a handling written before the shift and the year were
        // removed still opens, and its files leave with their real dates.
        for dates in ["shift", "year"] {
            let declared = serde_json::json!({"on_release": {"dates": dates, "uids": "remap"}});
            let p = Policy::of_handling(&declared, &uid::Root::default());
            assert_eq!(p.uids, Uids::Remap);
            assert_eq!(p.as_json()["dates"], "keep");
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
