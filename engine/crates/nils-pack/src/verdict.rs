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
    /// What the tier read: header, name, inferred, default and so on
    /// ([`crate::rules::basis_of`]).
    pub basis: String,
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
    /// What the tier read, so a reader sees whether the header or the name
    /// decided the axis ([`crate::rules::basis_of`]).
    pub basis: String,
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

/// A rule as it stands in the pack's ranking (record 55 H3, 2026-10-09):
/// what it said, what it cited, its tier and confidence, and where it sits
/// in the order the pack runs its rules.
///
/// The order is the rank. Rule sets run in the pack's `order`, and an axis a
/// set decides is closed to every set after it; inside a set that decides,
/// the first rule that fires decides and the set stops; inside an exclusion
/// group the lower priority wins. A tier or a confidence never decides who
/// wins: they are what the winner wrote.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Ranked {
    pub rule_set: String,
    pub rule: String,
    /// What it cited: the flag, the keyword, or the words its condition
    /// gives for itself.
    pub matched: String,
    pub tier: String,
    pub confidence: f64,
    /// Its rule set's place among the sets the pack runs, from 0.
    pub rule_set_at: usize,
    /// Its own place in that set, from 0.
    pub rule_at: usize,
}

/// One answer the pack's ranking decided over another (record 55 H3,
/// 2026-10-09): never a question, kept on the stack's classification as
/// evidence of who beat whom.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Override {
    pub axis: String,
    /// What the winner stored.
    pub value: String,
    pub by: Ranked,
    /// What the rule it was decided over would have stored.
    pub other: String,
    pub over: Ranked,
    /// What ranked the one above the other: `rule_set_order` (an earlier
    /// rule set closed the axis), `rule_order` (an earlier rule of the same
    /// set fired first), `priority` (a lower priority of one exclusion group)
    /// or `answer` (a person's answer held the axis).
    pub rank: String,
}

/// One side of an equal-rank disagreement.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Side {
    pub value: String,
    #[serde(flatten)]
    pub ranked: Ranked,
}

/// Two answers on one axis that nothing in the pack's ranking puts one above
/// the other (record 55 H3, 2026-10-09): a defect of the pack, reported to
/// whoever tunes it and never a question.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EqualRank {
    pub axis: String,
    /// Why nothing ranked them: `one_rule` (one rule set two values on an
    /// axis that holds one), `collected` (rules of a set that collects wrote
    /// two values on an axis that holds one) or `priority_tie` (two values of
    /// one exclusion group at the same priority, where the first collected
    /// is kept).
    pub why: String,
    pub sides: Vec<Side>,
    /// What the stack carries: the values as a row stores them.
    pub kept: String,
}

impl EqualRank {
    /// Each pair of sides that disagree, as `axis: set/rule=value |
    /// set/rule=value` with the two sides in a fixed order, so the same two
    /// rules disagreeing on many stacks are one line.
    pub fn pairs(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, a) in self.sides.iter().enumerate() {
            for b in &self.sides[i + 1..] {
                if a.value == b.value {
                    continue;
                }
                let one = format!("{}/{}={}", a.ranked.rule_set, a.ranked.rule, a.value);
                let two = format!("{}/{}={}", b.ranked.rule_set, b.ranked.rule, b.value);
                let (x, y) = if one <= two { (one, two) } else { (two, one) };
                let line = format!("{}: {x} | {y}", self.axis);
                if !out.contains(&line) {
                    out.push(line);
                }
            }
        }
        out
    }
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
/// [`Vote`] is one of these holding on a stack, with a value. A voter is
/// identified by where it sits (set, rule, clause, axis, tier); `restates`
/// follows from that place, so equality and hashing leave it out and a
/// vote can find its voter without knowing it.
#[derive(Debug, Clone, Serialize)]
pub struct Voter {
    pub rule_set: String,
    pub rule: String,
    pub clause: usize,
    pub axis: String,
    pub tier: String,
    /// The clause only restates another axis ([`crate::Rule::restates`]):
    /// an implication of the schema, which a label model must not count as
    /// a second witness.
    pub restates: bool,
}

impl PartialEq for Voter {
    fn eq(&self, other: &Self) -> bool {
        (
            &self.rule_set,
            &self.rule,
            self.clause,
            &self.axis,
            &self.tier,
        ) == (
            &other.rule_set,
            &other.rule,
            other.clause,
            &other.axis,
            &other.tier,
        )
    }
}

impl Eq for Voter {}

impl std::hash::Hash for Voter {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        (
            &self.rule_set,
            &self.rule,
            self.clause,
            &self.axis,
            &self.tier,
        )
            .hash(h);
    }
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
                        restates: rule.restates(clause),
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
    /// Record 55 H3: every answer the ranking decided over another, uncapped,
    /// with both rules' tiers and confidences. The `axis_conflict`
    /// diagnostics count the same cases per batch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// Record 55 H3: the answers on one axis nothing ranked, a defect of the
    /// pack.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equal_rank: Vec<EqualRank>,
    /// The axes of this phase that hold one value, that no rule named a
    /// value for and that have no default, uncapped: what `axis_unresolved`
    /// counts, less the multi-valued axes, whose empty set is the answer
    /// "none". An axis a rule decided to be nothing is not here either; that
    /// is an answer too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<String>,
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
