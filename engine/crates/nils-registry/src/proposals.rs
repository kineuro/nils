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
//! The bands split at the task's threshold and, above it, at
//! [`BANDS`]. A group at or above the threshold gets a staged group
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
//! in force, is not asked again: it is counted, and left out.

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

/// What an ingest did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ingested {
    pub groups: Vec<Group>,
    /// Members under the groups.
    pub members: i64,
    /// Members of staged groups.
    pub staged_members: i64,
    /// Proposals left out because a person's or an agent's decision on
    /// the axis is in force on the stack.
    pub decided: i64,
    /// Why a group at or above the threshold was not staged, once per
    /// model: not admitted or promoted, or registered for another task.
    pub not_staged: Vec<String>,
}

impl Ingested {
    pub fn to_json(&self) -> Value {
        json!({
            "items": self.groups.len(),
            "members": self.members,
            "staged_members": self.staged_members,
            "staged_decisions": self.groups.iter().filter(|g| g.staged.is_some()).count(),
            "decided": self.decided,
            "not_staged": self.not_staged,
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

/// Turn a run's proposals into grouped `<axis>:model` review items, and
/// stage a decision by the model on each group at or above `threshold`,
/// all in one transaction: every item and decision is written, or none.
/// A run is ingested once; a second ingest of it is refused.
pub fn ingest(
    registry: &mut Registry,
    run: &Run<'_>,
    proposals: &[Proposal],
    threshold: f64,
) -> Result<Ingested, Error> {
    if !threshold.is_finite() || threshold <= 0.0 || threshold > 1.0 {
        return Err(invalid(format!(
            "the threshold is {threshold}, and a threshold is a probability above 0 and at most 1"
        )));
    }
    let store = registry.store();
    let d = store.dialect();
    let prefix = format!("run:{}|", run.id);
    let held = store
        .query_opt(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE group_key LIKE {}",
                store.qualified("review_item"),
                d.param(1, Type::Text)
            ),
            &[Param::from(format!("{prefix}%"))],
        )?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0);
    if held > 0 {
        return Err(Error::Refused(format!(
            "the proposals of run {} are in already, as {held} review item(s)",
            run.id
        )));
    }
    let resolved = checked(store, proposals)?;
    // the stacks a person or an agent decided, per axis
    let mut decided: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
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
            labels.iter().filter_map(|l| l.stack_id).collect(),
        );
    }
    let mut out = Ingested::default();
    // (axis, model, value, band) -> members
    let mut groups: BTreeMap<(String, i64, String, String), Gathered> = BTreeMap::new();
    let mut models: BTreeMap<i64, Model> = BTreeMap::new();
    for (i, m) in resolved {
        let p = &proposals[i];
        if decided
            .get(&p.axis)
            .is_some_and(|s| s.contains(&p.stack_id))
        {
            out.decided += 1;
            continue;
        }
        let confidence = p.confidence();
        let evidence = json!({
            "axis": p.axis,
            "value": p.value,
            "confidence": confidence,
            "probabilities": p.probabilities,
            "model_id": m.id,
            "run_id": run.id,
            "note": p.note,
        });
        groups
            .entry((
                p.axis.clone(),
                m.id,
                p.value.clone(),
                band(confidence, threshold),
            ))
            .or_default()
            .members
            .push((p.stack_id, confidence, evidence));
        models.insert(m.id, m);
    }
    registry.store().begin()?;
    match write(registry, run, threshold, groups, &models, &mut out) {
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

fn write(
    registry: &mut Registry,
    run: &Run<'_>,
    threshold: f64,
    groups: BTreeMap<(String, i64, String, String), Gathered>,
    models: &BTreeMap<i64, Model>,
    out: &mut Ingested,
) -> Result<(), Error> {
    let now = now_iso();
    let mut refusals: BTreeSet<String> = BTreeSet::new();
    for ((axis, model_id, value, band), mut g) in groups {
        let m = &models[&model_id];
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
                        "run {}: {} of {members} stack(s) at probability {confidence:.3} or more, threshold {threshold}",
                        run.id, value
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
