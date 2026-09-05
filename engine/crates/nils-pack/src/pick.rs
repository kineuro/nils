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
#[derive(Debug, Clone)]
pub struct Borders {
    /// The runner-up is within this fraction of the winner.
    pub runner_up_within: f64,
    /// The winning value of this axis is held by less than this share of the
    /// population, so the pick is right by the numbers and odd by the protocol.
    pub rare_within: Option<(String, f64)>,
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
    /// And how the outputs of one acquisition are merged back together.
    pub family: Option<Family>,
}

/// One acquisition that produced several images, merged back into one
/// candidate (v0's stage 2).
///
/// A Dixon produces an in-phase, an out-of-phase, a water and a fat image, and
/// without this they compete with each other for the session, four ways.
#[derive(Debug, Clone)]
pub struct Family {
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
        if let Some(f) = &self.family {
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
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The stack ids, which is what a pick names.
    pub stacks: Vec<i64>,
    /// Everything a component may read, by the name the pack writes: an axis
    /// as the group agrees on it, a fingerprint field taken at its largest
    /// across the group. One map, because a component should not have to know
    /// which of the two it is naming, and because a bundle's slice count is
    /// the fullest volume in it and not an arbitrary one.
    pub values: BTreeMap<String, String>,
}

impl Candidate {
    pub fn get(&self, name: &str) -> &str {
        self.values.get(name).map(String::as_str).unwrap_or("")
    }

    fn num(&self, name: &str) -> Option<f64> {
        self.values.get(name)?.trim().parse().ok()
    }

    fn holds(&self, name: &str, token: &str) -> bool {
        self.get(name)
            .split(',')
            .any(|t| t.trim().eq_ignore_ascii_case(token))
    }
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
}

impl Border {
    pub fn name(self) -> &'static str {
        match self {
            Border::TooClose => "too_close",
            Border::Rare => "rare",
            Border::Nothing => "nothing_eligible",
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
    if second.is_some() && margin <= model.borders.runner_up_within {
        borders.push(Border::TooClose);
    }
    if let Some((of, floor)) = &model.borders.rare_within {
        let share = reference.share(of, candidates[first].get(of));
        if reference.total > 0 && share < *floor {
            borders.push(Border::Rare);
        }
    }

    Picked {
        role: role.to_string(),
        winner: Some(candidates[first].clone()),
        scored: Some(best),
        runner_up: second.map(|(i, _)| candidates[i].clone()),
        runner_up_score,
        margin,
        borders,
        considered,
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
            },
            same_acquisition: vec!["technique".into()],
            family: None,
        }
    }

    fn candidate(stacks: &[i64], pairs: &[(&str, &str)]) -> Candidate {
        Candidate {
            stacks: stacks.to_vec(),
            values: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
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
}
