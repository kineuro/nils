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
        list: Vec<String>,
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

/// A value a rule sets on an axis, kept only when its condition holds.
#[derive(Debug, Clone)]
pub struct SetValue {
    /// Index into the axis's declared values.
    pub value: usize,
    pub when: Option<Expr>,
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
}

impl Axis {
    pub fn value_index(&self, id: &str) -> Option<usize> {
        self.values.iter().position(|v| v.id == id)
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
    /// Every rule that fires contributes, rather than the first one deciding.
    /// True for a multi-valued axis (modifier, construct, acceleration),
    /// false for everything else, including every route.
    pub collect: bool,
    /// The axes a rule of this set may write. Checked at load: a rule that
    /// sets an axis its set does not declare fails the pack.
    pub decides: Vec<usize>,
    /// A route is a rule set with one of these (§6.5). Nothing else
    /// distinguishes it.
    pub enter_when: Option<Expr>,
    pub rules: Vec<Rule>,
}
