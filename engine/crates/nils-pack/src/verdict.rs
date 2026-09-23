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

/// One witness on one stack (record 41, S2): a clause of a rule that held,
/// and the value its rule says for one axis.
///
/// The evidence records what decided an axis, which is the first clause of
/// the first rule that fired; a rule set that decides stops there, so the
/// rules behind it and the clauses behind the one cited are never heard. A
/// label model needs all of them, since a rule's accuracy cannot be
/// estimated from the stacks where an earlier rule spoke over it. A vote is
/// recorded wherever a rule set was entered, the rule's own condition held
/// and one of its clauses held, whether or not the rule decided anything;
/// it changes no verdict.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Vote {
    pub axis: String,
    /// The value as a row stores it, or the empty string where the rule
    /// decides the axis to nothing.
    pub value: String,
    pub rule_set: String,
    pub rule: String,
    /// The clause's place in its rule, from 0: an axis file's value rule
    /// has its exclusive flag, its words and its combination in that order.
    pub clause: usize,
    /// The clause's kind, as evidence names a tier.
    pub tier: String,
}

/// Who may vote: one clause of one rule, for one axis its rule writes. A
/// [`Vote`] is one of these holding on a stack, with a value.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
pub struct Voter {
    pub rule_set: String,
    pub rule: String,
    pub clause: usize,
    pub axis: String,
    pub tier: String,
}

/// Every voter a pack has, in the order its rule sets, rules, clauses and
/// the axes each rule writes are declared: what a vote can name, known before
/// any stack is read, so that a store can number them once per pack.
pub fn voters(pack: &crate::Pack) -> Vec<Voter> {
    let mut out: Vec<Voter> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for set in &pack.rule_sets {
        for rule in &set.rules {
            for (clause, c) in rule.clauses.iter().enumerate() {
                for sets in &rule.sets {
                    let v = Voter {
                        rule_set: set.name.clone(),
                        rule: rule.id.clone(),
                        clause,
                        axis: pack.axes[sets.axis].name.clone(),
                        tier: c.tier().name().to_string(),
                    };
                    if seen.insert(v.clone()) {
                        out.push(v);
                    }
                }
            }
        }
    }
    out
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
    /// Record 41, S2: every clause that held, when the verdict was asked for
    /// with its votes. Empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub votes: Vec<Vote>,
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
