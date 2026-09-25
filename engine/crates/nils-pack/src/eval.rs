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
    /// The private elements the pack ingests, aligned with `pack.ingest`,
    /// empty where the series has none (Wave 4a §5.2). Handed in by whoever
    /// has the row, because they sit beside the fingerprint and not in it.
    private: Vec<String>,
}

impl<'a> Evaluated<'a> {
    /// Evaluate with no private elements at hand: every ingested field reads
    /// as absent.
    pub fn new(pack: &'a Pack, stack: &'a Stack) -> Evaluated<'a> {
        Evaluated::with_private(pack, stack, Vec::new())
    }

    /// Evaluate with the series' private elements, one text per entry of
    /// `pack.ingest` in order; a shorter vector reads as absent past its end.
    pub fn with_private(pack: &'a Pack, stack: &'a Stack, private: Vec<String>) -> Evaluated<'a> {
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
            private,
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

impl Evaluated<'_> {
    /// A field past the fingerprint's own: the pack's derived text first,
    /// then its ingested private elements, in the order the loader numbered
    /// them. `None` for a field of the fingerprint itself.
    fn extra(&self, field: usize) -> Option<&str> {
        let i = field.checked_sub(crate::stack::FIELDS.len())?;
        match i.checked_sub(self.derived.len()) {
            None => Some(self.derived[i].as_str()),
            Some(j) => Some(self.private.get(j).map_or("", String::as_str)),
        }
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
        match self.extra(field) {
            Some(t) => t.trim().parse().ok(),
            None => self.stack.num(field),
        }
    }
    fn present(&self, field: usize) -> bool {
        match self.extra(field) {
            Some(t) => !t.is_empty(),
            None => self.stack.present(field),
        }
    }
    fn text(&self, field: usize) -> &str {
        match self.extra(field) {
            Some(t) => t,
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

use crate::rules::{AxisPhase, Clause, Rule, Tier, Which};
use crate::verdict::{AxisVerdict, DIAGNOSTICS_MAX, Diagnostic, Evidence, Verdict, Vote};

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
    /// The text `matched` was found in, where the clause read exactly one.
    /// A pack normalizes its fingerprint more than once (`search_text` drops
    /// the boilerplate, `anatomy_text` keeps it), so a citation without the
    /// text it came from cannot be compared with another rule's.
    text: Option<usize>,
    /// The clause's place in its rule.
    clause: usize,
}

impl Evaluated<'_> {
    /// The pack's verdict on this stack, with the evidence that made it.
    pub fn classify(&self) -> Verdict {
        self.run(AxisPhase::Class, &[], &[], false)
    }

    /// The same verdict, with every rule's vote beside it (record 41, S2):
    /// each clause that held, of each rule whose set was entered, whether or
    /// not the rule decided anything. The rest of the verdict is exactly
    /// what [`Evaluated::classify`] says.
    pub fn classify_with_votes(&self) -> Verdict {
        self.run(AxisPhase::Class, &[], &[], true)
    }

    /// What to do with this stack, from what the rules and the passes decided
    /// (Wave 3 §7).
    ///
    /// `decided` is one entry per axis of the pack, in the pack's order,
    /// holding what is stored for that axis. It is seeded rather than
    /// recomputed because the passes have run since, and a disposition worked
    /// out from the rules alone would be worked out from a gap.
    pub fn dispose(&self, decided: &[Vec<String>]) -> Verdict {
        self.run(AxisPhase::Disposition, decided, &[], false)
    }

    /// [`Evaluated::dispose`], with every rule's vote beside it.
    pub fn dispose_with_votes(&self, decided: &[Vec<String>]) -> Verdict {
        self.run(AxisPhase::Disposition, decided, &[], true)
    }

    /// Record 48: the pack's verdict with some axes held at a person's
    /// answer. `pins` is one entry per axis of the pack, in the pack's
    /// order; an axis with `Some` is taken as decided to those stored
    /// values before any rule set runs, so every rule that reads it reads
    /// the answer and none may change it. Both phases run, the class phase
    /// and then the disposition from what it decided, and the passes do
    /// not, since they read other stacks. The pinned axes come back as
    /// given, of tier `answer`.
    ///
    /// `skip` names rules left out of the run, which never fire: the
    /// derivation leaves out a route that decides an asked axis from the
    /// file's own words, since its answer is the rater's now.
    pub fn with_pins(
        &self,
        pins: &[Option<Vec<String>>],
        skip: &dyn Fn(&crate::rules::RuleSet, &Rule) -> bool,
    ) -> Verdict {
        let class = self.run_skipping(AxisPhase::Class, &[], pins, false, skip);
        let seed: Vec<Vec<String>> = self
            .pack
            .axes
            .iter()
            .enumerate()
            .map(|(i, a)| match pins.get(i).and_then(Option::as_ref) {
                Some(p) => p.clone(),
                None => class
                    .axis(&a.name)
                    .map(|v| v.values.clone())
                    .unwrap_or_default(),
            })
            .collect();
        let mut out = self.run_skipping(AxisPhase::Disposition, &seed, pins, false, skip);
        let mut axes = class.axes;
        axes.append(&mut out.axes);
        out.axes = axes;
        out
    }

    fn run(
        &self,
        phase: AxisPhase,
        seed: &[Vec<String>],
        pins: &[Option<Vec<String>>],
        voting: bool,
    ) -> Verdict {
        self.run_skipping(phase, seed, pins, voting, &|_, _| false)
    }

    fn run_skipping(
        &self,
        phase: AxisPhase,
        seed: &[Vec<String>],
        pins: &[Option<Vec<String>>],
        voting: bool,
        skip: &dyn Fn(&crate::rules::RuleSet, &Rule) -> bool,
    ) -> Verdict {
        let pack = self.pack;
        let mut verdict = Verdict::default();
        {
            let mut d = self.decided.borrow_mut();
            for (i, v) in d.iter_mut().enumerate() {
                v.clear();
                if let Some(from) = seed.get(i) {
                    v.extend(from.iter().cloned());
                }
            }
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
        // Wave 4c §6.6: who closed each axis, so that a later rule reaching
        // it can be recorded against the one that won. Set, rule, the stored
        // value and the citation.
        let mut decided_by: Vec<Option<(String, String, String, String)>> =
            vec![None; pack.axes.len()];
        // Record 48: an axis held at a person's answer is decided before
        // anything runs, and closed, so no rule set moves it.
        for (i, pin) in pins.iter().enumerate().take(pack.axes.len()) {
            if let Some(p) = pin {
                closed[i] = true;
                let mut d = self.decided.borrow_mut();
                d[i].clear();
                d[i].extend(p.iter().cloned());
            }
        }

        for set in &pack.rule_sets {
            if set.phase != phase {
                continue;
            }
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
            for (ri, rule) in set.rules.iter().enumerate() {
                if skip(set, rule) {
                    continue;
                }
                // A rule whose every axis is single-valued and already
                // decided has nothing left to say. It is still evaluated,
                // because a rule that would have said something different
                // is the diagnostic of Wave 4c §6.6, and the evidence rows
                // cannot tell: they record only what was cited.
                let all_closed = rule.sets.iter().all(|s| closed[s.axis]);
                let Some(fired) = self.fire(rule) else {
                    continue;
                };
                // Record 41, S2: what the rule says, heard before it acts,
                // so that a value's condition reads what the rule's own
                // answer reads.
                let held = if voting {
                    self.held(rule, fired.clause)
                } else {
                    Vec::new()
                };
                if all_closed {
                    for sets in &rule.sets {
                        self.vote(&mut verdict.votes, set, rule, &held, sets, &derived);
                        self.conflict(&mut verdict, set, rule, &fired, sets, &derived, &decided_by);
                    }
                    continue;
                }
                for sets in &rule.sets {
                    let axis = &pack.axes[sets.axis];
                    self.vote(&mut verdict.votes, set, rule, &held, sets, &derived);
                    if closed[sets.axis] {
                        self.conflict(&mut verdict, set, rule, &fired, sets, &derived, &decided_by);
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
                                text: fired.text,
                                clause: fired.clause,
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
                        decided_by[sets.axis] = Some((
                            set.name.clone(),
                            rule.id.clone(),
                            self.would_store(axis, sets, &derived),
                            fired.matched.clone(),
                        ));
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
                    // A later rule of this same set that would have stored
                    // something else is a disagreement about the answer and
                    // not only a keyword that went uncited: the set stops at
                    // its first firing rule, so nothing else records that
                    // the other rule would have fired at all. v0's brain
                    // beside a spine is this case, and it is what a person
                    // reviewing the stack has to see.
                    //
                    // Only where both rules read the stack's own text, and
                    // cited different words in it. An ordered set puts the
                    // specific rule before the general one and both are true
                    // of the same evidence, so a later rule firing on the
                    // same flags is the order doing its work: an MPRAGE is a
                    // spoiled gradient echo, and asking about that is v0's
                    // queue. Two different words in one text pointing at two
                    // different answers is the archive disagreeing with
                    // itself.
                    //
                    // The same text, and not merely text: record 35 slice S2
                    // gave the MRI pack a second normalization, so a rule
                    // may read `anatomy_text` where the rule before it read
                    // `search_text`. A word the one keeps and the other
                    // drops is not the archive contradicting itself, it is
                    // two readings of one description, and a conflict
                    // recorded across them would cite a word the winning
                    // rule never saw.
                    if fired.source == "text" && fired.text.is_some() {
                        for later in &set.rules[ri + 1..] {
                            let Some(also) = self.fire(later) else {
                                continue;
                            };
                            if also.source != "text"
                                || also.text != fired.text
                                || also.matched.eq_ignore_ascii_case(&fired.matched)
                            {
                                continue;
                            }
                            for sets in &later.sets {
                                if closed[sets.axis] && decided_by[sets.axis].is_some() {
                                    self.conflict(
                                        &mut verdict,
                                        set,
                                        later,
                                        &also,
                                        sets,
                                        &derived,
                                        &decided_by,
                                    );
                                }
                            }
                        }
                    }
                    // Wave 4c §6.6: what else would have matched on this
                    // stack, in this rule and in the rest of the set, and
                    // was never cited because this rule won. A keyword
                    // shadowed on every stack of a batch is a keyword that
                    // can never match.
                    self.shadowed(&mut verdict, set, ri, rule, &fired);
                    // Record 41, S2: the rules the set never reached, heard
                    // anyway. They read what the winner decided, as they
                    // would have had they run.
                    if voting {
                        for later in &set.rules[ri + 1..] {
                            let Some(also) = self.fire(later) else {
                                continue;
                            };
                            let held = self.held(later, also.clause);
                            for sets in &later.sets {
                                self.vote(&mut verdict.votes, set, later, &held, sets, &derived);
                            }
                        }
                    }
                    break;
                }
            }
        }

        for (ai, axis) in pack.axes.iter().enumerate() {
            // Only this phase's axes: the others were decided elsewhere, and
            // emitting them again would overwrite a pass's answer with the
            // seed it was read from.
            if axis.phase != phase {
                continue;
            }
            if let Some(Some(p)) = pins.get(ai) {
                let mut values = p.clone();
                values.sort();
                values.dedup();
                verdict.axes.push(AxisVerdict {
                    axis: axis.name.clone(),
                    values,
                    confidence: 1.0,
                    tier: "answer".to_string(),
                });
                continue;
            }
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
                if axis.default.is_none() && verdict.diagnostics.len() < DIAGNOSTICS_MAX {
                    verdict.diagnostics.push(Diagnostic {
                        kind: "axis_unresolved".into(),
                        axis: axis.name.clone(),
                        ..Diagnostic::default()
                    });
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

    /// What a rule's `sets` entry would store for this stack: the values
    /// whose conditions hold, as the axis stores them, joined as a row does.
    fn would_store(
        &self,
        axis: &crate::rules::Axis,
        sets: &crate::rules::Sets,
        derived: &[Option<usize>],
    ) -> String {
        sets.values
            .iter()
            .filter(|v| v.when.as_ref().is_none_or(|w| w.eval(None, self)))
            .filter_map(|v| which(v.value, derived))
            .map(|i| axis.stored(i).to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The clauses of a rule that hold on this stack, from the first, which
    /// [`Evaluated::fire`] found, to the last. The rule's `requires` has
    /// already been read by then.
    fn held(&self, rule: &Rule, first: usize) -> Vec<usize> {
        let mut out = vec![first];
        out.extend(
            rule.clauses
                .iter()
                .enumerate()
                .skip(first + 1)
                .filter(|(_, c)| self.holds(c))
                .map(|(i, _)| i),
        );
        out
    }

    /// Record 41, S2: one vote per clause that held and per value the
    /// rule's `sets` entry gives this stack. A value whose own condition
    /// fails is not said; a rule that decides the axis to nothing says the
    /// empty value.
    fn vote(
        &self,
        votes: &mut Vec<Vote>,
        set: &crate::rules::RuleSet,
        rule: &Rule,
        held: &[usize],
        sets: &crate::rules::Sets,
        derived: &[Option<usize>],
    ) {
        if held.is_empty() {
            return;
        }
        let axis = &self.pack.axes[sets.axis];
        for v in &sets.values {
            if let Some(w) = &v.when
                && !w.eval(None, self)
            {
                continue;
            }
            let value = which(v.value, derived)
                .map(|i| axis.stored(i).to_string())
                .unwrap_or_default();
            for &c in held {
                votes.push(Vote {
                    axis: axis.name.clone(),
                    value: value.clone(),
                    rule_set: set.name.clone(),
                    rule: rule.id.clone(),
                    clause: c,
                    tier: rule.clauses[c].tier().name().to_string(),
                });
            }
        }
    }

    /// Wave 4c §6.6, `axis_conflict`: a rule fired for an axis an earlier
    /// set closed, and would have stored something else. The order decided;
    /// both are recorded. The same answer is agreement and says nothing.
    #[allow(clippy::too_many_arguments)]
    fn conflict(
        &self,
        verdict: &mut Verdict,
        set: &crate::rules::RuleSet,
        rule: &Rule,
        fired: &Fired,
        sets: &crate::rules::Sets,
        derived: &[Option<usize>],
        decided_by: &[Option<(String, String, String, String)>],
    ) {
        if verdict.diagnostics.len() >= DIAGNOSTICS_MAX {
            return;
        }
        let axis = &self.pack.axes[sets.axis];
        let value = self.would_store(axis, sets, derived);
        let (by_set, by_rule, by_value, by_matched) = match &decided_by[sets.axis] {
            Some((s, r, v, m)) => (s.clone(), r.clone(), v.clone(), m.clone()),
            None => (String::new(), String::new(), String::new(), String::new()),
        };
        if value == by_value {
            return;
        }
        verdict.diagnostics.push(Diagnostic {
            kind: "axis_conflict".into(),
            axis: axis.name.clone(),
            rule_set: set.name.clone(),
            rule: rule.id.clone(),
            value,
            matched: fired.matched.clone(),
            by_rule_set: by_set,
            by_rule,
            by_value,
            by_matched,
        });
    }

    /// Every keyword of a rule that is found in its field on this stack, in
    /// list order. `fire` cites the first; the rest are what it hid.
    fn keyword_hits(&self, rule: &Rule) -> Vec<String> {
        let mut out = Vec::new();
        for c in &rule.clauses {
            if let Clause::Keywords { field, list, .. } = c {
                let text = <Self as Ctx>::text(self, *field).to_lowercase();
                if text.is_empty() {
                    continue;
                }
                for kw in list {
                    if text.contains(&kw.to_lowercase()) {
                        out.push(kw.clone());
                    }
                }
            }
        }
        out
    }

    /// Wave 4c §6.6, `keyword_shadowed`: the keywords that matched on this
    /// stack and were not cited, because the winning rule cited another, or
    /// because a later rule of a deciding set never ran.
    fn shadowed(
        &self,
        verdict: &mut Verdict,
        set: &crate::rules::RuleSet,
        won: usize,
        winner: &Rule,
        fired: &Fired,
    ) {
        let axis_of = |r: &Rule| {
            r.sets
                .first()
                .map(|s| self.pack.axes[s.axis].name.clone())
                .unwrap_or_default()
        };
        let by_value = winner
            .sets
            .first()
            .map(|s| {
                let axis = &self.pack.axes[s.axis];
                s.values
                    .iter()
                    .filter_map(|v| match v.value {
                        Which::Fixed(i) => Some(axis.stored(i).to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let mut push = |rule: &Rule, kw: String| {
            if verdict.diagnostics.len() >= DIAGNOSTICS_MAX {
                return;
            }
            verdict.diagnostics.push(Diagnostic {
                kind: "keyword_shadowed".into(),
                axis: axis_of(rule),
                rule_set: set.name.clone(),
                rule: rule.id.clone(),
                value: String::new(),
                matched: kw,
                by_rule_set: set.name.clone(),
                by_rule: winner.id.clone(),
                by_value: by_value.clone(),
                by_matched: fired.matched.clone(),
            });
        };
        for kw in self.keyword_hits(winner) {
            if !kw.eq_ignore_ascii_case(&fired.matched) {
                push(winner, kw);
            }
        }
        for rule in &set.rules[won + 1..] {
            if self.fire(rule).is_none() {
                continue;
            }
            for kw in self.keyword_hits(rule) {
                push(rule, kw);
            }
        }
    }

    /// The first clause of a rule that holds, with what it cites. A rule with
    /// a `requires` says nothing at all unless it holds.
    fn fire(&self, rule: &Rule) -> Option<Fired> {
        if let Some(g) = &rule.requires
            && !g.eval(None, self)
        {
            return None;
        }
        for (clause, c) in rule.clauses.iter().enumerate() {
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
                            text: None,
                            clause,
                        });
                    }
                }
                Clause::Keywords {
                    tier,
                    confidence,
                    field,
                    list,
                    ..
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
                            text: Some(*field),
                            clause,
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
                            text: None,
                            clause,
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
                            text: None,
                            clause,
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
                            text: expr.one_text(),
                            clause,
                        });
                    }
                }
            }
        }
        None
    }
}

impl Evaluated<'_> {
    /// Whether one clause holds on this stack, by the test
    /// [`Evaluated::fire`] applies to it.
    fn holds(&self, c: &Clause) -> bool {
        match c {
            Clause::Flag { flag, .. } => self.flags[*flag],
            Clause::Keywords { field, list, .. } => {
                let text = <Self as Ctx>::text(self, *field).to_lowercase();
                !text.is_empty() && list.iter().any(|k| text.contains(&k.to_lowercase()))
            }
            Clause::AnyFlag { flags, .. } => flags.iter().any(|f| self.flags[*f]),
            Clause::Combination { flags, .. } => {
                !flags.is_empty() && flags.iter().all(|f| self.flags[*f])
            }
            Clause::When { expr, .. } => expr.eval(None, self),
        }
    }

    /// The text the pack derived, by the name it published it under.
    pub fn derived_text(&self, name: &str) -> Option<&str> {
        let i = self.pack.derived.iter().position(|d| d.into == name)?;
        Some(&self.derived[i])
    }
}
