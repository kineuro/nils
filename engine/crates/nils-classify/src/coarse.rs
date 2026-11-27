// SPDX-License-Identifier: AGPL-3.0-only

//! A committed answer on one of two axes that say one thing at two grains.
//!
//! A pack may declare one axis the coarse form of another (`coarse_of`,
//! [`nils_pack::coarse`]): the MRI pack's body region is its body part
//! folded into four. The rules fold the finer answer into the coarse one,
//! and they fold the *rules'* finer answer, since a decision is applied
//! after the rules ran. So a decision on one of the two axes left the
//! other at what the rules made of the stack's words: a body part read
//! from a word in the header stood beside a committed region that does
//! not cover it, and a committed body part left the region at what the
//! word would have folded to.
//!
//! A committed answer is the point of committing, and a rule's answer
//! never stands against it. For every coarse form the pack declares
//! between two axes that hold one value each, one stack at a time:
//!
//! | the finer axis | the coarse axis | what the sort writes |
//! |---|---|---|
//! | the rules' value, or none | the rules' value, or none | both as the rules decided them |
//! | the rules' value | a decision that covers it | both as they are |
//! | the rules' value | a decision that does not cover it | no value on the finer axis: its row is not written, the rules' evidence stays and a row beside it says what set it aside ([`SetAside`]) |
//! | a decision | no decision | the coarse value that covers the decided one, at tier [`TIER`], or no row where none covers it ([`Folded`]) |
//! | a decision | a decision that covers it | both as they are |
//! | a decision | a decision of higher standing that does not cover it | the coarse decision holds, and the finer one is set aside as a rule's value is |
//! | a decision of higher standing | a decision that does not cover it | the finer decision holds, and the coarse axis takes the value that covers it, as where it had no decision |
//! | a decision | a decision of the same standing that does not cover it | both as they are, no question, counted ([`Held::apart`]) |
//!
//! Standing is what ranks two decisions on one axis: a person's over an
//! agent's over a model's ([`nils_registry::review::rank`]). So a person
//! who corrects one of the two axes is not left beside a model's answer on
//! the other that says otherwise, and two people, or two models, who do
//! not agree are left as they decided and counted, since nothing ranks
//! them. The evidence of a value set aside or folded names who decided the
//! value that did it, and the decision that gave way where there was one.
//!
//! A decision that an axis holds nothing, or one whose text names no
//! value of its axis, covers nothing and is covered by nothing: it moves
//! nothing here.
//!
//! Where a pack stacks three grains, a value folded from the finest is
//! read as committed by the form above it, with the standing of the
//! decision it was folded from: the fold runs up, and a decision on the
//! top grain is held against it as against a decision. Setting aside does
//! not run down: a decided top value that does not cover the rules' middle
//! value sets that aside and leaves the finest axis as the rules decided
//! it. Where two coarse axes fold one finer axis and a decision on one of
//! them set the rules' finer value aside, the other coarse axis does not
//! keep what the rules folded from the value set aside: it is set aside
//! with it.
//!
//! [`held`] is the one place the table is worked out. The sort
//! ([`crate::classify`]) and the rehearsal of a rule change
//! ([`crate::effect`]) both read it, so what a rehearsal counts is what
//! the sort writes.

use nils_pack::coarse::Form;
use nils_pack::rules::{Axis, AxisPhase};
use nils_pack::verdict::Verdict;

use crate::classify::Author;

/// The tier of a coarse value folded from a committed finer one, on its
/// row and on the evidence that says where it came from.
pub const TIER: &str = nils_registry::coarse::FOLDED;

/// The tier of the evidence row that says a value was set aside. No row
/// of an axis carries it: the axis is left with no row.
pub const SET_ASIDE: &str = nils_registry::coarse::SET_ASIDE;

/// What the evidence of either names as its rule set: the pack's key.
pub const RULE_SET: &str = nils_registry::coarse::RULE_SET;

/// The confidence of a folded value and of both evidence rows: a
/// decision's. A fold adds no doubt to the answer it folds and no
/// certainty either, and a committed answer is written at 1.0.
pub const CONFIDENCE: f64 = 1.0;

/// A committed answer on an axis of a stack: the value, in the form a row
/// stores, and who decided it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Standing {
    pub value: String,
    pub author: Author,
}

impl Standing {
    fn rank(&self) -> u8 {
        nils_registry::review::rank(&self.author.kind)
    }
}

/// Who decided, as an evidence row cites it beside the row's own author.
fn said(author: &Author) -> String {
    format!("{} ({})", author.who, author.kind)
}

/// A value of an axis that a committed value of the other axis set aside:
/// the rules' value of the finer axis, or a decision of lower standing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SetAside {
    /// The axis left with no value.
    pub axis: String,
    /// What was decided on it, as a row would store it.
    pub value: String,
    pub tier: String,
    pub confidence: f64,
    /// The axis whose decision set it aside, its value and who decided it.
    pub by_axis: String,
    pub by_value: String,
    pub by: Author,
    /// Who decided the value set aside, where a decision gave way and no
    /// rule's value.
    pub of: Option<Author>,
    /// For a coarse axis set aside with the finer value the rules folded
    /// it from: that finer axis.
    pub through: Option<String>,
}

impl SetAside {
    /// What the evidence row cites.
    pub(crate) fn cited(&self) -> String {
        match (&self.through, &self.of) {
            (Some(fine), _) => format!(
                "{} was decided and set aside the {fine} that {} was folded from",
                self.by_value, self.value
            ),
            (None, Some(of)) => format!(
                "{} was decided and does not cover {}, which {} decided",
                self.by_value,
                self.value,
                said(of)
            ),
            (None, None) => format!(
                "{} was decided and does not cover {}",
                self.by_value, self.value
            ),
        }
    }

    /// The note the stack's classification keeps, since the axis has no
    /// row left to carry it.
    pub(crate) fn note(&self) -> serde_json::Value {
        let mut note = serde_json::json!({
            "axis": self.axis,
            "value": self.value,
            "tier": self.tier,
            "confidence": self.confidence,
            "by": {
                "axis": self.by_axis,
                "value": self.by_value,
                "who": self.by.who,
                "kind": self.by.kind,
            },
        });
        if let Some(of) = &self.of {
            note["decided_by"] = serde_json::json!({"who": of.who, "kind": of.kind});
        }
        if let Some(fine) = &self.through {
            note["through"] = serde_json::json!(fine);
        }
        note
    }
}

/// A coarse axis decided by a committed value of the finer one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Folded {
    /// The coarse axis.
    pub axis: String,
    /// The value that covers the decided one, as a row stores it; none
    /// where no value of the coarse axis covers it.
    pub value: Option<String>,
    /// The finer axis, its decided value and who decided it.
    pub from_axis: String,
    pub from_value: String,
    pub by: Author,
    /// The decision of lower standing on the coarse axis that gave way.
    pub over: Option<Standing>,
}

impl Folded {
    /// What the evidence row cites.
    pub(crate) fn cited(&self) -> String {
        let folded = match self.value {
            Some(_) => format!("{} was decided", self.from_value),
            None => format!("{} was decided, and no value covers it", self.from_value),
        };
        match &self.over {
            Some(over) => format!(
                "{folded}, over {}, which {} decided",
                over.value,
                said(&over.author)
            ),
            None => folded,
        }
    }
}

/// The pack's coarse forms a sort holds a stack to: those between two
/// axes of the class phase, which is where a decision is applied.
pub(crate) fn forms(pack: &nils_pack::Pack) -> Vec<Form<'_>> {
    nils_pack::coarse::forms(pack)
        .into_iter()
        .filter(|f| f.coarse.phase == AxisPhase::Class && f.fine.phase == AxisPhase::Class)
        .collect()
}

/// What the pack's coarse forms make of one stack.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Held {
    pub set_aside: Vec<SetAside>,
    pub folded: Vec<Folded>,
    /// The coarse axes whose decision does not cover the decision on the
    /// finer axis, where the two are of one standing: left as they are,
    /// and counted.
    pub apart: Vec<String>,
    /// The axes whose own decision gave way to one of higher standing on
    /// the other axis: it is not applied.
    pub overruled: Vec<String>,
}

impl Held {
    pub(crate) fn set_aside(&self, axis: &str) -> Option<&SetAside> {
        self.set_aside.iter().find(|s| s.axis == axis)
    }

    pub(crate) fn folded(&self, axis: &str) -> Option<&Folded> {
        self.folded.iter().find(|f| f.axis == axis)
    }

    /// Whether the decision on this axis gave way and is not applied.
    pub(crate) fn overruled(&self, axis: &str) -> bool {
        self.overruled.iter().any(|a| a == axis)
    }
}

/// One stack held to the pack's coarse forms: `verdict` is the rules'
/// answer, and `decided` the decision in force on an axis of this stack
/// in the stored form of the value it names (the empty text for a
/// decision that the axis holds nothing) with its author, none where
/// there is none.
pub(crate) fn held(
    forms: &[Form<'_>],
    verdict: &Verdict,
    decided: impl Fn(&Axis) -> Option<Standing>,
) -> Held {
    let mut out = Held::default();
    for form in forms {
        let (coarse, fine) = (&form.coarse.name, &form.fine.name);
        // The finer axis as the forms below left it: with no value where
        // one set it aside or folded it to nothing, with the folded value
        // at the standing of the decision it was folded from, else with
        // its own decision.
        let below = out.folded(fine).map(|x| {
            x.value.clone().map(|value| Standing {
                value,
                author: x.by.clone(),
            })
        });
        let gone = out.set_aside(fine).is_some() || below == Some(None);
        let f = match (gone, below) {
            (true, _) => None,
            (false, Some(folded)) => folded,
            (false, None) => decided(form.fine),
        };
        match (f, decided(form.coarse)) {
            (Some(f), Some(c)) => {
                if form.covers(&c.value, &f.value) != Some(false) {
                    continue;
                }
                match c.rank().cmp(&f.rank()) {
                    std::cmp::Ordering::Equal => out.apart.push(coarse.clone()),
                    std::cmp::Ordering::Greater => {
                        // the coarse decision holds: the finer one, or the
                        // value folded onto the finer axis, is set aside
                        let was_folded = out.folded(fine).is_some();
                        out.folded.retain(|x| x.axis != *fine);
                        if !was_folded {
                            out.overruled.push(fine.clone());
                        }
                        out.set_aside.push(SetAside {
                            axis: fine.clone(),
                            value: f.value,
                            tier: if was_folded {
                                TIER.to_string()
                            } else {
                                nils_registry::coarse::DECISION.to_string()
                            },
                            confidence: CONFIDENCE,
                            by_axis: coarse.clone(),
                            by_value: c.value,
                            by: c.author,
                            of: Some(f.author),
                            through: None,
                        });
                    }
                    std::cmp::Ordering::Less => {
                        // the finer decision holds: the coarse axis takes
                        // the value that covers it
                        out.overruled.push(coarse.clone());
                        if let Some(to) = form.fold(&f.value) {
                            out.folded.push(Folded {
                                axis: coarse.clone(),
                                value: to.map(str::to_string),
                                from_axis: fine.clone(),
                                from_value: f.value,
                                by: f.author,
                                over: Some(c),
                            });
                        }
                    }
                }
            }
            (None, Some(c)) => {
                if gone {
                    continue;
                }
                let Some(rules) = verdict.axis(fine) else {
                    continue;
                };
                let value = rules.stored();
                if form.covers(&c.value, &value) == Some(false) {
                    out.set_aside.push(SetAside {
                        axis: fine.clone(),
                        value,
                        tier: rules.tier.clone(),
                        confidence: rules.confidence,
                        by_axis: coarse.clone(),
                        by_value: c.value,
                        by: c.author,
                        of: None,
                        through: None,
                    });
                }
            }
            (Some(f), None) => {
                let Some(to) = form.fold(&f.value) else {
                    continue;
                };
                out.folded.push(Folded {
                    axis: coarse.clone(),
                    value: to.map(str::to_string),
                    from_axis: fine.clone(),
                    from_value: f.value,
                    by: f.author,
                    over: None,
                });
            }
            (None, None) => {}
        }
    }
    // A second coarse axis over a finer one whose rules' value another
    // coarse axis's decision set aside: the rules folded its value from the
    // value set aside, so it is no answer either. One that has a decision
    // of its own, or a value folded from a decision, keeps it.
    for form in forms {
        let (coarse, fine) = (&form.coarse.name, &form.fine.name);
        let Some(aside) = out.set_aside(fine).filter(|s| s.by_axis != *coarse) else {
            continue;
        };
        if out.set_aside(coarse).is_some()
            || out.folded(coarse).is_some()
            || decided(form.coarse).is_some()
        {
            continue;
        }
        let Some(rules) = verdict.axis(coarse) else {
            continue;
        };
        if rules.stored().is_empty() {
            continue;
        }
        let aside = SetAside {
            axis: coarse.clone(),
            value: rules.stored(),
            tier: rules.tier.clone(),
            confidence: rules.confidence,
            by_axis: aside.by_axis.clone(),
            by_value: aside.by_value.clone(),
            by: aside.by.clone(),
            of: None,
            through: Some(fine.clone()),
        };
        out.set_aside.push(aside);
    }
    out
}
