// SPDX-License-Identifier: AGPL-3.0-only

//! A pipeline's proposals (record 43 S6, study section 3.4): what a model
//! says about the stacks it saw, as review items and staged decisions.
//!
//! A run's `results.json` carries `proposals`
//! (`contracts/job/v1/proposals.schema.json`): per stack, an axis, the
//! value proposed, the probability of every class, and the registered
//! model that answered. [`ingest`] groups them into `<axis>:model` review
//! items, one grouped question per axis, model, value and confidence band,
//! the way the review spine groups a classifier run's questions
//! ([`crate::review::group_run`]), each stack a member with its
//! probabilities as its evidence.
//!
//! The bands split at the model's threshold and, above it, at
//! [`BANDS`]. The threshold is the model card's (`threshold`,
//! `contracts/model/v1`); whoever runs the model may raise it for a run and
//! never lower it ([`threshold`]), and a model whose card names none has
//! every proposal asked as a review item. A group at or above the threshold gets a staged group
//! decision authored by the model (`author_kind` model, its `model_id`),
//! written through the one write path, [`crate::review::apply_within`], so
//! that it is refused for a model that is not admitted or promoted, or
//! registered for another task. Nothing is in force: a model's answer is
//! evidence until a person commits it (record 42 R6), by id, or by filter
//! with `nils review commit --min-confidence`, which reads the group's
//! `confidence`, the lowest of its members'. A group below the threshold
//! is a review item and nothing more, open for a person or a campaign.
//!
//! A stack whose axis a person or an agent has decided, with the decision
//! in force, is not asked again: it is counted, and left out. Record 51
//! R5: where the decision is a person's and the model proposes another
//! value at or above its threshold, the disagreement is shown, never
//! applied: an `<axis>:decision` item on the stack, the kind that already
//! says "a decision stands and something disagrees", with the model, its
//! value and its confidence as the evidence. Nothing is staged and the
//! person's decision stays in force. There is one open such item per stack,
//! axis and model; a newer run of the model refreshes it, or closes it as
//! superseded where it no longer disagrees. Below the threshold the
//! proposal is only counted.
//!
//! A newer run of a model supersedes what its earlier runs proposed on the
//! same axis, on the stacks it proposes again, and nobody took: their
//! `<axis>:model` items still open or staged are closed as `superseded`,
//! and the staged decisions nobody committed are withdrawn, so stale
//! proposals do not pile up. A stack the new run does not propose keeps its
//! earlier proposal: where it shared a group with stacks proposed again, it
//! is carried into an item of its own, staged again where the group was. A
//! decision a person committed stays, and so does an item an unfinished
//! campaign asks.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::Registry;
use crate::labels::{self, DecisionQuery};
use crate::model::{self, Model};
use crate::review::{self, Apply, Author};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

/// The edges above the threshold where one band ends and the next starts,
/// so that a commit by minimum confidence can take the surest part of a
/// value without the rest. An edge at or below the threshold is not one.
pub const BANDS: [f64; 3] = [0.9, 0.95, 0.99];

/// How far a stack's probabilities may sum from one.
pub const SUM_TOLERANCE: f64 = 0.01;

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    /// The proposals are not what the contract says: the run's fault.
    Invalid(String),
    /// Well formed and not allowed as things are: a run already ingested,
    /// a stack or a model that is not there.
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Invalid(m) | Error::Refused(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

impl From<review::Error> for Error {
    fn from(e: review::Error) -> Error {
        match e {
            review::Error::Store(s) => Error::Store(s),
            review::Error::Refused(m) | review::Error::Forbidden(m) => Error::Refused(m),
        }
    }
}

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

/// One proposal, as `results.json` carries it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub stack_id: i64,
    pub axis: String,
    pub value: String,
    /// Every class the model weighed, with its probability.
    pub probabilities: BTreeMap<String, f64>,
    /// The registered model, by the id the runner's manifest gave it.
    #[serde(default)]
    pub model_id: Option<i64>,
    /// Or by its digest, which the image can compute from what it loaded.
    #[serde(default)]
    pub model_digest: Option<String>,
    /// Words a person reads beside the numbers, such as a rule the image
    /// applied after the model (v0's axial brain-neck rule).
    #[serde(default)]
    pub note: Option<String>,
}

impl Proposal {
    /// The probability of the value proposed.
    pub fn confidence(&self) -> f64 {
        self.probabilities.get(&self.value).copied().unwrap_or(0.0)
    }
}

/// The proposals of a whole `results.json`: none when it carries none.
pub fn parse(results: &Value) -> Result<Vec<Proposal>, Error> {
    match results.get("proposals") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(p) => serde_json::from_value(p.clone())
            .map_err(|e| invalid(format!("the results' proposals are not proposals: {e}"))),
    }
}

/// The run the proposals came from.
#[derive(Debug, Clone)]
pub struct Run<'a> {
    /// The pipeline run's id.
    pub id: i64,
    /// The job that ran it, which the review items name.
    pub job_id: Option<i64>,
    /// Who the model's decisions are written by: the principal the run was
    /// started for.
    pub principal: &'a str,
    /// The stacks of the run's selection; a proposal on another stack is
    /// dropped and counted, never written. None takes every stack (a caller
    /// with no selection, such as an import).
    pub stacks: Option<&'a BTreeSet<i64>>,
    /// The models the run may speak for: those it was given and those it
    /// registered itself. A proposal by another is dropped and counted.
    pub models: Option<&'a BTreeSet<i64>>,
}

/// One group written.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub item: i64,
    pub axis: String,
    pub value: String,
    pub model_id: i64,
    /// `below`, or `p>=<edge>` for a band at or above the threshold.
    pub band: String,
    pub members: i64,
    /// The lowest probability of the value among the members.
    pub confidence: f64,
    /// The staged decision, for a group at or above the threshold whose
    /// model answers the axis.
    pub staged: Option<i64>,
}

/// Record 51 R5: a model that disagrees with a person's decision, at or
/// above its threshold, raised as an `<axis>:decision` item.
#[derive(Debug, Clone, PartialEq)]
pub struct Disagreement {
    /// The review item, raised or refreshed.
    pub item: i64,
    pub stack_id: i64,
    pub axis: String,
    /// The person's decision in force, or none where they decided nothing.
    pub decision: Option<String>,
    /// What the model proposes instead, and how sure it is.
    pub value: String,
    pub model_id: i64,
    pub confidence: f64,
    /// Whether an earlier run's open item was refreshed rather than a new
    /// one raised.
    pub refreshed: bool,
}

/// A decision in force on a stack's axis, as the ingest reads it.
#[derive(Debug, Clone)]
struct Decided {
    value: Option<String>,
    person: bool,
    decision_id: Option<i64>,
    actor: String,
}

impl Decided {
    /// Whether the decision holds `value`, one of a multi-valued axis's
    /// values included.
    fn holds(&self, value: &str) -> bool {
        self.value
            .as_deref()
            .is_some_and(|v| v.split(',').any(|x| x.trim() == value))
    }
}

/// What a run proposed on a stack a person decided, before it is written.
#[derive(Debug, Clone)]
struct Against {
    stack_id: i64,
    axis: String,
    model_id: i64,
    /// The proposal's evidence, when it disagrees at or above the
    /// threshold; none when it agrees or falls below, which closes an
    /// earlier run's item of the model on the stack.
    evidence: Option<Value>,
    decision: Option<String>,
    value: String,
    confidence: f64,
}

/// What an ingest did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ingested {
    pub groups: Vec<Group>,
    /// Record 48, D1 of the move: proposals left out because their stack
    /// is of a sample sealed now, which never becomes a review item.
    pub sealed: i64,
    /// Members under the groups.
    pub members: i64,
    /// Members of staged groups.
    pub staged_members: i64,
    /// Proposals left out because a person's or an agent's decision on
    /// the axis is in force on the stack.
    pub decided: i64,
    /// Record 51 R5: of those, the proposals that disagree with a person's
    /// decision at or above the model's threshold, each raised as (or
    /// refreshing) an `<axis>:decision` item, never staged.
    pub disagreements: Vec<Disagreement>,
    /// The `<axis>:decision` items of the model's earlier runs closed as
    /// superseded because this run no longer disagrees on their stack.
    pub agreed_again: i64,
    /// Why a group at or above the threshold was not staged, once per
    /// model: not admitted or promoted, or registered for another task.
    pub not_staged: Vec<String>,
    /// The items of the model's earlier runs on the axis closed as
    /// superseded, and the staged decisions of theirs withdrawn.
    pub superseded: i64,
    pub withdrawn: i64,
    /// Items made for the members of a superseded group the run did not
    /// propose again, which keep their earlier proposal.
    pub carried: i64,
    /// Proposals dropped because their stack is not the run's or their
    /// model is not one the run was given or made, and why, once per kind.
    pub out_of_run: i64,
    pub out_of_run_why: Vec<String>,
}

impl Ingested {
    pub fn to_json(&self) -> Value {
        json!({
            "items": self.groups.len(),
            "members": self.members,
            "staged_members": self.staged_members,
            "staged_decisions": self.groups.iter().filter(|g| g.staged.is_some()).count(),
            "decided": self.decided,
            "disagreements": self.disagreements.iter().map(|d| json!({
                "item": d.item, "stack_id": d.stack_id, "axis": d.axis, "decision": d.decision,
                "value": d.value, "model_id": d.model_id, "confidence": d.confidence,
                "refreshed": d.refreshed,
            })).collect::<Vec<_>>(),
            "agreed_again": self.agreed_again,
            "sealed": self.sealed,
            "not_staged": self.not_staged,
            "superseded": self.superseded,
            "withdrawn": self.withdrawn,
            "carried": self.carried,
            "out_of_run": self.out_of_run,
            "out_of_run_why": self.out_of_run_why,
            "groups": self.groups.iter().map(|g| json!({
                "item": g.item, "axis": g.axis, "value": g.value, "model_id": g.model_id,
                "band": g.band, "members": g.members, "confidence": g.confidence,
                "staged": g.staged,
            })).collect::<Vec<_>>(),
        })
    }
}

fn axis_ok(axis: &str) -> bool {
    !axis.is_empty() && axis.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// The band a probability falls in.
pub fn band(p: f64, threshold: f64) -> String {
    if p < threshold {
        return "below".into();
    }
    let edge = BANDS
        .iter()
        .copied()
        .filter(|e| *e > threshold && p >= *e)
        .fold(threshold, f64::max);
    format!("p>={edge}")
}

/// Check every proposal before anything is written, and resolve its model.
fn checked(store: &mut Store, proposals: &[Proposal]) -> Result<Vec<(usize, Model)>, Error> {
    let mut models: BTreeMap<String, Model> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(proposals.len());
    for (i, p) in proposals.iter().enumerate() {
        let at = format!("proposals[{i}]");
        if !axis_ok(&p.axis) {
            return Err(invalid(format!(
                "{at}: {:?} is not an axis: lowercase letters and underscores",
                p.axis
            )));
        }
        if p.value.trim().is_empty() {
            return Err(invalid(format!("{at}: the value is empty")));
        }
        if p.probabilities.is_empty() {
            return Err(invalid(format!(
                "{at}: a proposal carries the probability of every class"
            )));
        }
        for (class, q) in &p.probabilities {
            if !q.is_finite() || !(0.0..=1.0).contains(q) {
                return Err(invalid(format!(
                    "{at}: the probability of {class} is {q}, not one between 0 and 1"
                )));
            }
        }
        let sum: f64 = p.probabilities.values().sum();
        if (sum - 1.0).abs() > SUM_TOLERANCE {
            return Err(invalid(format!(
                "{at}: the probabilities sum to {sum:.4}, not 1"
            )));
        }
        if !p.probabilities.contains_key(&p.value) {
            return Err(invalid(format!(
                "{at}: the value {} is not among the classes weighed",
                p.value
            )));
        }
        let key = match (p.model_id, p.model_digest.as_deref()) {
            (Some(id), _) => id.to_string(),
            (None, Some(d)) => d.to_string(),
            (None, None) => {
                return Err(invalid(format!(
                    "{at}: a proposal names its model, by model_id or model_digest"
                )));
            }
        };
        let m = match models.get(&key) {
            Some(m) => m.clone(),
            None => {
                let m = model::resolve(store, &key)?.ok_or_else(|| {
                    Error::Refused(format!("{at}: no registered model answers to {key}"))
                })?;
                models.insert(key, m.clone());
                m
            }
        };
        if let Some(d) = p.model_digest.as_deref()
            && d != m.digest
        {
            return Err(invalid(format!(
                "{at}: model {} has the digest {}, not {d}",
                m.id, m.digest
            )));
        }
        if !seen.insert((p.stack_id, p.axis.clone(), m.id)) {
            return Err(invalid(format!(
                "{at}: stack {} has a second proposal of model {} on {}",
                p.stack_id, m.id, p.axis
            )));
        }
        out.push((i, m));
    }
    // every stack is one the registry holds
    let stacks: BTreeSet<i64> = proposals.iter().map(|p| p.stack_id).collect();
    let ids: Vec<i64> = stacks.iter().copied().collect();
    let mut found = BTreeSet::new();
    for chunk in ids.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id FROM {} WHERE id IN ({list})",
            store.qualified("stack")
        );
        for r in store.query(&sql, &[])? {
            found.insert(r.int(0)?);
        }
    }
    if let Some(missing) = stacks.difference(&found).next() {
        return Err(Error::Refused(format!(
            "stack {missing} is not in the registry"
        )));
    }
    Ok(out)
}

/// Why a model's confident group is not staged, or nothing when it is.
fn why_not(m: &Model, axis: &str) -> Option<String> {
    if !m.answers() {
        return Some(format!(
            "model {} ({}) is {}; only an admitted or promoted model's answer is staged",
            m.id,
            m.label(),
            m.state
        ));
    }
    let task = format!("axis:{axis}");
    (m.task != task).then(|| {
        format!(
            "model {} ({}) answers {}, not {task}",
            m.id,
            m.label(),
            m.task
        )
    })
}

/// A group being gathered: the members' stacks, probabilities and evidence.
#[derive(Default)]
struct Gathered {
    members: Vec<(i64, f64, Value)>,
}

/// The threshold a model's proposals are staged at in a run: its card's,
/// or a higher one the caller asks for. Refused for a caller's value that
/// is not a probability or is below the card's. None when the card names
/// none: then nothing of the model is staged, whatever the caller asks,
/// since a caller only raises the card's threshold.
pub fn threshold(m: &Model, asked: Option<f64>) -> Result<Option<f64>, Error> {
    if let Some(a) = asked
        && (!a.is_finite() || a <= 0.0 || a > 1.0)
    {
        return Err(invalid(format!(
            "the threshold asked is {a}, and a threshold is a probability above 0 and at most 1"
        )));
    }
    let Some(card) = m.threshold() else {
        return Ok(None);
    };
    match asked {
        Some(a) if a < card => Err(Error::Refused(format!(
            "model {} ({}) stages its proposals at {card}, as its card says; a run may raise that threshold, not lower it to {a}",
            m.id,
            m.label()
        ))),
        Some(a) => Ok(Some(a)),
        None => Ok(Some(card)),
    }
}

/// How many review items hold a run's own proposals: its grouped
/// `<axis>:model` items, keyed `run:<id>|<axis>:model|...`, with those
/// carried under its key when a later run superseded part of a group.
/// A unit's `pipeline:qc` item, keyed `run:<id>|unit:<unit>`, is the run's
/// too but holds no proposal, so it is never counted.
pub fn held(store: &mut Store, run_id: i64) -> Result<i64, Error> {
    let d = store.dialect();
    Ok(store
        .query_opt(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE group_key LIKE {} AND kind LIKE '%:model' \
                 AND group_key LIKE {} || kind || '|%'",
                store.qualified("review_item"),
                d.param(1, Type::Text),
                d.param(2, Type::Text)
            ),
            &[
                Param::from(format!("run:{run_id}|%")),
                Param::from(format!("run:{run_id}|")),
            ],
        )?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0))
}

/// Turn a run's proposals into grouped `<axis>:model` review items, and
/// stage a decision by the model on each group at or above the model's
/// threshold ([`threshold`]: its card's, or higher where `asked` raises
/// it), all in one transaction: every item and decision is written, or
/// none. A run is ingested once; a second ingest of it is refused, as
/// [`held`] counts it. The items its failed units raised do not count.
pub fn ingest(
    registry: &mut Registry,
    run: &Run<'_>,
    proposals: &[Proposal],
    asked: Option<f64>,
) -> Result<Ingested, Error> {
    let held = held(registry.store(), run.id)?;
    if held > 0 {
        return Err(Error::Refused(format!(
            "the proposals of run {} are in already, as {held} review item(s)",
            run.id
        )));
    }
    let store = registry.store();
    let resolved = checked(store, proposals)?;
    // only the run's own stacks, and only the models it may speak for
    let mut dropped: BTreeMap<String, i64> = BTreeMap::new();
    let resolved: Vec<(usize, Model)> = resolved
        .into_iter()
        .filter(|(i, m)| {
            let p = &proposals[*i];
            let why = if run.stacks.is_some_and(|s| !s.contains(&p.stack_id)) {
                Some("a stack outside the run's selection".to_string())
            } else if run.models.is_some_and(|s| !s.contains(&m.id)) {
                Some(format!(
                    "model {} ({}), which the run was neither given nor made",
                    m.id,
                    m.label()
                ))
            } else {
                None
            };
            if let Some(w) = why {
                *dropped.entry(w).or_default() += 1;
                return false;
            }
            true
        })
        .collect();
    // each model's threshold, refused before anything is written
    let mut thresholds: BTreeMap<i64, Option<f64>> = BTreeMap::new();
    for (_, m) in &resolved {
        if let std::collections::btree_map::Entry::Vacant(e) = thresholds.entry(m.id) {
            e.insert(threshold(m, asked)?);
        }
    }
    // the stacks a person or an agent decided, per axis, with the decision
    let mut decided: BTreeMap<String, BTreeMap<i64, Decided>> = BTreeMap::new();
    let axes: BTreeSet<&str> = proposals.iter().map(|p| p.axis.as_str()).collect();
    let persons = ["person".to_string(), "agent".to_string()];
    for axis in axes {
        let stacks: Vec<i64> = proposals
            .iter()
            .filter(|p| p.axis == axis)
            .map(|p| p.stack_id)
            .collect();
        let labels = labels::decision_labels(
            store,
            &DecisionQuery {
                axis,
                stacks: Some(&stacks),
                authors: &persons,
                campaign: None,
                staged_too: false,
            },
        )
        .map_err(|e| match e {
            labels::Error::Store(s) => Error::Store(s),
            other => Error::Refused(other.to_string()),
        })?;
        decided.insert(
            axis.to_string(),
            labels
                .iter()
                .filter_map(|l| {
                    l.stack_id.map(|s| {
                        (
                            s,
                            Decided {
                                value: l.value.clone(),
                                person: l.author_kind == "person",
                                decision_id: l.decision_id,
                                actor: l.author.clone(),
                            },
                        )
                    })
                })
                .collect(),
        );
    }
    let mut out = Ingested {
        out_of_run: dropped.values().sum(),
        out_of_run_why: dropped
            .iter()
            .map(|(w, n)| format!("{n} proposal(s) on {w}"))
            .collect(),
        ..Ingested::default()
    };
    // the stacks the run proposes again, per axis and model, which is what
    // it supersedes of the model's earlier runs
    let mut again: BTreeMap<(String, i64), BTreeSet<i64>> = BTreeMap::new();
    for (i, m) in &resolved {
        let p = &proposals[*i];
        again
            .entry((p.axis.clone(), m.id))
            .or_default()
            .insert(p.stack_id);
    }
    // record 45: what each stack holds now on each axis, before the
    // proposal, so the change a proposal would make reads from and to
    let mut before: BTreeMap<String, BTreeMap<i64, Vec<String>>> = BTreeMap::new();
    for (i, _) in &resolved {
        let p = &proposals[*i];
        before
            .entry(p.axis.clone())
            .or_default()
            .insert(p.stack_id, Vec::new());
    }
    for (axis, stacks) in before.iter_mut() {
        let ids: Vec<i64> = stacks.keys().copied().collect();
        *stacks = review::holds_now(registry.store(), axis, &ids).map_err(|e| match e {
            review::Error::Store(s) => Error::Store(s),
            other => Error::Refused(other.to_string()),
        })?;
    }
    // record 48, D1 of the move: a model's answer on a stack of a sample
    // sealed now is neither a review item nor a decision
    let proposed: Vec<i64> = resolved
        .iter()
        .map(|(i, _)| proposals[*i].stack_id)
        .collect();
    let (sealed, _) =
        labels::sealed_now(registry.store(), &proposed, &[]).map_err(|e| match e {
            labels::Error::Store(s) => Error::Store(s),
            other => Error::Refused(other.to_string()),
        })?;
    // (axis, model, value, band) -> members
    let mut groups: BTreeMap<(String, i64, String, String), Gathered> = BTreeMap::new();
    let mut models: BTreeMap<i64, Model> = BTreeMap::new();
    let mut against: Vec<Against> = Vec::new();
    for (i, m) in resolved {
        let p = &proposals[i];
        if sealed.contains(&p.stack_id) {
            out.sealed += 1;
            continue;
        }
        if let Some(dec) = decided.get(&p.axis).and_then(|s| s.get(&p.stack_id)) {
            out.decided += 1;
            // record 51 R5: a model that disagrees with a person's decision
            // at or above its threshold is asked about, never applied
            let confidence = p.confidence();
            let at = thresholds[&m.id].unwrap_or(f64::INFINITY);
            let disagrees = dec.person && confidence >= at && !dec.holds(&p.value);
            let evidence = disagrees.then(|| {
                json!({
                    "axis": p.axis,
                    "decision": dec.value,
                    "decision_id": dec.decision_id,
                    "decided_by": dec.actor,
                    "source": "model",
                    "model": m.named(),
                    "model_id": m.id,
                    "value": p.value,
                    "confidence": confidence,
                    "threshold": thresholds[&m.id],
                    "probabilities": p.probabilities,
                    "run_id": run.id,
                    "note": p.note,
                })
            });
            against.push(Against {
                stack_id: p.stack_id,
                axis: p.axis.clone(),
                model_id: m.id,
                evidence,
                decision: dec.value.clone(),
                value: p.value.clone(),
                confidence,
            });
            continue;
        }
        let confidence = p.confidence();
        let from = before
            .get(&p.axis)
            .and_then(|b| b.get(&p.stack_id))
            .filter(|v| !v.is_empty())
            .map(|v| v.join(","));
        let evidence = json!({
            "axis": p.axis,
            "value": p.value,
            "from": from,
            "confidence": confidence,
            "probabilities": p.probabilities,
            "model_id": m.id,
            "run_id": run.id,
            "note": p.note,
        });
        let at = thresholds[&m.id].unwrap_or(f64::INFINITY);
        groups
            .entry((p.axis.clone(), m.id, p.value.clone(), band(confidence, at)))
            .or_default()
            .members
            .push((p.stack_id, confidence, evidence));
        models.insert(m.id, m);
    }
    registry.store().begin()?;
    match write(
        registry,
        run,
        &thresholds,
        &again,
        groups,
        &models,
        &mut out,
    )
    .and_then(|()| disagree(registry, run, &against, &mut out))
    {
        Ok(()) => {
            registry.store().commit()?;
            Ok(out)
        }
        Err(e) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            Err(e)
        }
    }
}

/// How many of a group's members hold each value now, from their
/// evidence's `from`: the change matrix's row of the group (record 45). A
/// stack the axis holds nothing on counts under the empty key.
fn from_counts<'a>(members: impl Iterator<Item = &'a Value>) -> Value {
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for ev in members {
        let key = ev["from"].as_str().unwrap_or("").to_string();
        *counts.entry(key).or_default() += 1;
    }
    json!(counts)
}

fn write(
    registry: &mut Registry,
    run: &Run<'_>,
    thresholds: &BTreeMap<i64, Option<f64>>,
    again: &BTreeMap<(String, i64), BTreeSet<i64>>,
    groups: BTreeMap<(String, i64, String, String), Gathered>,
    models: &BTreeMap<i64, Model>,
    out: &mut Ingested,
) -> Result<(), Error> {
    let now = now_iso();
    let mut refusals: BTreeSet<String> = BTreeSet::new();
    for ((axis, model_id), stacks) in again {
        let done = supersede(registry, run, axis, *model_id, stacks, &now)?;
        out.superseded += done.0;
        out.withdrawn += done.1;
        out.carried += done.2;
    }
    for ((axis, model_id, value, band), mut g) in groups {
        let m = &models[&model_id];
        let threshold = thresholds[&model_id];
        if threshold.is_none() {
            refusals.insert(format!(
                "model {} ({}) names no threshold on its card, so its proposals are review items only",
                m.id,
                m.label()
            ));
        }
        g.members.sort_by_key(|(stack, _, _)| *stack);
        let confidence = g
            .members
            .iter()
            .map(|(_, c, _)| *c)
            .fold(f64::INFINITY, f64::min);
        let mean = g.members.iter().map(|(_, c, _)| *c).sum::<f64>() / g.members.len() as f64;
        let kind = format!("{axis}:model");
        let key = format!("run:{}|{kind}|{value}|{band}|model:{model_id}", run.id);
        let above = band != "below";
        let evidence = json!({
            "axis": axis,
            "value": value,
            "from": from_counts(g.members.iter().map(|(_, _, ev)| ev)),
            "tier": band,
            "confidence": confidence,
            "mean_confidence": mean,
            "threshold": threshold,
            "model": m.named(),
            "run_id": run.id,
            "members": g.members.len(),
            "group": key,
        });
        let store = registry.store();
        let item = store
            .insert(
                &Insert::new(
                    table("review_item"),
                    &[
                        "kind",
                        "scope",
                        "ref",
                        "evidence",
                        "status",
                        "created_at",
                        "job_id",
                        "members",
                        "group_key",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(kind.as_str()),
                    Param::from("group"),
                    Param::from(json!({"group": key, "run_id": run.id}).to_string()),
                    Param::from(evidence.to_string()),
                    Param::from("open"),
                    Param::from(now.as_str()),
                    run.job_id.map_or(Param::Null, Param::Int),
                    Param::Int(g.members.len() as i64),
                    Param::from(key.as_str()),
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the group item was not written back".into()))?
            .int(0)?;
        let rows: Vec<Vec<Param>> = g
            .members
            .iter()
            .map(|(stack, _, ev)| {
                vec![
                    Param::Int(item),
                    Param::Int(*stack),
                    Param::from(ev.to_string()),
                ]
            })
            .collect();
        for chunk in rows.chunks(500) {
            store.insert(
                &Insert::new(table("review_member"), &["item_id", "stack_id", "evidence"]),
                chunk,
            )?;
        }
        let members = g.members.len() as i64;
        out.members += members;
        let mut staged = None;
        if above {
            match why_not(m, &axis) {
                Some(why) => {
                    refusals.insert(why);
                }
                None => {
                    let why = format!(
                        "run {}: {} of {members} stack(s) at probability {confidence:.3} or more, threshold {}",
                        run.id,
                        value,
                        threshold.unwrap_or(1.0)
                    );
                    let applied = review::apply_within(
                        registry,
                        &Apply {
                            item,
                            member: None,
                            scope: "stack",
                            value: Some(value.as_str()),
                            author: Author {
                                who: run.principal,
                                kind: "model",
                                version: None,
                                model: Some(model_id),
                            },
                            stage: true,
                            why: Some(why.as_str()),
                            campaign: None,
                        },
                    )?;
                    debug_assert!(applied.staged, "a model's answer is staged (R6)");
                    staged = Some(applied.decision);
                    out.staged_members += members;
                }
            }
        }
        out.groups.push(Group {
            item,
            axis,
            value,
            model_id,
            band,
            members,
            confidence,
            staged,
        });
    }
    out.not_staged = refusals.into_iter().collect();
    Ok(())
}

/// Record 51 R5: raise, refresh or close the `<axis>:decision` items of a
/// model that disagrees with a person's decision. One open item per stack,
/// axis and model: a newer run refreshes its evidence where the model still
/// disagrees at or above its threshold, and closes it as superseded where
/// it no longer does. An item an unfinished campaign asks is left to the
/// campaign, and no second one is raised beside it while the campaign asks.
/// Nothing is staged and no decision is touched.
fn disagree(
    registry: &mut Registry,
    run: &Run<'_>,
    against: &[Against],
    out: &mut Ingested,
) -> Result<(), Error> {
    let now = now_iso();
    let store = registry.store();
    let d = store.dialect();
    let t = table("review_item");
    let find = format!(
        "SELECT id, {}, CASE WHEN id IN (SELECT ci.review_item_id FROM {ci} ci JOIN {c} c \
         ON c.id = ci.campaign_id WHERE c.status IN ('open', 'closing') \
         AND ci.review_item_id IS NOT NULL) THEN 1 ELSE 0 END FROM {ri} WHERE kind = {} \
         AND scope = 'stack' AND status = 'open' AND ref = {} ORDER BY id",
        d.text_of(t.column("evidence").expect("evidence")),
        d.param(1, Type::Text),
        d.param(2, Type::Json),
        ri = store.qualified("review_item"),
        ci = store.qualified("campaign_item"),
        c = store.qualified("campaign"),
    );
    for a in against {
        let kind = format!("{}:decision", a.axis);
        let reference = json!({"stack_id": a.stack_id}).to_string();
        let store = registry.store();
        // the model's open items on the stack: those free to refresh or
        // close, and whether a campaign asks one, which is left to it
        let mut earlier: Vec<i64> = Vec::new();
        let mut asked = false;
        for r in store.query(
            &find,
            &[Param::from(kind.as_str()), Param::from(reference.as_str())],
        )? {
            let ev: Value = r
                .opt_text(1)?
                .and_then(|e| serde_json::from_str(e).ok())
                .unwrap_or(Value::Null);
            if ev["source"] == "model" && ev["model_id"].as_i64() == Some(a.model_id) {
                if r.int(2)? == 1 {
                    asked = true;
                } else {
                    earlier.push(r.int(0)?);
                }
            }
        }
        let Some(evidence) = &a.evidence else {
            // the model no longer disagrees: its earlier question is closed
            let why = json!({"superseded_by": {"run_id": run.id, "model_id": a.model_id}});
            for item in earlier {
                out.agreed_again += store.execute(
                    &format!(
                        "UPDATE {} SET status = '{}', decided_at = {}, actor = {}, decision = {} \
                         WHERE id = {} AND status = 'open'",
                        store.qualified("review_item"),
                        review::RESOLVED,
                        d.param(1, Type::Timestamp),
                        d.param(2, Type::Text),
                        d.param(3, Type::Json),
                        d.param(4, Type::Int)
                    ),
                    &[
                        Param::from(now.as_str()),
                        Param::from(run.principal),
                        Param::from(why.to_string()),
                        Param::Int(item),
                    ],
                )? as i64;
            }
            continue;
        };
        let (item, refreshed) = match earlier.first() {
            // a campaign asks the question already: one open item per stack,
            // axis and model, so none is raised beside it
            None if asked => continue,
            Some(&item) => {
                store.execute(
                    &format!(
                        "UPDATE {} SET evidence = {}, job_id = {} WHERE id = {}",
                        store.qualified("review_item"),
                        d.param(1, Type::Json),
                        d.param(2, Type::Int),
                        d.param(3, Type::Int)
                    ),
                    &[
                        Param::from(evidence.to_string()),
                        run.job_id.map_or(Param::Null, Param::Int),
                        Param::Int(item),
                    ],
                )?;
                (item, true)
            }
            None => {
                let item = store
                    .insert(
                        &Insert::new(
                            table("review_item"),
                            &[
                                "kind",
                                "scope",
                                "ref",
                                "evidence",
                                "status",
                                "created_at",
                                "job_id",
                            ],
                        )
                        .returning(&["id"]),
                        &[vec![
                            Param::from(kind.as_str()),
                            Param::from("stack"),
                            Param::from(reference.as_str()),
                            Param::from(evidence.to_string()),
                            Param::from("open"),
                            Param::from(now.as_str()),
                            run.job_id.map_or(Param::Null, Param::Int),
                        ]],
                    )?
                    .first()
                    .ok_or_else(|| {
                        StoreError::Message("the disagreement item was not written back".into())
                    })?
                    .int(0)?;
                (item, false)
            }
        };
        out.disagreements.push(Disagreement {
            item,
            stack_id: a.stack_id,
            axis: a.axis.clone(),
            decision: a.decision.clone(),
            value: a.value.clone(),
            model_id: a.model_id,
            confidence: a.confidence,
            refreshed,
        });
    }
    Ok(())
}

/// Close what a model's earlier runs proposed on an axis, on `stacks`, and
/// nobody took: an item open or staged holding any of them becomes
/// `superseded`, and its staged decision that nobody committed is
/// withdrawn. Its members the run does not propose again, and no person
/// decided on their own, are carried into a new item under the earlier
/// run's key, staged by the model again where the item was staged and the
/// model still answers. An item an unfinished campaign asks is left to the
/// campaign. Answers the items superseded, the decisions withdrawn and the
/// items carried.
fn supersede(
    registry: &mut Registry,
    run: &Run<'_>,
    axis: &str,
    model_id: i64,
    stacks: &BTreeSet<i64>,
    now: &str,
) -> Result<(i64, i64, i64), Error> {
    let store = registry.store();
    let d = store.dialect();
    let t = table("review_item");
    let sql = format!(
        "SELECT id, decision_id, status, group_key, {}, job_id FROM {ri} WHERE kind = {} \
         AND status IN ('open', 'staged') AND group_key LIKE {} AND group_key NOT LIKE {} \
         AND id NOT IN (SELECT ci.review_item_id FROM {ci} ci JOIN {c} c ON c.id = ci.campaign_id \
         WHERE c.status IN ('open', 'closing'))",
        d.text_of(t.column("evidence").expect("evidence")),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
        ri = store.qualified("review_item"),
        ci = store.qualified("campaign_item"),
        c = store.qualified("campaign"),
    );
    let stale = store.query(
        &sql,
        &[
            Param::from(format!("{axis}:model")),
            Param::from(format!("%|model:{model_id}")),
            Param::from(format!("run:{}|%", run.id)),
        ],
    )?;
    let why = json!({"superseded_by": {"run_id": run.id, "model_id": model_id}});
    let (mut items, mut decisions, mut carried) = (0i64, 0i64, 0i64);
    for r in &stale {
        let (item, decision) = (r.int(0)?, r.opt_int(1)?);
        let staged = r.text(2)? == "staged";
        let key = r.text(3)?.to_string();
        let evidence: Value = r
            .opt_text(4)?
            .and_then(|e| serde_json::from_str(e).ok())
            .unwrap_or(Value::Null);
        let job_id = r.opt_int(5)?;
        let members = review::members(registry.store(), item)?;
        if !members.iter().any(|m| stacks.contains(&m.stack_id)) {
            continue;
        }
        // a staged group marks every member decided; an open one only the
        // members a person answered on their own, which are theirs
        let kept: Vec<&review::Member> = members
            .iter()
            .filter(|m| !stacks.contains(&m.stack_id) && (staged || m.decided_at.is_none()))
            .collect();
        let store = registry.store();
        // who staged it, so a carried decision is the same author's
        let mut author = run.principal.to_string();
        if let Some(decision) = decision {
            if let Some(row) = store.query_opt(
                &format!(
                    "SELECT actor FROM {} WHERE id = {}",
                    store.qualified("decision"),
                    d.param(1, Type::Int)
                ),
                &[Param::Int(decision)],
            )? && let Some(a) = row.opt_text(0)?
            {
                author = a.to_string();
            }
            decisions += store.execute(
                &format!(
                    "UPDATE {} SET withdrawn_at = {} WHERE id = {} AND withdrawn_at IS NULL \
                     AND staged_at IS NOT NULL AND committed_at IS NULL",
                    store.qualified("decision"),
                    d.param(1, Type::Timestamp),
                    d.param(2, Type::Int)
                ),
                &[Param::from(now), Param::Int(decision)],
            )? as i64;
        }
        items += store.execute(
            &format!(
                "UPDATE {} SET status = '{}', decided_at = {}, actor = {}, decision = {} WHERE id = {}",
                store.qualified("review_item"),
                review::RESOLVED,
                d.param(1, Type::Timestamp),
                d.param(2, Type::Text),
                d.param(3, Type::Json),
                d.param(4, Type::Int)
            ),
            &[
                Param::from(now),
                Param::from(run.principal),
                Param::from(why.to_string()),
                Param::Int(item),
            ],
        )? as i64;
        if kept.is_empty() {
            continue;
        }
        // the members not proposed again keep their proposal, in an item of
        // their own under the earlier run's key
        let tail = format!("|model:{model_id}");
        let carried_key = format!(
            "{}|carried:{item}{tail}",
            key.strip_suffix(&tail).unwrap_or(&key)
        );
        let confidences: Vec<f64> = kept
            .iter()
            .filter_map(|m| m.evidence["confidence"].as_f64())
            .collect();
        let mut ev = evidence.clone();
        ev["members"] = json!(kept.len());
        ev["from"] = from_counts(kept.iter().map(|m| &m.evidence));
        ev["group"] = json!(carried_key);
        ev["carried_from"] = json!(item);
        if !confidences.is_empty() {
            ev["confidence"] = json!(confidences.iter().copied().fold(f64::INFINITY, f64::min));
            ev["mean_confidence"] =
                json!(confidences.iter().sum::<f64>() / confidences.len() as f64);
        }
        let reference = json!({"group": carried_key, "run_id": evidence["run_id"]});
        let new = store
            .insert(
                &Insert::new(
                    table("review_item"),
                    &[
                        "kind",
                        "scope",
                        "ref",
                        "evidence",
                        "status",
                        "created_at",
                        "job_id",
                        "members",
                        "group_key",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(format!("{axis}:model")),
                    Param::from("group"),
                    Param::from(reference.to_string()),
                    Param::from(ev.to_string()),
                    Param::from("open"),
                    Param::from(now),
                    job_id.map_or(Param::Null, Param::Int),
                    Param::Int(kept.len() as i64),
                    Param::from(carried_key.as_str()),
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the carried item was not written back".into()))?
            .int(0)?;
        let rows: Vec<Vec<Param>> = kept
            .iter()
            .map(|m| {
                vec![
                    Param::Int(new),
                    Param::Int(m.stack_id),
                    Param::from(m.evidence.to_string()),
                ]
            })
            .collect();
        for chunk in rows.chunks(500) {
            store.insert(
                &Insert::new(table("review_member"), &["item_id", "stack_id", "evidence"]),
                chunk,
            )?;
        }
        carried += 1;
        // staged again only where it was and the model still answers;
        // otherwise the carried item stays open for a person
        if staged
            && let Some(value) = evidence["value"].as_str()
            && model::author(registry.store(), model_id).is_ok()
        {
            let why = format!(
                "carried from review item {item}: run {} did not propose these stacks again",
                run.id
            );
            review::apply_within(
                registry,
                &Apply {
                    item: new,
                    member: None,
                    scope: "stack",
                    value: Some(value),
                    author: Author {
                        who: &author,
                        kind: "model",
                        version: None,
                        model: Some(model_id),
                    },
                    stage: true,
                    why: Some(why.as_str()),
                    campaign: None,
                },
            )?;
        }
    }
    Ok((items, decisions, carried))
}

// ------------------------------------- per model and axis (UX round 1)

/// A count of a model's review groups and the scans they hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Part {
    pub groups: i64,
    pub stacks: i64,
}

impl Part {
    fn add(&mut self, stacks: i64) {
        self.groups += 1;
        self.stacks += stacks;
    }

    pub fn as_json(self) -> Value {
        json!({"groups": self.groups, "stacks": self.stacks})
    }
}

/// One model's proposals on one axis that still wait, as Review's
/// Proposals page acts on them (UX round 1, 2026-11-05: "accept all that
/// are above threshold and make the rest a campaign"): what a commit by the
/// model and the axis puts in force, and what a campaign of the rest asks.
#[derive(Debug, Clone, PartialEq)]
pub struct Standing {
    pub model: Model,
    pub axis: String,
    /// The thresholds its runs staged at, lowest first.
    pub thresholds: Vec<f64>,
    /// At or above the threshold, staged as the model's decisions.
    pub staged: Part,
    /// Of those, the ones on a stack an open campaign asks the axis of: a
    /// commit leaves them staged until that campaign closes (record 51 R3).
    pub held: Part,
    /// Of the staged decisions, the ones staged before the registry moved
    /// on, which a commit names as such (`anyway`).
    pub drifted: i64,
    /// Below the threshold and asked by no open campaign: the scans not
    /// decided yet, which a campaign of the rest asks.
    pub below: Part,
    /// Below the threshold and asked by an open campaign already.
    pub asked: Part,
    /// The open campaigns that ask them, by id and name.
    pub campaigns: Vec<(i64, String)>,
    /// The runs the proposals came from.
    pub runs: Vec<i64>,
    /// A campaign name no campaign has yet, for a campaign of the rest.
    pub name: String,
    /// The median seconds an answer on this axis took in closed campaigns
    /// (UX round 5), for the estimate of a campaign of the rest; none
    /// where no closed campaign asked the axis.
    pub pace_s: Option<f64>,
}

impl Standing {
    /// What a commit by the model and the axis takes: the staged part no
    /// open campaign holds.
    pub fn commits(&self) -> Part {
        Part {
            groups: self.staged.groups - self.held.groups,
            stacks: self.staged.stacks - self.held.stacks,
        }
    }

    pub fn as_json(&self) -> Value {
        json!({
            "model": self.model.named(),
            "label": self.model.label(),
            "axis": self.axis,
            "threshold": self.thresholds.first(),
            "thresholds": self.thresholds,
            "staged": self.staged.as_json(),
            "held": self.held.as_json(),
            "commit": self.commits().as_json(),
            "drifted": self.drifted,
            "below": {"groups": self.below.groups, "stacks": self.below.stacks, "pace_s": self.pace_s},
            "asked": self.asked.as_json(),
            "campaigns": self.campaigns.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
            "runs": self.runs,
            "name": self.name,
        })
    }
}

/// Ids as a list inside `IN (...)`: integers only.
fn in_list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A model's `<axis>:model` item that still waits, as read.
struct Waiting {
    id: i64,
    staged: bool,
    decision: Option<i64>,
    model: i64,
    axis: String,
    threshold: Option<f64>,
    run: Option<i64>,
    evidence: Value,
}

/// The `<axis>:model` items open or staged, of one axis when named.
fn waiting(store: &mut Store, axis: Option<&str>) -> Result<Vec<Waiting>, StoreError> {
    let t = table("review_item");
    let d = store.dialect();
    let mut params = Vec::new();
    let kind = match axis {
        Some(a) => {
            params.push(Param::from(format!("{a}:model")));
            format!("kind = {}", d.param(1, Type::Text))
        }
        None => "kind LIKE '%:model'".to_string(),
    };
    let sql = format!(
        "SELECT id, status, decision_id, {} FROM {} WHERE status IN ('open', 'staged') \
         AND scope IN ('stack', 'group') AND {kind} ORDER BY id",
        d.text_of(t.column("evidence").expect("evidence")),
        store.qualified("review_item"),
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &params)? {
        let evidence: Value = r
            .opt_text(3)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        let (Some(model), Some(axis)) =
            (evidence["model"]["id"].as_i64(), evidence["axis"].as_str())
        else {
            continue;
        };
        out.push(Waiting {
            id: r.int(0)?,
            staged: r.text(1)? == "staged",
            decision: r.opt_int(2)?,
            model,
            axis: axis.to_string(),
            threshold: evidence["threshold"].as_f64(),
            run: evidence["run_id"].as_i64(),
            evidence,
        });
    }
    Ok(out)
}

/// Each item's members: (every one, the ones not decided yet).
fn member_counts(store: &mut Store, ids: &[i64]) -> Result<BTreeMap<i64, (i64, i64)>, StoreError> {
    let mut out = BTreeMap::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT item_id, COUNT(*), SUM(CASE WHEN decided_at IS NULL THEN 1 ELSE 0 END) \
             FROM {} WHERE item_id IN ({}) GROUP BY item_id",
            store.qualified("review_member"),
            in_list(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?, (r.int(1)?, r.opt_int(2)?.unwrap_or(0)));
        }
    }
    Ok(out)
}

/// A campaign name for a model's proposals on an axis that no campaign has.
fn free_name(store: &mut Store, model: &Model, axis: &str) -> Result<String, Error> {
    let word: String = format!("{}-{axis}-unsure", model.name)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut name = word.clone();
    let mut n = 1;
    while crate::campaign::by_name(store, &name)
        .map_err(|e| Error::Refused(e.to_string()))?
        .is_some()
    {
        n += 1;
        name = format!("{word}-{n}");
    }
    Ok(name)
}

/// Every model's proposals that still wait, per model and axis, the most
/// scans first (UX round 1): counts only, never a stack.
pub fn standing(registry: &mut Registry) -> Result<Vec<Standing>, Error> {
    let epoch = registry.meta().epoch;
    let store = registry.store();
    let items = waiting(store, None)?;
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = items.iter().map(|w| w.id).collect();
    let members = member_counts(store, &ids)?;
    let asked_by = crate::campaign::held_review_items(store).map_err(|e| match e {
        crate::campaign::Error::Store(s) => Error::Store(s),
        other => Error::Refused(other.to_string()),
    })?;
    let decisions: Vec<i64> = items
        .iter()
        .filter(|w| w.staged)
        .filter_map(|w| w.decision)
        .collect();
    let held = review::held_decisions(store, &decisions)?;
    let mut staged_at: BTreeMap<i64, Option<i64>> = BTreeMap::new();
    for chunk in decisions.chunks(500) {
        let sql = format!(
            "SELECT id, epoch_staged FROM {} WHERE id IN ({}) AND committed_at IS NULL AND withdrawn_at IS NULL",
            store.qualified("decision"),
            in_list(chunk)
        );
        for r in store.query(&sql, &[])? {
            staged_at.insert(r.int(0)?, r.opt_int(1)?);
        }
    }
    let mut by: BTreeMap<(i64, String), Standing> = BTreeMap::new();
    let mut thresholds: BTreeMap<(i64, String), BTreeSet<String>> = BTreeMap::new();
    let mut runs: BTreeMap<(i64, String), BTreeSet<i64>> = BTreeMap::new();
    let mut campaigns: BTreeMap<(i64, String), BTreeSet<String>> = BTreeMap::new();
    for w in &items {
        let key = (w.model, w.axis.clone());
        if !by.contains_key(&key) {
            let Some(model) = model::get(store, w.model)? else {
                continue;
            };
            by.insert(
                key.clone(),
                Standing {
                    model,
                    axis: w.axis.clone(),
                    thresholds: Vec::new(),
                    staged: Part::default(),
                    held: Part::default(),
                    drifted: 0,
                    below: Part::default(),
                    asked: Part::default(),
                    campaigns: Vec::new(),
                    runs: Vec::new(),
                    name: String::new(),
                    pace_s: None,
                },
            );
        }
        let s = by.get_mut(&key).expect("inserted above");
        if let Some(t) = w.threshold {
            thresholds
                .entry(key.clone())
                .or_default()
                .insert(format!("{t:.6}"));
        }
        if let Some(r) = w.run {
            runs.entry(key.clone()).or_default().insert(r);
        }
        let (all, open) = members.get(&w.id).copied().unwrap_or((0, 0));
        if w.staged {
            // a staged decision that is no longer staged waits for nothing
            let Some(d) = w.decision.filter(|d| staged_at.contains_key(d)) else {
                continue;
            };
            s.staged.add(all);
            if held.contains_key(&d) {
                s.held.add(all);
            }
            if staged_at[&d].is_some_and(|e| e != epoch) {
                s.drifted += 1;
            }
        } else if open > 0 {
            match asked_by.get(&w.id) {
                Some(c) => {
                    s.asked.add(open);
                    campaigns.entry(key).or_default().insert(c.clone());
                }
                None => s.below.add(open),
            }
        }
    }
    let mut out: Vec<Standing> = Vec::new();
    for (key, mut s) in by {
        s.thresholds = thresholds
            .remove(&key)
            .unwrap_or_default()
            .iter()
            .filter_map(|t| t.parse::<f64>().ok())
            .collect();
        s.thresholds.sort_by(f64::total_cmp);
        s.runs = runs.remove(&key).unwrap_or_default().into_iter().collect();
        if s.staged.groups + s.below.groups + s.asked.groups == 0 {
            continue;
        }
        for name in campaigns.remove(&key).unwrap_or_default() {
            let id = crate::campaign::by_name(registry.store(), &name)
                .map_err(|e| Error::Refused(e.to_string()))?
                .map_or(0, |c| c.id);
            s.campaigns.push((id, name));
        }
        s.name = free_name(registry.store(), &s.model, &s.axis)?;
        s.pace_s = pace_s(registry.store(), &s.axis)?;
        out.push(s);
    }
    out.sort_by(|a, b| {
        (b.staged.stacks + b.below.stacks + b.asked.stacks)
            .cmp(&(a.staged.stacks + a.below.stacks + a.asked.stacks))
            .then_with(|| a.axis.cmp(&b.axis))
            .then_with(|| a.model.id.cmp(&b.model.id))
    });
    Ok(out)
}

/// The pace a person answers the axis at (UX round 5, 2026-11-08): the
/// median seconds from the claim to the answer over the raters' answers in
/// closed campaigns whose question asks the axis, for what a campaign of
/// the rest would take; none where no closed campaign asked it, or none
/// of their answers was timed.
pub fn pace_s(store: &mut Store, axis: &str) -> Result<Option<f64>, Error> {
    let t = table("campaign");
    let d = store.dialect();
    let sql = format!(
        "SELECT id, {} FROM {} WHERE status = 'closed'",
        d.text_of(t.column("question").expect("question")),
        store.qualified("campaign"),
    );
    let mut asked: Vec<i64> = Vec::new();
    for r in store.query(&sql, &[])? {
        let q: Value = r
            .opt_text(1)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        let asks = q["axes"]
            .as_array()
            .is_some_and(|axes| axes.iter().any(|a| a.as_str() == Some(axis)))
            || q["axis"].as_str() == Some(axis);
        if asks {
            asked.push(r.int(0)?);
        }
    }
    if asked.is_empty() {
        return Ok(None);
    }
    let mut seconds: Vec<f64> = Vec::new();
    for chunk in asked.chunks(500) {
        let sql = format!(
            "SELECT seconds FROM {} WHERE campaign_id IN ({}) AND role = 'rater' AND seconds IS NOT NULL AND seconds > 0",
            store.qualified("campaign_answer"),
            in_list(chunk)
        );
        for r in store.query(&sql, &[])? {
            if let Ok(Some(v)) = r.opt_double(0) {
                seconds.push(v);
            }
        }
    }
    if seconds.is_empty() {
        return Ok(None);
    }
    seconds.sort_by(f64::total_cmp);
    let n = seconds.len();
    let median = if n % 2 == 1 {
        seconds[n / 2]
    } else {
        (seconds[n / 2 - 1] + seconds[n / 2]) / 2.0
    };
    Ok(Some((median * 10.0).round() / 10.0))
}

/// Which of a model's unsure proposals a campaign of the rest takes first
/// (UX round 5): the least sure, the most sure, or a random draw by a seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    LeastSure,
    MostSure,
    Random,
}

impl Order {
    pub fn parse(text: &str) -> Option<Order> {
        match text.trim() {
            "least_sure" | "least-sure" | "least" => Some(Order::LeastSure),
            "most_sure" | "most-sure" | "most" => Some(Order::MostSure),
            "random" => Some(Order::Random),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Order::LeastSure => "least_sure",
            Order::MostSure => "most_sure",
            Order::Random => "random",
        }
    }
}

/// The words the order takes, for a refusal.
pub const ORDERS: &str = "least_sure, most_sure or random";

/// A draw's place for an item: the seed and the id mixed (splitmix64), the
/// same on every machine.
fn drawn(seed: u64, id: i64) -> u64 {
    let mut z = seed ^ (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A campaign of what one model proposed on one axis below its threshold
/// (UX round 1, 2026-11-05): the person who makes it its one rater, held
/// back none, the model's answer suggested on every item, closing into
/// decisions in force. UX round 5: at most `limit` scans, taken in `order`,
/// so that a campaign nobody can finish is not one move.
#[derive(Debug, Clone)]
pub struct RestCampaign<'a> {
    /// The model: its id, its digest or `name@version`.
    pub model: &'a str,
    pub axis: &'a str,
    /// What it is called; a free name made from the model and the axis
    /// when none is given.
    pub name: Option<&'a str>,
    /// Who makes it, and its one rater.
    pub owner: &'a str,
    /// The question, made by the caller with its pack: the axis alone.
    pub question: &'a Value,
    pub pack_version: Option<&'a str>,
    /// Every name a value goes by, in lower case, to its identity.
    pub names: &'a BTreeMap<String, String>,
    /// At most this many scans; none for every one. An item is taken whole,
    /// so a group's scans may reach past it where no scan came before.
    pub limit: Option<i64>,
    pub order: Order,
    /// The seed of a random draw; one is made from the clock where none is
    /// given and the order is random.
    pub seed: Option<u64>,
}

/// What a campaign of the rest made.
#[derive(Debug, Clone)]
pub struct MadeRest {
    pub campaign: crate::campaign::Campaign,
    /// The review items it adopted.
    pub adopted: Vec<i64>,
    /// The suggestions written, the model's answers.
    pub suggestions: crate::suggestion::Imported,
    /// The scans the limit left out, for a later campaign.
    pub left: i64,
    pub limit: Option<i64>,
    pub order: Order,
    pub seed: Option<u64>,
}

impl MadeRest {
    pub fn as_json(&self) -> Value {
        json!({
            "campaign": {"id": self.campaign.id, "name": self.campaign.name},
            "adopted": self.adopted.len(),
            "suggestions": self.suggestions.as_json(),
            "left": self.left,
            "limit": self.limit,
            "order": self.order.name(),
            "seed": self.seed,
        })
    }
}

/// Make the campaign of the rest, with the model's answer suggested on
/// each item, in one transaction: the campaign, its items and their
/// suggestions are written together or not at all. The items are the
/// model's open `<axis>:model` items no open campaign asks, a scan for each
/// member not decided yet. Each suggestion is the model's value for the
/// scan with its probability of every class, authored by the model
/// (`name@version`), so a gallery groups like scans by it. Refused when no
/// such item waits.
pub fn campaign_of_rest(
    registry: &mut Registry,
    r: &RestCampaign<'_>,
) -> Result<MadeRest, crate::campaign::Error> {
    use crate::campaign::{self as c, Error as CErr};
    let m = model::resolve(registry.store(), r.model)?
        .ok_or_else(|| CErr::NotFound(format!("no registered model answers to {}", r.model)))?;
    let store = registry.store();
    let asked_by = c::held_review_items(store)?;
    let mut items: Vec<Waiting> = waiting(store, Some(r.axis))?
        .into_iter()
        .filter(|w| !w.staged && w.model == m.id && !asked_by.contains_key(&w.id))
        .collect();
    if items.is_empty() {
        return Err(CErr::Refused(format!(
            "nothing {} proposed on {} waits below its threshold outside a campaign",
            m.label(),
            r.axis
        )));
    }
    // UX round 5: in the order asked, up to the limit of scans; the scans
    // not decided yet of each item are what the campaign asks
    let every: Vec<i64> = items.iter().map(|w| w.id).collect();
    let open_of = member_counts(store, &every)?;
    let open = |w: &Waiting| open_of.get(&w.id).map_or(0, |(_, open)| *open);
    let sure = |w: &Waiting| {
        w.evidence["confidence"]
            .as_f64()
            .or_else(|| w.evidence["mean_confidence"].as_f64())
            .unwrap_or(0.0)
    };
    let seed = match r.order {
        Order::Random => Some(r.seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1)
                | 1
        })),
        _ => None,
    };
    match r.order {
        Order::LeastSure => items.sort_by(|a, b| sure(a).total_cmp(&sure(b)).then(a.id.cmp(&b.id))),
        Order::MostSure => items.sort_by(|a, b| sure(b).total_cmp(&sure(a)).then(a.id.cmp(&b.id))),
        Order::Random => {
            let s = seed.unwrap_or(1);
            items.sort_by_key(|w| (drawn(s, w.id), w.id));
        }
    }
    let wanted: i64 = items.iter().map(open).sum();
    let mut taken = 0i64;
    let mut adopted: Vec<i64> = Vec::new();
    for w in &items {
        let n = open(w);
        if let Some(limit) = r.limit
            && !adopted.is_empty()
            && taken + n > limit
        {
            continue;
        }
        taken += n;
        adopted.push(w.id);
    }
    let left = wanted - taken;
    let adopted_set: BTreeSet<i64> = adopted.iter().copied().collect();
    items.retain(|w| adopted_set.contains(&w.id));
    adopted.sort_unstable();
    // each scan's own answer: the member's value and probabilities
    let t = table("review_member");
    let d = store.dialect();
    let value_of: BTreeMap<i64, String> = items
        .iter()
        .filter_map(|w| w.evidence["value"].as_str().map(|v| (w.id, v.to_string())))
        .collect();
    let mut rows: Vec<crate::suggestion::Given> = Vec::new();
    for chunk in adopted.chunks(500) {
        let sql = format!(
            "SELECT item_id, stack_id, {} FROM {} WHERE item_id IN ({}) AND decided_at IS NULL ORDER BY item_id, stack_id",
            d.text_of(t.column("evidence").expect("evidence")),
            store.qualified("review_member"),
            in_list(chunk)
        );
        for row in store.query(&sql, &[])? {
            let ev: Value = row
                .opt_text(2)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            let confidences: BTreeMap<String, f64> = ev["probabilities"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_f64().map(|p| (k.clone(), p)))
                        .collect()
                })
                .unwrap_or_default();
            let item = row.int(0)?;
            let value = ev["value"]
                .as_str()
                .map(str::to_string)
                .or_else(|| value_of.get(&item).cloned());
            rows.push(crate::suggestion::Given {
                stack: Some(row.int(1)?),
                value,
                confidences,
                ..Default::default()
            });
        }
    }
    let name = match r.name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => free_name(store, &m, r.axis).map_err(|e| CErr::Refused(e.to_string()))?,
    };
    // an author is a word; a model's name that is not one is said by its id
    let label = m.label();
    let author = if !label.is_empty()
        && label.len() <= 100
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._:@/+-".contains(ch))
    {
        label.clone()
    } else {
        format!("model:{}", m.id)
    };
    let runs: BTreeSet<i64> = items.iter().filter_map(|w| w.run).collect();
    let thresholds: Vec<f64> =
        items
            .iter()
            .filter_map(|w| w.threshold)
            .fold(Vec::new(), |mut v, t| {
                if !v.contains(&t) {
                    v.push(t);
                }
                v
            });
    let source_words = format!(
        "{label}'s proposals on {} below its threshold, run(s) {}",
        r.axis,
        runs.iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let adjudication = json!({"when": "never", "metric": "exact"});
    let new = c::New {
        name: &name,
        owner: r.owner,
        question: r.question,
        source: json!({"proposals": {
            "model": m.id, "label": label, "axis": r.axis, "items": adopted,
            "runs": runs, "thresholds": thresholds,
        }}),
        items: c::Items::Review(adopted.clone()),
        handle_id: None,
        content_hash: None,
        pack_version: r.pack_version,
        raters_per_item: 1,
        raters: vec![r.owner.to_string()],
        adjudicators: Vec::new(),
        adjudication: &adjudication,
        closes_into: "decision",
        lease_seconds: 3600,
        inputs: BTreeMap::new(),
        hold_back: Some(0.0),
        suggest: Some(c::Suggest::Imported),
        hide_header: false,
    };
    let now = now_iso();
    let import = crate::suggestion::Import {
        campaign: 0,
        rows: &rows,
        author: Some(&author),
        source: Some(&source_words),
        who: r.owner,
        names: r.names,
        dry_run: false,
    };
    let mut imported = None;
    let made = c::create_with(registry, &new, |store, id| {
        let made = c::get(store, id)?.ok_or_else(|| CErr::NotFound(format!("no campaign {id}")))?;
        let im = crate::suggestion::Import {
            campaign: id,
            ..import.clone()
        };
        imported = Some(crate::suggestion::import_within(store, &made, &im, &now)?);
        Ok(())
    })?;
    let suggestions = imported.unwrap_or_default();
    let im = crate::suggestion::Import {
        campaign: made.id,
        ..import
    };
    crate::suggestion::audit_import(registry, &made, &im, &suggestions)?;
    Ok(MadeRest {
        campaign: made,
        adopted,
        suggestions,
        left,
        limit: r.limit,
        order: r.order,
        seed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_band_is_below_the_threshold_or_the_highest_edge_reached() {
        assert_eq!(band(0.5, 0.8), "below");
        assert_eq!(band(0.8, 0.8), "p>=0.8");
        assert_eq!(band(0.89, 0.8), "p>=0.8");
        assert_eq!(band(0.9, 0.8), "p>=0.9");
        assert_eq!(band(0.97, 0.8), "p>=0.95");
        assert_eq!(band(1.0, 0.8), "p>=0.99");
        // an edge at or under the threshold is not one
        assert_eq!(band(0.93, 0.92), "p>=0.92");
        assert_eq!(band(0.96, 0.95), "p>=0.95");
    }

    #[test]
    fn proposals_parse_from_results_and_refuse_what_they_do_not_know() {
        let r = json!({"units": [], "proposals": [{
            "stack_id": 3, "axis": "body_part", "value": "brain",
            "probabilities": {"brain": 0.9, "spine": 0.1}, "model_id": 2,
        }]});
        let p = parse(&r).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].confidence(), 0.9);
        assert!(parse(&json!({"units": []})).unwrap().is_empty());
        let e = parse(
            &json!({"proposals": [{"stack_id": 3, "axis": "a", "value": "b",
            "probabilities": {"b": 1.0}, "model_id": 1, "extra": 1}]}),
        )
        .unwrap_err();
        assert!(e.to_string().contains("extra"), "{e}");
    }
}
