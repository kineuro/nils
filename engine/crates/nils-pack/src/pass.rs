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

/// How many axes one vote may be about. Four is more than v0 has ever needed
/// and it keeps an answer a small copied value: a bin can hold a quarter of a
/// million rows, and every one of them is counted for every stack that lands
/// in it, so an answer that allocates turns a second into an hour.
pub const MAX_VOTE_AXES: usize = 4;

/// One answer: a value index per axis voted on, `u16::MAX` for the axes this
/// vote is not about.
pub type Answer = [u16; MAX_VOTE_AXES];

const NO_VALUE: u16 = u16::MAX;

fn answer_of(values: &[usize]) -> Answer {
    let mut a = [NO_VALUE; MAX_VOTE_AXES];
    for (i, v) in values.iter().enumerate().take(MAX_VOTE_AXES) {
        a[i] = *v as u16;
    }
    a
}

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
    /// Per bin, the distinct answers with how many stacks gave them, in the
    /// order they were first seen. A bin can hold a quarter of a million
    /// stacks and be searched by a quarter of a million others, so what a bin
    /// says is counted once, when it is built, and not once per question.
    bins: HashMap<Key, Vec<(Answer, u32)>>,
    total: usize,
}

impl Pool {
    pub fn add(&mut self, key: Key, answer: Answer) {
        let bin = self.bins.entry(key).or_default();
        match bin.iter_mut().find(|(a, _)| *a == answer) {
            Some((_, n)) => *n += 1,
            None => bin.push((answer, 1)),
        }
        self.total += 1;
    }

    pub fn len(&self) -> usize {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The neighbours, and how far the search had to go to find them.
    fn near<'a>(&'a self, v: &Vote, k: &Key) -> (Counted<'a>, String) {
        if let Some(hit) = self.bins.get(k) {
            return (Counted::Bin(hit), "exact_bin".to_string());
        }
        let mut acc: Vec<(Answer, u32)> = Vec::new();
        let merge = |rows: &[(Answer, u32)], acc: &mut Vec<(Answer, u32)>| {
            for (a, n) in rows {
                match acc.iter_mut().find(|(x, _)| x == a) {
                    Some((_, m)) => *m += n,
                    None => acc.push((*a, *n)),
                }
            }
        };
        for distance in 1..=v.max_distance {
            for nk in adjacent(v, k, distance) {
                if let Some(rows) = self.bins.get(&nk) {
                    merge(rows, &mut acc);
                }
            }
            if !acc.is_empty() {
                return (Counted::Merged(acc), "expanded_single".to_string());
            }
            for nk in adjacent_pairs(v, k, distance) {
                if let Some(rows) = self.bins.get(&nk) {
                    merge(rows, &mut acc);
                }
            }
            if !acc.is_empty() {
                return (Counted::Merged(acc), "expanded_multi".to_string());
            }
        }
        for nk in relaxed_keys(v, k) {
            if let Some(rows) = self.bins.get(&nk) {
                merge(rows, &mut acc);
            }
        }
        if !acc.is_empty() {
            let method = v
                .relaxed
                .as_ref()
                .map(|r| r.method.clone())
                .unwrap_or_else(|| "expanded_relaxed".to_string());
            return (Counted::Merged(acc), method);
        }
        (Counted::Merged(Vec::new()), "no_match".to_string())
    }
}

/// The neighbours a search found: a bin as it stands, or several merged.
enum Counted<'a> {
    Bin(&'a [(Answer, u32)]),
    Merged(Vec<(Answer, u32)>),
}

impl Counted<'_> {
    fn rows(&self) -> &[(Answer, u32)] {
        match self {
            Counted::Bin(b) => b,
            Counted::Merged(v) => v,
        }
    }

    /// How many stacks the search saw, which is what a share is taken of.
    fn total(&self) -> usize {
        self.rows().iter().map(|(_, n)| *n as usize).sum()
    }
}

/// What a vote decided, and everything a person would need to see why.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub method: String,
    /// One value index per axis in `vote_on`, when the vote decided.
    pub answer: Option<Answer>,
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
    let seen = neighbours.total();
    let silent = |method: &str, matches: usize| Outcome {
        method: method.to_string(),
        answer: None,
        matches,
        neighbours: seen,
        pool: pool.len(),
        partition: partition.to_string(),
    };
    if seen == 0 {
        return silent("no_match", 0);
    }

    let mut ranked: Vec<(Answer, usize)> = neighbours
        .rows()
        .iter()
        .map(|(a, n)| (*a, *n as usize))
        .collect();
    // Stable, so an equal count keeps the order the reference was seen in.
    // That order is what v0 lets decide a tie; here it only decides which of
    // two equal answers is reported as the tie.
    ranked.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    for (i, (answer, count)) in ranked.iter().enumerate() {
        // What v0 reads from its own column, which is the value as stored:
        // its family table is keyed on that, and `BOLD` and `BOLD-EPI` are
        // two rows of it with two different families.
        let judged = &axes[v.vote_on[v.compat_axis]];
        let candidate = judged.stored(answer[v.compat_axis] as usize);
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
            answer: Some(*answer),
            matches: *count,
            neighbours: seen,
            pool: pool.len(),
            partition: partition.to_string(),
        };
    }
    silent("no_compatible_match", 0)
}

// ---------------------------------------------------------------------------
// The corpus a pass runs over, and running one over it. Both callers use this:
// the engine's job, which reads the registry, and the checker, which reads a
// CSV. There is one implementation of the algorithm, and one of what it reads.

/// Half a million stacks, holding only what the passes named.
///
/// A pass reads a handful of fields and the decided axes, so carrying every
/// column of every stack costs gigabytes for nothing. What is here is the
/// fields the passes actually name, interned, and the axes as small indices
/// into their own vocabularies.
pub struct Corpus {
    /// Field index to slot, for the fields any pass reads.
    slot: Vec<Option<u32>>,
    slots: usize,
    axes: usize,
    /// Per slot, the distinct values seen; index 0 is always the empty one.
    pool: Vec<Vec<Box<str>>>,
    seen: Vec<HashMap<Box<str>, u32>>,
    cells: Vec<u32>,
    axis_pool: Vec<Vec<Box<str>>>,
    axis_seen: Vec<HashMap<Box<str>, u16>>,
    axis_cells: Vec<u16>,
    pub ids: Vec<i64>,
}

impl Corpus {
    /// Sized for what this pack's passes read, and nothing else.
    pub fn new(pack: &crate::Pack) -> Corpus {
        let mut needed: Vec<usize> = Vec::new();
        for pass in &pack.passes {
            if let Some(t) = &pass.target {
                t.fields(&mut needed);
            }
            for c in &pass.reference.filter {
                if let What::Field(i) = c.what {
                    needed.push(i);
                }
            }
            if let Some(v) = pass.vote() {
                needed.extend(v.dims.iter().map(|d| d.field));
                needed.push(v.compat.subject_field);
                for r in &v.compat.rules {
                    r.when.fields(&mut needed);
                    r.allow.fields(&mut needed);
                }
            }
        }
        needed.sort_unstable();
        needed.dedup();
        let width = needed.iter().max().map(|m| m + 1).unwrap_or(0);
        let mut slot = vec![None; width];
        for (i, f) in needed.iter().enumerate() {
            slot[*f] = Some(i as u32);
        }
        let slots = needed.len();
        Corpus {
            slot,
            slots,
            axes: pack.axes.len(),
            pool: (0..slots).map(|_| vec![Box::from("")]).collect(),
            seen: (0..slots).map(|_| HashMap::new()).collect(),
            cells: Vec::new(),
            axis_pool: (0..pack.axes.len()).map(|_| vec![Box::from("")]).collect(),
            axis_seen: (0..pack.axes.len()).map(|_| HashMap::new()).collect(),
            axis_cells: Vec::new(),
            ids: Vec::new(),
        }
    }

    /// Whether any pass reads this field, so a caller need not fetch it.
    pub fn reads(&self, field: usize) -> bool {
        self.slot.get(field).copied().flatten().is_some()
    }

    /// The fields any pass reads, in order.
    pub fn needed(&self) -> Vec<usize> {
        let mut out: Vec<usize> = (0..self.slot.len()).filter(|f| self.reads(*f)).collect();
        out.sort_by_key(|f| self.slot[*f]);
        out
    }

    fn intern(pool: &mut Vec<Box<str>>, seen: &mut HashMap<Box<str>, u32>, v: &str) -> u32 {
        if v.is_empty() {
            return 0;
        }
        if let Some(i) = seen.get(v) {
            return *i;
        }
        let i = pool.len() as u32;
        pool.push(Box::from(v));
        seen.insert(Box::from(v), i);
        i
    }

    /// Add one stack: what the fingerprint says, and what the rules decided.
    pub fn push(
        &mut self,
        id: i64,
        field: impl Fn(usize) -> String,
        axis: impl Fn(usize) -> String,
    ) {
        self.ids.push(id);
        for f in 0..self.slot.len() {
            let Some(s) = self.slot[f] else { continue };
            let v = field(f);
            let at = self.cells.len() - (self.cells.len() % self.slots.max(1));
            let _ = at;
            let ix = Corpus::intern(&mut self.pool[s as usize], &mut self.seen[s as usize], &v);
            // The slots of one stack are contiguous and in slot order, which
            // the loop above guarantees.
            self.cells.push(ix);
        }
        for a in 0..self.axes {
            let v = axis(a);
            let ix = if v.is_empty() {
                0
            } else if let Some(i) = self.axis_seen[a].get(v.as_str()) {
                *i
            } else {
                let i = self.axis_pool[a].len() as u16;
                self.axis_pool[a].push(Box::from(v.as_str()));
                self.axis_seen[a].insert(Box::from(v.as_str()), i);
                i
            };
            self.axis_cells.push(ix);
        }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// One stack, as an expression sees it.
    pub fn row(&self, i: usize) -> Row<'_> {
        Row {
            corpus: self,
            at: i,
        }
    }

    /// What the rules decided about this axis of this stack.
    pub fn axis_of(&self, i: usize, axis: usize) -> &str {
        &self.axis_pool[axis][self.axis_cells[i * self.axes + axis] as usize]
    }
}

/// One stack of a [`Corpus`], and the only thing a pass's expressions may
/// read: the fields the pack named, and the axes the rules decided.
pub struct Row<'a> {
    corpus: &'a Corpus,
    at: usize,
}

impl Row<'_> {
    fn cell(&self, field: usize) -> &str {
        match self.corpus.slot.get(field).copied().flatten() {
            None => "",
            Some(s) => {
                let ix = self.corpus.cells[self.at * self.corpus.slots + s as usize];
                &self.corpus.pool[s as usize][ix as usize]
            }
        }
    }
}

impl Ctx for Row<'_> {
    fn pred(&self, _parser: usize, _pred: usize) -> bool {
        unreachable!("a pass may not name a parser predicate")
    }
    fn subject(&self, _parser: usize) -> Subject<'_> {
        unreachable!("a pass may not name a parser")
    }
    fn flag(&self, _flag: usize) -> bool {
        unreachable!("a pass may not name a flag")
    }
    fn num(&self, field: usize) -> Option<f64> {
        self.cell(field).parse().ok()
    }
    fn present(&self, field: usize) -> bool {
        !self.cell(field).is_empty()
    }
    fn text(&self, field: usize) -> &str {
        self.cell(field)
    }
    fn re(&self, _idx: usize) -> &regex::Regex {
        unreachable!("a pass's expressions carry no patterns of their own")
    }
    fn axis_is(&self, axis: usize, value: &str) -> bool {
        self.corpus.axis_of(self.at, axis) == value
    }
    fn axis_empty(&self, axis: usize) -> bool {
        self.corpus.axis_of(self.at, axis).is_empty()
    }
}

impl Cond {
    /// Whether a row of the reference passes this condition.
    pub fn holds(&self, corpus: &Corpus, at: usize) -> bool {
        let row = corpus.row(at);
        let value = match self.what {
            What::Field(i) => row.cell(i),
            What::Axis(i) => corpus.axis_of(at, i),
        };
        let present = !value.is_empty();
        if let Some(want) = self.present
            && present != want
        {
            return false;
        }
        if !present {
            return self.is.is_empty();
        }
        (self.is.is_empty() || self.is.iter().any(|w| w == value))
            && !self.not.iter().any(|w| w == value)
    }
}

/// What a pass decided about one stack.
pub struct Answered {
    pub at: usize,
    pub outcome: Outcome,
    /// The axes it would write, and what it would write to them. Whether the
    /// write happens is the caller's: an axis that already says something
    /// keeps it unless the pack said otherwise.
    pub writes: Vec<(usize, String)>,
}

/// Run one vote over a corpus: build the reference the pack declared, then
/// answer for every stack the pass targets. `all` asks for every stack
/// whether or not it is a target, which is how the result is compared with
/// v0's, since v0 votes for everything and then decides what to keep.
pub fn run_vote(
    pack: &crate::Pack,
    pass: &Pass,
    vote: &Vote,
    corpus: &Corpus,
    all: bool,
) -> (Vec<Answered>, usize, usize) {
    let mut pools: HashMap<String, Pool> = HashMap::new();
    let mut global = Pool::default();
    for i in 0..corpus.len() {
        if !pass.reference.filter.iter().all(|c| c.holds(corpus, i)) {
            continue;
        }
        let found: Option<Vec<usize>> = vote
            .vote_on
            .iter()
            .map(|a| {
                let axis = &pack.axes[*a];
                let stored = corpus.axis_of(i, *a);
                (0..axis.values.len()).find(|v| axis.stored(*v) == stored)
            })
            .collect();
        let Some(found) = found else { continue };
        let answer = answer_of(&found);
        let row = corpus.row(i);
        let values: Vec<Option<f64>> = vote.dims.iter().map(|d| row.num(d.field)).collect();
        let key = key_of(vote, &values);
        if let Some(p) = pass.reference.partition_by {
            let name = corpus.axis_of(i, p).to_string();
            pools.entry(name).or_default().add(key, answer);
        }
        global.add(key, answer);
    }

    let mut out = Vec::new();
    for i in 0..corpus.len() {
        let row = corpus.row(i);
        if !all
            && let Some(t) = &pass.target
            && !t.eval(None, &row)
        {
            continue;
        }
        let values: Vec<Option<f64>> = vote.dims.iter().map(|d| row.num(d.field)).collect();
        let key = key_of(vote, &values);
        let sequence = row.text(vote.compat.subject_field).to_string();
        let partition = pass
            .reference
            .partition_by
            .map(|p| corpus.axis_of(i, p).to_string());
        let ask = |pool: &Pool, name: &str| {
            take(vote, pool, &key, &sequence, &pack.axes, &pack.regexes, name)
        };
        let mut outcome = match &partition {
            Some(name)
                if pools.contains_key(name) && !pass.reference.fallback_except.contains(name) =>
            {
                ask(&pools[name], "scoped")
            }
            _ => ask(&global, "global"),
        };
        // Its own pool said nothing, so the whole one is asked, which is what
        // the pack means by a fallback.
        if outcome.answer.is_none()
            && pass.reference.fallback
            && outcome.partition != "global"
            && pass.reference.fallback_when.contains(&outcome.method)
        {
            outcome = ask(&global, "global");
        }
        let writes = match &outcome.answer {
            None => Vec::new(),
            Some(answer) => vote
                .writes
                .iter()
                .map(|a| {
                    let at = vote
                        .vote_on
                        .iter()
                        .position(|x| x == a)
                        .expect("an axis written is an axis voted on");
                    (*a, pack.axes[*a].stored(answer[at] as usize).to_string())
                })
                .collect(),
        };
        out.push(Answered {
            at: i,
            outcome,
            writes,
        });
    }
    (out, global.len(), pools.len())
}
