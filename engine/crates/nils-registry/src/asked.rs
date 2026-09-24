// SPDX-License-Identifier: AGPL-3.0-only

//! System 1's question (record 45 R5): the `classify.asked` review item, one
//! open per stack, whose evidence is the shape wave 44 writes and wave 45's
//! desk reads. It is fixed here and in the review-item contract
//! (`contracts/review-item/v4/classify.asked.example.json`) before anything
//! produces it, so the two waves are built against one shape.
//!
//! The evidence:
//!
//! - `axes`: the axes the stack is asked about;
//! - `candidates`: the joint answers the pack allows, most probable first,
//!   each `{values: {axis: value | [values] | null}, p}`: the conformal set,
//!   nothing illegal in it;
//! - `systems`: both systems' evidence. `rules` is the pack's own, per axis
//!   the value, the rule set and rule that decided it, every rule's vote and
//!   the label model's probabilities; `model` is the registered model's
//!   (`model_id`, `digest`, `name`, `version`) probabilities per axis;
//! - `agree`: the axes where both systems name the same value;
//! - `certificate`: what the stack's certification says, the risk level it
//!   is held to (`risk_level`, with `delta`), the group it was calibrated in,
//!   the threshold and the score, and whether the stack was decided without
//!   a person (`auto_decided`);
//! - `confidence`: the first candidate's probability, which the Review
//!   queue's cost order and a commit by minimum confidence read as they read
//!   every item's.
//!
//! Choosing a candidate answers every axis at once: `review::apply_values`
//! in Review, one decision per axis, or one `axes` answer in a campaign.
//!
//! The engine raises none yet; [`raise`] is the writer tests and the rig use
//! until wave 44's is written, and it holds what it writes to the shape.

use serde_json::{Value, json};

use crate::campaign;
use crate::schema::table;
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

/// The kind.
pub const KIND: &str = "classify.asked";

/// How far the candidates' probabilities may sum past one.
const SUM_TOLERANCE: f64 = 1e-6;

/// The item's group key: one open per stack.
pub fn key(stack: i64) -> String {
    format!("asked:stack:{stack}")
}

fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|x| x.as_str().map(str::to_string))
        .collect()
}

/// Whether evidence is the shape of the kind; with `constraints` (an axes
/// question's, `nils_pack::legal`), whether every candidate is legal too.
pub fn check(evidence: &Value, constraints: Option<&Value>) -> Result<(), String> {
    let axes = strings(&evidence["axes"])
        .filter(|a| !a.is_empty())
        .ok_or("axes: the axes the stack is asked about, a list")?;
    let candidates = evidence["candidates"]
        .as_array()
        .filter(|c| !c.is_empty())
        .ok_or("candidates: the legal joint answers, most probable first, a list")?;
    let mut last = f64::INFINITY;
    let mut sum = 0.0;
    let mut seen: Vec<String> = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        let values = c["values"]
            .as_object()
            .ok_or_else(|| format!("candidates[{i}].values: {{axis: value | [values] | null}}"))?;
        for a in &axes {
            if !values.contains_key(a) {
                return Err(format!("candidates[{i}] names no value of {a}"));
            }
        }
        if let Some(extra) = values.keys().find(|k| !axes.contains(k)) {
            return Err(format!(
                "candidates[{i}] names {extra}, which is not one of the axes asked"
            ));
        }
        for (a, v) in values {
            let fits = match v {
                Value::Null | Value::String(_) => true,
                Value::Array(list) => list.iter().all(Value::is_string),
                _ => false,
            };
            if !fits {
                return Err(format!(
                    "candidates[{i}].values.{a}: a value, a list of values, or null"
                ));
            }
        }
        let p = c["p"]
            .as_f64()
            .filter(|p| (0.0..=1.0).contains(p))
            .ok_or_else(|| format!("candidates[{i}].p: a probability"))?;
        if p > last {
            return Err(format!(
                "candidates[{i}] is more probable than the one before it; they are listed most probable first"
            ));
        }
        last = p;
        sum += p;
        let text = Value::Object(values.clone()).to_string();
        if seen.contains(&text) {
            return Err(format!("candidates[{i}] repeats an earlier candidate"));
        }
        seen.push(text);
        if let Some(cons) = constraints {
            let joint = campaign::joint_of(&axes, cons, &Value::Object(values.clone()).to_string())
                .map_err(|e| format!("candidates[{i}]: {e}"))?;
            campaign::legal(cons, &joint).map_err(|e| format!("candidates[{i}]: {e}"))?;
        }
    }
    if sum > 1.0 + SUM_TOLERANCE {
        return Err(format!(
            "the candidates' probabilities sum to {sum:.4}, past one"
        ));
    }
    let systems = &evidence["systems"];
    if !systems["rules"].is_object() || !systems["rules"]["axes"].is_object() {
        return Err("systems.rules: the pack's evidence, {pack, axes: {axis: {value, rule_set, rule, votes, label_model_p}}}".into());
    }
    let model = &systems["model"];
    if !model.is_object() || !model["p"].is_object() {
        return Err("systems.model: the model's evidence, {model_id, digest, name, version, p: {axis: {value: p}}}".into());
    }
    if !(model["model_id"].is_i64() || model["digest"].is_string()) {
        return Err("systems.model names the registered model, by model_id or digest".into());
    }
    let agree =
        strings(&evidence["agree"]).ok_or("agree: the axes both systems agree on, a list")?;
    if let Some(a) = agree.iter().find(|a| !axes.contains(a)) {
        return Err(format!(
            "agree names {a}, which is not one of the axes asked"
        ));
    }
    let cert = &evidence["certificate"];
    if !cert["risk_level"]
        .as_f64()
        .is_some_and(|r| r > 0.0 && r < 1.0)
    {
        return Err(
            "certificate.risk_level: the risk the stack is held to, between 0 and 1".into(),
        );
    }
    if !cert["group"].is_string() {
        return Err("certificate.group: the group it was calibrated in".into());
    }
    if !cert["auto_decided"].is_boolean() {
        return Err(
            "certificate.auto_decided: whether the stack was decided without a person".into(),
        );
    }
    let first = candidates[0]["p"].as_f64().unwrap_or_default();
    if evidence["confidence"].as_f64() != Some(first) {
        return Err(format!(
            "confidence is the first candidate's probability, {first}"
        ));
    }
    Ok(())
}

/// Raise a `classify.asked` item on a stack, the evidence held to its shape
/// (and to the pack's constraints when given): the open one on the stack, if
/// any, is superseded. Answers the item's id. A fixture writer until wave 44
/// writes the real one.
pub fn raise(
    store: &mut Store,
    stack: i64,
    evidence: &Value,
    constraints: Option<&Value>,
    job_id: Option<i64>,
) -> Result<i64, crate::review::Error> {
    check(evidence, constraints).map_err(crate::review::Error::Refused)?;
    let d = store.dialect();
    let now = now_iso();
    let supersede = format!(
        "UPDATE {} SET status = 'superseded', decided_at = {} WHERE kind = {} AND group_key = {} AND status = 'open'",
        store.qualified("review_item"),
        d.param(1, crate::schema::Type::Timestamp),
        d.param(2, crate::schema::Type::Text),
        d.param(3, crate::schema::Type::Text),
    );
    store.execute(
        &supersede,
        &[
            Param::from(now.as_str()),
            Param::from(KIND),
            Param::from(key(stack)),
        ],
    )?;
    let id = store
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
                Param::from(KIND),
                Param::from("stack"),
                Param::from(json!({"stack_id": stack}).to_string()),
                Param::from(evidence.to_string()),
                Param::from("open"),
                Param::from(now.as_str()),
                job_id.map_or(Param::Null, Param::Int),
                Param::Int(1),
                Param::from(key(stack)),
            ]],
        )?
        .first()
        .ok_or_else(|| StoreError::Message("the item was not written back".into()))?
        .int(0)?;
    Ok(id)
}
