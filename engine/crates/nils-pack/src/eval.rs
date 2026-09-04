// SPDX-License-Identifier: AGPL-3.0-only

//! Evaluating a pack against one stack: the parsers, then the flags.
//!
//! Nothing is stored. Deriving 220 predicates and 145 flags for the whole
//! live corpus costs 5.5 seconds on one core (`spikes/pack/README.md`), which
//! is why a flag lives in a pack and not in a column.

use std::collections::HashSet;

use regex::Regex;

use crate::expr::{Ctx, Subject};
use crate::pack::Pack;
use crate::stack::Stack;

/// One stack, mid-evaluation.
pub struct Evaluated<'a> {
    pack: &'a Pack,
    stack: &'a Stack,
    raws: Vec<String>,
    tokens: Vec<HashSet<String>>,
    preds: Vec<Vec<bool>>,
    flags: Vec<bool>,
    /// What each axis has been decided to be, as classification proceeds, so
    /// that a later rule set can read what an earlier one said.
    decided: std::cell::RefCell<Vec<Vec<String>>>,
    /// The text the pack derives for itself, computed once per stack.
    derived: Vec<String>,
}

impl<'a> Evaluated<'a> {
    pub fn new(pack: &'a Pack, stack: &'a Stack) -> Evaluated<'a> {
        let mut raws = Vec::with_capacity(pack.parsers.len());
        let mut tokens = Vec::with_capacity(pack.parsers.len());
        for p in &pack.parsers {
            let raw = p.case.apply(stack.text(p.field)).into_owned();
            let mut set = HashSet::new();
            if let Some(split) = &p.split {
                let stripped = match &p.strip {
                    Some(s) => s.replace_all(&raw, "").into_owned(),
                    None => raw.clone(),
                };
                for t in split.split(&stripped) {
                    if !t.is_empty() {
                        set.insert(t.to_string());
                    }
                }
            }
            raws.push(raw);
            tokens.push(set);
        }
        // The pack's own text, before anything reads it.
        let derived: Vec<String> = pack
            .derived
            .iter()
            .map(|n| {
                let parts: Vec<&str> = n.from.iter().map(|i| stack.text(*i)).collect();
                n.apply(&parts).unwrap_or_default()
            })
            .collect();
        let mut e = Evaluated {
            pack,
            stack,
            derived,
            raws,
            tokens,
            preds: Vec::with_capacity(pack.parsers.len()),
            flags: vec![false; pack.flags.len()],
            decided: std::cell::RefCell::new((0..pack.axes.len()).map(|_| Vec::new()).collect()),
        };
        // Predicates, parser by parser, in file order. `preds` grows as it
        // goes, so a predicate may name an earlier one; a forward reference
        // reads false, which the loader is what stops.
        for (pi, p) in pack.parsers.iter().enumerate() {
            e.preds.push(Vec::with_capacity(p.preds.len()));
            for expr in &p.preds {
                let v = {
                    let subj = Subject {
                        raw: &e.raws[pi],
                        tokens: Some(&e.tokens[pi]),
                    };
                    expr.eval(Some(&subj), &e)
                };
                e.preds[pi].push(v);
            }
        }
        for i in &pack.flag_order {
            let v = pack.flags[*i].eval(None, &e);
            e.flags[*i] = v;
        }
        e
    }

    pub fn flag(&self, name: &str) -> Option<bool> {
        self.pack.flag_index(name).map(|i| self.flags[i])
    }

    /// The flags that hold, by name, in the pack's declared order.
    pub fn flags_on(&self) -> Vec<&str> {
        self.pack
            .flag_names
            .iter()
            .enumerate()
            .filter(|(i, _)| self.flags[*i])
            .map(|(_, n)| n.as_str())
            .collect()
    }

    pub fn predicate(&self, parser: &str, name: &str) -> Option<bool> {
        let pi = self.pack.parser_index(parser)?;
        let px = self.pack.parsers[pi]
            .pred_names
            .iter()
            .position(|n| n == name)?;
        Some(self.preds[pi][px])
    }
}

impl Ctx for Evaluated<'_> {
    fn pred(&self, parser: usize, pred: usize) -> bool {
        self.preds
            .get(parser)
            .and_then(|v| v.get(pred))
            .copied()
            .unwrap_or(false)
    }
    fn subject(&self, parser: usize) -> Subject<'_> {
        Subject {
            raw: &self.raws[parser],
            tokens: Some(&self.tokens[parser]),
        }
    }
    fn flag(&self, flag: usize) -> bool {
        self.flags[flag]
    }
    fn num(&self, field: usize) -> Option<f64> {
        match field.checked_sub(crate::stack::FIELDS.len()) {
            Some(i) => self.derived.get(i).and_then(|t| t.trim().parse().ok()),
            None => self.stack.num(field),
        }
    }
    fn present(&self, field: usize) -> bool {
        match field.checked_sub(crate::stack::FIELDS.len()) {
            Some(i) => self.derived.get(i).is_some_and(|t| !t.is_empty()),
            None => self.stack.present(field),
        }
    }
    fn text(&self, field: usize) -> &str {
        match field.checked_sub(crate::stack::FIELDS.len()) {
            Some(i) => self.derived.get(i).map_or("", String::as_str),
            None => self.stack.text(field),
        }
    }
    fn re(&self, idx: usize) -> &Regex {
        &self.pack.regexes[idx]
    }
    fn axis_is(&self, axis: usize, value: &str) -> bool {
        self.decided
            .borrow()
            .get(axis)
            .is_some_and(|vs| vs.iter().any(|v| v == value))
    }

    fn axis_empty(&self, axis: usize) -> bool {
        self.decided
            .borrow()
            .get(axis)
            .is_none_or(|vs| vs.is_empty())
    }
}

// ---------------------------------------------------------------------------
// Classifying: the rule sets in order, each leaving alone what an earlier one
// decided (§6.3).

use crate::rules::{Clause, Rule, Tier, Which};
use crate::verdict::{AxisVerdict, Evidence, Verdict};

/// The value index a set names: the one it wrote, or the one the rule set
/// worked out for this stack.
fn which(w: Which, derived: &[Option<usize>]) -> Option<usize> {
    match w {
        Which::Fixed(i) => Some(i),
        Which::Nothing => None,
        Which::Derived(d) => derived.get(d).copied().flatten(),
    }
}

/// What fired, and what it cites.
#[derive(Clone)]
struct Fired {
    tier: Tier,
    confidence: f64,
    source: String,
    matched: String,
}

impl Evaluated<'_> {
    /// The pack's verdict on this stack, with the evidence that made it.
    pub fn classify(&self) -> Verdict {
        let pack = self.pack;
        let mut verdict = Verdict::default();
        for v in self.decided.borrow_mut().iter_mut() {
            v.clear();
        }
        // Per axis: the value indices collected so far, and their evidence.
        let mut collected: Vec<Vec<(usize, Fired, String, String)>> =
            (0..pack.axes.len()).map(|_| Vec::new()).collect();
        // An axis a rule set has closed: no later set may add to it. A set
        // that collects (a multi-valued axis's own rules) never closes one,
        // which is how several modifiers accumulate while a route's construct
        // list replaces rather than joins.
        let mut closed: Vec<bool> = vec![false; pack.axes.len()];
        // An axis a rule decided to be nothing is closed too: the default is
        // for an axis nobody spoke about, not for one told to stay empty.
        let mut said_nothing: Vec<bool> = vec![false; pack.axes.len()];

        for set in &pack.rule_sets {
            if let Some(e) = &set.enter_when
                && !e.eval(None, self)
            {
                continue;
            }
            verdict.entered.push(set.name.clone());
            // What the set works out for this stack before its rules run.
            let derived: Vec<Option<usize>> = set
                .derives
                .iter()
                .map(|d| {
                    d.cases
                        .iter()
                        .find(|c| c.when.as_ref().is_none_or(|w| w.eval(None, self)))
                        .map(|c| c.value)
                })
                .collect();
            for rule in &set.rules {
                // A rule whose every axis is single-valued and already
                // decided has nothing left to say.
                if rule.sets.iter().all(|s| closed[s.axis]) {
                    continue;
                }
                let Some(fired) = self.fire(rule) else {
                    continue;
                };
                for sets in &rule.sets {
                    let axis = &pack.axes[sets.axis];
                    if closed[sets.axis] {
                        continue;
                    }
                    for v in &sets.values {
                        if let Some(w) = &v.when
                            && !w.eval(None, self)
                        {
                            continue;
                        }
                        let Some(value) = which(v.value, &derived) else {
                            // Decided, and the answer is nothing.
                            closed[sets.axis] = true;
                            said_nothing[sets.axis] = true;
                            continue;
                        };
                        collected[sets.axis].push((
                            value,
                            Fired {
                                tier: fired.tier,
                                confidence: rule.confidence.unwrap_or(fired.confidence),
                                source: fired.source.clone(),
                                matched: fired.matched.clone(),
                            },
                            set.name.clone(),
                            rule.id.clone(),
                        ));
                    }
                    // A rule set that decides rather than collects decides
                    // the axis whole: a route replaces the construct list, it
                    // does not add to what an axis's own rules would say.
                    if !set.collect && !set.adds.contains(&sets.axis) {
                        closed[sets.axis] = true;
                    }
                    // A later rule set reads what this one decided. The
                    // conditions are evaluated before the borrow, because
                    // evaluating one may read this very cell.
                    let just_set: Vec<String> = sets
                        .values
                        .iter()
                        .filter(|v| v.when.as_ref().is_none_or(|w| w.eval(None, self)))
                        .filter_map(|v| which(v.value, &derived))
                        .map(|i| axis.stored(i).to_string())
                        .collect();
                    self.decided.borrow_mut()[sets.axis].extend(just_set);
                }
                if !set.collect {
                    break;
                }
            }
        }

        for (ai, axis) in pack.axes.iter().enumerate() {
            let mut hits = std::mem::take(&mut collected[ai]);

            // At most one member of an exclusion group may hold, and the
            // lower priority number wins; a tie keeps the one that comes
            // first in the axis's order, which is what v0 does.
            let mut winner: std::collections::BTreeMap<&str, (i64, usize)> =
                std::collections::BTreeMap::new();
            for (v, ..) in &hits {
                let Some(g) = axis.values[*v].group.as_deref() else {
                    continue;
                };
                let p = axis.values[*v].priority.unwrap_or(i64::MAX);
                match winner.get(g) {
                    Some((wp, _)) if *wp <= p => {}
                    _ => {
                        winner.insert(g, (p, *v));
                    }
                }
            }
            hits.retain(|(v, ..)| match axis.values[*v].group.as_deref() {
                None => true,
                Some(g) => winner.get(g).is_some_and(|(_, w)| w == v),
            });

            if hits.is_empty() {
                if said_nothing[ai] {
                    continue;
                }
                if let Some(d) = &axis.default {
                    verdict.axes.push(AxisVerdict {
                        axis: axis.name.clone(),
                        values: vec![d.clone()],
                        confidence: axis.default_confidence,
                        tier: Tier::Default.name().to_string(),
                    });
                    verdict.evidence.push(Evidence {
                        axis: axis.name.clone(),
                        value: d.clone(),
                        tier: Tier::Default.name().to_string(),
                        confidence: axis.default_confidence,
                        rule_set: axis.name.clone(),
                        rule: "default".into(),
                        source: "default".into(),
                        matched: String::new(),
                    });
                }
                continue;
            }

            for (v, fired, set_name, rule_id) in &hits {
                verdict.evidence.push(Evidence {
                    axis: axis.name.clone(),
                    value: axis.stored(*v).to_string(),
                    tier: fired.tier.name().to_string(),
                    confidence: fired.confidence,
                    rule_set: set_name.clone(),
                    rule: rule_id.clone(),
                    source: fired.source.clone(),
                    matched: fired.matched.clone(),
                });
            }

            let mut values: Vec<String> = hits
                .iter()
                .map(|(v, ..)| axis.stored(*v).to_string())
                .collect();
            if axis.multi {
                // v0 stores a multi-valued axis sorted and de-duplicated.
                values.sort();
                values.dedup();
            }
            let confidence = hits
                .iter()
                .map(|(_, f, ..)| f.confidence)
                .fold(f64::NAN, f64::max);
            verdict.axes.push(AxisVerdict {
                axis: axis.name.clone(),
                values,
                confidence: if confidence.is_nan() { 0.0 } else { confidence },
                tier: hits[0].1.tier.name().to_string(),
            });
        }
        // Last, because it reads what was decided.
        verdict.silent = pack
            .review
            .silent_when
            .as_ref()
            .is_some_and(|e| e.eval(None, self));
        verdict
    }

    /// The first clause of a rule that holds, with what it cites. A rule with
    /// a `requires` says nothing at all unless it holds.
    fn fire(&self, rule: &Rule) -> Option<Fired> {
        if let Some(g) = &rule.requires
            && !g.eval(None, self)
        {
            return None;
        }
        for c in &rule.clauses {
            match c {
                Clause::Flag {
                    tier,
                    confidence,
                    name,
                    flag,
                } => {
                    if self.flags[*flag] {
                        return Some(Fired {
                            tier: *tier,
                            confidence: *confidence,
                            source: "flags".into(),
                            matched: name.clone(),
                        });
                    }
                }
                Clause::Keywords {
                    tier,
                    confidence,
                    field,
                    list,
                } => {
                    // v0 matches a keyword as a case-insensitive substring and
                    // cites the first in the list that hits, not the longest.
                    let text = <Self as Ctx>::text(self, *field).to_lowercase();
                    if !text.is_empty()
                        && let Some(kw) = list.iter().find(|k| text.contains(&k.to_lowercase()))
                    {
                        return Some(Fired {
                            tier: *tier,
                            confidence: *confidence,
                            source: "text".into(),
                            matched: kw.clone(),
                        });
                    }
                }
                Clause::AnyFlag {
                    tier,
                    confidence,
                    names,
                    flags,
                } => {
                    if let Some(i) = flags.iter().position(|f| self.flags[*f]) {
                        return Some(Fired {
                            tier: *tier,
                            confidence: *confidence,
                            source: "flags".into(),
                            matched: names[i].clone(),
                        });
                    }
                }
                Clause::Combination {
                    tier,
                    confidence,
                    names,
                    flags,
                } => {
                    if !flags.is_empty() && flags.iter().all(|f| self.flags[*f]) {
                        return Some(Fired {
                            tier: *tier,
                            confidence: *confidence,
                            source: "flags".into(),
                            matched: names.join("+"),
                        });
                    }
                }
                Clause::When {
                    tier,
                    confidence,
                    cite,
                    source,
                    expr,
                } => {
                    if expr.eval(None, self) {
                        return Some(Fired {
                            tier: *tier,
                            confidence: *confidence,
                            source: source.clone(),
                            matched: cite.clone(),
                        });
                    }
                }
            }
        }
        None
    }
}

impl Evaluated<'_> {
    /// The text the pack derived, by the name it published it under.
    pub fn derived_text(&self, name: &str) -> Option<&str> {
        let i = self.pack.derived.iter().position(|d| d.into == name)?;
        Some(&self.derived[i])
    }
}
