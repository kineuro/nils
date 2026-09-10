// SPDX-License-Identifier: AGPL-3.0-only

//! `diagnose` (§10): taxonomy errors with their paths and the next call,
//! the repairs applied, the warnings, a one line zero row explanation in
//! domain words, per clause and per null drop counts, ties per pick,
//! unresolved values rows, rows coarser than a clause's unit, the cost
//! class, and the funnel (Q1): the subjects surviving each stage of each
//! named set, so an author sees where subjects fall out. The leave one out
//! variant is a job's.

use std::collections::{BTreeMap, BTreeSet};

use nils_registry::home::Registry;
use nils_registry::session::Scheme;
use nils_registry::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ast::{Ask, Clause, Grain, Level, Out, Set};
use crate::describe::clause_text;
use crate::exec::Bounds;
use crate::repair::Repair;
use crate::run::{RunError, Runner, count_level, inline_selections, keys_level};
use crate::validate::{Issue, Names, Scope, validate};
use crate::{Error as AskError, prepare};

/// One stage of the funnel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub set: String,
    pub grain: String,
    pub stage: String,
    pub rows: i64,
    pub subjects: i64,
    /// Whether the set lies on the subject's path from the cohort down (a
    /// cohort, subject or session set); a stack or event set is a helper a
    /// partner relation reads, and a subject absent from it has not fallen
    /// out.
    pub on_path: bool,
    /// The subject keys surviving, when asked for; bounded by the row cap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<i64>,
}

/// A where clause of the answer's set, and what it dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drop {
    pub set: String,
    pub clause: String,
    pub before: i64,
    pub after: i64,
    /// Rows the clause could not judge: its first field was null.
    pub nulls: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub sets: usize,
    pub near: usize,
    pub groups: usize,
    pub algebra: usize,
    pub class: String,
}

/// One clause group of one set (Wave 5 section 12.3): the funnel keyed by
/// set and group in the language's own order (source, near, attach, has,
/// where, pick, out), so a step's own counts are computable without the
/// document being rewritten. `kept` is the rows after the group's last
/// clause; `lost` is what the group took from the rows before it, so a
/// set's source rows less the sum of its groups' losses is its answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClauseGroup {
    pub set: String,
    pub grain: String,
    pub group: String,
    pub clauses: usize,
    pub kept: i64,
    pub subjects: i64,
    pub lost: i64,
}

pub const GROUPS: [&str; 7] = ["source", "near", "attach", "has", "where", "pick", "out"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnosis {
    pub valid: bool,
    pub issues: Vec<Issue>,
    pub repairs: Vec<Repair>,
    pub warnings: Vec<Issue>,
    pub zero_rows: Option<String>,
    pub drops: Vec<Drop>,
    /// A picked set and how many of its rows tied.
    pub ties: Vec<(String, i64)>,
    pub unresolved: Vec<Value>,
    /// A dated set and how many of its rows are coarser than a day.
    pub coarse: Vec<(String, i64)>,
    pub cost: Cost,
    pub funnel: Vec<Stage>,
    /// How the funnel is keyed: `set` (the stages) or `clause` (the groups
    /// below are filled).
    #[serde(default = "by_set")]
    pub by: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<ClauseGroup>,
    pub next: Vec<String>,
}

fn by_set() -> String {
    "set".to_string()
}

impl Diagnosis {
    /// Key the funnel by clause group (Wave 5 section 12.3): the stages of
    /// each set folded into the language's groups, in order; a group a set
    /// has no clause of is not a row; the answer's set ends with `out`.
    pub fn by_clause(mut self, out_set: &str) -> Diagnosis {
        let mut groups: Vec<ClauseGroup> = Vec::new();
        let mut sets: Vec<&str> = Vec::new();
        for st in &self.funnel {
            if !sets.contains(&st.set.as_str()) {
                sets.push(&st.set);
            }
        }
        for set in sets {
            let stages: Vec<&Stage> = self.funnel.iter().filter(|s| s.set == set).collect();
            let mut before: Option<i64> = None;
            for group in GROUPS.iter().take(6) {
                let mine: Vec<&&Stage> = stages
                    .iter()
                    .filter(|s| s.stage == *group || s.stage.starts_with(&format!("{group} ")))
                    .collect();
                let Some(last) = mine.last() else {
                    continue;
                };
                let kept = last.rows;
                let lost = before.map(|b| b - kept).unwrap_or(0).max(0);
                groups.push(ClauseGroup {
                    set: set.to_string(),
                    grain: last.grain.clone(),
                    group: group.to_string(),
                    clauses: mine.len(),
                    kept,
                    subjects: last.subjects,
                    lost,
                });
                before = Some(kept);
            }
            if set == out_set
                && let Some(last) = stages.last()
            {
                groups.push(ClauseGroup {
                    set: set.to_string(),
                    grain: last.grain.clone(),
                    group: "out".to_string(),
                    clauses: 1,
                    kept: last.rows,
                    subjects: last.subjects,
                    lost: 0,
                });
            }
        }
        self.by = "clause".to_string();
        self.groups = groups;
        self
    }

    /// The first stage on the subject's path a subject is missing from, in
    /// funnel order (Q1's layered reading).
    pub fn falls_out(&self, subject: i64) -> Option<&Stage> {
        self.funnel
            .iter()
            .find(|s| s.on_path && !s.keys.is_empty() && !s.keys.contains(&subject))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Source,
    Near(usize),
    Attach(usize),
    Has(usize),
    Where(usize),
    Pick,
}

fn steps(set: &Set) -> Vec<(Step, String)> {
    let mut out = vec![(Step::Source, "source".to_string())];
    for (i, n) in set.near.iter().enumerate() {
        out.push((Step::Near(i), format!("near {}", n.as_)));
    }
    for (i, a) in set.attach.iter().enumerate() {
        out.push((Step::Attach(i), format!("attach {}", a.as_)));
    }
    for (i, h) in set.has.iter().enumerate() {
        out.push((Step::Has(i), format!("has {}", h.set)));
    }
    for (i, c) in set.where_.iter().enumerate() {
        out.push((Step::Where(i), format!("where {}", clause_text(c))));
    }
    if set.pick.is_some() {
        out.push((Step::Pick, "pick".to_string()));
    }
    out
}

fn paths_in(c: &Clause, out: &mut BTreeSet<String>) {
    let mut all = Vec::new();
    c.walk(&mut all);
    for x in all {
        if x.op == "field"
            && let Some(p) = x.ref_name()
        {
            out.insert(p.to_string());
        }
    }
}

/// The set cut after one step: later relations gone, the where clauses
/// past the step gone, the bindings that read a dropped partner or count
/// gone, the pick gone unless the step is the pick.
fn truncate(set: &mut Set, step: &Step) {
    let (near, attach, has, wheres, pick) = match step {
        Step::Source => (0, 0, 0, 0, false),
        Step::Near(i) => (i + 1, 0, 0, 0, false),
        Step::Attach(i) => (set.near.len(), i + 1, 0, 0, false),
        Step::Has(i) => (set.near.len(), set.attach.len(), i + 1, 0, false),
        Step::Where(i) => (
            set.near.len(),
            set.attach.len(),
            set.has.len(),
            i + 1,
            false,
        ),
        Step::Pick => (
            set.near.len(),
            set.attach.len(),
            set.has.len(),
            set.where_.len(),
            true,
        ),
    };
    let dropped_as: BTreeSet<String> = set
        .near
        .iter()
        .skip(near)
        .map(|n| n.as_.clone())
        .chain(set.attach.iter().skip(attach).map(|a| a.as_.clone()))
        .chain(set.has.iter().skip(has).filter_map(|h| h.as_.clone()))
        .collect();
    set.near.truncate(near);
    set.attach.truncate(attach);
    set.has.truncate(has);
    set.where_.truncate(wheres);
    if !pick {
        set.pick = None;
    }
    if !dropped_as.is_empty() {
        set.bind.0.retain(|(_, c)| {
            let mut paths = BTreeSet::new();
            paths_in(c, &mut paths);
            !paths.iter().any(|p| {
                dropped_as.contains(p) || dropped_as.iter().any(|d| p.starts_with(&format!("{d}.")))
            })
        });
        // a where clause kept may read a dropped binding: none are kept
        // past a dropped relation, since where comes after every relation
    }
}

/// Every set that reads `target`, transitively.
fn readers_of(ask: &Ask, target: &str) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    loop {
        let mut grew = false;
        for (name, s) in &ask.sets {
            if name == target || out.contains(name) {
                continue;
            }
            if s.reads().iter().any(|r| *r == target || out.contains(*r)) {
                out.insert(name.clone());
                grew = true;
            }
        }
        if !grew {
            return out;
        }
    }
}

/// The document with `target` cut after `step`, its readers gone, and
/// `target` as the answer.
fn variant(ask: &Ask, target: &str, step: &Step, level: Level) -> Ask {
    let mut a = ask.clone();
    for r in readers_of(ask, target) {
        a.sets.remove(&r);
    }
    if let Some(s) = a.sets.get_mut(target) {
        truncate(s, step);
    }
    a.keep = Vec::new();
    a.out = Out {
        set: target.to_string(),
        level,
        columns: Vec::new(),
        measures: Vec::new(),
        identifiers: Vec::new(),
        order: Vec::new(),
        limit: None,
    };
    a
}

struct Counter<'a> {
    runner: Runner<'a>,
    names: &'a dyn Names,
    scope: &'a Scope,
}

impl Counter<'_> {
    fn count(&mut self, registry: &mut Registry, ask: &Ask) -> Result<(i64, i64), RunError> {
        let v = validate(ask, self.names, self.scope).map_err(AskError::Invalid)?;
        let (_, a) = self.runner.answer(registry, ask, &v, None, None)?;
        let row = a.rows.first();
        Ok((
            row.map(|r| r.int(0)).transpose()?.unwrap_or(0),
            row.map(|r| r.int(1)).transpose()?.unwrap_or(0),
        ))
    }

    /// The subject keys of a record level variant, bounded by the cap.
    fn keys(&mut self, registry: &mut Registry, ask: &Ask) -> Result<(i64, Vec<i64>), RunError> {
        let v = validate(ask, self.names, self.scope).map_err(AskError::Invalid)?;
        let (_, a) = self.runner.answer(registry, ask, &v, None, None)?;
        let mut keys: Vec<i64> = a
            .rows
            .iter()
            .filter_map(|r| {
                r.opt_int(1)
                    .ok()
                    .flatten()
                    .or_else(|| r.opt_int(0).ok().flatten())
            })
            .collect();
        keys.sort_unstable();
        keys.dedup();
        Ok((a.rows.len() as i64, keys))
    }
}

/// Diagnose a document: its issues when invalid (no query), else the
/// funnel and the counts.
#[allow(clippy::too_many_arguments)]
pub fn diagnose(
    registry: &mut Registry,
    ask: Ask,
    repairs: Vec<Repair>,
    names: &dyn Names,
    scope: &Scope,
    scheme: &Scheme,
    bounds: Bounds,
    with_keys: bool,
    reader: Option<&mut Store>,
) -> Result<Diagnosis, RunError> {
    let cost = cost_of(&ask);
    let prepared = match prepare(ask, names, scope) {
        Ok(p) => p,
        Err(AskError::Invalid(issues)) => {
            let next: Vec<String> = issues.iter().map(|i| i.next.clone()).collect();
            return Ok(Diagnosis {
                valid: false,
                issues,
                repairs,
                warnings: Vec::new(),
                zero_rows: None,
                drops: Vec::new(),
                ties: Vec::new(),
                unresolved: Vec::new(),
                coarse: Vec::new(),
                cost,
                funnel: Vec::new(),
                by: by_set(),
                groups: Vec::new(),
                next,
            });
        }
        Err(e) => return Err(e.into()),
    };
    let mut ask = prepared.ask;
    let warnings = prepared.validated.warnings.clone();
    let inlined = inline_selections(registry, &mut ask)?;
    let validated = if inlined.is_empty() {
        prepared.validated
    } else {
        validate(&ask, names, scope).map_err(AskError::Invalid)?
    };
    let mut counter = Counter {
        runner: Runner {
            names,
            scheme,
            bounds,
            reader,
        },
        names,
        scope,
    };
    // the funnel, set by set in topological order, stage by stage
    let mut funnel = Vec::new();
    let mut stage_counts: BTreeMap<(String, String), (i64, i64)> = BTreeMap::new();
    for name in &validated.order {
        if name.contains("__") {
            continue;
        }
        let Some(set) = ask.sets.get(name) else {
            continue;
        };
        if set.grain == Grain::Group {
            continue;
        }
        for (step, label) in steps(set) {
            let (rows, subjects, keys) = if with_keys {
                let v = variant(&ask, name, &step, Level::Record);
                let (rows, keys) = counter.keys(registry, &v)?;
                (rows, keys.len() as i64, keys)
            } else {
                let v = variant(&ask, name, &step, Level::Count);
                let (rows, subjects) = counter.count(registry, &v)?;
                (rows, subjects, Vec::new())
            };
            stage_counts.insert((name.clone(), label.clone()), (rows, subjects));
            funnel.push(Stage {
                set: name.clone(),
                grain: set.grain.name().to_string(),
                stage: label,
                rows,
                subjects,
                on_path: matches!(set.grain, Grain::Cohort | Grain::Subject | Grain::Session),
                keys,
            });
        }
    }
    // the answer's where clauses: before, after and the nulls
    let mut drops = Vec::new();
    if let Some(out_set) = ask.sets.get(&ask.out.set)
        && out_set.grain != Grain::Group
    {
        let all = steps(out_set);
        for c in &out_set.where_ {
            let this = format!("where {}", clause_text(c));
            let position = all.iter().position(|(_, l)| *l == this).unwrap_or(0);
            let before_label = position
                .checked_sub(1)
                .map(|p| all[p].1.clone())
                .unwrap_or_else(|| "source".into());
            let before = stage_counts
                .get(&(ask.out.set.clone(), before_label))
                .map(|c| c.0)
                .unwrap_or(0);
            let after = stage_counts
                .get(&(ask.out.set.clone(), this.clone()))
                .map(|c| c.0)
                .unwrap_or(0);
            let mut paths = BTreeSet::new();
            paths_in(c, &mut paths);
            let nulls = match paths.iter().next() {
                Some(p) => {
                    // the set as it stands before this clause, plus "the
                    // field is unknown"
                    let previous = position
                        .checked_sub(1)
                        .map(|q| all[q].0.clone())
                        .unwrap_or(Step::Source);
                    let mut v = variant(&ask, &ask.out.set, &previous, Level::Count);
                    if let Some(s) = v.sets.get_mut(&ask.out.set) {
                        s.where_.push(
                            Clause::new("is_null").arg(crate::ast::Arg::Clause(Clause::field(p))),
                        );
                    }
                    counter.count(registry, &v).map(|c| c.0).unwrap_or(0)
                }
                None => 0,
            };
            drops.push(Drop {
                set: ask.out.set.clone(),
                clause: this,
                before,
                after,
                nulls,
            });
        }
    }
    // ties per pick
    let mut ties = Vec::new();
    for (name, set) in &ask.sets {
        if name.contains("__") || set.pick.is_none() {
            continue;
        }
        let mut v = keys_level(&ask, name);
        for r in readers_of(&ask, name) {
            v.sets.remove(&r);
        }
        v.out.columns = vec![Clause::field("pick.tied")];
        let vv = validate(&v, names, scope).map_err(AskError::Invalid)?;
        let (_, a) = counter.runner.answer(registry, &v, &vv, None, None)?;
        let tied = a
            .rows
            .iter()
            .filter(|r| r.opt_int(2).ok().flatten() == Some(1))
            .count() as i64;
        ties.push((name.clone(), tied));
    }
    // rows coarser than a day, per dated set with a precision
    let mut coarse = Vec::new();
    for name in &validated.order {
        let Some(set) = ask.sets.get(name) else {
            continue;
        };
        if name.contains("__") || set.grain != Grain::Event {
            continue;
        }
        let mut v = count_level(&ask, name);
        for r in readers_of(&ask, name) {
            v.sets.remove(&r);
        }
        if let Some(s) = v.sets.get_mut(name) {
            s.where_.push(
                Clause::new("<>")
                    .arg(crate::ast::Arg::Clause(Clause::field("precision")))
                    .arg(crate::ast::Arg::Text("day".into())),
            );
        }
        let (rows, _) = counter.count(registry, &v)?;
        if rows > 0 {
            coarse.push((name.clone(), rows));
        }
    }
    // unresolved values
    let mut unresolved = Vec::new();
    for (name, decl) in &ask.values {
        if let Some(shape) = crate::values::shape(registry.store(), &decl.upload)
            .map_err(|e| RunError::Message(e.to_string()))?
        {
            unresolved.push(json!({"values": name, "upload": decl.upload, "shape": shape}));
        }
    }
    // the zero row explanation, in domain words
    let answer_rows = stage_counts
        .iter()
        .rfind(|((s, _), _)| *s == ask.out.set)
        .map(|(_, c)| c.0);
    let zero_rows = match answer_rows {
        Some(0) => funnel
            .iter()
            .find(|s| s.subjects == 0 && s.rows == 0)
            .map(|s| {
                format!(
                    "no {} of {} survives {}; everything after it is empty",
                    s.grain, s.set, s.stage
                )
            })
            .or_else(|| Some(format!("{} answers with no rows", ask.out.set))),
        _ => None,
    };
    let mut next = Vec::new();
    if zero_rows.is_some() {
        next.push("POST /api/ask/options on the set named, for a move that widens it".into());
    }
    if !warnings.is_empty() {
        next.push("read the warnings; a selection_outdated warning carries an update move".into());
    }
    Ok(Diagnosis {
        valid: true,
        issues: Vec::new(),
        repairs,
        warnings,
        zero_rows,
        drops,
        ties,
        unresolved,
        coarse,
        cost,
        funnel,
        by: by_set(),
        groups: Vec::new(),
        next,
    })
}

/// The cost class of a document, by its shape.
pub fn cost_of(ask: &Ask) -> Cost {
    let sets = ask.sets.len();
    let near: usize = ask.sets.values().map(|s| s.near.len()).sum();
    let groups = ask.sets.values().filter(|s| s.group.is_some()).count();
    let algebra = ask.sets.values().filter(|s| s.algebra.is_some()).count();
    let class = if sets <= 3 && near == 0 && groups == 0 {
        "small"
    } else if sets <= 8 && near <= 2 {
        "medium"
    } else {
        "large"
    };
    Cost {
        sets,
        near,
        groups,
        algebra,
        class: class.into(),
    }
}
