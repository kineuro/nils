// SPDX-License-Identifier: AGPL-3.0-only

//! Rule sets (`docs/specs/wave2-fingerprint-and-classify.md`, §6.3).
//!
//! One concept where v0 has two. Its detector scans an axis's values in
//! priority order and, per value, tries an exclusive flag, then a keyword,
//! then a combination of flags; its branch scans its own rules in a
//! deliberate order and, per rule, tries a flag, then a text hit. Value
//! major, tier minor, in both. So:
//!
//! - a **rule** is an ordered list of **clauses**; the first that holds fires
//!   it and is what the evidence cites, and the clause's tier fixes the
//!   confidence unless the rule states one;
//! - a **rule set** declares the axes it may decide, an optional condition
//!   for entering at all, and its rules in order;
//! - rule sets run in the pack's declared order, and an axis a rule set
//!   decided is not decided again.
//!
//! v0's `skip_base_detection`, `skip_construct_detection` and
//! `skip_technique_detection` booleans, threaded through its pipeline by
//! hand, are gone: "already decided" is the mechanism.

use crate::expr::Expr;

/// Which clause kind fired, which is what fixes the confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// A single definitive flag.
    Exclusive,
    /// A keyword found in the text.
    Keywords,
    /// Several flags together.
    Combination,
    /// Any one of several flags: v0's `alternative_flags`.
    Alternative,
    /// A physics window: a comparison on the acquisition's numbers.
    Physics,
    /// A rule written longhand, whose confidence the rule states.
    Stated,
    /// Nothing matched and the axis has a default.
    Default,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Exclusive => "exclusive",
            Tier::Keywords => "keywords",
            Tier::Combination => "combination",
            Tier::Alternative => "alternative",
            Tier::Physics => "physics",
            Tier::Stated => "stated",
            Tier::Default => "default",
        }
    }
}

/// One condition of a rule, with what it cites when it fires.
#[derive(Debug, Clone)]
pub enum Clause {
    /// A named flag holds. Cites the flag.
    Flag {
        tier: Tier,
        confidence: f64,
        name: String,
        flag: usize,
    },
    /// A keyword is found in a text field. Cites the keyword that matched,
    /// which is the first in the list order, as v0 does.
    Keywords {
        tier: Tier,
        confidence: f64,
        field: usize,
        /// The words, after any overlay.
        list: Vec<String>,
        /// The bucket the list was taken from, when the pack named one.
        bucket: Option<String>,
    },
    /// Any one of the named flags holds. Cites the one that did.
    AnyFlag {
        tier: Tier,
        confidence: f64,
        names: Vec<String>,
        flags: Vec<usize>,
    },
    /// Every named flag holds. Cites them all.
    Combination {
        tier: Tier,
        confidence: f64,
        names: Vec<String>,
        flags: Vec<usize>,
    },
    /// Anything the expression language can say. Cites what the pack wrote.
    When {
        tier: Tier,
        confidence: f64,
        cite: String,
        source: String,
        expr: Expr,
    },
}

impl Clause {
    pub fn tier(&self) -> Tier {
        match self {
            Clause::Flag { tier, .. }
            | Clause::Keywords { tier, .. }
            | Clause::AnyFlag { tier, .. }
            | Clause::Combination { tier, .. }
            | Clause::When { tier, .. } => *tier,
        }
    }

    /// What the clause fixes the confidence at when it fires, unless the rule
    /// states one of its own.
    pub fn confidence(&self) -> f64 {
        match self {
            Clause::Flag { confidence, .. }
            | Clause::Keywords { confidence, .. }
            | Clause::AnyFlag { confidence, .. }
            | Clause::Combination { confidence, .. }
            | Clause::When { confidence, .. } => *confidence,
        }
    }
}

impl RuleSet {
    /// Every axis any rule of this set reads, for the loader's phase check.
    pub fn axes_read(&self) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some(e) = &self.enter_when {
            e.axes_read(&mut out);
        }
        for d in &self.derives {
            for c in &d.cases {
                if let Some(w) = &c.when {
                    w.axes_read(&mut out);
                }
            }
        }
        for r in &self.rules {
            if let Some(g) = &r.requires {
                g.axes_read(&mut out);
            }
            for c in &r.clauses {
                if let Clause::When { expr, .. } = c {
                    expr.axes_read(&mut out);
                }
            }
            for s in &r.sets {
                for v in &s.values {
                    if let Some(w) = &v.when {
                        w.axes_read(&mut out);
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// A value a rule sets on an axis, kept only when its condition holds.
#[derive(Debug, Clone)]
pub struct SetValue {
    pub value: Which,
    pub when: Option<Expr>,
}

/// Which value: one the rule names, or one the rule set derived for this
/// stack. A route needs the second, because one SWI acquisition's outputs are
/// all GRE or all EPI and the rule that names the output should not have to
/// say which twice.
#[derive(Debug, Clone, Copy)]
pub enum Which {
    /// An index into the axis's declared values.
    Fixed(usize),
    /// Decided, and the answer is nothing. A quantitative map has no base
    /// contrast: it is a measurement, not a weighting, and v0 stores null
    /// there rather than a guess.
    Nothing,
    /// An index into the rule set's `derives`, whose cases are all values of
    /// the axis it is assigned to; the loader checks that.
    Derived(usize),
}

/// One derived value: ordered cases, the first that holds decides.
#[derive(Debug, Clone)]
pub struct Derive {
    pub name: String,
    pub cases: Vec<DeriveCase>,
}

#[derive(Debug, Clone)]
pub struct DeriveCase {
    pub when: Option<Expr>,
    /// The axis value this case gives.
    pub value: usize,
}

/// What a rule writes: one axis, one or more values.
#[derive(Debug, Clone)]
pub struct Sets {
    pub axis: usize,
    pub values: Vec<SetValue>,
}

/// One rule of a rule set.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    /// A condition the whole rule is gated on, whatever its clauses say.
    /// v0's `requires_derived` and its per-value provenance gate are these.
    pub requires: Option<Expr>,
    pub clauses: Vec<Clause>,
    pub sets: Vec<Sets>,
    /// The confidence the rule states, when it states one rather than taking
    /// the clause's tier.
    pub confidence: Option<f64>,
    /// Why, for the person reading the evidence later.
    pub why: Option<String>,
}

/// A set of values one axis may take, and how they are resolved.
#[derive(Debug, Clone)]
pub struct Axis {
    pub name: String,
    /// When this axis is decided (Wave 3 §7).
    pub phase: AxisPhase,
    /// Several values may hold at once (modifier, construct, acceleration).
    pub multi: bool,
    /// The identity of each value, in the order they are tried.
    pub values: Vec<AxisValue>,
    /// What the axis takes when nothing matched. A literal, not a value index:
    /// `Unknown` is what the axis says when the vocabulary said nothing, and
    /// no rule can reach it.
    pub default: Option<String>,
    /// The confidence the default carries. v0's provenance says 0.8 for "no
    /// specific provenance detected"; most axes say nothing and mean zero.
    pub default_confidence: f64,
    /// Whether a row stores the value's identity or its label. v0 stores the
    /// identity for technique and the label for modifier, which is how one
    /// thing came to have two names; a pack says which, once, per axis.
    pub stores_label: bool,
}

/// When an axis is decided.
///
/// A **disposition** axis says what to do with a stack rather than what it is,
/// and it is decided from the axes that say what it is. So it runs in a phase
/// of its own, after the passes, because a pass fills an axis and a disposition
/// worked out before that would be worked out from a gap.
///
/// The two phases are kept apart by the loader as well as by the runner: a rule
/// set decides axes of one phase only, and a rule of the first phase may not
/// read an axis of the second, because there is nothing there to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisPhase {
    /// What the stack is. Decided by the rules, over the fingerprint.
    #[default]
    Class,
    /// What to do with it. Decided after the passes, over what was decided.
    Disposition,
}

impl AxisPhase {
    pub fn name(self) -> &'static str {
        match self {
            AxisPhase::Class => "class",
            AxisPhase::Disposition => "disposition",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AxisValue {
    /// The identity, which every table is keyed on.
    pub id: String,
    /// The exclusion group this value belongs to, when it does: at most one
    /// member of a group may hold.
    pub group: Option<String>,
    /// Within a group, the lower number wins, and a tie keeps the one that
    /// comes first in the axis's order. v0 resolves it exactly so.
    pub priority: Option<i64>,
    /// What the row stores. v0 keys some tables on the identity and some on
    /// this, which is how `3D-TSE` and `SPACE` came to be two names for one
    /// thing; here the identity is the identity.
    pub label: String,
    /// The physics family, when the axis has them.
    pub family: Option<String>,
    /// Whether the axis's own rules try this value: a flag, a word or a
    /// window of the axis file reaches it. A value none reaches is
    /// vocabulary a route sets, or the axis's default, and an overlay may
    /// not make the axis try it.
    pub tried: bool,
    /// The words that reach this value in the text, after any overlay: the
    /// list a site amends as `lists.<axis>.<value>` (pack contract 5). The
    /// axis file's own list, or a longhand keyword rule's, or a bucket's.
    pub keywords: Vec<String>,
    /// The bucket the list is taken from, when the pack names one.
    pub bucket: Option<String>,
    /// How the value is reached other than by a word, as the pack wrote it.
    pub detection: Detection,
}

/// How an axis value is reached other than by a word, kept in the words the
/// pack wrote so that the packs door can say so (record 26, decision 12).
/// The flags and the physics stay the pack's: an overlay amends none of it.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    /// The one flag that decides the value outright.
    pub exclusive: Option<String>,
    /// Any one of these flags decides it.
    pub alternative: Vec<String>,
    /// Sets of flags that together decide it, each written `a+b` as the
    /// evidence cites it.
    pub combination: Vec<String>,
    /// The physics windows that reach it, tried after the vocabulary.
    pub physics: Vec<Window>,
}

/// One physics window of an axis value: the condition as the pack wrote it.
#[derive(Debug, Clone)]
pub struct Window {
    /// The `when` expression as written, or what a longhand rule cites.
    pub when: String,
    /// The confidence the window states, when it states one.
    pub confidence: Option<f64>,
    pub why: Option<String>,
}

/// The word lists a site may amend, as `axis.value` by identity (pack
/// contract 5): every value an axis's own rules try, and every value a
/// longhand rule reaches by a keyword clause. In the order the axes are
/// decided and their values tried, so that a door lists them the same way.
pub fn amendable(axes: &[Axis], rule_sets: &[RuleSet]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in axes {
        for v in &a.values {
            let by_rule = rule_sets.iter().any(|s| {
                s.rules.iter().any(|r| {
                    r.clauses
                        .iter()
                        .any(|c| matches!(c, Clause::Keywords { .. }))
                        && r.sets.iter().any(|s| {
                            axes[s.axis].name == a.name
                                && s.values.iter().any(|sv| {
                                    matches!(sv.value, Which::Fixed(i) if a.values[i].id == v.id)
                                })
                        })
                })
            });
            if v.tried || by_rule {
                out.push(format!("{}.{}", a.name, v.id));
            }
        }
    }
    out
}

/// What each axis value's rules say about it, gathered from the rule sets
/// once they are loaded: its words after any overlay, the bucket they came
/// from, and the flags and windows that reach it. The axis file's own
/// physics windows are written by the loader, which has them as written;
/// a longhand rule's are its citation.
pub fn describe(axes: &mut [Axis], rule_sets: &[RuleSet]) {
    for set in rule_sets {
        for rule in &set.rules {
            for s in &rule.sets {
                let own = set.name == axes[s.axis].name;
                let targets: Vec<usize> = s
                    .values
                    .iter()
                    .filter_map(|v| match v.value {
                        Which::Fixed(i) => Some(i),
                        _ => None,
                    })
                    .collect();
                for i in targets {
                    let value = &mut axes[s.axis].values[i];
                    for c in &rule.clauses {
                        match c {
                            Clause::Keywords { list, bucket, .. } => {
                                value.keywords = crate::overlay::merge(
                                    &value.keywords,
                                    &crate::overlay::Edit {
                                        add: list.clone(),
                                        remove: Vec::new(),
                                    },
                                );
                                if value.bucket.is_none() {
                                    value.bucket = bucket.clone();
                                }
                            }
                            Clause::Flag { name, .. } => {
                                if value.detection.exclusive.is_none() {
                                    value.detection.exclusive = Some(name.clone());
                                } else if !value.detection.alternative.contains(name) {
                                    value.detection.alternative.push(name.clone());
                                }
                            }
                            Clause::AnyFlag { names, .. } => {
                                for n in names {
                                    if !value.detection.alternative.contains(n) {
                                        value.detection.alternative.push(n.clone());
                                    }
                                }
                            }
                            Clause::Combination { names, .. } => {
                                let joined = names.join("+");
                                if !value.detection.combination.contains(&joined) {
                                    value.detection.combination.push(joined);
                                }
                            }
                            Clause::When { tier, cite, .. } => {
                                if *tier == Tier::Physics && !own {
                                    value.detection.physics.push(Window {
                                        when: cite.clone(),
                                        confidence: rule.confidence,
                                        why: rule.why.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Axis {
    pub fn value_index(&self, id: &str) -> Option<usize> {
        self.values.iter().position(|v| v.id == id)
    }

    /// The identity of the value a row stores.
    ///
    /// The inverse of [`Axis::stored`], and needed because an axis may store
    /// the label: `base` stores `T2*w` and its identity is `T2starw`, which is
    /// also the word BIDS uses. Anything keyed on the identity, a BIDS mapping
    /// among them, reads a row through this.
    pub fn id_of_stored(&self, stored: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|v| {
                if self.stores_label {
                    v.label == stored
                } else {
                    v.id == stored
                }
            })
            .map(|v| v.id.as_str())
    }

    /// What a row stores for this value.
    pub fn stored(&self, i: usize) -> &str {
        if self.stores_label {
            &self.values[i].label
        } else {
            &self.values[i].id
        }
    }
}

/// An ordered list of rules, the axes they may decide, and when the set is
/// entered at all.
#[derive(Debug, Clone)]
pub struct RuleSet {
    pub name: String,
    /// Values this rule set works out per stack before its rules run.
    pub derives: Vec<Derive>,
    /// Every rule that fires contributes, rather than the first one deciding.
    /// True for a multi-valued axis (modifier, construct, acceleration),
    /// false for everything else, including every route.
    pub collect: bool,
    /// Axes this set contributes to rather than decides: what it writes joins
    /// what an axis's own rules say instead of replacing it. v0's branches
    /// replace the construct list and add to the modifiers, and the
    /// difference is 35 stacks on the live corpus.
    pub adds: Vec<usize>,
    /// The axes a rule of this set may write. Checked at load: a rule that
    /// sets an axis its set does not declare fails the pack.
    pub decides: Vec<usize>,
    /// A route is a rule set with one of these (§6.5). Nothing else
    /// distinguishes it.
    pub enter_when: Option<Expr>,
    pub rules: Vec<Rule>,
    /// The phase of the axes it decides, which the loader checks are all one.
    pub phase: AxisPhase,
}
