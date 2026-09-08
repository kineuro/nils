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

/// What the evaluator noticed and did not act on (Wave 4c §6.6, Wave 2 §10's
/// names): a rule that reached an axis an earlier set had closed with a
/// different answer (`axis_conflict`, both recorded), an axis of this phase
/// that ended with no value and no default (`axis_unresolved`), and a keyword
/// that matched on this stack but was never cited because an earlier rule
/// or an earlier keyword in the same list won (`keyword_shadowed`). None of
/// these change a verdict; they are counted per batch by whoever stores it.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Diagnostic {
    pub kind: String,
    pub axis: String,
    /// The rule set and rule that were pre-empted, and what they would have
    /// said, citing what.
    pub rule_set: String,
    pub rule: String,
    pub value: String,
    pub matched: String,
    /// What pre-empted it: the set, the rule, its value and its citation.
    pub by_rule_set: String,
    pub by_rule: String,
    pub by_value: String,
    pub by_matched: String,
}

/// The most diagnostics one verdict keeps. A stack that trips more than this
/// is a question about the pack, and the count says so without the list.
pub const DIAGNOSTICS_MAX: usize = 64;

/// What a pack decided about one stack.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Verdict {
    pub axes: Vec<AxisVerdict>,
    pub evidence: Vec<Evidence>,
    /// The rule sets that were entered, in the order they ran.
    pub entered: Vec<String>,
    /// Whether the pack says nobody is to be asked about this stack, however
    /// weakly an axis resolved (§8.2). A localizer excluded on purpose is
    /// the case it exists for.
    pub silent: bool,
    /// Wave 4c §6.6: what the evaluator noticed and did not act on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
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
