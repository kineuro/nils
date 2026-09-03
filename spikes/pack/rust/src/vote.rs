// SPDX-License-Identifier: AGPL-3.0-only
//! The `nearest_neighbour_vote` pass kind.
//!
//! A pass is an algorithm, so the engine owns it; the pack owns every number
//! in it. This is v0's `sort/gap_filling.py` with the bins, the widening
//! schedule, the minimum and the compatibility table read from the pack.

use crate::expr::{Case, Ctx, Expr, Subject};
use crate::pack::{Compat, VotePass};
use regex::Regex;
use std::collections::HashMap;

pub type Key = [Option<i64>; 5];

/// Python's `round` is half-to-even and Rust's is half-away-from-zero, which
/// puts a TR of exactly 50 ms in a different bin. The mode is part of the
/// pack, not part of whichever language happens to run it.
fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && (r as i64) % 2 != 0 {
        r - x.signum()
    } else {
        r
    }
}

pub fn key_of(p: &VotePass, vals: &[Option<f64>]) -> Key {
    let mut k: Key = [None; 5];
    for (i, d) in p.dims.iter().enumerate().take(5) {
        k[i] = match vals[i] {
            None => None,
            Some(v) => {
                if let Some(step) = d.round {
                    let q = v / step;
                    let r = if d.half_even { round_half_even(q) } else { q.round() };
                    Some((r * step) as i64)
                } else if let Some(step) = d.ceil {
                    if v > 0.0 {
                        Some(((v / step).ceil() * step) as i64)
                    } else {
                        None
                    }
                } else {
                    Some(v as i64)
                }
            }
        };
    }
    k
}

fn step_of(p: &VotePass, i: usize) -> i64 {
    let d = &p.dims[i];
    d.round.or(d.ceil).unwrap_or(1.0) as i64
}

/// One bin moved in one dimension, for every dimension that has a value.
fn adjacent(p: &VotePass, k: &Key, distance: i64) -> Vec<Key> {
    let mut out = Vec::new();
    for i in 0..5 {
        let Some(cur) = k[i] else { continue };
        let step = step_of(p, i);
        for delta in -distance..=distance {
            if delta == 0 {
                continue;
            }
            let v = cur + delta * step;
            if v < 0 {
                continue;
            }
            let mut nk = *k;
            nk[i] = Some(v);
            out.push(nk);
        }
    }
    out
}

/// The two pairs that co-vary in practice, both moved at once.
fn adjacent_pairs(p: &VotePass, k: &Key, distance: i64) -> Vec<Key> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (a, b) in &p.pairs {
        let (Some(va), Some(vb)) = (k[*a], k[*b]) else {
            continue;
        };
        let (sa, sb) = (step_of(p, *a), step_of(p, *b));
        for d1 in -distance..=distance {
            for d2 in -distance..=distance {
                if d1 == 0 || d2 == 0 {
                    continue;
                }
                let (na, nb) = (va + d1 * sa, vb + d2 * sb);
                if na < 0 || nb < 0 {
                    continue;
                }
                let mut nk = *k;
                nk[*a] = Some(na);
                nk[*b] = Some(nb);
                if seen.insert(nk) {
                    out.push(nk);
                }
            }
        }
    }
    out
}

/// The last resort for inversion-recovery sequences, whose TI ranges widely
/// across vendors and field strengths.
fn relaxed(p: &VotePass, k: &Key) -> Vec<Key> {
    let Some((ti, ti_max, te, te_max)) = p.relaxed else {
        return Vec::new();
    };
    let Some(ti_val) = k[ti] else {
        return Vec::new();
    };
    let (ti_step, te_step) = (step_of(p, ti), step_of(p, te));
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for d_ti in -ti_max..=ti_max {
        for d_te in -te_max..=te_max {
            if d_ti == 0 {
                continue;
            }
            let new_ti = ti_val + d_ti * ti_step;
            if new_ti < 0 {
                continue;
            }
            let mut new_te = k[te];
            if let Some(t) = k[te]
                && d_te != 0
            {
                let v = t + d_te * te_step;
                if v < 0 {
                    continue;
                }
                new_te = Some(v);
            }
            let mut nk = *k;
            nk[ti] = Some(new_ti);
            nk[te] = new_te;
            if seen.insert(nk) {
                out.push(nk);
            }
        }
    }
    out
}

/// A reference stack, reduced to what a vote reads.
#[derive(Clone, Copy)]
pub struct Ref {
    pub base: u32,
    pub technique: u32,
}

#[derive(Default)]
pub struct Pool {
    pub bins: HashMap<Key, Vec<Ref>>,
    pub total: usize,
}

impl Pool {
    pub fn add(&mut self, k: Key, r: Ref) {
        self.bins.entry(k).or_default().push(r);
        self.total += 1;
    }

    fn expanded(&self, p: &VotePass, k: &Key) -> (Vec<Ref>, &'static str) {
        if let Some(hit) = self.bins.get(k) {
            return (hit.clone(), "exact_bin");
        }
        let mut acc: Vec<Ref> = Vec::new();
        for distance in 1..=p.max_distance {
            for nk in adjacent(p, k, distance) {
                if let Some(v) = self.bins.get(&nk) {
                    acc.extend_from_slice(v);
                }
            }
            if !acc.is_empty() {
                return (acc, "expanded_single");
            }
            for nk in adjacent_pairs(p, k, distance) {
                if let Some(v) = self.bins.get(&nk) {
                    acc.extend_from_slice(v);
                }
            }
            if !acc.is_empty() {
                return (acc, "expanded_multi");
            }
        }
        for nk in relaxed(p, k) {
            if let Some(v) = self.bins.get(&nk) {
                acc.extend_from_slice(v);
            }
        }
        if !acc.is_empty() {
            return (acc, "expanded_relaxed_ti");
        }
        (Vec::new(), "no_match")
    }
}

pub struct Outcome {
    pub method: &'static str,
    pub base: Option<u32>,
    pub technique: Option<u32>,
    pub matches: usize,
    pub total_in_bin: usize,
}

/// The subject a compatibility rule reads: the stack's ScanningSequence, and
/// the candidate technique the rule is judging.
struct CompatCtx<'a> {
    seq: String,
    candidate: &'a str,
    family: &'a str,
    empty_re: &'a [Regex],
}

impl Ctx for CompatCtx<'_> {
    fn pred(&self, _p: usize, _x: usize) -> bool {
        false
    }
    fn subject(&self, _p: usize) -> Subject<'_> {
        Subject::text(&self.seq)
    }
    fn flag(&self, _f: usize) -> bool {
        false
    }
    fn num(&self, _f: usize) -> Option<f64> {
        None
    }
    fn scalar(&self, _f: usize) -> Option<&str> {
        None
    }
    fn text(&self, _f: usize, case: Case) -> std::borrow::Cow<'_, str> {
        case.apply(&self.seq)
    }
    fn re(&self, i: usize) -> &Regex {
        &self.empty_re[i]
    }
    fn candidate(&self) -> &str {
        self.candidate
    }
    fn candidate_family(&self) -> &str {
        self.family
    }
}

fn compatible(c: &Compat, seq: &str, technique: &str, regexes: &[Regex]) -> bool {
    let family = c
        .family_of
        .get(technique)
        .map(String::as_str)
        .unwrap_or(&c.default_family);
    let cx = CompatCtx {
        seq: c.subject_case.apply(seq).into_owned(),
        candidate: technique,
        family,
        empty_re: regexes,
    };
    let subj = Subject::text(&cx.seq);
    for r in &c.rules {
        if r.when.eval(Some(&subj), &cx) {
            return r.allow.eval(Some(&subj), &cx);
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub fn vote(
    p: &VotePass,
    pool: &Pool,
    key: &Key,
    seq: &str,
    names: &[String],
    regexes: &[Regex],
) -> Outcome {
    let (matches, method) = pool.expanded(p, key);
    if matches.is_empty() {
        return Outcome {
            method: "no_match",
            base: None,
            technique: None,
            matches: 0,
            total_in_bin: 0,
        };
    }
    // Count pairs, keeping first-seen order so that a tie resolves the way the
    // reference arrived, which is what v0 does, and what v1 will not.
    let mut order: Vec<(u32, u32)> = Vec::new();
    let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
    for m in &matches {
        let k = (m.base, m.technique);
        if counts.insert(k, counts.get(&k).copied().unwrap_or(0) + 1).is_none() {
            order.push(k);
        }
    }
    let mut ranked: Vec<((u32, u32), usize)> = order.iter().map(|k| (*k, counts[k])).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));

    for ((b, t), count) in ranked {
        if !compatible(&p.compat, seq, &names[t as usize], regexes) {
            continue;
        }
        if count < p.min_matches {
            return Outcome {
                method: "insufficient_matches",
                base: None,
                technique: None,
                matches: count,
                total_in_bin: matches.len(),
            };
        }
        return Outcome {
            method,
            base: Some(b),
            technique: Some(t),
            matches: count,
            total_in_bin: matches.len(),
        };
    }
    Outcome {
        method: "no_compatible_match",
        base: None,
        technique: None,
        matches: 0,
        total_in_bin: matches.len(),
    }
}
