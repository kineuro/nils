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

use crate::ast::{Ask, Clause, Grain, Level, Out, Set, Src};
use crate::describe::clause_text;
use crate::exec::Bounds;
use crate::repair::Repair;
use crate::run::{RunError, Runner, count_level, inline_selections, keys_level};
use crate::validate::{Issue, Names, Scope, validate};
use crate::{Error as AskError, prepare};

/// One stage of the funnel. Record 55 K7, small counts at plain (the
/// ruling of 2026-10-12): below detail quasi, the rows and the subjects of
/// a stage a quasi identifying field chose are null (held back) for fewer
/// than 5 subjects, and where the stage dropped fewer than 5 subjects from
/// the stage before it, which the two counts would say by subtraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub set: String,
    pub grain: String,
    pub stage: String,
    pub rows: Option<i64>,
    pub subjects: Option<i64>,
    /// Whether the set lies on the subject's path from the cohort down (a
    /// cohort, subject or session set); a stack or event set is a helper a
    /// partner relation reads, and a subject absent from it has not fallen
    /// out.
    pub on_path: bool,
    /// The subject keys surviving, when asked for; bounded by the row cap.
    /// None where the stage's counts are held back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<i64>,
    /// Why the stage's counts are held back, where they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_back: Option<String>,
}

/// A where clause of the answer's set, and what it dropped; a count held
/// back is null (record 55 K7, small counts at plain).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drop {
    pub set: String,
    pub clause: String,
    pub before: Option<i64>,
    pub after: Option<i64>,
    /// Rows the clause could not judge: its first field was null.
    pub nulls: Option<i64>,
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
    pub kept: Option<i64>,
    pub subjects: Option<i64>,
    pub lost: Option<i64>,
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
    /// A picked set and how many of its rows tied; null where held back.
    pub ties: Vec<(String, Option<i64>)>,
    pub unresolved: Vec<Value>,
    /// A dated set and how many of its rows are coarser than a day; null
    /// where held back.
    pub coarse: Vec<(String, Option<i64>)>,
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
    /// A document refused before anything runs: its issues and their next
    /// calls, the repairs made, its cost class, and no counts.
    pub fn refused(issues: Vec<Issue>, repairs: Vec<Repair>, cost: Cost) -> Diagnosis {
        let next: Vec<String> = issues.iter().map(|i| i.next.clone()).collect();
        Diagnosis {
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
        }
    }

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
            let mut before: Option<Option<i64>> = None;
            for group in GROUPS.iter().take(6) {
                let mine: Vec<&&Stage> = stages
                    .iter()
                    .filter(|s| s.stage == *group || s.stage.starts_with(&format!("{group} ")))
                    .collect();
                let Some(last) = mine.last() else {
                    continue;
                };
                let kept = last.rows;
                // a count held back holds what it lost with it
                let lost = match before {
                    None => Some(0),
                    Some(b) => b.zip(kept).map(|(b, k)| (b - k).max(0)),
                };
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
                    lost: Some(0),
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

/// A count the diagnosis read: its rows and subjects, null where held back
/// (record 55 K7, small counts at plain), and whether a quasi identifying
/// field chose the rows it counts below detail quasi.
#[derive(Debug, Clone, Copy, Default)]
struct Count {
    rows: Option<i64>,
    subjects: Option<i64>,
    quasi: bool,
}

impl Counter<'_> {
    /// A count level variant's rows and subjects, as the ask answers them
    /// under the caller's scope.
    fn count(&mut self, registry: &mut Registry, ask: &Ask) -> Result<Count, RunError> {
        let v = validate(ask, self.names, self.scope).map_err(AskError::Invalid)?;
        let quasi = v.quasi_sets.contains(&ask.out.set);
        let (_, a) = self.runner.answer(registry, ask, &v, None, None)?;
        let row = a.rows.first();
        let cell = |i: usize| -> Result<Option<i64>, RunError> {
            Ok(match row {
                Some(r) => r.opt_int(i)?,
                None => Some(0),
            })
        };
        Ok(Count {
            rows: cell(0)?,
            subjects: cell(1)?,
            quasi,
        })
    }

    /// The subject keys of a record level variant, bounded by the cap,
    /// none where a quasi identifying field chose fewer than
    /// [`crate::validate::MEASURE_K`] subjects below detail quasi, whose
    /// count is then held back as a count level variant's is.
    fn keys(&mut self, registry: &mut Registry, ask: &Ask) -> Result<(Count, Vec<i64>), RunError> {
        let v = validate(ask, self.names, self.scope).map_err(AskError::Invalid)?;
        let quasi = v.quasi_sets.contains(&ask.out.set);
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
        let (rows, subjects) = (a.rows.len() as i64, keys.len() as i64);
        if quasi && subjects > 0 && subjects < crate::validate::MEASURE_K {
            return Ok((
                Count {
                    rows: None,
                    subjects: None,
                    quasi,
                },
                Vec::new(),
            ));
        }
        Ok((
            Count {
                rows: Some(rows),
                subjects: Some(subjects),
                quasi,
            },
            keys,
        ))
    }
}

/// Diagnose a document: its issues when invalid or when the compiler
/// refuses it (no query), else the funnel and the counts.
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
    // the diagnosis compiles what it counts, so what the compiler refuses
    // is refused before anything is counted, as what the validator refuses
    // is: a clause the validator passed and the compiler refused, a
    // `bucket` bound without its unit, failed a count, and the draft door
    // answered a bare 500 (2026-10-12)
    let unreadable = crate::run::not_compilable(registry, &ask, names, scope, scheme);
    let prepared = match prepare(ask, names, scope) {
        Ok(p) => p,
        Err(AskError::Invalid(issues)) => return Ok(Diagnosis::refused(issues, repairs, cost)),
        Err(e) => return Err(e.into()),
    };
    if !unreadable.is_empty() {
        return Ok(Diagnosis::refused(unreadable, repairs, cost));
    }
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
            unsealed: scope.unsealed,
        },
        names,
        scope,
    };
    // the funnel, set by set in topological order, stage by stage
    let mut funnel = Vec::new();
    let mut stage_counts: BTreeMap<(String, String), (Option<i64>, Option<i64>)> = BTreeMap::new();
    // the sets whose last stage is held back: a set that narrows one, or
    // reads below it, starts from the same rows and holds its source too
    let mut last_held: BTreeSet<String> = BTreeSet::new();
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
        // the subjects of the stage before, as shown
        let mut before: Option<i64> = None;
        let from_held = matches!(&set.from, Some(Src::Set(x)) if last_held.contains(x))
            || set.of.as_ref().is_some_and(|o| last_held.contains(o));
        let mut held_last = false;
        for (step, label) in steps(set) {
            let (mut count, mut keys) = if with_keys {
                let v = variant(&ask, name, &step, Level::Record);
                counter.keys(registry, &v)?
            } else {
                let v = variant(&ask, name, &step, Level::Count);
                (counter.count(registry, &v)?, Vec::new())
            };
            // record 55 K7, small counts at plain (the ruling of
            // 2026-10-12): a stage a quasi identifying field chose that
            // drops fewer than MEASURE_K subjects from the stage before it
            // is held back, since the two counts would say the few dropped
            let k = crate::validate::MEASURE_K;
            let dropped = before.zip(count.subjects).map(|(b, s)| b - s);
            let source_held = step == Step::Source && from_held;
            if count.quasi && (source_held || dropped.is_some_and(|d| d > 0 && d < k)) {
                count.rows = None;
                count.subjects = None;
                keys = Vec::new();
            }
            let held_back =
                (count.quasi && count.subjects.is_none()).then(crate::validate::held_count_why);
            before = count.subjects;
            held_last = held_back.is_some();
            stage_counts.insert((name.clone(), label.clone()), (count.rows, count.subjects));
            funnel.push(Stage {
                set: name.clone(),
                grain: set.grain.name().to_string(),
                stage: label,
                rows: count.rows,
                subjects: count.subjects,
                on_path: matches!(set.grain, Grain::Cohort | Grain::Subject | Grain::Session),
                keys,
                held_back,
            });
        }
        if held_last {
            last_held.insert(name.clone());
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
                .map_or(Some(0), |c| c.0);
            let after = stage_counts
                .get(&(ask.out.set.clone(), this.clone()))
                .map_or(Some(0), |c| c.0);
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
                    counter.count(registry, &v).map_or(Some(0), |c| c.rows)
                }
                None => Some(0),
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
        let quasi = vv.quasi_sets.contains(name);
        let (_, a) = counter.runner.answer(registry, &v, &vv, None, None)?;
        let tied = a
            .rows
            .iter()
            .filter(|r| r.opt_int(2).ok().flatten() == Some(1))
            .count() as i64;
        // record 55 K7, small counts at plain: a count of the rows a quasi
        // identifying field chose
        let small = quasi && tied > 0 && tied < crate::validate::MEASURE_K;
        ties.push((name.clone(), (!small).then_some(tied)));
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
        let rows = counter.count(registry, &v)?.rows;
        if rows != Some(0) {
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
        Some(Some(0)) => funnel
            .iter()
            .find(|s| s.subjects == Some(0) && s.rows == Some(0))
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
