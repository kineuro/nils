// SPDX-License-Identifier: AGPL-3.0-only

//! System 1's answers, a second opinion beside the rules' (record 55 D3,
//! record 56; spec §11.1). System 1 is a pipeline whose model is a site
//! model: the model chosen for the task [`TASK`] (record 55 D4). For each
//! stack and axis it writes its answer, its calibrated probability, a
//! reliability score for the rules on that header (the error model with
//! the signs of damage) and the fields that mattered most, each measured by
//! leaving it out. A run's `results.json` carries them as `opinions`
//! (`contracts/job/v1/opinions.schema.json`), and [`ingest`] keeps them.
//!
//! **An opinion is never a decision.** The rule pack decides every answer
//! (record 56, section 1). An opinion stages nothing and raises no review
//! item by itself: it is shown beside the questions Review holds, and on a
//! dataset flagged as having damaged headers it feeds the gate
//! ([`crate::round`]) once System 1 is confirmed by a recorded certificate.
//! The proposal path stays the image models' (`proposals`), whose answers
//! do become staged decisions.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::{self, Model};
use crate::schema::table;
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

/// The task System 1's models are registered for (record 55 D4: several
/// models per task, one chosen).
pub const TASK: &str = "system1";

/// The most fields an opinion names as having mattered.
pub const FIELDS_MAX: usize = 10;

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    /// Not what the contract says: the run's fault.
    Invalid(String),
    /// Well formed and not allowed: a model that may not answer, a stack
    /// outside the run.
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

/// A field that mattered, and how much: the change in the answer's
/// probability when the field is left out.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub field: String,
    pub effect: f64,
}

/// One opinion, as `results.json` carries it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Given {
    pub stack_id: i64,
    pub axis: String,
    /// A value, a list of values for a multi-valued axis, or null for "no
    /// value here".
    #[serde(default)]
    pub value: Value,
    /// The answer's calibrated probability.
    pub p: f64,
    /// The rules' reliability on this header, from 0 to 1; none where the
    /// model has no reliability part.
    #[serde(default)]
    pub reliability: Option<f64>,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub probabilities: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    pub model_id: Option<i64>,
    #[serde(default)]
    pub model_digest: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// The rules' answer on the axis as the model reads it, in the model's
    /// own vocabulary (`{value}`, the value a word, a list or null), the
    /// reading the model's certificate measured its agreement with. The gate
    /// and the suggestions compare the model's answer with this, never with
    /// the stored value, whose words are the pack's (a vendor's `SPACE`
    /// where the model says `3D-TSE`). Absent, they compare with the stored
    /// value.
    #[serde(default)]
    pub rules_read: Option<RulesRead>,
}

/// The rules' answer as a model reads it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesRead {
    #[serde(default)]
    pub value: Value,
}

/// A value as the registry keeps an axis's answer: one value, values joined
/// by a comma in order, or none.
pub fn canonical(value: &Value) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        Value::Array(list) => {
            let mut words: Vec<String> = list
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "a value list holds words only".to_string())
                })
                .collect::<Result<_, _>>()?;
            words.retain(|w| !w.is_empty());
            words.sort();
            words.dedup();
            Ok((!words.is_empty()).then(|| words.join(",")))
        }
        _ => Err("value: a word, a list of words, or null".into()),
    }
}

/// The opinions of a whole `results.json`: none when it carries none.
pub fn parse(results: &Value) -> Result<Vec<Given>, Error> {
    match results.get("opinions") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| Error::Invalid(format!("results.json opinions: {e}"))),
    }
}

/// Each opinion held to the contract, before anything is written.
fn check(o: &Given) -> Result<(), String> {
    if o.stack_id < 1 {
        return Err(format!("stack_id {} is not a stack", o.stack_id));
    }
    if o.axis.is_empty() || !o.axis.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
        return Err(format!("{:?} is not an axis", o.axis));
    }
    canonical(&o.value)?;
    if let Some(r) = &o.rules_read {
        canonical(&r.value).map_err(|e| format!("rules_read.{e}"))?;
    }
    if !(0.0..=1.0).contains(&o.p) || !o.p.is_finite() {
        return Err(format!(
            "stack {} {}: p is a probability",
            o.stack_id, o.axis
        ));
    }
    if let Some(r) = o.reliability
        && (!(0.0..=1.0).contains(&r) || !r.is_finite())
    {
        return Err(format!(
            "stack {} {}: reliability is between 0 and 1",
            o.stack_id, o.axis
        ));
    }
    if o.fields.len() > FIELDS_MAX {
        return Err(format!(
            "stack {} {}: at most {FIELDS_MAX} fields that mattered",
            o.stack_id, o.axis
        ));
    }
    if o.model_id.is_none() && o.model_digest.is_none() {
        return Err(format!(
            "stack {} {}: the model, by model_id or model_digest",
            o.stack_id, o.axis
        ));
    }
    Ok(())
}

/// The run an opinion came from: its id, and the stacks and the models it
/// may speak for.
pub struct Run<'a> {
    pub id: Option<i64>,
    pub stacks: Option<&'a BTreeSet<i64>>,
    pub models: Option<&'a BTreeSet<i64>>,
}

/// What [`ingest`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ingested {
    pub written: i64,
    pub stacks: i64,
    /// Opinions left out, by why.
    pub dropped: BTreeMap<String, i64>,
    pub models: Vec<i64>,
}

impl Ingested {
    pub fn to_json(&self) -> Value {
        json!({
            "written": self.written, "stacks": self.stacks, "dropped": self.dropped,
            "models": self.models,
        })
    }
}

/// The model an opinion names: registered for [`TASK`], and admitted,
/// promoted or chosen (a registered model is not yet allowed to speak).
fn model_of(store: &mut Store, o: &Given) -> Result<Model, String> {
    let m = match (o.model_id, &o.model_digest) {
        (Some(id), _) => model::get(store, id).map_err(|e| e.to_string())?,
        (None, Some(d)) => model::by_digest(store, d).map_err(|e| e.to_string())?,
        (None, None) => None,
    }
    .ok_or_else(|| "a model that is not registered".to_string())?;
    if let (Some(_), Some(d)) = (o.model_id, &o.model_digest)
        && &m.digest != d
    {
        return Err(format!(
            "model {} named with a digest that is not its own",
            m.id
        ));
    }
    if m.task != TASK {
        return Err(format!(
            "model {} ({}), registered for {} and not {TASK}",
            m.id,
            m.label(),
            m.task
        ));
    }
    if !m.answers() {
        return Err(format!(
            "model {} ({}), which is {} and neither admitted nor promoted",
            m.id,
            m.label(),
            m.state
        ));
    }
    Ok(m)
}

/// Keep a run's opinions: one row per stack and axis, newest read first.
/// Opinions on a stack outside the run, by a model the run was not given,
/// or by a model that may not answer are left out and counted; a stack of
/// a sample sealed now is left out too (record 48), as every model's
/// answer about one is. A malformed opinion refuses the whole lot.
pub fn ingest(store: &mut Store, run: &Run<'_>, given: &[Given]) -> Result<Ingested, Error> {
    for o in given {
        check(o).map_err(Error::Invalid)?;
    }
    let mut out = Ingested::default();
    let mut models: BTreeMap<(Option<i64>, Option<String>), Result<Model, String>> =
        BTreeMap::new();
    let ids: Vec<i64> = given
        .iter()
        .map(|o| o.stack_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let (sealed, _) =
        crate::labels::sealed_now(store, &ids, &[]).map_err(|e| Error::Refused(e.to_string()))?;
    let now = now_iso();
    let mut rows = Vec::new();
    let mut stacks = BTreeSet::new();
    let mut used = BTreeSet::new();
    for o in given {
        let key = (o.model_id, o.model_digest.clone());
        if !models.contains_key(&key) {
            let m = model_of(store, o);
            models.insert(key.clone(), m);
        }
        let why = match &models[&key] {
            Err(why) => Some(why.clone()),
            Ok(m) if run.models.is_some_and(|s| !s.contains(&m.id)) => Some(format!(
                "model {} ({}), which the run was not given",
                m.id,
                m.label()
            )),
            Ok(_) if run.stacks.is_some_and(|s| !s.contains(&o.stack_id)) => {
                Some("a stack outside the run's selection".into())
            }
            Ok(_) if sealed.contains(&o.stack_id) => Some("a stack of a sample sealed now".into()),
            Ok(_) => None,
        };
        if let Some(w) = why {
            *out.dropped.entry(w).or_default() += 1;
            continue;
        }
        let m = models[&key].as_ref().expect("checked above");
        used.insert(m.id);
        stacks.insert(o.stack_id);
        let value = canonical(&o.value).map_err(Error::Invalid)?;
        let fields = (!o.fields.is_empty()).then(|| {
            let mut f = o.fields.clone();
            f.sort_by(|a, b| {
                b.effect
                    .abs()
                    .partial_cmp(&a.effect.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            serde_json::to_string(&f).unwrap_or_default()
        });
        rows.push(vec![
            Param::Int(o.stack_id),
            Param::from(o.axis.as_str()),
            Param::Int(m.id),
            run.id.map_or(Param::Null, Param::Int),
            value.map_or(Param::Null, Param::Text),
            Param::Double(o.p),
            o.reliability.map_or(Param::Null, Param::Double),
            fields.map_or(Param::Null, Param::Text),
            o.probabilities
                .as_ref()
                .map_or(Param::Null, |p| Param::Text(json!(p).to_string())),
            o.note.clone().map_or(Param::Null, Param::Text),
            Param::Int(i64::from(o.rules_read.is_some())),
            o.rules_read
                .as_ref()
                .and_then(|r| canonical(&r.value).ok().flatten())
                .map_or(Param::Null, Param::Text),
            Param::from(now.as_str()),
        ]);
    }
    for chunk in rows.chunks(500) {
        store.insert(
            &Insert::new(
                table("opinion"),
                &[
                    "stack_id",
                    "axis",
                    "model_id",
                    "run_id",
                    "value",
                    "p",
                    "reliability",
                    "fields",
                    "probabilities",
                    "note",
                    "rules_read",
                    "rules_value",
                    "created_at",
                ],
            ),
            chunk,
        )?;
    }
    out.written = rows.len() as i64;
    out.stacks = stacks.len() as i64;
    out.models = used.into_iter().collect();
    Ok(out)
}

/// One opinion as it is read back.
#[derive(Debug, Clone, PartialEq)]
pub struct Opinion {
    pub id: i64,
    pub stack_id: i64,
    pub axis: String,
    pub model_id: i64,
    pub run_id: Option<i64>,
    pub value: Option<String>,
    pub p: f64,
    pub reliability: Option<f64>,
    pub fields: Value,
    pub probabilities: Value,
    pub note: Option<String>,
    /// The rules' answer as the model read it, where it said: `Some(None)`
    /// for "the rules gave no value" in the model's reading.
    pub rules_read: Option<Option<String>>,
    pub created_at: String,
}

impl Opinion {
    pub fn as_json(&self) -> Value {
        json!({
            "axis": self.axis, "value": self.value, "p": self.p,
            "reliability": self.reliability, "fields": self.fields,
            "probabilities": self.probabilities, "note": self.note,
            "model_id": self.model_id, "run_id": self.run_id, "at": self.created_at,
            "rules_read": self.rules_read.as_ref().map(|v| json!({"value": v})),
        })
    }

    /// Whether the model agrees with the rules on its axis: its answer
    /// against the rules' answer as it read them, where it said, else
    /// against `stored`, the rules' value as the registry keeps it.
    pub fn agrees(&self, stored: Option<&str>) -> bool {
        match &self.rules_read {
            Some(read) => self.value.as_deref() == read.as_deref(),
            None => self.value.as_deref() == stored,
        }
    }

    /// The rules' answer the model compares with: its reading, else the
    /// stored value.
    pub fn rules_compared<'a>(&'a self, stored: Option<&'a str>) -> Option<&'a str> {
        match &self.rules_read {
            Some(read) => read.as_deref(),
            None => stored,
        }
    }
}

/// The newest opinion of `model` (any model when none) on each axis of
/// each of `stacks`, by stack and axis. Read five hundred stacks a query.
pub fn newest(
    store: &mut Store,
    stacks: &[i64],
    model: Option<i64>,
) -> Result<BTreeMap<i64, BTreeMap<String, Opinion>>, StoreError> {
    let d = store.dialect();
    let t = table("opinion");
    let mut out: BTreeMap<i64, BTreeMap<String, Opinion>> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let by = match model {
            Some(m) => format!(" AND model_id = {m}"),
            None => String::new(),
        };
        let sql = format!(
            "SELECT id, stack_id, axis, model_id, run_id, value, p, reliability, {}, {}, note, {}, rules_read, rules_value \
             FROM {} WHERE stack_id IN ({list}){by} ORDER BY id",
            d.text_of(t.column("fields").expect("fields")),
            d.text_of(t.column("probabilities").expect("probabilities")),
            d.text_of(t.column("created_at").expect("created_at")),
            store.qualified("opinion"),
        );
        for r in store.query(&sql, &[])? {
            let json_of = |s: Option<&str>| {
                s.and_then(|t| serde_json::from_str::<Value>(t).ok())
                    .unwrap_or(Value::Null)
            };
            let o = Opinion {
                id: r.int(0)?,
                stack_id: r.int(1)?,
                axis: r.text(2)?.to_string(),
                model_id: r.int(3)?,
                run_id: r.opt_int(4)?,
                value: r.opt_text(5)?.map(str::to_string),
                p: r.double(6)?,
                reliability: r.opt_double(7)?,
                fields: json_of(r.opt_text(8)?),
                probabilities: json_of(r.opt_text(9)?),
                note: r.opt_text(10)?.map(str::to_string),
                created_at: r.text(11)?.to_string(),
                rules_read: (r.opt_int(12)?.unwrap_or(0) != 0)
                    .then(|| r.opt_text(13).map(|v| v.map(str::to_string)))
                    .transpose()?,
            };
            // ordered by id, so a later row replaces an earlier one
            out.entry(o.stack_id).or_default().insert(o.axis.clone(), o);
        }
    }
    Ok(out)
}

/// How many stacks carry an opinion of `model` (any model when none), of
/// the stacks `holds` keeps (a condition on a stack aliased `x`).
pub fn stacks_with(store: &mut Store, holds: &str, model: Option<i64>) -> Result<i64, StoreError> {
    let by = match model {
        Some(m) => format!(" AND o.model_id = {m}"),
        None => String::new(),
    };
    let sql = format!(
        "SELECT COUNT(DISTINCT o.stack_id) FROM {} o JOIN {} x ON x.id = o.stack_id WHERE {holds}{by}",
        store.qualified("opinion"),
        store.qualified("stack"),
    );
    Ok(store
        .query_opt(&sql, &[])?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0))
}

/// The model the opinions of `stack` come from now: the chosen System 1
/// model, when there is one.
pub fn chosen(store: &mut Store) -> Result<Option<model::Chosen>, StoreError> {
    model::chosen(store, TASK, "site")
}

/// Whether System 1 is confirmed (record 56 section 3, record 55 D2): a
/// recorded certificate names the model, on a sample sealed here or citing
/// its graded run (record 55 ruling 22 b). Until then its gate on flagged
/// datasets raises nothing and stages nothing, and Review shows its opinion
/// as not yet confirmed.
pub fn confirmed(store: &mut Store, model_id: i64) -> Result<Option<i64>, StoreError> {
    let certs = crate::labels::certificates(store).map_err(|e| match e {
        crate::labels::Error::Store(s) => s,
        other => StoreError::Message(other.to_string()),
    })?;
    Ok(certs
        .iter()
        .find(|c| c.model_ids.contains(&model_id))
        .map(|c| c.id))
}

/// The gate's thresholds a model's card sets (spec §11.2): per axis,
/// `lambda1` on System 1's probability and `lambda2` on the rules'
/// reliability, from `card.gate.axes`. Empty where the card sets none.
pub fn thresholds(m: &Model) -> BTreeMap<String, (f64, f64)> {
    let mut out = BTreeMap::new();
    for (axis, t) in m.card["gate"]["axes"].as_object().into_iter().flatten() {
        if let (Some(l1), Some(l2)) = (t["lambda1"].as_f64(), t["lambda2"].as_f64()) {
            out.insert(axis.clone(), (l1, l2));
        }
    }
    out
}

/// A model's id by its row, for the doors.
pub fn named(store: &mut Store, id: i64) -> Result<Option<Value>, StoreError> {
    Ok(model::get(store, id)?.map(|m| m.named()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_kept_as_the_registry_keeps_an_axis() {
        assert_eq!(canonical(&json!(null)).unwrap(), None);
        assert_eq!(canonical(&json!("")).unwrap(), None);
        assert_eq!(canonical(&json!("T1w")).unwrap().as_deref(), Some("T1w"));
        assert_eq!(
            canonical(&json!(["FLAIR", "FatSat", "FLAIR"]))
                .unwrap()
                .as_deref(),
            Some("FLAIR,FatSat")
        );
        assert_eq!(canonical(&json!([])).unwrap(), None);
        assert!(canonical(&json!(3)).is_err());
    }

    #[test]
    fn an_opinion_is_held_to_its_contract() {
        let ok: Given = serde_json::from_value(json!({
            "stack_id": 4, "axis": "base", "value": "T1w", "p": 0.9, "reliability": 0.99,
            "fields": [{"field": "text_series_description", "effect": 0.4}], "model_id": 2,
        }))
        .unwrap();
        assert!(check(&ok).is_ok());
        let mut bad = ok.clone();
        bad.p = 1.5;
        assert!(check(&bad).is_err());
        let mut bad = ok.clone();
        bad.model_id = None;
        assert!(check(&bad).is_err());
        let mut bad = ok.clone();
        bad.axis = "Base".into();
        assert!(check(&bad).is_err());
        assert!(
            serde_json::from_value::<Given>(json!({
                "stack_id": 4, "axis": "base", "p": 0.9, "model_id": 2, "votes": 3,
            }))
            .is_err(),
            "an unknown member is refused"
        );
    }
}
