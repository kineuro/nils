// SPDX-License-Identifier: AGPL-3.0-only

//! Passes: the phases that read more than one stack
//! (`docs/specs/wave2-fingerprint-and-classify.md`, §7).
//!
//! A rule is an expression and the pack writes it. A pass is an **algorithm**,
//! so the engine owns it and the pack owns every number in it: the bins, the
//! widening schedule, the minimum, the rounding mode, the compatibility table.
//! What a pack declares is a configured instance of a kind the engine
//! provides, and a pack that declares none gets none.
//!
//! Three things separate this from v0's gap filling. The reference is
//! **declared** rather than being whatever the database happened to hold when
//! the run started, so the same stack against the same reference gives the
//! same answer next year. The vote is **written down** as evidence, with the
//! method, the number of neighbours and the size of the pool, instead of
//! appearing as a value from nowhere. And a tie is **not** broken by the order
//! rows arrived in.

use std::collections::{HashMap, HashSet};

use crate::expr::{Case, Ctx, Expr, Subject};
use crate::rules::Axis;

/// When a pass runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before the rules, on what the fingerprint says.
    Before,
    /// After them, on what they decided.
    After,
}

/// One dimension of a vote's key: a field, and how a value is binned.
#[derive(Debug, Clone)]
pub struct KeyDim {
    pub name: String,
    pub field: usize,
    pub round: Option<f64>,
    pub ceil: Option<f64>,
    /// Python rounds a half to even and Rust rounds it away from zero, which
    /// puts a TR of exactly 50 ms in a different bin. The mode is the pack's.
    pub half_even: bool,
}

/// The last resort for inversion recovery, whose TI ranges widely by vendor
/// and field strength: vary TI, and TE with it.
#[derive(Debug, Clone)]
pub struct Relaxed {
    pub requires: usize,
    pub requires_span: i64,
    pub other: usize,
    pub other_span: i64,
    pub method: String,
}

/// What a vote does when two answers are equally popular.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnTie {
    /// Say nothing, which is what v1 does.
    Nothing,
    /// Take whichever was seen first, which is what v0 does, and which makes
    /// the answer depend on what the database returned in what order.
    FirstSeen,
}

/// One rule of the compatibility filter: when it holds, it decides.
#[derive(Debug, Clone)]
pub struct CompatRule {
    pub when: Expr,
    pub allow: Expr,
}

/// Whether a candidate answer fits the stack it would be written to. Ordered,
/// first match decides, which is the shape of a rule set.
#[derive(Debug, Clone)]
pub struct Compat {
    pub subject_field: usize,
    pub subject_case: Case,
    pub default_family: String,
    /// v0's gap-filling table, which is not the technique axis's own `family`:
    /// the two agree about SE, GRE and EPI and disagree about everything the
    /// axis calls MIXED, so the pass carries the one it was verified against.
    pub family_of: HashMap<String, String>,
    pub rules: Vec<CompatRule>,
}

/// What a reference row must be to count. Not a general expression: a filter
/// reads a row of the reference, which has fields and decided axes and
/// nothing else.
#[derive(Debug, Clone)]
pub struct Cond {
    pub what: What,
    pub is: Vec<String>,
    pub not: Vec<String>,
    pub present: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub enum What {
    Field(usize),
    Axis(usize),
}

/// Where the neighbours come from.
#[derive(Debug, Clone)]
pub struct Reference {
    /// Named and recorded on the row. v0 has no name for its reference
    /// because it has no reference: it reads the live table.
    pub scope: String,
    pub filter: Vec<Cond>,
    /// One pool per value of this axis, so an anatomical stack votes among
    /// anatomical ones.
    pub partition_by: Option<usize>,
    /// And the whole pool when the partition says nothing.
    pub fallback: bool,
    pub fallback_when: Vec<String>,
    /// Except for these partitions, which vote globally to begin with.
    pub fallback_except: Vec<String>,
}

/// What a pass writes down about itself.
#[derive(Debug, Clone)]
pub struct Emit {
    pub evidence: bool,
    pub review_below: f64,
    /// v0 sets its review flag on everything a pass touches, so a person who
    /// cleared a flag sees it return. A pack may ask for that; this one does
    /// not.
    pub review_all_touched: bool,
}

/// The `nearest_neighbour_vote` kind: fill an axis from the stacks whose
/// acquisition physics is the same.
#[derive(Debug, Clone)]
pub struct Vote {
    pub dims: Vec<KeyDim>,
    pub max_distance: i64,
    pub pairs: Vec<(usize, usize)>,
    pub relaxed: Option<Relaxed>,
    pub min_matches: usize,
    pub on_tie: OnTie,
    /// The axes voted on together, as one answer.
    pub vote_on: Vec<usize>,
    /// The axes the winner is written to.
    pub writes: Vec<usize>,
    /// And when: an axis that already says something keeps it.
    pub write_when: Vec<String>,
    pub compat: Compat,
    /// Which of `vote_on` the compatibility filter judges.
    pub compat_axis: usize,
}

#[derive(Debug, Clone)]
pub enum Kind {
    Vote(Vote),
}

#[derive(Debug, Clone)]
pub struct Pass {
    pub name: String,
    pub phase: Phase,
    pub kind: Kind,
    /// Which stacks it runs on. A pass with no target runs on all of them.
    pub target: Option<Expr>,
    pub reference: Reference,
    pub emit: Emit,
}

impl Pass {
    pub fn vote(&self) -> Option<&Vote> {
        match &self.kind {
            Kind::Vote(v) => Some(v),
        }
    }

    /// The kind's name, as the row records it.
    pub fn kind_name(&self) -> &'static str {
        match &self.kind {
            Kind::Vote(_) => "nearest_neighbour_vote",
        }
    }
}

// ---------------------------------------------------------------------------
// The vote itself. Carried from the prototype that was checked against v0 over
// the whole live corpus (spikes/pack), with the tie left unbroken.

/// A binned key. Five dimensions is what v0 uses and what the pack declares;
/// a dimension the stack has no value for is a hole, and a hole matches only
/// a hole.
pub type Key = [Option<i64>; 5];

fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && (r as i64) % 2 != 0 {
        r - x.signum()
    } else {
        r
    }
}

/// The bin a stack falls in.
pub fn key_of(v: &Vote, values: &[Option<f64>]) -> Key {
    let mut k: Key = [None; 5];
    for (i, d) in v.dims.iter().enumerate().take(5) {
        k[i] = match values.get(i).copied().flatten() {
            None => None,
            Some(x) => {
                if let Some(step) = d.round {
                    let q = x / step;
                    let r = if d.half_even {
                        round_half_even(q)
                    } else {
                        q.round()
                    };
                    Some((r * step) as i64)
                } else if let Some(step) = d.ceil {
                    (x > 0.0).then(|| ((x / step).ceil() * step) as i64)
                } else {
                    Some(x as i64)
                }
            }
        };
    }
    k
}

fn step_of(v: &Vote, i: usize) -> i64 {
    let d = &v.dims[i];
    d.round.or(d.ceil).unwrap_or(1.0) as i64
}

/// One bin moved in one dimension, for every dimension that has a value.
fn adjacent(v: &Vote, k: &Key, distance: i64) -> Vec<Key> {
    let mut out = Vec::new();
    for i in 0..v.dims.len().min(5) {
        let Some(cur) = k[i] else { continue };
        let step = step_of(v, i);
        for delta in -distance..=distance {
            if delta == 0 {
                continue;
            }
            let moved = cur + delta * step;
            if moved < 0 {
                continue;
            }
            let mut next = *k;
            next[i] = Some(moved);
            out.push(next);
        }
    }
    out
}

/// The pairs that co-vary in practice, both moved at once.
fn adjacent_pairs(v: &Vote, k: &Key, distance: i64) -> Vec<Key> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (a, b) in &v.pairs {
        let (Some(va), Some(vb)) = (k[*a], k[*b]) else {
            continue;
        };
        let (sa, sb) = (step_of(v, *a), step_of(v, *b));
        for d1 in -distance..=distance {
            for d2 in -distance..=distance {
                if d1 == 0 || d2 == 0 {
                    continue;
                }
                let (na, nb) = (va + d1 * sa, vb + d2 * sb);
                if na < 0 || nb < 0 {
                    continue;
                }
                let mut next = *k;
                next[*a] = Some(na);
                next[*b] = Some(nb);
                if seen.insert(next) {
                    out.push(next);
                }
            }
        }
    }
    out
}

fn relaxed_keys(v: &Vote, k: &Key) -> Vec<Key> {
    let Some(r) = &v.relaxed else {
        return Vec::new();
    };
    let Some(anchor) = k[r.requires] else {
        return Vec::new();
    };
    let (step_a, step_b) = (step_of(v, r.requires), step_of(v, r.other));
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for da in -r.requires_span..=r.requires_span {
        for db in -r.other_span..=r.other_span {
            if da == 0 {
                continue;
            }
            let moved = anchor + da * step_a;
            if moved < 0 {
                continue;
            }
            let mut other = k[r.other];
            if let Some(t) = k[r.other]
                && db != 0
            {
                let x = t + db * step_b;
                if x < 0 {
                    continue;
                }
                other = Some(x);
            }
            let mut next = *k;
            next[r.requires] = Some(moved);
            next[r.other] = other;
            if seen.insert(next) {
                out.push(next);
            }
        }
    }
    out
}

/// The reference, binned. One answer is the tuple of the axes voted on.
#[derive(Debug, Default, Clone)]
pub struct Pool {
    bins: HashMap<Key, Vec<Vec<usize>>>,
    total: usize,
}

impl Pool {
    pub fn add(&mut self, key: Key, answer: Vec<usize>) {
        self.bins.entry(key).or_default().push(answer);
        self.total += 1;
    }

    pub fn len(&self) -> usize {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The neighbours, and how far the search had to go to find them.
    fn near(&self, v: &Vote, k: &Key) -> (Vec<&Vec<usize>>, String) {
        if let Some(hit) = self.bins.get(k) {
            return (hit.iter().collect(), "exact_bin".to_string());
        }
        let mut acc: Vec<&Vec<usize>> = Vec::new();
        for distance in 1..=v.max_distance {
            for nk in adjacent(v, k, distance) {
                if let Some(rows) = self.bins.get(&nk) {
                    acc.extend(rows.iter());
                }
            }
            if !acc.is_empty() {
                return (acc, "expanded_single".to_string());
            }
            for nk in adjacent_pairs(v, k, distance) {
                if let Some(rows) = self.bins.get(&nk) {
                    acc.extend(rows.iter());
                }
            }
            if !acc.is_empty() {
                return (acc, "expanded_multi".to_string());
            }
        }
        for nk in relaxed_keys(v, k) {
            if let Some(rows) = self.bins.get(&nk) {
                acc.extend(rows.iter());
            }
        }
        if !acc.is_empty() {
            let method = v
                .relaxed
                .as_ref()
                .map(|r| r.method.clone())
                .unwrap_or_else(|| "expanded_relaxed".to_string());
            return (acc, method);
        }
        (Vec::new(), "no_match".to_string())
    }
}

/// What a vote decided, and everything a person would need to see why.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub method: String,
    /// One value index per axis in `vote_on`, when the vote decided.
    pub answer: Option<Vec<usize>>,
    pub matches: usize,
    pub neighbours: usize,
    pub pool: usize,
    /// Which pool answered: the partition's name, or `global`.
    pub partition: String,
}

impl Outcome {
    /// How sure a vote is: the share of the neighbours that agreed. A vote
    /// that decided nothing is not confident about nothing, it is silent.
    pub fn confidence(&self) -> f64 {
        if self.answer.is_none() || self.neighbours == 0 {
            return 0.0;
        }
        self.matches as f64 / self.neighbours as f64
    }
}

/// The subject a compatibility rule reads: the stack's own sequence, and the
/// candidate answer being judged against it.
struct CompatCtx<'a> {
    sequence: String,
    candidate: &'a str,
    family: &'a str,
    regexes: &'a [regex::Regex],
}

impl Ctx for CompatCtx<'_> {
    fn pred(&self, _parser: usize, _pred: usize) -> bool {
        false
    }
    fn subject(&self, _parser: usize) -> Subject<'_> {
        Subject::text(&self.sequence)
    }
    fn flag(&self, _flag: usize) -> bool {
        false
    }
    fn num(&self, _field: usize) -> Option<f64> {
        None
    }
    fn present(&self, _field: usize) -> bool {
        false
    }
    fn text(&self, _field: usize) -> &str {
        &self.sequence
    }
    fn re(&self, idx: usize) -> &regex::Regex {
        &self.regexes[idx]
    }
    fn candidate(&self) -> &str {
        self.candidate
    }
    fn candidate_family(&self) -> &str {
        self.family
    }
}

/// Whether this answer may be written to this stack.
pub fn compatible(c: &Compat, sequence: &str, candidate: &str, regexes: &[regex::Regex]) -> bool {
    let family = c
        .family_of
        .get(candidate)
        .map(String::as_str)
        .unwrap_or(&c.default_family);
    let cx = CompatCtx {
        sequence: c.subject_case.apply(sequence).into_owned(),
        candidate,
        family,
        regexes,
    };
    let subject = Subject::text(&cx.sequence);
    for r in &c.rules {
        if r.when.eval(Some(&subject), &cx) {
            return r.allow.eval(Some(&subject), &cx);
        }
    }
    true
}

/// Take the vote.
pub fn take(
    v: &Vote,
    pool: &Pool,
    key: &Key,
    sequence: &str,
    axes: &[Axis],
    regexes: &[regex::Regex],
    partition: &str,
) -> Outcome {
    let (neighbours, method) = pool.near(v, key);
    let silent = |method: &str, matches: usize| Outcome {
        method: method.to_string(),
        answer: None,
        matches,
        neighbours: neighbours.len(),
        pool: pool.len(),
        partition: partition.to_string(),
    };
    if neighbours.is_empty() {
        return silent("no_match", 0);
    }

    let mut order: Vec<&Vec<usize>> = Vec::new();
    let mut counts: HashMap<&Vec<usize>, usize> = HashMap::new();
    for answer in &neighbours {
        let n = counts.entry(*answer).or_insert(0);
        if *n == 0 {
            order.push(*answer);
        }
        *n += 1;
    }
    let mut ranked: Vec<(&Vec<usize>, usize)> = order.iter().map(|a| (*a, counts[*a])).collect();
    // Stable, so an equal count keeps the order the reference was seen in.
    // That order is what v0 lets decide a tie; here it only decides which of
    // two equal answers is reported as the tie.
    ranked.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    for (i, (answer, count)) in ranked.iter().enumerate() {
        let candidate = &axes[v.vote_on[v.compat_axis]].values[answer[v.compat_axis]].id;
        if !compatible(&v.compat, sequence, candidate, regexes) {
            continue;
        }
        if *count < v.min_matches {
            return silent("insufficient_matches", *count);
        }
        // Two answers are equally popular and the pack has not said to prefer
        // the one that arrived first. v0 would take it and never say so.
        if v.on_tie == OnTie::Nothing
            && ranked
                .get(i + 1)
                .is_some_and(|(other, n)| *n == *count && other != answer)
        {
            return silent("tie", *count);
        }
        return Outcome {
            method,
            answer: Some((*answer).clone()),
            matches: *count,
            neighbours: neighbours.len(),
            pool: pool.len(),
            partition: partition.to_string(),
        };
    }
    silent("no_compatible_match", 0)
}
