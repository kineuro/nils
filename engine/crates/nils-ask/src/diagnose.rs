// SPDX-License-Identifier: AGPL-3.0-only

//! `diagnose` (§10): taxonomy errors with their paths and the next call,
//! the repairs applied, the warnings, a one line zero row explanation in
//! domain words, per clause and per null drop counts, ties per pick,
//! unresolved values rows, rows coarser than a clause's unit, the cost
//! class, and the funnel (Q1): the subjects surviving each stage of each
//! named set, so an author sees where subjects fall out. The leave one out
//! variant is a job's.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

use nils_registry::home::Registry;
use nils_registry::session::Scheme;
use nils_registry::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ast::{Arg, Ask, Clause, Grain, GroupSpec, Level, Out, Set, Src};
use crate::compile::Compiled;
use crate::describe::clause_text;
use crate::exec::{Answer, Bounds, ExecError};
use crate::repair::Repair;
use crate::run::{RunError, Runner, count_level, inline_selections, keys_level};
use crate::validate::{Class, Issue, MEASURE_K, Names, Scope, Validated, validate};
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

/// The parts of a diagnosis that are counted, in the order they are: what
/// [`Unavailable::parts`] names.
pub const PARTS: [&str; 4] = ["funnel", "drops", "ties", "coarse"];

/// What a diagnosis did not count because the time it was given ran out
/// (2026-11-13). The document is valid all the same: validity and the cost
/// class come from the document alone, and only the counts are missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unavailable {
    /// Why, for a caller that reads no words: `timeout`.
    pub code: String,
    /// The parts not counted whole, of [`PARTS`]. The funnel keeps the
    /// stages counted before the time ran out; any other part named here is
    /// left empty rather than half counted.
    pub parts: Vec<String>,
    /// The time the counts were given as a whole, in milliseconds.
    pub budget_ms: u64,
    pub why: String,
}

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
    /// What was not counted because the counts ran past their time; absent
    /// where everything was counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<Unavailable>,
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
            unavailable: None,
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

/// The step a set stands at before its first `where`: its last relation,
/// or its source.
fn before_where(set: &Set) -> Step {
    if !set.has.is_empty() {
        Step::Has(set.has.len() - 1)
    } else if !set.attach.is_empty() {
        Step::Attach(set.attach.len() - 1)
    } else if !set.near.is_empty() {
        Step::Near(set.near.len() - 1)
    } else {
        Step::Source
    }
}

/// Where a step is among the stages one pass counts: 0 for the step before
/// the first `where`, then one more for each `where` clause.
fn stage_of(set: &Set, step: &Step) -> Option<usize> {
    match step {
        Step::Where(i) => Some(i + 1),
        other if *other == before_where(set) => Some(0),
        _ => None,
    }
}

/// Whether a set's `where` stages can be counted in one pass: a set with a
/// base relation of its own (not a group, not the result of an algebra)
/// that has a `where`, none of whose clauses reads the rows around its own
/// (`ordinal`, `prev`, `next`) or a subject's history (`change`), which a
/// `where` may not hold and a binding may.
fn one_pass(set: &Set) -> bool {
    let based = matches!(
        set.grain,
        Grain::Cohort
            | Grain::Subject
            | Grain::Session
            | Grain::Stack
            | Grain::Instance
            | Grain::Event
    );
    based
        && set.algebra.is_none()
        && set.group.is_none()
        && !set.where_.is_empty()
        && !set.where_.iter().any(|c| {
            let mut all = Vec::new();
            c.walk(&mut all);
            all.iter()
                .any(|x| matches!(x.op.as_str(), "ordinal" | "prev" | "next" | "change"))
        })
}

/// Whether a set reads the rows of its grain as they are, so that every
/// such set of one grain reads the same rows before its `where`, and one
/// pass can count them together: no source of its own, no relation, no
/// binding.
fn plain(set: &Set) -> bool {
    set.from.is_none()
        && set.of.is_none()
        && set.near.is_empty()
        && set.attach.is_empty()
        && set.has.is_empty()
        && set.same.is_empty()
        && set.every.is_empty()
        && set.pairs.is_none()
        && set.bind.0.is_empty()
        && set.pick.is_none()
        && !set.where_.iter().any(|c| {
            let mut all = Vec::new();
            c.walk(&mut all);
            all.iter()
                .any(|x| x.opts.contains_key("set") || x.opts.contains_key("over"))
        })
}

/// The most mixes of depths the sets counted together may have: the rows
/// of the pass's answer at most.
const TOGETHER: usize = 512;

thread_local! {
    /// The sets a pass counted on this thread, and those of them counted
    /// together with another.
    static PASSED: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// How many sets a pass has counted on this thread, and how many of them
/// together with another set: what a test reads to know that it held a
/// pass against the stages, and not the stages against themselves.
#[doc(hidden)]
pub fn sets_passed() -> (usize, usize) {
    PASSED.get()
}

/// A count level document without what its count does not read: the
/// bindings no clause of any of its sets names, by themselves or through
/// another binding, and then the sets nothing reads. A binding is kept
/// where its name is written anywhere else in the document, as a path, the
/// end of one or the start of one, which keeps more than are read and
/// never drops one that is. None where there is nothing to drop.
fn lean(ask: &Ask) -> Option<Ask> {
    let mut doc = ask.clone();
    let mut dropped = false;
    loop {
        let mut again = false;
        let names: Vec<(String, String)> = doc
            .sets
            .iter()
            .flat_map(|(s, set)| set.bind.names().map(move |b| (s.clone(), b.to_string())))
            .collect();
        for (s, b) in names {
            let mut without = doc.clone();
            if let Some(set) = without.sets.get_mut(&s) {
                set.bind.0.retain(|(n, _)| *n != b);
            }
            let Ok(text) = serde_json::to_string(&without) else {
                return None;
            };
            let read = [
                format!("\"{b}\""),
                format!(".{b}\""),
                format!("\"{b}."),
                format!(".{b}."),
            ]
            .iter()
            .any(|form| text.contains(form.as_str()));
            if !read {
                doc = without;
                dropped = true;
                again = true;
            }
        }
        if !again {
            break;
        }
    }
    if !dropped {
        return None;
    }
    // the sets nothing reads now
    loop {
        let read: BTreeSet<String> = doc
            .sets
            .values()
            .flat_map(|s| {
                s.reads()
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .chain([doc.out.set.clone()])
            .collect();
        let before = doc.sets.len();
        doc.sets.retain(|name, _| read.contains(name));
        if doc.sets.len() == before {
            break;
        }
    }
    Some(doc)
}

/// What one pass counted of a set: the stage before its first `where` and
/// the stage after each clause, and, for the answer's set, the rows each
/// clause's field is unknown on among the rows the clause met.
struct Pass {
    stages: Vec<Count>,
    /// By `where` clause: none where the pass did not count it.
    nulls: Vec<Option<Option<i64>>>,
}

/// A count as a count level statement shows it (record 55 K7): none where a
/// measure's rule holds fewer than [`MEASURE_K`] rows back, and none where
/// a quasi identifying field chose fewer than [`MEASURE_K`] of `who`, the
/// set's subjects, or its rows where it has no subjects.
fn shown(rows: i64, who: i64, quasi: bool, small: bool) -> Count {
    let held = (small && rows != 0 && rows < MEASURE_K) || (quasi && who != 0 && who < MEASURE_K);
    Count {
        rows: (!held).then_some(rows),
        subjects: (!held).then_some(who),
        quasi,
    }
}

fn case(when: Clause, then: Arg, otherwise: Arg) -> Clause {
    Clause::new("case")
        .arg(Arg::Clause(when))
        .arg(then)
        .arg(otherwise)
}

fn over(op: &str, set: &str, inner: Clause) -> Clause {
    Clause::new(op)
        .opt("set", Value::String(set.to_string()))
        .arg(Arg::Clause(inner))
}

struct Counter<'a> {
    runner: Runner<'a>,
    names: &'a dyn Names,
    scope: &'a Scope,
    /// The time the counts have as a whole, where the caller gave one.
    budget: Option<Budget>,
    /// Whether a stage's own statement is counted lean ([`lean`]); the
    /// reference path counts each as it is written.
    lean: bool,
    /// The statements listed and not run, or the answers already had
    /// ([`Ahead`]).
    ahead: &'a mut Ahead,
}

/// How the funnel's statements are had where a diagnosis may count several
/// at the same time ([`diagnose_beside`]). The diagnosis is gone through
/// twice. The first time nothing is run: every statement asked for is
/// listed, and answered with noughts, which takes the diagnosis down the
/// path it takes when every statement answers. The statements listed are
/// then run at the same time, each on a connection of its own. The second
/// time is the diagnosis as it always was, which finds the answer of a
/// statement it asks for among those already had, and runs any it does not
/// find: a statement the first time did not foresee is run where it was
/// always run, so what is shown is what a diagnosis one statement at a
/// time shows.
#[derive(Default)]
enum Ahead {
    /// One statement at a time, as before.
    #[default]
    None,
    /// The first time: the statements asked for, in the order asked.
    Listed(Vec<Compiled>),
    /// The second time: what the statements run ahead answered, by their
    /// text and parameters.
    Had(HashMap<(String, String), Had>),
}

/// What a statement run ahead came to.
enum Had {
    Answered(Box<Answer>),
    /// It ran past the time left for the counts.
    TimedOut,
}

fn key_of(c: &Compiled) -> (String, String) {
    (c.sql.clone(), format!("{:?}", c.params))
}

/// The answer of a statement that is listed and not run: one row of
/// noughts, wide enough for any count or pass to read.
fn noughts() -> Answer {
    Answer {
        columns: Vec::new(),
        rows: vec![nils_registry::store::Row(vec![
            nils_registry::store::Cell::Int(0);
            1024
        ])],
        truncated: false,
        content_hash: None,
        elapsed_ms: 0,
        shaped: Vec::new(),
        held_back: Vec::new(),
        held_counts: Vec::new(),
        rows_held: false,
    }
}

/// Run the statements listed, each on one of the connections, until all
/// are run or the time for the counts is spent. The reader's own
/// connection takes them from the first listed on, so that the stages
/// shown when the time runs out are the first ones; the connections
/// beside take them from the last listed back, since the last stages of a
/// funnel read every set before them and are the long ones, and a long
/// one started late is the one that does not end in time. A statement
/// that fails otherwise is not had, and the diagnosis runs it itself and
/// says what it says.
fn run_ahead(
    jobs: Vec<Compiled>,
    own: &mut Store,
    beside: &mut [Store],
    bounds: Bounds,
    budget: Option<(Instant, u64)>,
) -> HashMap<(String, String), Had> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut seen = BTreeSet::new();
    let jobs: Vec<Compiled> = jobs
        .into_iter()
        .filter(|c| seen.insert(key_of(c)))
        .collect();
    // the statements not yet taken: from `left.0` up to `left.1`
    let left: Mutex<(usize, usize)> = Mutex::new((0, jobs.len()));
    let spent = AtomicBool::new(false);
    let had: Mutex<HashMap<(String, String), Had>> = Mutex::new(HashMap::new());
    let work = |store: &mut Store, from_first: bool| {
        loop {
            let i = {
                let Ok(mut left) = left.lock() else { return };
                if left.0 >= left.1 || spent.load(Ordering::SeqCst) {
                    return;
                }
                if from_first {
                    left.0 += 1;
                    left.0 - 1
                } else {
                    left.1 -= 1;
                    left.1
                }
            };
            let mut b = bounds;
            if let Some((started, ms)) = budget {
                let left = ms.saturating_sub(started.elapsed().as_millis() as u64);
                if left == 0 {
                    spent.store(true, Ordering::SeqCst);
                    return;
                }
                b.timeout_ms = left.min(bounds.timeout_ms);
            }
            let came = match crate::exec::run(store, &jobs[i], b) {
                Ok(a) => Some(Had::Answered(Box::new(a))),
                Err(ExecError::Timeout(_)) if budget.is_some() => {
                    spent.store(true, Ordering::SeqCst);
                    Some(Had::TimedOut)
                }
                Err(_) => None,
            };
            if let (Some(came), Ok(mut had)) = (came, had.lock()) {
                had.insert(key_of(&jobs[i]), came);
            }
        }
    };
    std::thread::scope(|threads| {
        for store in beside.iter_mut() {
            threads.spawn(|| work(store, false));
        }
        work(own, true);
    });
    had.into_inner().unwrap_or_default()
}

/// The time a diagnosis has for all its counts together: each statement runs
/// under what is left of it, and once it is spent nothing more is counted.
struct Budget {
    started: Instant,
    ms: u64,
    /// The bound of one statement, which the time left never raises.
    statement_ms: u64,
    /// The part being counted, of [`PARTS`].
    part: usize,
    /// The part the time ran out in.
    spent_in: Option<usize>,
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
    /// One statement of the diagnosis, under what is left of its time: none
    /// once the time is spent, and none where the statement ran past it,
    /// which spends it. Without a time for the whole, the statement runs
    /// under its own bound and a timeout is the error it always was.
    fn answer(
        &mut self,
        registry: &mut Registry,
        ask: &Ask,
        validated: &Validated,
    ) -> Result<Option<(Compiled, Answer)>, RunError> {
        if let Some(b) = &mut self.budget {
            let left = b.ms.saturating_sub(b.started.elapsed().as_millis() as u64);
            if b.spent_in.is_some() || left == 0 {
                b.spent_in.get_or_insert(b.part);
                return Ok(None);
            }
            self.runner.bounds.timeout_ms = left.min(b.statement_ms);
        }
        let compiled = self.runner.compiled(registry, ask, validated, None, None)?;
        let had = match &mut *self.ahead {
            Ahead::None => None,
            Ahead::Listed(jobs) => {
                jobs.push(compiled.clone());
                return Ok(Some((compiled, noughts())));
            }
            Ahead::Had(had) => match had.get(&key_of(&compiled)) {
                Some(Had::Answered(a)) => Some(Ok((**a).clone())),
                Some(Had::TimedOut) => Some(Err(RunError::Exec(ExecError::Timeout(0)))),
                None => None,
            },
        };
        let answered = match had {
            Some(answered) => answered,
            None => self.runner.execute(registry, &compiled),
        };
        match answered.map(|a| (compiled, a)) {
            Ok(answered) => Ok(Some(answered)),
            Err(RunError::Exec(ExecError::Timeout(_))) if self.budget.is_some() => {
                if let Some(b) = &mut self.budget {
                    b.spent_in.get_or_insert(b.part);
                }
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Whether the time for the counts is spent: what is read after it is
    /// not a count and is not shown as one.
    fn spent(&self) -> bool {
        self.budget.as_ref().is_some_and(|b| b.spent_in.is_some())
    }

    /// The next part of [`PARTS`] is being counted.
    fn enter(&mut self, part: &str) {
        if let Some(b) = &mut self.budget {
            b.part = PARTS.iter().position(|p| *p == part).unwrap_or(b.part);
        }
    }

    /// What the time left uncounted: the part it ran out in and every part
    /// after it.
    fn unavailable(&self) -> Option<Unavailable> {
        let b = self.budget.as_ref()?;
        let from = b.spent_in?;
        Some(Unavailable {
            code: "timeout".to_string(),
            parts: PARTS[from..].iter().map(|p| p.to_string()).collect(),
            budget_ms: b.ms,
            why: format!(
                "the counts ran past {} ms and were stopped; the document is valid, and a job runs it under a job's time",
                b.ms
            ),
        })
    }

    /// The stages of a set from its last relation through every `where`
    /// clause, counted in one statement (2026-11-13). Counted a stage at a
    /// time, each stage is the whole set read again under one clause more,
    /// so a set of four clauses cost its question five times over. Here
    /// the set is read once without its `where`: a binding says how many
    /// of the clauses, in their order, each row meets (its depth), the rows
    /// are grouped by subject and the subjects by the deepest of their
    /// rows, and the stage after clause `i` is the rows of depth `i` or
    /// more and the subjects with such a row. For the answer's set the
    /// same pass counts, per clause, the rows its field is unknown on
    /// among the rows the clause stopped.
    ///
    /// `names` is the set, and after it the other sets the same read
    /// counts: sets of the same rows ([`plain`] sets of one grain), each
    /// with a depth of its own, so a document of three such sets reads
    /// the rows once and not three times.
    ///
    /// Every stage is the count its own statement gives: a row reaches a
    /// stage exactly where it meets every clause up to it, a clause it
    /// does not meet (false or unknown) ends its depth, and the bindings
    /// come before `where` in a set, so none of them reads fewer rows
    /// here. What a count level statement holds back is held back the
    /// same way, from each stage's own document as the caller's scope
    /// validates it ([`shown`]); the pass itself reads under a scope that
    /// holds nothing back and returns only the counts.
    ///
    /// None where the set is not one a pass can count ([`one_pass`]),
    /// where the pass's document does not validate or its statement fails
    /// (the stages are then counted one at a time, as before, which
    /// answers or fails as it always did), and where the time for the
    /// counts ran out, which the counter then says. A set after the first
    /// whose own stages do not validate is left out, to be refused in its
    /// own turn.
    fn pass(
        &mut self,
        registry: &mut Registry,
        ask: &Ask,
        names: &[String],
    ) -> Result<Option<Vec<(String, Pass)>>, RunError> {
        // The answer's set: the rows a clause's field is unknown on, among
        // the rows of the stage before the clause, as the drops count them.
        // Counted here where the clause holds the field itself against a
        // value (`=`, `in`, `has` or an order): a row whose field is
        // unknown never meets such a clause, so the unknown rows are among
        // those the clause stopped, and a row's depth says which those
        // are. Any other clause keeps its own count.
        struct Unknown {
            stage: usize,
            path: String,
            /// What its own document holds back; none where that document
            /// is refused, which the drops answer as no rows.
            flags: Option<(bool, bool)>,
        }
        struct Member {
            name: String,
            wheres: Vec<Clause>,
            /// What each stage's own document holds back.
            flags: Vec<(bool, bool)>,
            unknown: Vec<Unknown>,
            /// By clause, which of `unknown` counts it.
            asked: Vec<Option<usize>>,
            /// The binding that holds a row's depth in this set.
            depth: String,
        }
        let Some(host) = names.first() else {
            return Ok(None);
        };
        let Some(set) = ask.sets.get(host).filter(|s| one_pass(s)) else {
            return Ok(None);
        };
        let subjects = set.grain != Grain::Cohort;
        let mut taken: BTreeSet<String> = set
            .bind
            .names()
            .map(str::to_string)
            .chain(set.near.iter().map(|n| n.as_.clone()))
            .chain(set.attach.iter().map(|a| a.as_.clone()))
            .chain(set.has.iter().filter_map(|h| h.as_.clone()))
            .collect();
        let mut members: Vec<Member> = Vec::new();
        for (m, name) in names.iter().enumerate() {
            let Some(one) = ask.sets.get(name).filter(|s| one_pass(s)) else {
                return Ok(None);
            };
            let clauses = one.where_.len();
            // each stage's own document says what is held back of its count
            let mut flags: Vec<(bool, bool)> = Vec::with_capacity(clauses + 1);
            let mut refused = None;
            for step in std::iter::once(before_where(one)).chain((0..clauses).map(Step::Where)) {
                let v = variant(ask, name, &step, Level::Count);
                match validate(&v, self.names, self.scope) {
                    Ok(vv) => {
                        flags.push((vv.quasi_sets.contains(name), vv.small_cells.contains(name)));
                    }
                    Err(e) => {
                        refused = Some(e);
                        break;
                    }
                }
            }
            match refused {
                Some(e) if m == 0 => return Err(AskError::Invalid(e).into()),
                Some(_) => continue,
                None => {}
            }
            let mut unknown: Vec<Unknown> = Vec::new();
            let mut asked: Vec<Option<usize>> = vec![None; clauses];
            if *name == ask.out.set {
                let all = steps(one);
                for (w, c) in one.where_.iter().enumerate() {
                    let this = format!("where {}", clause_text(c));
                    let position = all.iter().position(|(_, l)| *l == this).unwrap_or(0);
                    let previous = position
                        .checked_sub(1)
                        .map(|q| all[q].0.clone())
                        .unwrap_or(Step::Source);
                    let mut paths = BTreeSet::new();
                    paths_in(c, &mut paths);
                    let (Some(p), Some(stage)) = (paths.iter().next(), stage_of(one, &previous))
                    else {
                        continue;
                    };
                    let stops = matches!(
                        c.op.as_str(),
                        "=" | "in" | "has" | ">" | ">=" | "<" | "<="
                    ) && matches!(c.args.first(), Some(Arg::Clause(f)) if f.op == "field" && f.ref_name() == Some(p.as_str()));
                    if !stops || stage >= clauses {
                        continue;
                    }
                    if let Some(k) = unknown.iter().position(|u| u.stage == stage) {
                        // the same clause written twice is counted once
                        asked[w] = (unknown[k].path == *p).then_some(k);
                        continue;
                    }
                    let mut v = variant(ask, name, &previous, Level::Count);
                    if let Some(s) = v.sets.get_mut(name) {
                        s.where_
                            .push(Clause::new("is_null").arg(Arg::Clause(Clause::field(p))));
                    }
                    let flags = validate(&v, self.names, self.scope)
                        .ok()
                        .map(|vv| (vv.quasi_sets.contains(name), vv.small_cells.contains(name)));
                    asked[w] = Some(unknown.len());
                    unknown.push(Unknown {
                        stage,
                        path: p.clone(),
                        flags,
                    });
                }
            }
            let mut depth = if m == 0 {
                "stage_depth".to_string()
            } else {
                format!("stage_depth_{m}")
            };
            while taken.contains(&depth) {
                depth.push('_');
            }
            taken.insert(depth.clone());
            members.push(Member {
                name: name.clone(),
                wheres: one.where_.clone(),
                flags,
                unknown,
                asked,
                depth,
            });
        }
        // The set without its `where`, each row with one number for each
        // set counted: its depth, the clauses it meets in their order
        // before the first it does not; or, where that clause's field is
        // unknown on the row and the drops ask, the depth written below
        // zero (`-depth - 1`). One binding read once, so each clause is
        // held against a row once.
        let mut doc = variant(ask, host, &Step::Where(set.where_.len() - 1), Level::Record);
        let (by_row, by_subject, by_depth) = (
            format!("{host}__rows"),
            format!("{host}__subjects"),
            format!("{host}__depths"),
        );
        if [&by_row, &by_subject, &by_depth]
            .iter()
            .any(|n| doc.sets.contains_key(*n))
        {
            return Ok(None);
        }
        {
            let s = doc.sets.get_mut(host).expect("the set of the pass");
            s.where_.clear();
            // only the bindings a clause reads, by itself or through
            // another: no binding chooses a row, so the rest count nothing
            let mut read: BTreeSet<String> = BTreeSet::new();
            for c in &members[0].wheres {
                paths_in(c, &mut read);
            }
            let reads = |read: &BTreeSet<String>, b: &str| {
                read.iter()
                    .any(|p| p == b || p.strip_prefix(b).is_some_and(|r| r.starts_with('.')))
            };
            loop {
                let before = read.len();
                for (b, c) in &s.bind.0 {
                    if reads(&read, b) {
                        paths_in(c, &mut read);
                    }
                }
                if read.len() == before {
                    break;
                }
            }
            s.bind.0.retain(|(b, _)| reads(&read, b));
            for member in &members {
                let mut met = Arg::Int(member.wheres.len() as i64);
                for (i, c) in member.wheres.iter().enumerate().rev() {
                    let stopped = match member.unknown.iter().find(|u| u.stage == i) {
                        Some(u) => {
                            let is_null =
                                Clause::new("is_null").arg(Arg::Clause(Clause::field(&u.path)));
                            Arg::Clause(case(
                                is_null,
                                Arg::Int(-(i as i64) - 1),
                                Arg::Int(i as i64),
                            ))
                        }
                        None => Arg::Int(i as i64),
                    };
                    met = Arg::Clause(case(c.clone(), met, stopped));
                }
                let Arg::Clause(met) = met else {
                    return Ok(None);
                };
                s.bind.0.push((member.depth.clone(), met));
            }
        }
        // the rows of each number, per subject; for a set with no subjects
        // that is the whole of the pass
        let depths = || members.iter().map(|m| Clause::field(&m.depth));
        let mut rows_of = Set::at(Grain::Group);
        rows_of.group = Some(GroupSpec {
            of: host.clone(),
            by: subjects
                .then(|| Clause::field("subject.id"))
                .into_iter()
                .chain(depths())
                .collect(),
        });
        doc.sets.insert(by_row.clone(), rows_of);
        let mut columns: Vec<Clause> = depths().collect();
        columns.push(Clause::field("_rows"));
        doc.out.set.clone_from(&by_row);
        if subjects {
            // per subject and set: its deepest row, its rows of each depth
            // or more, and its rows a clause's field is unknown on
            let real = |depth: &str| {
                let number = || Arg::Clause(Clause::field(depth));
                let below = Clause::new("-")
                    .arg(Arg::Clause(Clause::new("-").arg(Arg::Int(0)).arg(number())))
                    .arg(Arg::Int(1));
                case(
                    Clause::new(">=").arg(number()).arg(Arg::Int(0)),
                    number(),
                    Arg::Clause(below),
                )
            };
            let rows = || Arg::Clause(Clause::field("_rows"));
            let mut per = Set::at(Grain::Group);
            per.group = Some(GroupSpec {
                of: by_row.clone(),
                by: vec![Clause::field("subject.id")],
            });
            // and the subjects by the deepest of their rows in each set
            let mut outer = Set::at(Grain::Group);
            let sum = |of: &str| over("sum", &by_subject, Clause::field(of));
            columns = Vec::new();
            let mut counts: Vec<Clause> = Vec::new();
            let mut tops: Vec<Clause> = Vec::new();
            for (m, member) in members.iter().enumerate() {
                let top = format!("top{m}");
                per.bind
                    .0
                    .push((top.clone(), over("max", &by_row, real(&member.depth))));
                tops.push(Clause::field(&top));
                for j in 0..=member.wheres.len() {
                    let reached = Clause::new(">=")
                        .arg(Arg::Clause(real(&member.depth)))
                        .arg(Arg::Int(j as i64));
                    let n = format!("n{m}_{j}");
                    per.bind.0.push((
                        n.clone(),
                        over("sum", &by_row, case(reached, rows(), Arg::Int(0))),
                    ));
                    let total = format!("s{m}_{j}");
                    outer.bind.0.push((total.clone(), sum(&n)));
                    counts.push(Clause::field(&total));
                }
                for (k, u) in member.unknown.iter().enumerate() {
                    let is = Clause::new("=")
                        .arg(Arg::Clause(Clause::field(&member.depth)))
                        .arg(Arg::Int(-(u.stage as i64) - 1));
                    let z = format!("z{m}_{k}");
                    per.bind.0.push((
                        z.clone(),
                        over("sum", &by_row, case(is, rows(), Arg::Int(0))),
                    ));
                    let y = format!("y{m}_{k}");
                    outer.bind.0.push((y.clone(), sum(&z)));
                    counts.push(Clause::field(&y));
                    let any = Clause::new(">")
                        .arg(Arg::Clause(Clause::field(&z)))
                        .arg(Arg::Int(0));
                    let u = format!("u{m}_{k}");
                    outer.bind.0.push((
                        u.clone(),
                        over("sum", &by_subject, case(any, Arg::Int(1), Arg::Int(0))),
                    ));
                    counts.push(Clause::field(&u));
                }
            }
            outer.group = Some(GroupSpec {
                of: by_subject.clone(),
                by: tops.clone(),
            });
            columns.extend(tops);
            columns.push(Clause::field("_rows"));
            columns.extend(counts);
            doc.sets.insert(by_subject, per);
            doc.sets.insert(by_depth.clone(), outer);
            doc.out.set = by_depth;
        }
        doc.out.columns = columns;
        // the pass reads under a scope that holds nothing back; what each
        // stage shows is decided above, from its own document
        let whole = Scope {
            federated: false,
            classes: self
                .scope
                .classes
                .iter()
                .copied()
                .chain([Class::QuasiIdentifying, Class::Sensitive])
                .collect(),
            unsealed: self.scope.unsealed,
        };
        let Ok(vv) = validate(&doc, self.names, &whole) else {
            return Ok(None);
        };
        let answered = match self.answer(registry, &doc, &vv) {
            Ok(Some((_, a))) if !a.truncated && !a.rows_held => a,
            // the time ran out (the counter says so), or the statement
            // failed: nothing of the pass is a count
            _ => return Ok(None),
        };
        // by set: rows of depth `j` or more, and the subjects with such a
        // row; and the rows and subjects of each unknown field
        struct Sums {
            rows: Vec<i64>,
            who: Vec<i64>,
            null_rows: Vec<i64>,
            null_who: Vec<i64>,
        }
        let mut sums: Vec<Sums> = members
            .iter()
            .map(|m| Sums {
                rows: vec![0; m.wheres.len() + 1],
                who: vec![0; m.wheres.len() + 1],
                null_rows: vec![0; m.unknown.len()],
                null_who: vec![0; m.unknown.len()],
            })
            .collect();
        for r in &answered.rows {
            let cell = |i: usize| r.opt_int(2 + i).ok().flatten();
            let Some(n) = cell(members.len()) else {
                return Ok(None);
            };
            // the counts follow the depths and the rows, set by set
            let mut at = members.len() + 1;
            for (m, member) in members.iter().enumerate() {
                let Some(top) = cell(m) else {
                    return Ok(None);
                };
                let of = &mut sums[m];
                if subjects {
                    for j in 0..=member.wheres.len() {
                        let Some(reached) = cell(at) else {
                            return Ok(None);
                        };
                        at += 1;
                        of.rows[j] += reached;
                        if top >= j as i64 {
                            of.who[j] += n;
                        }
                    }
                    for k in 0..member.unknown.len() {
                        let (Some(y), Some(u)) = (cell(at), cell(at + 1)) else {
                            return Ok(None);
                        };
                        at += 2;
                        of.null_rows[k] += y;
                        of.null_who[k] += u;
                    }
                } else {
                    // a set with no subjects: each number with its rows
                    let real = if top >= 0 { top } else { -top - 1 };
                    for j in 0..=member.wheres.len() {
                        if real >= j as i64 {
                            of.rows[j] += n;
                            of.who[j] += n;
                        }
                    }
                    if let Some(k) = member
                        .unknown
                        .iter()
                        .position(|u| top == -(u.stage as i64) - 1)
                    {
                        of.null_rows[k] += n;
                        of.null_who[k] += n;
                    }
                }
            }
        }
        Ok(Some(
            members
                .iter()
                .zip(&sums)
                .map(|(member, of)| {
                    let nulls = member
                        .asked
                        .iter()
                        .map(|k| {
                            k.map(|k| match member.unknown[k].flags {
                                Some((quasi, small)) => {
                                    shown(of.null_rows[k], of.null_who[k], quasi, small).rows
                                }
                                None => Some(0),
                            })
                        })
                        .collect();
                    let stages = member
                        .flags
                        .iter()
                        .enumerate()
                        .map(|(j, (quasi, small))| shown(of.rows[j], of.who[j], *quasi, *small))
                        .collect();
                    (member.name.clone(), Pass { stages, nulls })
                })
                .collect(),
        ))
    }

    /// A count level variant's rows and subjects, as the ask answers them
    /// under the caller's scope.
    ///
    /// The statement that counts them is the variant without the bindings
    /// nothing in it reads, and without the sets nothing reads then
    /// ([`lean`]): no binding chooses a row, so the count is the same, and
    /// a stage that stops before the clause a binding was written for does
    /// not make it (a signature for every candidate of a pick, a hidden
    /// group for a `same` the stage has not reached). As in a pass, what
    /// the count shows is decided from the variant as it was written,
    /// validated under the caller's scope; the lean one is read under a
    /// scope that holds nothing back and the hold-back applied to what it
    /// counts ([`shown`]'s rule).
    fn count(&mut self, registry: &mut Registry, ask: &Ask) -> Result<Count, RunError> {
        let v = validate(ask, self.names, self.scope).map_err(AskError::Invalid)?;
        let quasi = v.quasi_sets.contains(&ask.out.set);
        if self.lean
            && let Some(count) = self.count_lean(registry, ask, &v)?
        {
            return Ok(count);
        }
        if self.spent() {
            return Ok(Count::default());
        }
        let Some((_, a)) = self.answer(registry, ask, &v)? else {
            return Ok(Count::default());
        };
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

    /// [`Counter::count`] by the lean statement; none where the variant
    /// has nothing to drop, is of a set whose count is not a plain count
    /// of rows and subjects, or where the lean one does not validate or
    /// answer, and then the variant is counted as it is written.
    fn count_lean(
        &mut self,
        registry: &mut Registry,
        ask: &Ask,
        written: &Validated,
    ) -> Result<Option<Count>, RunError> {
        let Some(set) = ask.sets.get(&ask.out.set) else {
            return Ok(None);
        };
        let plain = matches!(
            set.grain,
            Grain::Cohort
                | Grain::Subject
                | Grain::Session
                | Grain::Stack
                | Grain::Instance
                | Grain::Event
        ) && set.algebra.is_none();
        let Some(doc) = lean(ask).filter(|_| plain) else {
            return Ok(None);
        };
        let whole = Scope {
            federated: false,
            classes: self
                .scope
                .classes
                .iter()
                .copied()
                .chain([Class::QuasiIdentifying, Class::Sensitive])
                .collect(),
            unsealed: self.scope.unsealed,
        };
        let Ok(vv) = validate(&doc, self.names, &whole) else {
            return Ok(None);
        };
        let answered = match self.answer(registry, &doc, &vv) {
            Ok(Some((_, a))) if !a.truncated && !a.rows_held => a,
            _ => return Ok(None),
        };
        let Some(row) = answered.rows.first() else {
            return Ok(None);
        };
        let (Some(rows), Some(subjects)) = (row.opt_int(0)?, row.opt_int(1)?) else {
            return Ok(None);
        };
        let quasi = written.quasi_sets.contains(&ask.out.set);
        let small = written.small_cells.contains(&ask.out.set);
        let who = if set.grain == Grain::Cohort {
            rows
        } else {
            subjects
        };
        let held =
            (small && rows != 0 && rows < MEASURE_K) || (quasi && who != 0 && who < MEASURE_K);
        Ok(Some(Count {
            rows: (!held).then_some(rows),
            subjects: (!held).then_some(subjects),
            quasi,
        }))
    }

    /// The subject keys of a record level variant, bounded by the cap,
    /// none where a quasi identifying field chose fewer than
    /// [`crate::validate::MEASURE_K`] subjects below detail quasi, whose
    /// count is then held back as a count level variant's is.
    fn keys(&mut self, registry: &mut Registry, ask: &Ask) -> Result<(Count, Vec<i64>), RunError> {
        let v = validate(ask, self.names, self.scope).map_err(AskError::Invalid)?;
        let quasi = v.quasi_sets.contains(&ask.out.set);
        let Some((_, a)) = self.answer(registry, ask, &v)? else {
            return Ok((Count::default(), Vec::new()));
        };
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
        // a list held back whole (small lists at plain, the ruling of
        // 2026-10-13) read no row: its count is held, never none
        if a.rows_held || (quasi && subjects > 0 && subjects < crate::validate::MEASURE_K) {
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
    diagnose_within(
        registry, ask, repairs, names, scope, scheme, bounds, with_keys, reader, None,
    )
}

/// [`diagnose`] with a time for all its counts together, `budget_ms`
/// (2026-11-13). A funnel is a count for each stage of each set, and each
/// had the whole of `bounds.timeout_ms`, so a document whose statement is
/// slow took that time once per stage and then failed as a whole, valid or
/// not. With a budget, each statement runs under what is left of it, nothing
/// more is counted once it is spent, and the diagnosis says so in
/// `unavailable` rather than failing: whether a document is valid never
/// waits for a slow statement.
#[allow(clippy::too_many_arguments)]
pub fn diagnose_within(
    registry: &mut Registry,
    ask: Ask,
    repairs: Vec<Repair>,
    names: &dyn Names,
    scope: &Scope,
    scheme: &Scheme,
    bounds: Bounds,
    with_keys: bool,
    reader: Option<&mut Store>,
    budget_ms: Option<u64>,
) -> Result<Diagnosis, RunError> {
    diagnose_as(
        registry,
        ask,
        repairs,
        names,
        scope,
        scheme,
        bounds,
        with_keys,
        reader,
        budget_ms,
        true,
        &mut [],
    )
}

/// [`diagnose_within`] with connections to count beside the reader's own
/// (2026-11-14). The stages of a set through its `where` are one
/// statement, and every stage after them is a statement of its own that
/// makes the sets it reads again: a question of two picks, two attaches,
/// a `has` and a `same` is a dozen statements that each make the picks,
/// 15 s one after another where the question itself takes 3. They do not
/// depend on each other, so here they are run at the same time, one on
/// each of `beside` and one on the reader, and the diagnosis reads what
/// they answered ([`Ahead`]): the same statements, the same numbers. The
/// time for the counts is the time on the clock. With no connection
/// beside it is [`diagnose_within`].
#[allow(clippy::too_many_arguments)]
pub fn diagnose_beside(
    registry: &mut Registry,
    ask: Ask,
    repairs: Vec<Repair>,
    names: &dyn Names,
    scope: &Scope,
    scheme: &Scheme,
    bounds: Bounds,
    with_keys: bool,
    reader: Option<&mut Store>,
    budget_ms: Option<u64>,
    beside: &mut [Store],
) -> Result<Diagnosis, RunError> {
    diagnose_as(
        registry, ask, repairs, names, scope, scheme, bounds, with_keys, reader, budget_ms, true,
        beside,
    )
}

/// [`diagnose_within`] with every stage counted by a statement of its own,
/// as all were before a set's `where` stages were counted in one pass
/// (`Counter::pass`): what the pass is held to, stage for stage, by the
/// tests and by the scale fixture.
#[allow(clippy::too_many_arguments)]
pub fn diagnose_by_stage(
    registry: &mut Registry,
    ask: Ask,
    repairs: Vec<Repair>,
    names: &dyn Names,
    scope: &Scope,
    scheme: &Scheme,
    bounds: Bounds,
    with_keys: bool,
    reader: Option<&mut Store>,
    budget_ms: Option<u64>,
) -> Result<Diagnosis, RunError> {
    diagnose_as(
        registry,
        ask,
        repairs,
        names,
        scope,
        scheme,
        bounds,
        with_keys,
        reader,
        budget_ms,
        false,
        &mut [],
    )
}

#[allow(clippy::too_many_arguments)]
fn diagnose_as(
    registry: &mut Registry,
    ask: Ask,
    repairs: Vec<Repair>,
    names: &dyn Names,
    scope: &Scope,
    scheme: &Scheme,
    bounds: Bounds,
    with_keys: bool,
    mut reader: Option<&mut Store>,
    budget_ms: Option<u64>,
    in_one_pass: bool,
    beside: &mut [Store],
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
    // Where there are connections to count beside this one, the statements
    // are listed first, run at the same time, and the diagnosis then reads
    // what they answered ([`Ahead`]). A diagnosis that lists keys counts
    // one statement at a time, and so does the reference path.
    let started = Instant::now();
    let mut ahead = Ahead::None;
    if in_one_pass && !with_keys && !beside.is_empty() {
        let mut listed = Ahead::Listed(Vec::new());
        // what the listing comes to is not the diagnosis: it read noughts
        let _ = counted(
            registry,
            &ask,
            &validated,
            repairs.clone(),
            warnings.clone(),
            cost.clone(),
            names,
            scope,
            scheme,
            bounds,
            with_keys,
            lend(&mut reader),
            None,
            in_one_pass,
            &mut listed,
        );
        if let Ahead::Listed(jobs) = listed {
            let own: &mut Store = match lend(&mut reader) {
                Some(r) => r,
                None => registry.store(),
            };
            ahead = Ahead::Had(run_ahead(
                jobs,
                own,
                beside,
                bounds,
                budget_ms.map(|ms| (started, ms)),
            ));
        }
    }
    counted(
        registry,
        &ask,
        &validated,
        repairs,
        warnings,
        cost,
        names,
        scope,
        scheme,
        bounds,
        with_keys,
        reader,
        budget_ms.map(|ms| (started, ms)),
        in_one_pass,
        &mut ahead,
    )
}

/// A reborrow of the door's reader for one more use.
#[allow(clippy::needless_option_as_deref)]
fn lend<'b>(reader: &'b mut Option<&mut Store>) -> Option<&'b mut Store> {
    reader.as_deref_mut()
}

/// The counts of a diagnosis whose document is valid: the funnel, the
/// drops, the ties and the coarse dates.
#[allow(clippy::too_many_arguments)]
fn counted(
    registry: &mut Registry,
    ask: &Ask,
    validated: &Validated,
    repairs: Vec<Repair>,
    warnings: Vec<Issue>,
    cost: Cost,
    names: &dyn Names,
    scope: &Scope,
    scheme: &Scheme,
    bounds: Bounds,
    with_keys: bool,
    reader: Option<&mut Store>,
    budget: Option<(Instant, u64)>,
    in_one_pass: bool,
    ahead: &mut Ahead,
) -> Result<Diagnosis, RunError> {
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
        lean: in_one_pass,
        ahead,
        budget: budget.map(|(started, ms)| Budget {
            started,
            ms,
            statement_ms: bounds.timeout_ms,
            part: 0,
            spent_in: None,
        }),
    };
    // the funnel, set by set in topological order, stage by stage
    let mut funnel = Vec::new();
    let mut stage_counts: BTreeMap<(String, String), (Option<i64>, Option<i64>)> = BTreeMap::new();
    // the sets whose last stage is held back: a set that narrows one, or
    // reads below it, starts from the same rows and holds its source too
    let mut last_held: BTreeSet<String> = BTreeSet::new();
    // what a pass over the answer's set counted of the rows each of its
    // clauses' fields is unknown on, by clause
    let mut out_nulls: Vec<Option<Option<i64>>> = Vec::new();
    // what a pass counted of each set, none where it counted nothing; and
    // whether sets of the same rows are still counted together
    let mut passed: BTreeMap<String, Option<Pass>> = BTreeMap::new();
    let mut alone = false;
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
            // the stages one pass counts, counted when the first of them
            // is reached, so the stages before it are counted first; with
            // them the stages of every later set of the same rows
            let stage = stage_of(set, &step).filter(|_| in_one_pass && !with_keys);
            if stage.is_some() && !passed.contains_key(name) {
                let mut together = vec![name.clone()];
                if !alone && plain(set) {
                    // the pass answers a row for each mix of depths its
                    // subjects have, at most the product of the sets'
                    // stages: sets are added while that stays a short
                    // answer, and the rest are counted by a later pass
                    let mut mixes = set.where_.len() + 1;
                    for other in &validated.order {
                        let Some(o) = ask.sets.get(other) else {
                            continue;
                        };
                        if other == name
                            || other.contains("__")
                            || passed.contains_key(other)
                            || o.grain != set.grain
                            || !plain(o)
                            || !one_pass(o)
                            || mixes * (o.where_.len() + 1) > TOGETHER
                        {
                            continue;
                        }
                        mixes *= o.where_.len() + 1;
                        together.push(other.clone());
                    }
                }
                let mut counted = counter.pass(registry, ask, &together)?;
                if counted.is_none() && together.len() > 1 && !counter.spent() {
                    // the sets do not count together: each by itself
                    alone = true;
                    together.truncate(1);
                    counted = counter.pass(registry, ask, &together)?;
                }
                passed.insert(name.clone(), None);
                let counted = counted.unwrap_or_default();
                let (sets, shared) = PASSED.get();
                PASSED.set((
                    sets + counted.len(),
                    shared + if counted.len() > 1 { counted.len() } else { 0 },
                ));
                for (of, pass) in counted {
                    if of == ask.out.set {
                        out_nulls.clone_from(&pass.nulls);
                    }
                    passed.insert(of, Some(pass));
                }
            }
            let counted = stage.and_then(|j| passed.get(name)?.as_ref()?.stages.get(j).copied());
            let (mut count, mut keys) = if let Some(count) = counted {
                (count, Vec::new())
            } else if with_keys {
                let v = variant(ask, name, &step, Level::Record);
                counter.keys(registry, &v)?
            } else {
                let v = variant(ask, name, &step, Level::Count);
                (counter.count(registry, &v)?, Vec::new())
            };
            // the time for the counts is spent: this stage and every one
            // after it is not counted, and is not a stage of the funnel
            if counter.spent() {
                break;
            }
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
    counter.enter("drops");
    let mut drops = Vec::new();
    if let Some(out_set) = ask.sets.get(&ask.out.set)
        && out_set.grain != Grain::Group
    {
        let all = steps(out_set);
        for (w, c) in out_set.where_.iter().enumerate() {
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
                Some(_) if out_nulls.get(w).is_some_and(Option::is_some) => {
                    out_nulls[w].unwrap_or(Some(0))
                }
                Some(p) => {
                    // the set as it stands before this clause, plus "the
                    // field is unknown"
                    let previous = position
                        .checked_sub(1)
                        .map(|q| all[q].0.clone())
                        .unwrap_or(Step::Source);
                    let mut v = variant(ask, &ask.out.set, &previous, Level::Count);
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
    counter.enter("ties");
    let mut ties = Vec::new();
    for (name, set) in &ask.sets {
        if name.contains("__") || set.pick.is_none() {
            continue;
        }
        let mut v = keys_level(ask, name);
        for r in readers_of(ask, name) {
            v.sets.remove(&r);
        }
        v.out.columns = vec![Clause::field("pick.tied")];
        let vv = validate(&v, names, scope).map_err(AskError::Invalid)?;
        let quasi = vv.quasi_sets.contains(name);
        let Some((_, a)) = counter.answer(registry, &v, &vv)? else {
            break;
        };
        let tied = a
            .rows
            .iter()
            .filter(|r| r.opt_int(2).ok().flatten() == Some(1))
            .count() as i64;
        // record 55 K7, small counts at plain: a count of the rows a quasi
        // identifying field chose
        let small = a.rows_held || (quasi && tied > 0 && tied < crate::validate::MEASURE_K);
        ties.push((name.clone(), (!small).then_some(tied)));
    }
    // rows coarser than a day, per dated set with a precision
    counter.enter("coarse");
    let mut coarse = Vec::new();
    for name in &validated.order {
        let Some(set) = ask.sets.get(name) else {
            continue;
        };
        if name.contains("__") || set.grain != Grain::Event {
            continue;
        }
        let mut v = count_level(ask, name);
        for r in readers_of(ask, name) {
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
    // what the time left uncounted is said, and a part it cut is left empty
    // rather than shown half counted; the funnel keeps its counted stages
    let unavailable = counter.unavailable();
    if let Some(u) = &unavailable {
        let cut = |part: &str| u.parts.iter().any(|p| p == part);
        if cut("drops") {
            drops.clear();
        }
        if cut("ties") {
            ties.clear();
        }
        if cut("coarse") {
            coarse.clear();
        }
        next.push(job_next(None));
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
        unavailable,
    })
}

/// The next call where the counts ran past their time: the job path, by the
/// document's id where it is stored.
pub fn job_next(document: Option<i64>) -> String {
    match document {
        Some(id) => {
            format!("POST /api/ask/jobs with document_id {id} runs it as a job, under a job's time")
        }
        None => "POST /api/ask/jobs runs the document as a job, under a job's time".to_string(),
    }
}

impl Diagnosis {
    /// The same diagnosis of a document stored as `document`: where the
    /// counts ran past their time, the next call names it.
    pub fn of_stored(mut self, document: i64) -> Diagnosis {
        if self.unavailable.is_some() {
            let unnamed = job_next(None);
            self.next.retain(|n| *n != unnamed);
            self.next.push(job_next(Some(document)));
        }
        self
    }
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
