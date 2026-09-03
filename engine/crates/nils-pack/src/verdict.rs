// SPDX-License-Identifier: AGPL-3.0-only

//! What a pack decided, and what made it decide
//! (`docs/specs/wave2-fingerprint-and-classify.md`, §8.1).
//!
//! v0 computes evidence and confidence and then throws both away: its upsert
//! writes the verdict alone, so nothing about a classified stack explains
//! itself. Here the evidence is the verdict's other half, and it is what a
//! review queue shows a person and what makes a pack diff readable.

use serde::Serialize;

/// Why one value was decided.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Evidence {
    pub axis: String,
    pub value: String,
    /// Which clause kind fired: exclusive, keywords, combination, physics,
    /// stated, default.
    pub tier: String,
    pub confidence: f64,
    /// The rule set and the rule inside it, so `nils explain` can name them.
    pub rule_set: String,
    pub rule: String,
    /// Where the evidence was read: a flag, a text field, the provenance.
    pub source: String,
    /// What was found there: the flag's name, the keyword that matched.
    pub matched: String,
}

/// What one axis resolved to.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AxisVerdict {
    pub axis: String,
    /// One value for a single-valued axis, several for a multi-valued one,
    /// sorted and de-duplicated as v0 stores them.
    pub values: Vec<String>,
    pub confidence: f64,
    pub tier: String,
}

impl AxisVerdict {
    /// The axis as a row stores it: one value, or the values comma-joined.
    pub fn stored(&self) -> String {
        self.values.join(",")
    }
}

/// What a pack decided about one stack.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Verdict {
    pub axes: Vec<AxisVerdict>,
    pub evidence: Vec<Evidence>,
    /// The rule sets that were entered, in the order they ran.
    pub entered: Vec<String>,
}

impl Verdict {
    pub fn axis(&self, name: &str) -> Option<&AxisVerdict> {
        self.axes.iter().find(|a| a.axis == name)
    }

    /// The axis as a row stores it, or the empty string when the axis
    /// resolved to nothing at all.
    pub fn stored(&self, name: &str) -> String {
        self.axis(name).map(AxisVerdict::stored).unwrap_or_default()
    }
}
