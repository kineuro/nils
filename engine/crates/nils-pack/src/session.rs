// SPDX-License-Identifier: AGPL-3.0-only

//! The session pass (record 53, S2): a stack whose own header is silent is
//! decided from the other stacks of its session.
//!
//! The deep research's case 9: a classifier that reads one series at a time
//! cannot tell a GE susceptibility image written ORIGINAL with nothing beside
//! it from the magnitude GE writes beside its computed outputs, and the
//! session can: the computed outputs sit at a related series number, with the
//! same geometry and frame of reference. The research recommends allowing
//! that only as a flagged secondary rule, certified apart, which never
//! overrides what a series' own header says. So:
//!
//! - **Only a rule of a `session_context` pass reads the session.** A pack
//!   declares which rules those are; a rule of a rule set never sees another
//!   stack.
//! - **A sibling is seen by its header alone**, through the fields the pass
//!   names (`sibling_fields`), and never by what a rule or a person decided
//!   about it. So the answer does not depend on the order stacks were
//!   classified in, and a header packet that carries those fields for each
//!   sibling replays the pass exactly.
//! - **It never overrides what the stack's own header states.** A value in
//!   force that a header tier decided (exclusive, combination, alternative,
//!   physics: flags and numbers the scanner wrote), that a person answered or
//!   a decision set, stays whatever the pack says. A value read from the
//!   series' name (keywords), inferred by a longhand rule (stated), a default
//!   or a vote may be replaced, and only at or below the pass's
//!   `replaces_at_most`: a name like `Ax SWAN` says the acquisition, not
//!   which of its outputs a stack is, which is the question the session
//!   answers. A rule whose answer would change a protected axis changes
//!   nothing at all, and says why it held back. The protection is the
//!   engine's, not a knob.
//! - **It is flagged.** Every value it writes has tier `session`, whose basis
//!   is `session`, so a reader, a grade and a learner can tell it from what
//!   the header decided and count it apart.
//!
//! This module is the decision, for one stack and its siblings. The engine
//! runs it over the registry (`nils-classify`), and a replay runs it over
//! header packets; both call [`decide`].

use crate::expr::Expr;
use crate::stack::Stack;
use crate::{Evaluated, Pack};

/// The tier a session answer is written at, and its basis.
pub const TIER: &str = "session";

/// The `session_context` kind: what a sibling is seen by, and the rules.
#[derive(Debug, Clone)]
pub struct Session {
    /// The fields of a sibling a rule may read, by field number. A sibling's
    /// `when` may read nothing else, through any flag, parser or derived
    /// text, and the loader checks it; a packet's session list carries these
    /// and nothing more.
    pub sibling_fields: Vec<usize>,
    /// A value in force decided at a confidence strictly above this stays,
    /// whatever decided it.
    pub replaces_at_most: f64,
    /// Tried in order; the first whose conditions hold decides.
    pub rules: Vec<SessionRule>,
}

/// One rule that may read the session.
#[derive(Debug, Clone)]
pub struct SessionRule {
    pub name: String,
    /// On the stack itself, beyond the pass's target: its fields, flags and
    /// decided axes.
    pub when: Option<Expr>,
    pub sibling: Sibling,
    pub require: Require,
    /// What it writes: an axis, by number, and its values as a row stores
    /// them, none for an axis decided to nothing.
    pub sets: Vec<(usize, Vec<String>)>,
    pub confidence: f64,
    pub why: String,
    pub sources: Vec<String>,
}

/// Which other stacks of the session count, and what one must hold.
#[derive(Debug, Clone)]
pub struct Sibling {
    /// How the series numbers must relate, when said.
    pub series_number: Option<SeriesNumber>,
    /// Fields whose values must equal the stack's own, as text.
    pub same: Vec<usize>,
    /// The sibling's series must share the stack's frame of reference. A
    /// sibling whose frame is unknown does not count.
    pub same_frame_of_reference: bool,
    /// Whether the other stacks of the stack's own series count. By default
    /// they do not: a sibling series is another acquisition's output.
    pub own_series: bool,
    /// What the sibling itself must hold, over its `sibling_fields`.
    pub when: Expr,
}

/// How two series numbers relate: within `within` of each other, or one of
/// them a numbering family of the other (GE numbers what the scanner
/// computed from series 5 as 500 to 599: family 100; Siemens moves a
/// single-band reference by 1000: family 1000).
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesNumber {
    pub within: f64,
    pub family: Vec<f64>,
}

impl SeriesNumber {
    pub fn relates(&self, a: f64, b: f64) -> bool {
        if (a - b).abs() <= self.within {
            return true;
        }
        self.family.iter().any(|k| {
            *k > 0.0 && ((a >= *k && (a / k).floor() == b) || (b >= *k && (b / k).floor() == a))
        })
    }
}

/// Whether some sibling must hold, or none may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Require {
    Some,
    None,
}

impl Require {
    pub fn name(self) -> &'static str {
        match self {
            Require::Some => "some",
            Require::None => "none",
        }
    }
}

/// One other stack of the session, as the pass sees it.
#[derive(Debug, Clone)]
pub struct Sib {
    pub id: i64,
    /// Its fields; only those the pass names are read.
    pub stack: Stack,
    /// Its series' ingested private elements, aligned with `pack.ingest`.
    pub private: Vec<String>,
    /// Whether it is of the stack's own series.
    pub same_series: bool,
    /// Whether its series shares the stack's frame of reference; unknown
    /// where either has none.
    pub same_frame_of_reference: Option<bool>,
}

/// An axis of the stack as it stands: its values as stored, the tier that
/// decided it and at what confidence. Empty values and tier for an axis
/// nothing decided.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InForce {
    pub values: Vec<String>,
    pub tier: String,
    pub confidence: f64,
}

impl InForce {
    /// Whether a session answer may take this axis's place: nothing decided
    /// it, or what decided it read no header flag or number and no person
    /// said it, at no more than `at_most`. The tier is what protects an axis,
    /// not its values: a person's answer, a decision or a header rule that
    /// decided an axis to nothing stays nothing.
    pub fn replaceable(&self, at_most: f64) -> bool {
        if self.tier.is_empty() {
            return self.values.iter().all(String::is_empty);
        }
        matches!(
            self.tier.as_str(),
            "keywords" | "stated" | "default" | "vote" | TIER
        ) && self.confidence <= at_most + 1e-9
    }
}

/// What the pass said of one stack.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The rule that held.
    pub rule: String,
    /// What it writes, axis by number, only the axes whose values change.
    pub writes: Vec<(usize, Vec<String>)>,
    pub confidence: f64,
    /// The siblings that held, by id; none for a rule that requires none.
    pub cited: Vec<i64>,
    /// Why a rule that held changed nothing: an axis it would write was
    /// decided from the header, by a person or above `replaces_at_most`.
    pub held: Option<String>,
}

/// Whether the pass's target holds on a stack, with its axes as decided.
pub fn targets(
    pack: &Pack,
    target: Option<&Expr>,
    stack: &Stack,
    private: &[String],
    in_force: &[InForce],
) -> bool {
    let Some(t) = target else { return true };
    let e = Evaluated::with_private(pack, stack, private.to_vec());
    e.holds_with(t, &decided(in_force))
}

fn decided(in_force: &[InForce]) -> Vec<Vec<String>> {
    in_force
        .iter()
        .map(|a| a.values.iter().filter(|v| !v.is_empty()).cloned().collect())
        .collect()
}

/// The pass on one stack of the session: the first rule whose conditions
/// hold, and what it writes. None where the target does not hold or no rule
/// does. `in_force` is one entry per axis of the pack, in its order.
pub fn decide(
    pack: &Pack,
    target: Option<&Expr>,
    session: &Session,
    stack: &Stack,
    private: &[String],
    in_force: &[InForce],
    siblings: &[Sib],
) -> Option<Answer> {
    let own = Evaluated::with_private(pack, stack, private.to_vec());
    let seed = decided(in_force);
    if let Some(t) = target
        && !own.holds_with(t, &seed)
    {
        return None;
    }
    let series_number = crate::stack::field_index("series_number").expect("a field");
    for rule in &session.rules {
        if let Some(w) = &rule.when
            && !own.holds_with(w, &seed)
        {
            continue;
        }
        let s = &rule.sibling;
        let mut cited = Vec::new();
        for sib in siblings {
            if sib.same_series && !s.own_series {
                continue;
            }
            if s.same_frame_of_reference && sib.same_frame_of_reference != Some(true) {
                continue;
            }
            if let Some(n) = &s.series_number {
                match (stack.num(series_number), sib.stack.num(series_number)) {
                    (Some(a), Some(b)) if n.relates(a, b) => {}
                    _ => continue,
                }
            }
            if s.same
                .iter()
                .any(|f| !stack.present(*f) || stack.as_text(*f) != sib.stack.as_text(*f))
            {
                continue;
            }
            let theirs = Evaluated::with_private(pack, &sib.stack, sib.private.clone());
            if theirs.holds_with(&s.when, &[]) {
                cited.push(sib.id);
            }
        }
        let holds = match rule.require {
            Require::Some => !cited.is_empty(),
            Require::None => cited.is_empty(),
        };
        if !holds {
            continue;
        }
        cited.sort_unstable();
        let mut writes = Vec::new();
        let mut held = Vec::new();
        for (axis, values) in &rule.sets {
            let now = in_force.get(*axis).cloned().unwrap_or_default();
            let mut have: Vec<&String> = now.values.iter().filter(|v| !v.is_empty()).collect();
            have.sort();
            let mut want: Vec<&String> = values.iter().collect();
            want.sort();
            if have == want {
                continue;
            }
            if !now.replaceable(session.replaces_at_most) {
                held.push(format!(
                    "{} is {} by {} at {}",
                    pack.axes[*axis].name,
                    now.values.join(","),
                    now.tier,
                    now.confidence
                ));
                continue;
            }
            writes.push((*axis, values.clone()));
        }
        if !held.is_empty() {
            // All or nothing: a stack is never left half moved.
            return Some(Answer {
                rule: rule.name.clone(),
                writes: Vec::new(),
                confidence: rule.confidence,
                cited,
                held: Some(held.join("; ")),
            });
        }
        return Some(Answer {
            rule: rule.name.clone(),
            writes,
            confidence: rule.confidence,
            cited,
            held: None,
        });
    }
    None
}

/// The fields a session pass reads of a sibling: its `sibling_fields`, the
/// fields every rule compares (`same`) and the series number when a rule
/// relates series numbers. What a packet's session list must carry.
pub fn sibling_reads(session: &Session) -> Vec<usize> {
    let mut out = session.sibling_fields.clone();
    for r in &session.rules {
        out.extend(r.sibling.same.iter().copied());
        if r.sibling.series_number.is_some() {
            out.extend(crate::stack::field_index("series_number"));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Record 53: every sibling condition of every session pass reads only the
/// pass's `sibling_fields` and `same`, through every flag, parser and
/// derived text, so what a packet's session list carries is enough to replay
/// it. Refused with the first field it reads beyond them.
pub fn check(pack: &Pack) -> Result<(), crate::error::Error> {
    for pass in &pack.passes {
        let Some(s) = pass.session() else { continue };
        let allowed = sibling_reads(s);
        for r in &s.rules {
            for f in crate::reads::expr_fields(pack, &r.sibling.when) {
                if !allowed.contains(&f) {
                    let name = crate::reads::field_name(pack, f)
                        .map(|(n, _)| n)
                        .unwrap_or_else(|| format!("field {f}"));
                    return Err(crate::error::Error::at(
                        format!("pass {}.rules.{}.sibling.when", pass.name, r.name),
                        format!(
                            "reads {name}, which is not among the session's sibling_fields; \
                             a sibling is seen by those alone"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_number_relates_within_or_by_family() {
        let n = SeriesNumber {
            within: 1.0,
            family: vec![100.0],
        };
        assert!(n.relates(5.0, 6.0));
        assert!(n.relates(5.0, 4.0));
        assert!(!n.relates(5.0, 7.0));
        assert!(n.relates(5.0, 500.0));
        assert!(n.relates(599.0, 5.0));
        assert!(!n.relates(5.0, 600.0));
        assert!(!n.relates(0.0, 50.0), "series 0 has no family below 100");
        let siemens = SeriesNumber {
            within: 0.0,
            family: vec![1000.0],
        };
        assert!(siemens.relates(1007.0, 1.0));
        assert!(!siemens.relates(7.0, 8.0));
    }

    #[test]
    fn a_header_decided_value_is_never_replaced() {
        let at = |tier: &str, c: f64| InForce {
            values: vec!["X".into()],
            tier: tier.into(),
            confidence: c,
        };
        assert!(InForce::default().replaceable(0.7), "nothing decided");
        assert!(at("stated", 0.7).replaceable(0.7));
        assert!(at("default", 0.5).replaceable(0.7));
        assert!(at("vote", 0.6).replaceable(0.7));
        assert!(
            at("keywords", 0.85).replaceable(0.85),
            "a name, within the bound"
        );
        assert!(!at("keywords", 0.85).replaceable(0.7));
        assert!(!at("stated", 0.75).replaceable(0.7), "above the bound");
        for tier in [
            "exclusive",
            "combination",
            "alternative",
            "physics",
            "answer",
            "decision",
            "something new",
        ] {
            assert!(!at(tier, 0.1).replaceable(0.99), "{tier}");
        }
        // An axis decided to nothing is protected by what decided it.
        let nothing = |tier: &str, c: f64| InForce {
            values: Vec::new(),
            tier: tier.into(),
            confidence: c,
        };
        for tier in [
            "exclusive",
            "combination",
            "alternative",
            "physics",
            "answer",
            "decision",
            "something new",
        ] {
            assert!(!nothing(tier, 1.0).replaceable(0.99), "{tier} said none");
        }
        assert!(nothing("stated", 0.5).replaceable(0.7));
        assert!(
            !nothing("keywords", 0.9).replaceable(0.85),
            "above the bound"
        );
    }
}
