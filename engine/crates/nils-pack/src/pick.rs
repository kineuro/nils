// SPDX-License-Identifier: AGPL-3.0-only

//! Choosing one stack per session and role
//! (`docs/specs/wave3-anonymize-and-bids.md`, §10).
//!
//! The number that makes this mandatory: **82.5 percent of the archive's
//! sessions that hold a T1w hold more than one, and the worst holds 462.**
//!
//! This already exists in v0 as tuned data. `qc/cohort_main/main_qc_weights.yaml`
//! says in its own header "edit this file to tune the auto-pick algorithm, no
//! code change required", and carries eight component weights, a provenance
//! penalty, per-role technique tiers, canonical-construct preferences and the
//! thresholds that raise a needs-check. It is carried rather than reinvented,
//! and what changes is where it lives and what it leaves behind.
//!
//! Two things about the shape.
//!
//! The engine provides **kinds** and the pack provides the numbers, which is
//! the pass layer's arrangement (§7 of Wave 2) for the same reason: an
//! algorithm in a pack is a program nobody can review, and a number in the
//! engine is knowledge nobody can edit. A pack that wants a component gone
//! gives it no weight; one that wants them in another order writes them in
//! another order; neither needs a release.
//!
//! Three of the eight components read the **population** rather than the stack:
//! how common this technique is, how the cohort splits between 2D and 3D, and
//! where this slice count falls among the rest. Those are real priors, and
//! v0 is right to use them. What v0 does not do is say which population it
//! read, so the same stack scored against two cohorts gets two answers and
//! neither row says so. Here the population is a named **reference** carried on
//! the answer, which is what Wave 2 §7.4 settled for the vote.

use std::collections::BTreeMap;

/// How a component turns one candidate into a number in `[0, 1]`.
#[derive(Debug, Clone)]
pub enum Kind {
    /// A score per value of an axis, with an optional penalty proportional to
    /// how much of the population holds another named value.
    ///
    /// v0's dimension component: 3D scores 1.0 outright, and 2D scores 0.85
    /// less 0.30 times the share of the cohort that is 3D, so 2D scores well
    /// in an all-2D cohort and poorly beside 3D.
    Choice {
        of: String,
        scores: BTreeMap<String, f64>,
        missing: f64,
        /// The value whose share is subtracted, and by how much.
        crowded_by: Option<(String, f64)>,
    },
    /// A score per value of an axis, chosen from a table **per role**, plus a
    /// bonus for each named token of another axis that is present.
    ///
    /// v0's technique component: a T1w's MPRAGE is 1.00 and its TSE is 0.40,
    /// and the tables differ per role because a FLAIR's TSE is 0.90.
    Tier {
        of: String,
        per_role: BTreeMap<String, BTreeMap<String, f64>>,
        missing: f64,
        bonuses: Vec<Bonus>,
    },
    /// A base, plus a delta for each named token of a multi-valued axis that
    /// is present, chosen per role.
    Tokens {
        of: String,
        base: f64,
        per_role: BTreeMap<String, BTreeMap<String, f64>>,
    },
    /// Where a field falls among the population's values of it.
    ///
    /// Bucketed rather than interpolated, which is v0's shape: below the fifth
    /// percentile is 0.20 and above the ninety-fifth is 1.0, with three steps
    /// between. A field with no value scores `missing`, and a population too
    /// small to bucket scores `unknown`, which are different things.
    Percentile {
        of: String,
        /// The name of the population, which may be split by an axis value.
        population: String,
        split_by: Option<String>,
        missing: f64,
        unknown: f64,
    },
    /// How common this axis's value is in the population, scaled so that a
    /// share at or above `tops_out` is full marks, then mixed with a floor.
    ///
    /// v0 mixes 0.7 of the share with 0.3 of a 0.5 floor, and for a Dixon or
    /// water-excitation bundle 0.4 of the share with 0.6 of a 0.85 floor: a
    /// deliberate statement that those are good acquisitions even where they
    /// are rare.
    Share {
        of: String,
        tops_out: f64,
        share_coef: f64,
        floor_coef: f64,
        floor_value: f64,
        /// The same three numbers, when one of the bonus tokens is present.
        when_bonus: Option<(f64, f64, f64)>,
    },
    /// A score for a field having a value at all, and another for not.
    Present {
        of: Vec<String>,
        base: f64,
        each: f64,
    },
}

/// A bonus on a tier, and the token of an axis that earns it.
#[derive(Debug, Clone)]
pub struct Bonus {
    pub of: String,
    pub token: String,
    pub amount: f64,
    /// When set, the bonus is earned only if the candidate also holds one of
    /// these values on `needs_axis`. v0 gives the Dixon bonus only where a
    /// canonical construct exists, because a Dixon family with neither an
    /// in-phase nor a water image is not a usable T1w.
    pub needs: Option<String>,
    pub needs_any: Vec<String>,
}

/// One weighted component of a pick's score.
#[derive(Debug, Clone)]
pub struct Component {
    pub name: String,
    pub weight: f64,
    pub kind: Kind,
}

/// A multiplier applied after every component, by the value of an axis.
///
/// v0's is the EPIMix penalty, 0.5, whose comment says it "is allowed as a
/// fallback but never preferred when RawRecon is available". A multiplier
/// rather than a component because that is what "never preferred" means: no
/// amount of slices buys it back.
#[derive(Debug, Clone)]
pub struct Penalty {
    pub of: String,
    pub by_value: BTreeMap<String, f64>,
}

/// When a pick is not to be trusted on its own.
///
/// v0 raised nine reasons (`qc/cohort_main/service.py`, `_pick_for_session`);
/// record 51 R6 carries every one of them, each defined as v0 computed it and
/// with v0's number. A reason the pack does not declare is never raised, so a
/// pack or an overlay written before keeps its borders.
#[derive(Debug, Clone, Default)]
pub struct Borders {
    /// The runner-up is within this fraction of the winner. `within`
    /// includes the fraction itself: a margin of exactly this is too close.
    pub runner_up_within: f64,
    /// The winning value of this axis is held by a share of the population
    /// strictly below this, so the pick is right by the numbers and odd by
    /// the protocol. A share of exactly this is not rare.
    pub rare_within: Option<(String, f64)>,
    /// v0's `retake`: the winner holds more than one stack of what should be
    /// one image.
    pub retake: Option<Retake>,
    /// v0's `unknown_dim`: the winner has no value on this name.
    pub unknown_dim: Option<String>,
    /// v0's `slice_count_outlier`.
    pub slice_outlier: Option<SliceOutlier>,
    /// v0's `pre_post_twin`.
    pub pre_post_twin: Option<Twin>,
    /// v0's `epimix_fallback`: a stack of the winner holds one of these
    /// values of this name, `(of, is)`. Record 53 (pack contract 8): several
    /// values, so a NeuroMix winner raises it as an EPIMix one does.
    pub fallback: Option<(String, Vec<String>)>,
    /// v0's `dixon_vs_plain`.
    pub dixon_vs_plain: Option<Plain>,
}

/// v0's retake, and its partial-volume demotion read first.
///
/// A candidate outside a family is a retake when it holds more than one
/// stack, counting only the stacks at or above `partial_below` of its largest
/// slice count when that count is at least `partial_min_slices`: v0 demotes a
/// short stack of one acquisition as a partial-volume helper or an aborted
/// scan, "170 vs 40 is caught; 192 vs 168 is not". A candidate a family made
/// is a retake when it keeps more stacks than the family's `retake_above`
/// (v0: more than one of a Dixon's canonical construct, more than two of an
/// MP2RAGE's).
#[derive(Debug, Clone)]
pub struct Retake {
    /// The slice count, a field of the fingerprint.
    pub of: String,
    pub partial_below: f64,
    pub partial_min_slices: f64,
}

/// v0's slice-count outlier: the winner's largest slice count is strictly
/// below the `below` quantile or strictly above the `above` quantile of the
/// named percentile population, in the winner's own bucket of it.
#[derive(Debug, Clone)]
pub struct SliceOutlier {
    /// A population a `percentile` component of the pick builds.
    pub population: String,
    pub below: f64,
    pub above: f64,
}

/// v0's pre and post twin: another candidate scores at least `at_least` of
/// the winner, and its values of `of` share none with the winner's. v0 also
/// made the twin a main; here it is a border only, and a person picks.
#[derive(Debug, Clone)]
pub struct Twin {
    pub of: String,
    pub at_least: f64,
}

/// v0's Dixon against plain: the winner is a candidate of the named family,
/// and another candidate holding none of `plain_without` on `of` is within
/// `within` of its score, the number itself included.
#[derive(Debug, Clone)]
pub struct Plain {
    pub family: String,
    pub of: String,
    pub plain_without: Vec<String>,
    pub within: f64,
}

/// Everything a pick needs, as the pack declares it.
#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    /// The roles it picks for. A role with no entry here is never picked.
    pub roles: Vec<String>,
    pub components: Vec<Component>,
    pub penalty: Option<Penalty>,
    pub borders: Borders,
    /// The names whose values identify one acquisition, so that two stacks of
    /// one acquisition are one candidate. v0's stage-1 bundle key.
    pub same_acquisition: Vec<String>,
    /// And how the outputs of one acquisition are merged back together, a
    /// family per kind of acquisition that writes several images (record 51:
    /// a Dixon, and an MP2RAGE). A stack belongs to the first whose token it
    /// holds.
    pub families: Vec<Family>,
}

/// One acquisition that produced several images, merged back into one
/// candidate (v0's stage 2).
///
/// A Dixon produces an in-phase, an out-of-phase, a water and a fat image, and
/// without this they compete with each other for the session, four ways.
#[derive(Debug, Clone)]
pub struct Family {
    /// What the pack calls it, which a border and a pick's evidence name.
    pub name: String,
    /// Held when the candidate carries this token, and otherwise not merged.
    pub when: (String, String),
    /// The name whose values are the variants, dropped from the key.
    pub over: String,
    /// And what else is dropped, because the outputs of one acquisition may
    /// differ slightly in it. v0 drops the timing for exactly this reason.
    pub ignoring: Vec<String>,
    /// Which variants are worth keeping, best first. A family holding none of
    /// them is not a candidate at all: v0 drops it, and its comment says why,
    /// which is that a Dixon with neither an in-phase nor a water image is not
    /// a T1w anybody would measure on.
    pub canonical: Vec<String>,
    /// What happens to a family holding none of `canonical`: dropped, which
    /// is v0's Dixon, or left apart as the acquisitions it was before the
    /// merge, which is v0's MP2RAGE ("tag all", for a cohort whose MP2RAGE
    /// carries no labelled output).
    pub apart_without_canonical: bool,
    /// A candidate of this family keeping more stacks than this is a retake
    /// (v0: 1 for a Dixon's canonical construct, 2 for an MP2RAGE's).
    pub retake_above: usize,
}

impl Model {
    /// Every name this model reads, so that a caller knows what to fetch.
    ///
    /// Collected from the model rather than listed beside it, because a list
    /// beside it is a list that goes stale, and a component whose value was
    /// never fetched reads as nothing and says so about the candidate.
    pub fn reads(&self) -> Vec<String> {
        let mut out = self.same_acquisition.clone();
        for c in &self.components {
            match &c.kind {
                Kind::Choice { of, crowded_by, .. } => {
                    out.push(of.clone());
                    let _ = crowded_by;
                }
                Kind::Tier { of, bonuses, .. } => {
                    out.push(of.clone());
                    for b in bonuses {
                        out.push(b.of.clone());
                        if let Some(n) = &b.needs {
                            out.push(n.clone());
                        }
                    }
                }
                Kind::Tokens { of, .. } | Kind::Share { of, .. } => out.push(of.clone()),
                Kind::Percentile { of, split_by, .. } => {
                    out.push(of.clone());
                    if let Some(s) = split_by {
                        out.push(s.clone());
                    }
                }
                Kind::Present { of, .. } => out.extend(of.iter().cloned()),
            }
        }
        if let Some(p) = &self.penalty {
            out.push(p.of.clone());
        }
        if let Some((of, _)) = &self.borders.rare_within {
            out.push(of.clone());
        }
        let b = &self.borders;
        out.extend(b.retake.iter().map(|r| r.of.clone()));
        out.extend(b.unknown_dim.iter().cloned());
        out.extend(b.pre_post_twin.iter().map(|t| t.of.clone()));
        out.extend(b.fallback.iter().map(|(of, _)| of.clone()));
        out.extend(b.dixon_vs_plain.iter().map(|p| p.of.clone()));
        for f in &self.families {
            out.push(f.when.0.clone());
            out.push(f.over.clone());
            out.extend(f.ignoring.iter().cloned());
        }
        out.sort();
        out.dedup();
        out
    }

    /// The populations the percentile components read, and what each is of.
    pub fn populations(&self) -> Vec<(String, String, Option<String>)> {
        self.components
            .iter()
            .filter_map(|c| match &c.kind {
                Kind::Percentile {
                    of,
                    population,
                    split_by,
                    ..
                } => Some((population.clone(), of.clone(), split_by.clone())),
                _ => None,
            })
            .collect()
    }
}

/// What a population says about itself, for the components that read one.
///
/// Built by the caller from whatever it decided the population is, and named
/// on the answer. v0 computes the same numbers and records none of them, so a
/// pick cannot be reproduced from what is stored.
#[derive(Debug, Clone, Default)]
pub struct Reference {
    /// What it is, for the row: `cohort:ms-2026`, `selection:42`.
    pub name: String,
    /// Per axis name, how many candidates held each value.
    pub counts: BTreeMap<String, BTreeMap<String, i64>>,
    /// Per population name, the five percentiles.
    pub percentiles: BTreeMap<String, Percentiles>,
    pub total: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percentiles {
    pub p5: f64,
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
    pub p95: f64,
}

impl Percentiles {
    /// The quantiles a population keeps, which are the only ones a border may
    /// name.
    pub const KEPT: [f64; 5] = [0.05, 0.25, 0.50, 0.75, 0.95];

    /// One of the five, by its fraction.
    pub fn at(&self, q: f64) -> Option<f64> {
        let near = |x: f64| (q - x).abs() < 1e-9;
        if near(0.05) {
            Some(self.p5)
        } else if near(0.25) {
            Some(self.p25)
        } else if near(0.50) {
            Some(self.p50)
        } else if near(0.75) {
            Some(self.p75)
        } else if near(0.95) {
            Some(self.p95)
        } else {
            None
        }
    }
}

impl Reference {
    /// The share of the population holding `value` on `axis`.
    pub fn share(&self, axis: &str, value: &str) -> f64 {
        let Some(counts) = self.counts.get(axis) else {
            return 0.0;
        };
        let total: i64 = counts.values().sum();
        if total <= 0 {
            return 0.0;
        }
        counts.get(value).copied().unwrap_or(0) as f64 / total as f64
    }

    /// Five percentiles of a population, or none when too few to bucket.
    ///
    /// Three is v0's floor and is kept: two values have no middle, and a
    /// bucket drawn from two numbers says more about the two than about the
    /// population.
    pub fn of(values: &[f64]) -> Option<Percentiles> {
        let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
        if v.len() < 3 {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let at = |q: f64| -> f64 {
            let i = ((q * (v.len() - 1) as f64).round() as usize).min(v.len() - 1);
            v[i]
        };
        Some(Percentiles {
            p5: at(0.05),
            p25: at(0.25),
            p50: at(0.50),
            p75: at(0.75),
            p95: at(0.95),
        })
    }
}

/// One candidate: the stacks of one acquisition, judged together.
#[derive(Debug, Clone, Default)]
pub struct Candidate {
    /// The stack ids, which is what a pick names.
    pub stacks: Vec<i64>,
    /// Everything a component may read, by the name the pack writes: an axis
    /// as the group agrees on it, a fingerprint field taken at its largest
    /// across the group. One map, because a component should not have to know
    /// which of the two it is naming, and because a bundle's slice count is
    /// the fullest volume in it and not an arbitrary one.
    pub values: BTreeMap<String, String>,
    /// Each stack's own values, in the order of `stacks`, for the borders
    /// that read the stacks one by one (a retake, a fallback). Empty where
    /// the caller did not say, and then `values` stands for every stack.
    pub each: Vec<BTreeMap<String, String>>,
    /// The family whose outputs were merged into this candidate, by name.
    pub family: Option<String>,
}

impl Candidate {
    pub fn get(&self, name: &str) -> &str {
        self.values.get(name).map(String::as_str).unwrap_or("")
    }

    fn num(&self, name: &str) -> Option<f64> {
        self.values.get(name)?.trim().parse().ok()
    }

    fn holds(&self, name: &str, token: &str) -> bool {
        holds(self.get(name), token)
    }

    /// Each stack's values, or the candidate's own where the caller gave
    /// none per stack.
    fn stacks_values(&self) -> Vec<&BTreeMap<String, String>> {
        if self.each.is_empty() {
            vec![&self.values; self.stacks.len().max(1)]
        } else {
            self.each.iter().collect()
        }
    }

    /// The values of a multi-valued name as a set of tokens, where nothing is
    /// a value of its own: v0 compared `post_contrast` as sets that could
    /// hold `None`.
    fn tokens(&self, name: &str) -> std::collections::BTreeSet<String> {
        let v = self.get(name);
        if v.trim().is_empty() {
            return [String::new()].into_iter().collect();
        }
        v.split(',')
            .map(|t| t.trim().to_ascii_lowercase())
            .collect()
    }
}

fn holds(csv: &str, token: &str) -> bool {
    csv.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// What one component said, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub score: f64,
    pub weight: f64,
    /// What the component read to get there, for the row.
    pub saw: String,
}

/// A candidate's score, with every part of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub score: f64,
    pub parts: Vec<Part>,
    pub penalty: f64,
}

/// Score one candidate for one role.
pub fn score(model: &Model, role: &str, c: &Candidate, reference: &Reference) -> Scored {
    let mut parts = Vec::with_capacity(model.components.len());
    let mut total = 0.0;

    for comp in &model.components {
        let (s, saw) = match &comp.kind {
            Kind::Choice {
                of,
                scores,
                missing,
                crowded_by,
            } => {
                let v = c.get(of);
                if v.is_empty() {
                    (*missing, "nothing".to_string())
                } else {
                    let base = scores.get(v).copied().unwrap_or(*missing);
                    match crowded_by {
                        Some((other, by)) if other != v => {
                            let share = reference.share(of, other);
                            (
                                (base - by * share).clamp(0.0, 1.0),
                                format!("{v} against {:.0}% {other}", share * 100.0),
                            )
                        }
                        _ => (base, v.to_string()),
                    }
                }
            }
            Kind::Tier {
                of,
                per_role,
                missing,
                bonuses,
            } => {
                let table = per_role.get(role);
                let v = c.get(of);
                let mut s = table.and_then(|t| t.get(v).copied()).unwrap_or_else(|| {
                    table
                        .and_then(|t| t.get("Unknown").copied())
                        .unwrap_or(*missing)
                });
                let mut said = if v.is_empty() { "nothing" } else { v }.to_string();
                for b in bonuses {
                    if !c.holds(&b.of, &b.token) {
                        continue;
                    }
                    if let Some(needs) = &b.needs
                        && !b.needs_any.iter().any(|w| c.holds(needs, w))
                    {
                        continue;
                    }
                    s = (s + b.amount).min(1.0);
                    said.push_str(&format!(" +{}", b.token));
                }
                (s, said)
            }
            Kind::Tokens { of, base, per_role } => {
                let mut s = *base;
                let mut said = Vec::new();
                if let Some(table) = per_role.get(role) {
                    for (token, delta) in table {
                        if c.holds(of, token) {
                            s += *delta;
                            said.push(format!("{token}{delta:+}"));
                        }
                    }
                }
                (
                    s.clamp(0.0, 1.0),
                    if said.is_empty() {
                        "nothing".to_string()
                    } else {
                        said.join(" ")
                    },
                )
            }
            Kind::Percentile {
                of,
                population,
                split_by,
                missing,
                unknown,
            } => {
                let key = match split_by {
                    Some(a) => format!("{population}:{}", c.get(a)),
                    None => population.clone(),
                };
                match (c.num(of), reference.percentiles.get(&key)) {
                    (None, _) => (*missing, "nothing".to_string()),
                    (Some(_), None) => (*unknown, format!("{key}, too few to bucket")),
                    (Some(v), Some(p)) => {
                        let s = if v <= p.p5 {
                            0.20
                        } else if v <= p.p25 {
                            0.40
                        } else if v <= p.p50 {
                            0.60
                        } else if v <= p.p75 {
                            0.80
                        } else {
                            1.0
                        };
                        (s, format!("{v:.0} in {key}"))
                    }
                }
            }
            Kind::Share {
                of,
                tops_out,
                share_coef,
                floor_coef,
                floor_value,
                when_bonus,
            } => {
                let v = c.get(of);
                let share = reference.share(of, v);
                // The bonus coefficients apply when any tier bonus token is
                // held, which is what v0 keys its `dixon_or_waterexc` row on.
                let bonus = model.components.iter().any(|k| match &k.kind {
                    Kind::Tier { bonuses, .. } => bonuses.iter().any(|b| c.holds(&b.of, &b.token)),
                    _ => false,
                });
                let (sc, fc, fv) = match (bonus, when_bonus) {
                    (true, Some((sc, fc, fv))) => (*sc, *fc, *fv),
                    _ => (*share_coef, *floor_coef, *floor_value),
                };
                let s = (sc * (share / tops_out).min(1.0) + fc * fv).clamp(0.0, 1.0);
                (s, format!("{v} is {:.0}% of them", share * 100.0))
            }
            Kind::Present { of, base, each } => {
                let held = of.iter().filter(|n| !c.get(n).is_empty()).count();
                (
                    (base + each * held as f64).clamp(0.0, 1.0),
                    format!("{held} of {}", of.len()),
                )
            }
        };
        total += comp.weight * s;
        parts.push(Part {
            name: comp.name.clone(),
            score: s,
            weight: comp.weight,
            saw,
        });
    }

    let penalty = match &model.penalty {
        Some(p) => p.by_value.get(c.get(&p.of)).copied().unwrap_or(1.0),
        None => 1.0,
    };
    Scored {
        score: (total * penalty).clamp(0.0, 1.0),
        parts,
        penalty,
    }
}

/// Why a pick is worth a person's eye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Border {
    /// Two candidates are close enough that the order between them is noise.
    /// v0 calls this a border and so does this; what differs is that the
    /// answer says so rather than the first row quietly winning.
    TooClose,
    /// The winner is the best of what is here and unlike what the rest of the
    /// population did, which usually means the protocol changed or the session
    /// is missing its real one.
    Rare,
    /// Nothing was eligible.
    Nothing,
    /// The winner holds more than one stack of what should be one image:
    /// the acquisition was run twice, or a family kept more of its canonical
    /// output than it makes. v0's `retake`, `retake_dixon_canonical` and
    /// `retake_mp2rage`, one reason with the variant in the evidence.
    Retake,
    /// The winner's dimension is not known, so the dimension component
    /// scored it on a guess.
    UnknownDim,
    /// The winner's slice count is outside the 5th to 95th percentile of its
    /// dimension's population.
    SliceOutlier,
    /// A candidate nearly as good differs from the winner in whether
    /// contrast was given, so which of the two the session means is a
    /// person's call.
    PrePostTwin,
    /// The winner is a fallback the pack penalises (v0: an EPIMix), which
    /// wins only where nothing better was taken.
    EpimixFallback,
    /// The winner is a Dixon and a plain acquisition is close behind it.
    DixonVsPlain,
}

impl Border {
    /// Every border, in the order a pick reports them.
    pub const ALL: [Border; 9] = [
        Border::TooClose,
        Border::Rare,
        Border::Nothing,
        Border::Retake,
        Border::UnknownDim,
        Border::SliceOutlier,
        Border::PrePostTwin,
        Border::EpimixFallback,
        Border::DixonVsPlain,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Border::TooClose => "too_close",
            Border::Rare => "rare",
            Border::Nothing => "nothing_eligible",
            Border::Retake => "retake",
            Border::UnknownDim => "unknown_dim",
            Border::SliceOutlier => "slice_count_outlier",
            Border::PrePostTwin => "pre_post_twin",
            Border::EpimixFallback => "epimix_fallback",
            Border::DixonVsPlain => "dixon_vs_plain",
        }
    }
}

/// The chosen candidate, the one behind it, and whether to trust the order.
#[derive(Debug, Clone)]
pub struct Picked {
    pub role: String,
    pub winner: Option<Candidate>,
    pub scored: Option<Scored>,
    pub runner_up: Option<Candidate>,
    pub runner_up_score: f64,
    /// How much of the winner's score separates them, as a fraction.
    pub margin: f64,
    pub borders: Vec<Border>,
    /// What a border found, by the border's name, for the evidence: the
    /// variant of a retake, the twin's stacks, the plain candidate's stacks,
    /// the slice count and the bounds it fell outside.
    pub notes: BTreeMap<&'static str, String>,
    /// Every candidate's score, for the row: what the alternatives were.
    pub considered: Vec<(Vec<i64>, f64)>,
}

/// Choose one candidate for one role.
///
/// Candidates are ordered by score, and a tie is **reported** rather than
/// broken by row order: v0 sorts and takes the first, so a session whose two
/// best differ by nothing gets whichever the database returned, and re-running
/// the same cohort can return the other.
pub fn pick(model: &Model, role: &str, candidates: &[Candidate], reference: &Reference) -> Picked {
    let mut scored: Vec<(usize, Scored)> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (i, score(model, role, c, reference)))
        .collect();
    // Highest first, and on an exact tie the lower stack id, so that the order
    // is fixed even before the border below reports it.
    scored.sort_by(|(ia, a), (ib, b)| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| candidates[*ia].stacks.cmp(&candidates[*ib].stacks))
    });

    let considered = scored
        .iter()
        .map(|(i, s)| (candidates[*i].stacks.clone(), s.score))
        .collect();
    let Some((first, best)) = scored.first().cloned() else {
        return Picked {
            role: role.to_string(),
            winner: None,
            scored: None,
            runner_up: None,
            runner_up_score: 0.0,
            margin: 0.0,
            borders: vec![Border::Nothing],
            notes: BTreeMap::new(),
            considered,
        };
    };

    let second = scored.get(1).cloned();
    let runner_up_score = second.as_ref().map(|(_, s)| s.score).unwrap_or(0.0);
    let margin = if best.score > 0.0 {
        (best.score - runner_up_score) / best.score
    } else {
        0.0
    };

    let mut borders = Vec::new();
    // `within` takes the number itself and `below` does not, as each is
    // written: a margin of exactly the fraction is too close, and a share of
    // exactly the floor is not rare.
    if second.is_some()
        && (margin <= model.borders.runner_up_within
            || crate::pack::at_threshold(margin, model.borders.runner_up_within))
    {
        borders.push(Border::TooClose);
    }
    if let Some((of, floor)) = &model.borders.rare_within {
        let share = reference.share(of, candidates[first].get(of));
        if reference.total > 0 && crate::pack::weaker_than(share, *floor) {
            borders.push(Border::Rare);
        }
    }

    let mut notes = BTreeMap::new();
    let winner = &candidates[first];
    let others: Vec<(&Candidate, f64)> = scored[1..]
        .iter()
        .map(|(i, s)| (&candidates[*i], s.score))
        .collect();
    more_borders(
        model,
        winner,
        best.score,
        &others,
        reference,
        &mut borders,
        &mut notes,
    );

    Picked {
        role: role.to_string(),
        winner: Some(winner.clone()),
        scored: Some(best),
        runner_up: second.map(|(i, _)| candidates[i].clone()),
        runner_up_score,
        margin,
        borders,
        notes,
        considered,
    }
}

fn stacks_text(stacks: &[i64]) -> String {
    stacks
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Record 51 R6: v0's six other reasons, each as v0 computed it in
/// `_pick_for_session` at 5ee391f, in v0's order, and each only where the
/// pack declares it. `others` are the other candidates, best first.
fn more_borders(
    model: &Model,
    winner: &Candidate,
    score: f64,
    others: &[(&Candidate, f64)],
    reference: &Reference,
    borders: &mut Vec<Border>,
    notes: &mut BTreeMap<&'static str, String>,
) {
    let b = &model.borders;
    // A number the fingerprint wrote, parsed.
    let num = |v: &BTreeMap<String, String>, name: &str| -> Option<f64> {
        v.get(name)?.trim().parse::<f64>().ok()
    };

    // Retake. A family's candidate is judged on what the family kept; any
    // other on its stacks, less the short ones v0 demotes first.
    if let Some(r) = &b.retake {
        let family = winner
            .family
            .as_ref()
            .and_then(|n| model.families.iter().find(|f| &f.name == n));
        match family {
            Some(f) => {
                if winner.stacks.len() > f.retake_above {
                    borders.push(Border::Retake);
                    notes.insert(
                        "retake",
                        format!(
                            "{}: {} stacks of its canonical output, more than {}",
                            f.name,
                            winner.stacks.len(),
                            f.retake_above
                        ),
                    );
                }
            }
            None => {
                let slices: Vec<f64> = winner
                    .stacks_values()
                    .iter()
                    .map(|v| num(v, &r.of).unwrap_or(0.0))
                    .collect();
                let largest = slices.iter().copied().fold(0.0, f64::max);
                let (kept, short) = if slices.len() >= 2
                    && (largest >= r.partial_min_slices
                        || crate::pack::at_threshold(largest, r.partial_min_slices))
                {
                    let cutoff = r.partial_below * largest;
                    let kept = slices
                        .iter()
                        .filter(|n| **n >= cutoff || crate::pack::at_threshold(**n, cutoff))
                        .count();
                    (kept, slices.len() - kept)
                } else {
                    (slices.len(), 0)
                };
                if kept > 1 {
                    borders.push(Border::Retake);
                    notes.insert(
                        "retake",
                        if short > 0 {
                            format!(
                                "plain: {kept} full stacks of one acquisition, {short} short one(s) set aside"
                            )
                        } else {
                            format!("plain: {kept} stacks of one acquisition")
                        },
                    );
                }
            }
        }
    }

    // The dimension, unknown.
    if let Some(of) = &b.unknown_dim
        && winner.get(of).trim().is_empty()
    {
        borders.push(Border::UnknownDim);
    }

    // The slice count, outside its population. v0 compares the largest
    // slice count strictly with the fifth and the ninety-fifth percentile of
    // the winner's dimension bucket, and says nothing where the population
    // had too few to bucket.
    if let Some(o) = &b.slice_outlier
        && let Some((of, key)) = model.components.iter().find_map(|c| match &c.kind {
            Kind::Percentile {
                of,
                population,
                split_by,
                ..
            } if *population == o.population => Some((
                of.clone(),
                match split_by {
                    Some(a) => format!("{population}:{}", winner.get(a)),
                    None => population.clone(),
                },
            )),
            _ => None,
        })
        && let Some(p) = reference.percentiles.get(&key)
        && let Some(n) = winner.num(&of)
        && n > 0.0
        && let (Some(lo), Some(hi)) = (p.at(o.below), p.at(o.above))
        && (n < lo || n > hi)
    {
        borders.push(Border::SliceOutlier);
        notes.insert(
            "slice_count_outlier",
            format!("{n:.0} outside {lo:.0} to {hi:.0} in {key}"),
        );
    }

    // A twin across contrast: the first candidate, best first, still at or
    // above the fraction of the winner whose contrast shares nothing with
    // the winner's. v0 stops looking at the first one below it.
    if let Some(t) = &b.pre_post_twin {
        let mine = winner.tokens(&t.of);
        for (c, s) in others {
            let floor = t.at_least * score;
            if *s < floor && !crate::pack::at_threshold(*s, floor) {
                break;
            }
            if c.tokens(&t.of).is_disjoint(&mine) {
                borders.push(Border::PrePostTwin);
                notes.insert("pre_post_twin", stacks_text(&c.stacks));
                break;
            }
        }
    }

    // A fallback won: any stack of the winner holds one of its values.
    if let Some((of, is)) = &b.fallback {
        let held: std::collections::BTreeSet<&str> = winner
            .stacks_values()
            .iter()
            .filter_map(|v| v.get(of))
            .flat_map(|x| is.iter().filter(|i| holds(x, i)).map(String::as_str))
            .collect();
        if !held.is_empty() {
            borders.push(Border::EpimixFallback);
            notes.insert(
                "epimix_fallback",
                held.into_iter().collect::<Vec<_>>().join(", "),
            );
        }
    }

    // A Dixon won and a plain acquisition is close behind: within the
    // fraction of the winner's score, the fraction itself included.
    if let Some(p) = &b.dixon_vs_plain
        && winner.family.as_deref() == Some(p.family.as_str())
        && score > 0.0
    {
        for (c, s) in others {
            if p.plain_without.iter().any(|t| c.holds(&p.of, t)) {
                continue;
            }
            let gap = (score - s) / score;
            if gap <= p.within || crate::pack::at_threshold(gap, p.within) {
                borders.push(Border::DixonVsPlain);
                notes.insert("dixon_vs_plain", stacks_text(&c.stacks));
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        let table = |pairs: &[(&str, f64)]| -> BTreeMap<String, f64> {
            pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
        };
        Model {
            name: "main".into(),
            roles: vec!["t1w".into()],
            components: vec![
                Component {
                    name: "dim".into(),
                    weight: 0.5,
                    kind: Kind::Choice {
                        of: "dim".into(),
                        scores: table(&[("3D", 1.0), ("2D", 0.85)]),
                        missing: 0.4,
                        crowded_by: Some(("3D".to_string(), 0.30)),
                    },
                },
                Component {
                    name: "tech".into(),
                    weight: 0.5,
                    kind: Kind::Tier {
                        of: "technique".into(),
                        per_role: [("t1w".to_string(), table(&[("MPRAGE", 1.0), ("TSE", 0.4)]))]
                            .into_iter()
                            .collect(),
                        missing: 0.3,
                        bonuses: vec![Bonus {
                            of: "modifier".into(),
                            token: "Dixon".into(),
                            amount: 0.1,
                            needs: Some("construct".into()),
                            needs_any: vec!["InPhase".into(), "Water".into()],
                        }],
                    },
                },
            ],
            penalty: Some(Penalty {
                of: "provenance".into(),
                by_value: table(&[("EPIMix", 0.5)]),
            }),
            borders: Borders {
                runner_up_within: 0.05,
                rare_within: Some(("technique".into(), 0.10)),
                ..Borders::default()
            },
            same_acquisition: vec!["technique".into()],
            families: Vec::new(),
        }
    }

    fn candidate(stacks: &[i64], pairs: &[(&str, &str)]) -> Candidate {
        Candidate {
            stacks: stacks.to_vec(),
            values: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            ..Candidate::default()
        }
    }

    fn reference(techniques: &[(&str, i64)], dims: &[(&str, i64)]) -> Reference {
        let count = |pairs: &[(&str, i64)]| -> BTreeMap<String, i64> {
            pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
        };
        Reference {
            name: "test".into(),
            counts: [
                ("technique".to_string(), count(techniques)),
                ("dim".to_string(), count(dims)),
            ]
            .into_iter()
            .collect(),
            percentiles: BTreeMap::new(),
            total: techniques.iter().map(|(_, n)| n).sum(),
        }
    }

    #[test]
    fn the_better_technique_wins() {
        let m = model();
        let a = candidate(&[1], &[("technique", "MPRAGE"), ("dim", "3D")]);
        let b = candidate(&[2], &[("technique", "TSE"), ("dim", "3D")]);
        let r = reference(&[("MPRAGE", 8), ("TSE", 2)], &[("3D", 10)]);
        let p = pick(&m, "t1w", &[b, a], &r);
        assert_eq!(p.winner.unwrap().stacks, [1]);
        assert_eq!(p.runner_up.unwrap().stacks, [2]);
        assert!(p.margin > 0.05, "{}", p.margin);
        assert!(p.borders.is_empty());
    }

    #[test]
    fn a_tie_is_reported_and_not_broken_by_the_order_they_arrived_in() {
        // v0 sorts and takes the first, so a session whose two best differ by
        // nothing gets whichever the database returned.
        let m = model();
        let a = candidate(&[1], &[("technique", "MPRAGE"), ("dim", "3D")]);
        let b = candidate(&[2], &[("technique", "MPRAGE"), ("dim", "3D")]);
        let r = reference(&[("MPRAGE", 10)], &[("3D", 10)]);
        let forward = pick(&m, "t1w", &[a.clone(), b.clone()], &r);
        let backward = pick(&m, "t1w", &[b, a], &r);
        assert_eq!(
            forward.winner.as_ref().unwrap().stacks,
            backward.winner.as_ref().unwrap().stacks,
            "the same pair picks the same way whichever order it arrives in"
        );
        assert!(forward.borders.contains(&Border::TooClose));
        assert_eq!(forward.margin, 0.0);
    }

    #[test]
    fn a_dimension_is_worth_less_where_the_cohort_is_mostly_the_other_one() {
        // v0's argument: 2D scores well in an all-2D cohort and poorly beside
        // 3D, because what a 2D acquisition means depends on what else was
        // available at the time.
        let m = model();
        let two_d = candidate(&[1], &[("technique", "MPRAGE"), ("dim", "2D")]);
        let all_2d = reference(&[("MPRAGE", 10)], &[("2D", 10)]);
        let mostly_3d = reference(&[("MPRAGE", 10)], &[("3D", 9), ("2D", 1)]);
        let alone = score(&m, "t1w", &two_d, &all_2d);
        let beside = score(&m, "t1w", &two_d, &mostly_3d);
        assert!(alone.score > beside.score, "{alone:?} {beside:?}");
    }

    #[test]
    fn a_bonus_needs_the_construct_that_makes_it_worth_having() {
        // A Dixon family with neither an in-phase nor a water image is not a
        // T1w anybody would measure on, so it earns no bonus for being Dixon.
        let m = model();
        let r = reference(&[("MPRAGE", 10)], &[("3D", 10)]);
        let with = candidate(
            &[1],
            &[
                ("technique", "MPRAGE"),
                ("dim", "3D"),
                ("modifier", "Dixon"),
                ("construct", "Water"),
            ],
        );
        let without = candidate(
            &[2],
            &[
                ("technique", "MPRAGE"),
                ("dim", "3D"),
                ("modifier", "Dixon"),
                ("construct", "Fat"),
            ],
        );
        let a = score(&m, "t1w", &with, &r);
        let b = score(&m, "t1w", &without, &r);
        assert!(a.parts[1].saw.contains("+Dixon"), "{:?}", a.parts[1]);
        assert!(!b.parts[1].saw.contains("+Dixon"), "{:?}", b.parts[1]);
    }

    #[test]
    fn a_penalty_is_not_something_more_slices_can_buy_back() {
        let m = model();
        let r = reference(&[("MPRAGE", 10)], &[("3D", 10)]);
        let ordinary = candidate(&[1], &[("technique", "MPRAGE"), ("dim", "3D")]);
        let mixed = candidate(
            &[2],
            &[
                ("technique", "MPRAGE"),
                ("dim", "3D"),
                ("provenance", "EPIMix"),
            ],
        );
        let a = score(&m, "t1w", &ordinary, &r);
        let b = score(&m, "t1w", &mixed, &r);
        assert_eq!(b.penalty, 0.5);
        assert!((b.score - a.score * 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_technique_almost_nobody_used_is_worth_a_look() {
        let m = model();
        let odd = candidate(&[1], &[("technique", "FIESTA"), ("dim", "3D")]);
        let r = reference(&[("MPRAGE", 99), ("FIESTA", 1)], &[("3D", 100)]);
        let p = pick(&m, "t1w", &[odd], &r);
        assert!(p.borders.contains(&Border::Rare));
    }

    #[test]
    fn a_role_with_nothing_eligible_says_so() {
        let m = model();
        let r = reference(&[], &[]);
        let p = pick(&m, "t1w", &[], &r);
        assert!(p.winner.is_none());
        assert_eq!(p.borders, [Border::Nothing]);
    }

    #[test]
    fn a_population_too_small_to_bucket_is_not_the_same_as_a_missing_value() {
        let m = Model {
            components: vec![Component {
                name: "slices".into(),
                weight: 1.0,
                kind: Kind::Percentile {
                    of: "n_instances".into(),
                    population: "slices".into(),
                    split_by: None,
                    missing: 0.40,
                    unknown: 0.60,
                },
            }],
            ..model()
        };
        let r = Reference::default();
        let has = candidate(&[1], &[("n_instances", "176")]);
        let has_not = candidate(&[2], &[]);
        assert_eq!(score(&m, "t1w", &has, &r).parts[0].score, 0.60);
        assert_eq!(score(&m, "t1w", &has_not, &r).parts[0].score, 0.40);
    }

    #[test]
    fn percentiles_need_three_values_to_mean_anything() {
        assert!(Reference::of(&[1.0, 2.0]).is_none());
        let p = Reference::of(&[10.0, 20.0, 30.0, 40.0, 50.0]).unwrap();
        assert_eq!(p.p5, 10.0);
        assert_eq!(p.p50, 30.0);
        assert_eq!(p.p95, 50.0);
    }

    // ------------------------------------------------------------ record 51
    //
    // The boundaries of v0's six other reasons, on a model whose scores are
    // chosen: one `choice` component over `q`, so a candidate scores what its
    // `q` is worth and a fraction of the winner is exact. The cases ported
    // from v0's own tests, on the MRI pack's numbers, are in
    // `tests/borders.rs`.

    fn chosen(borders: Borders, families: Vec<Family>) -> Model {
        Model {
            name: "main".into(),
            roles: vec!["t1w".into()],
            components: vec![
                Component {
                    name: "q".into(),
                    weight: 1.0,
                    kind: Kind::Choice {
                        of: "q".into(),
                        scores: [
                            ("top", 1.0),
                            ("at_85", 0.85),
                            ("under_85", 0.849),
                            ("at_90", 0.90),
                            ("under_90", 0.899),
                        ]
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                        missing: 0.0,
                        crowded_by: None,
                    },
                },
                Component {
                    name: "slices".into(),
                    weight: 0.0,
                    kind: Kind::Percentile {
                        of: "n_instances".into(),
                        population: "slices".into(),
                        split_by: Some("dim".into()),
                        missing: 0.4,
                        unknown: 0.6,
                    },
                },
            ],
            penalty: None,
            borders: Borders {
                // Far from anything below, so that too_close never joins in.
                runner_up_within: 0.0,
                ..borders
            },
            same_acquisition: vec!["q".into()],
            families,
        }
    }

    fn with_each(stacks: &[(i64, &[(&str, &str)])], family: Option<&str>) -> Candidate {
        let map = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect()
        };
        let mut values = BTreeMap::new();
        for (_, pairs) in stacks {
            for (k, v) in map(pairs) {
                values.entry(k).or_insert(v);
            }
        }
        // The slice count at its largest, as the registry side builds it.
        if let Some(n) = stacks
            .iter()
            .filter_map(|(_, p)| p.iter().find(|(k, _)| *k == "n_instances"))
            .filter_map(|(_, v)| v.parse::<f64>().ok())
            .reduce(f64::max)
        {
            values.insert("n_instances".into(), format!("{n}"));
        }
        Candidate {
            stacks: stacks.iter().map(|(s, _)| *s).collect(),
            values,
            each: stacks.iter().map(|(_, p)| map(p)).collect(),
            family: family.map(str::to_string),
        }
    }

    fn retake() -> Borders {
        Borders {
            retake: Some(Retake {
                of: "n_instances".into(),
                partial_below: 0.50,
                partial_min_slices: 60.0,
            }),
            ..Borders::default()
        }
    }

    fn family(name: &str, retake_above: usize) -> Family {
        Family {
            name: name.into(),
            when: ("modifier".into(), name.into()),
            over: "construct".into(),
            ignoring: Vec::new(),
            canonical: vec!["InPhase".into()],
            apart_without_canonical: false,
            retake_above,
        }
    }

    #[test]
    fn a_retake_is_two_full_stacks_of_one_acquisition_and_a_short_one_is_set_aside() {
        let m = chosen(retake(), Vec::new());
        let r = Reference::default();
        let of = |a: &str, b: &str| {
            with_each(
                &[
                    (1, &[("q", "top"), ("n_instances", a)]),
                    (2, &[("q", "top"), ("n_instances", b)]),
                ],
                None,
            )
        };
        // v0's own words: "a 170 vs 40 gap is caught; 192 vs 168 is not".
        let p = pick(&m, "t1w", &[of("170", "40")], &r);
        assert!(!p.borders.contains(&Border::Retake), "{:?}", p.borders);
        let p = pick(&m, "t1w", &[of("192", "168")], &r);
        assert_eq!(p.borders, [Border::Retake]);
        assert!(p.notes["retake"].starts_with("plain: 2"), "{:?}", p.notes);
        // Half the largest is kept: `below` is strictly below.
        let p = pick(&m, "t1w", &[of("176", "88")], &r);
        assert_eq!(
            p.borders,
            [Border::Retake],
            "88 is half of 176, not below it"
        );
        let p = pick(&m, "t1w", &[of("176", "87")], &r);
        assert!(p.borders.is_empty(), "{:?}", p.borders);
        // Under 60 slices nothing is demoted: v0 demotes only where the
        // largest is at least `partial_volume_min_slices`, and 60 is.
        let p = pick(&m, "t1w", &[of("59", "20")], &r);
        assert_eq!(p.borders, [Border::Retake], "59 demotes nothing");
        let p = pick(&m, "t1w", &[of("60", "20")], &r);
        assert!(p.borders.is_empty(), "60 demotes the 20: {:?}", p.borders);
        // One stack is no retake.
        let one = with_each(&[(1, &[("q", "top"), ("n_instances", "176")])], None);
        assert!(pick(&m, "t1w", &[one], &r).borders.is_empty());
    }

    #[test]
    fn a_family_s_retake_is_counted_by_the_family() {
        // v0: more than one of a Dixon's canonical construct, more than two
        // of an MP2RAGE's. Sisters of one Dixon are no retake, because the
        // family keeps only its canonical output.
        let m = chosen(retake(), vec![family("dixon", 1), family("mp2rage", 2)]);
        let r = Reference::default();
        let stacks = |n: i64, fam: &str| {
            let rows: Vec<(i64, &[(&str, &str)])> =
                (1..=n).map(|i| (i, &[("q", "top")][..])).collect();
            with_each(&rows, Some(fam))
        };
        assert!(
            pick(&m, "t1w", &[stacks(1, "dixon")], &r)
                .borders
                .is_empty()
        );
        let p = pick(&m, "t1w", &[stacks(2, "dixon")], &r);
        assert_eq!(p.borders, [Border::Retake]);
        assert!(p.notes["retake"].starts_with("dixon: 2"), "{:?}", p.notes);
        assert!(
            pick(&m, "t1w", &[stacks(2, "mp2rage")], &r)
                .borders
                .is_empty()
        );
        let p = pick(&m, "t1w", &[stacks(3, "mp2rage")], &r);
        assert_eq!(p.borders, [Border::Retake]);
        assert!(p.notes["retake"].starts_with("mp2rage: 3"), "{:?}", p.notes);
    }

    #[test]
    fn an_unknown_dimension_is_a_border_and_a_known_one_is_not() {
        let m = chosen(
            Borders {
                unknown_dim: Some("dim".into()),
                ..Borders::default()
            },
            Vec::new(),
        );
        let r = Reference::default();
        let p = pick(&m, "t1w", &[candidate(&[1], &[("q", "top")])], &r);
        assert_eq!(p.borders, [Border::UnknownDim]);
        let p = pick(
            &m,
            "t1w",
            &[candidate(&[1], &[("q", "top"), ("dim", "2D")])],
            &r,
        );
        assert!(p.borders.is_empty());
    }

    #[test]
    fn a_slice_count_outside_its_bucket_s_fifth_to_ninety_fifth_is_a_border() {
        let m = chosen(
            Borders {
                slice_outlier: Some(SliceOutlier {
                    population: "slices".into(),
                    below: 0.05,
                    above: 0.95,
                }),
                ..Borders::default()
            },
            Vec::new(),
        );
        let mut r = Reference::default();
        r.percentiles.insert(
            "slices:3D".into(),
            Percentiles {
                p5: 160.0,
                p25: 170.0,
                p50: 176.0,
                p75: 176.0,
                p95: 192.0,
            },
        );
        let at = |n: &str, dim: &str| {
            pick(
                &m,
                "t1w",
                &[candidate(
                    &[1],
                    &[("q", "top"), ("n_instances", n), ("dim", dim)],
                )],
                &r,
            )
        };
        // Strictly outside, as v0 compares: `<` the fifth, `>` the ninety-fifth.
        assert_eq!(at("159", "3D").borders, [Border::SliceOutlier]);
        assert!(
            at("160", "3D").borders.is_empty(),
            "the fifth itself is inside"
        );
        assert!(
            at("192", "3D").borders.is_empty(),
            "the ninety-fifth itself is inside"
        );
        let p = at("193", "3D");
        assert_eq!(p.borders, [Border::SliceOutlier]);
        assert_eq!(
            p.notes["slice_count_outlier"],
            "193 outside 160 to 192 in slices:3D"
        );
        // In its own bucket only: a 2D population too small to bucket says
        // nothing, as v0's missing percentiles did.
        assert!(at("24", "2D").borders.is_empty());
        // And no slice count says nothing.
        let p = pick(
            &m,
            "t1w",
            &[candidate(&[1], &[("q", "top"), ("dim", "3D")])],
            &r,
        );
        assert!(p.borders.is_empty());
    }

    #[test]
    fn a_twin_across_contrast_is_at_least_the_fraction_and_shares_no_contrast() {
        let m = chosen(
            Borders {
                pre_post_twin: Some(Twin {
                    of: "post_contrast".into(),
                    at_least: 0.85,
                }),
                ..Borders::default()
            },
            Vec::new(),
        );
        let r = Reference::default();
        let winner = candidate(&[1], &[("q", "top"), ("post_contrast", "0")]);
        let twin = |q: &str, pc: &str| candidate(&[2], &[("q", q), ("post_contrast", pc)]);
        // `at_least` takes the number itself.
        let p = pick(&m, "t1w", &[winner.clone(), twin("at_85", "1")], &r);
        assert_eq!(p.borders, [Border::PrePostTwin]);
        assert_eq!(p.notes["pre_post_twin"], "2");
        let p = pick(&m, "t1w", &[winner.clone(), twin("under_85", "1")], &r);
        assert!(p.borders.is_empty(), "{:?}", p.borders);
        // The same contrast is no twin, however close.
        let p = pick(&m, "t1w", &[winner.clone(), twin("at_85", "0")], &r);
        assert!(p.borders.is_empty());
        // Nothing stated is a value of its own, as v0's None was.
        let p = pick(&m, "t1w", &[winner, candidate(&[2], &[("q", "at_85")])], &r);
        assert_eq!(p.borders, [Border::PrePostTwin]);
        let both_unstated = pick(
            &m,
            "t1w",
            &[
                candidate(&[1], &[("q", "top")]),
                candidate(&[2], &[("q", "at_85")]),
            ],
            &r,
        );
        assert!(both_unstated.borders.is_empty());
    }

    #[test]
    fn a_fallback_that_won_is_a_border_on_any_stack_of_the_winner() {
        let m = chosen(
            Borders {
                fallback: Some(("provenance".into(), vec!["EPIMix".into()])),
                ..Borders::default()
            },
            Vec::new(),
        );
        let r = Reference::default();
        let p = pick(
            &m,
            "t1w",
            &[candidate(&[1], &[("q", "top"), ("provenance", "EPIMix")])],
            &r,
        );
        assert_eq!(p.borders, [Border::EpimixFallback]);
        // v0 reads every stack of the winning bundle, not its first.
        let mixed = with_each(
            &[
                (1, &[("q", "top"), ("provenance", "RawRecon")]),
                (2, &[("q", "top"), ("provenance", "EPIMix")]),
            ],
            None,
        );
        assert_eq!(
            pick(&m, "t1w", &[mixed], &r).borders,
            [Border::EpimixFallback]
        );
        let p = pick(
            &m,
            "t1w",
            &[candidate(&[1], &[("q", "top"), ("provenance", "RawRecon")])],
            &r,
        );
        assert!(p.borders.is_empty());
        // Record 53: a list of values, each raising the one border, and the
        // note names which the winner held.
        let m = chosen(
            Borders {
                fallback: Some((
                    "provenance".into(),
                    vec!["EPIMix".into(), "NeuroMix".into()],
                )),
                ..Borders::default()
            },
            Vec::new(),
        );
        let p = pick(
            &m,
            "t1w",
            &[candidate(&[1], &[("q", "top"), ("provenance", "NeuroMix")])],
            &r,
        );
        assert_eq!(p.borders, [Border::EpimixFallback]);
        assert_eq!(p.notes["epimix_fallback"], "NeuroMix");
    }

    #[test]
    fn a_dixon_that_won_with_a_plain_one_within_a_tenth_is_a_border() {
        let m = chosen(
            Borders {
                dixon_vs_plain: Some(Plain {
                    family: "dixon".into(),
                    of: "modifier".into(),
                    plain_without: vec!["Dixon".into(), "WaterExc".into()],
                    within: 0.10,
                }),
                ..Borders::default()
            },
            vec![family("dixon", 1)],
        );
        let r = Reference::default();
        let dixon = with_each(
            &[(1, &[("q", "top"), ("modifier", "Dixon")])],
            Some("dixon"),
        );
        let other = |q: &str, modifier: &str| candidate(&[2], &[("q", q), ("modifier", modifier)]);
        // `within` takes the number itself.
        let p = pick(&m, "t1w", &[dixon.clone(), other("at_90", "")], &r);
        assert_eq!(p.borders, [Border::DixonVsPlain]);
        assert_eq!(p.notes["dixon_vs_plain"], "2");
        let p = pick(&m, "t1w", &[dixon.clone(), other("under_90", "")], &r);
        assert!(p.borders.is_empty(), "{:?}", p.borders);
        // A water-excited acquisition is not plain.
        let p = pick(&m, "t1w", &[dixon.clone(), other("at_90", "WaterExc")], &r);
        assert!(p.borders.is_empty());
        // And a winner that is not the family's asks nothing.
        let lone = candidate(&[1], &[("q", "top"), ("modifier", "Dixon")]);
        let p = pick(&m, "t1w", &[lone, other("at_90", "")], &r);
        assert!(p.borders.is_empty());
    }

    #[test]
    fn a_model_that_declares_none_of_the_six_raises_none_of_them() {
        // A pack written before record 51, or an overlay that keeps its
        // borders: every one of the six reasons planted at once, and only
        // the old three can come out.
        let m = chosen(Borders::default(), vec![family("dixon", 1)]);
        let r = Reference::default();
        let everything = with_each(
            &[
                (
                    1,
                    &[
                        ("q", "top"),
                        ("provenance", "EPIMix"),
                        ("modifier", "Dixon"),
                        ("n_instances", "176"),
                    ],
                ),
                (
                    2,
                    &[
                        ("q", "top"),
                        ("provenance", "EPIMix"),
                        ("modifier", "Dixon"),
                        ("n_instances", "176"),
                    ],
                ),
            ],
            Some("dixon"),
        );
        let twin = candidate(&[3], &[("q", "at_90"), ("post_contrast", "1")]);
        let p = pick(&m, "t1w", &[everything, twin], &r);
        assert!(p.borders.is_empty(), "{:?}", p.borders);
        assert!(p.notes.is_empty());
    }
}
