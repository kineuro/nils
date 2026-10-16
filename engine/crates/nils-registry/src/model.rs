// SPDX-License-Identifier: AGPL-3.0-only

//! The model registry (record 42 S2, D15): the models whose answers become
//! registry facts, identified by the digest of their canonical artifact and
//! described by the card of `contracts/model/v1`.
//!
//! A model passes through four states, the lifecycle Kvasir keeps for the
//! language models it serves: registered with its card; admitted by a check
//! that passed, which stays on it; promoted, which is refused unless it was
//! admitted; and retired. A failed check is kept as an event and moves
//! nothing. Every transition is an event row and an audit row, and nothing
//! is deleted, because a retired model still names what it decided.
//!
//! Record 55 D4 (spec 10.2): several admitted or promoted models answer one
//! task and slot, and promoting one retires none. The one a run takes is
//! the **chosen** model ([`chosen`]): the one a site's admin chose
//! ([`choose`]), audited, or by default the one a recorded certificate
//! names, then the one promoted last, then the newest admitted. A retired
//! model's files stay where its card says, so a past result can be run
//! again.
//!
//! Record 55 A5 (spec 4.2, 10.5): a card states its speed on a CPU and on
//! a GPU, its GPU need and the equivalence class of its CPU and GPU
//! answers; the admission suite measures that class again
//! ([`equivalence`]) and a model whose measured class is worse than its
//! card's is not admitted ([`admit`]).
//!
//! A model's answer is a decision with `author_kind` model and this model's
//! id ([`crate::review::apply`]), refused unless the model is admitted or
//! promoted, and staged until a person commits it (record 42 R6).
//!
//! A cascade (record 55 E2, the ruling of 2026-10-10) is one model whose
//! card names its parts, each a registered model by digest under the name of
//! the pipeline input it fills (`parts`); promoting it retires none of them.
//!
//! A card is kept as it was registered, with one exception (record 55,
//! ruling 31 (a), 2026-10-16): a registered card may gain the claims it
//! lacks, the label set it was trained on, its tested claims and its speed
//! ([`add_claims`]). Additions only: nothing a card states changes, each
//! claim is checked as registration checks it, and each addition is an
//! event (`claims_added`, with what was added) and an audit row.

use serde_json::{Value, json};

use crate::Registry;
use crate::audit::{self, Action, Entry};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};
use crate::time::now_iso;

/// The four states, in order.
pub const STATES: [&str; 4] = ["registered", "admitted", "promoted", "retired"];

/// The kinds the card names. `served` is Kvasir's; the engine does not
/// register one (study §4: reserved for the day a language model writes
/// decisions as a model).
pub const KINDS: [&str; 5] = ["encoder", "head", "pass", "segmenter", "served"];

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    /// The request is malformed: a card or a check that is not one.
    Invalid(String),
    /// No such model, or no such thing the card names.
    Unknown(String),
    /// Well formed, and not allowed in the state things are in.
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Invalid(m) | Error::Unknown(m) | Error::Refused(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

/// One registered model.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub digest: String,
    pub task: String,
    pub slot: String,
    pub state: String,
    pub card: Value,
    pub encoder_model_id: Option<i64>,
    /// Every encoder it reads, in the order it concatenates their features
    /// (record 43); the first is `encoder_model_id`.
    pub encoder_model_ids: Vec<i64>,
    /// The digest of the label set it was fitted on.
    pub trained_on: Option<String>,
    pub pack_version: Option<String>,
    /// The check that admitted it, or the last that failed while it was
    /// registered.
    pub check: Option<Value>,
    pub registered_by: String,
    pub registered_at: String,
    pub admitted_by: Option<String>,
    pub admitted_at: Option<String>,
    pub promoted_by: Option<String>,
    pub promoted_at: Option<String>,
    pub retired_by: Option<String>,
    pub retired_at: Option<String>,
    pub review_item: Option<i64>,
}

impl Model {
    /// `name@version`, how people call it.
    pub fn label(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// The digest shortened to twelve hex digits, for a line of text.
    pub fn short_digest(&self) -> &str {
        let hex = self.digest.strip_prefix("sha256:").unwrap_or(&self.digest);
        &hex[..hex.len().min(12)]
    }

    /// Whether its answers may be written: admitted or promoted.
    pub fn answers(&self) -> bool {
        matches!(self.state.as_str(), "admitted" | "promoted")
    }

    /// The probability at or above which its proposals are staged (record
    /// 43), as its card says; none when the card says nothing.
    pub fn threshold(&self) -> Option<f64> {
        self.card["threshold"].as_f64()
    }

    /// The name, version and digest, which is how a decision, an explain
    /// and a release name it.
    pub fn named(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "version": self.version,
            "digest": self.digest, "state": self.state,
        })
    }

    /// The whole model, as `GET /api/models/{id}` and `nils model show
    /// --json` answer it (`contracts/model/v1/lifecycle.schema.json`).
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "version": self.version,
            "kind": self.kind,
            "digest": self.digest,
            "task": self.task,
            "slot": self.slot,
            "state": self.state,
            "card": self.card,
            "encoder_model_id": self.encoder_model_id,
            "encoder_model_ids": self.encoder_model_ids,
            "threshold": self.threshold(),
            "trained_on": self.trained_on,
            "pack_version": self.pack_version,
            "check": self.check,
            "registered_by": self.registered_by,
            "registered_at": self.registered_at,
            "admitted_by": self.admitted_by,
            "admitted_at": self.admitted_at,
            "promoted_by": self.promoted_by,
            "promoted_at": self.promoted_at,
            "retired_by": self.retired_by,
            "retired_at": self.retired_at,
            "review_item": self.review_item,
            "complete": self.missing().is_empty(),
            "missing": self.missing(),
            "equivalence_measured": self
                .check
                .as_ref()
                .and_then(|c| c.get("equivalence"))
                .filter(|e| !e.is_null()),
        })
    }

    /// What its card lacks to be complete (record 55 D4, T22): the label
    /// set it was trained on, its tested claims, its speed on a CPU and,
    /// unless it needs no GPU, on one, its GPU need, its equivalence
    /// unless it needs no GPU, its intended use and its limits. Empty for
    /// a complete card.
    pub fn missing(&self) -> Vec<&'static str> {
        missing(&self.card)
    }

    /// What its card says it needs of a GPU; `none` where it says nothing.
    pub fn gpu(&self) -> &str {
        self.card["gpu"].as_str().unwrap_or("none")
    }
}

/// What a card lacks to be complete, by field (see [`Model::missing`]).
pub fn missing(card: &Value) -> Vec<&'static str> {
    let mut out = Vec::new();
    let text = |v: &Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    if !card["trained_on"]["label_set"]
        .as_str()
        .is_some_and(is_digest)
    {
        out.push("trained_on");
    }
    if !card["tested"].as_array().is_some_and(|a| !a.is_empty()) {
        out.push("tested");
    }
    let gpu = card["gpu"].as_str();
    if gpu.is_none() {
        out.push("gpu");
    }
    if !card["speed"]["cpu"].is_object() {
        out.push("speed.cpu");
    }
    let needs_gpu = gpu.is_none_or(|g| g != "none");
    if needs_gpu && !card["speed"]["gpu"].is_object() {
        out.push("speed.gpu");
    }
    if needs_gpu && !card["equivalence"].is_object() {
        out.push("equivalence");
    }
    if !text(&card["intended_use"]) {
        out.push("intended_use");
    }
    if !card["limits"]
        .as_array()
        .is_some_and(|a| a.iter().any(text))
    {
        out.push("limits");
    }
    out
}

/// The equivalence classes, best first (spec 16.3).
pub const CLASSES: [&str; 3] = ["I", "II", "III"];

/// The rank of a class, 0 the best; none for a text that is not one.
pub fn class_rank(class: &str) -> Option<usize> {
    CLASSES.iter().position(|c| *c == class)
}

/// The class I tolerance where a card gives none: the speed study's, for a
/// calibrated probability.
pub const CLASS_I_TOLERANCE: f64 = 0.001;

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn refused(m: impl Into<String>) -> Error {
    Error::Refused(m.into())
}

/// Whether a text is a digest as the card writes one.
pub fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn name_ok(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn task_ok(text: &str) -> bool {
    let mut parts = text.split(':');
    let head = parts.next().unwrap_or("");
    let mut first = head.chars();
    first.next().is_some_and(|c| c.is_ascii_lowercase())
        && first.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
        && parts.all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        })
}

fn text_field<'a>(card: &'a Value, key: &str) -> Result<&'a str, Error> {
    card[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(format!("a card names its {key}")))
}

fn select_sql(store: &Store, filter: &str) -> String {
    let d = store.dialect();
    let t = table("model");
    let text = |c: &str| d.text_of(t.column(c).expect("model column"));
    format!(
        "SELECT id, name, version, kind, digest, task, slot, state, {}, encoder_model_id, \
         trained_on, pack_version, {}, registered_by, {}, admitted_by, {}, promoted_by, {}, \
         retired_by, {}, review_item FROM {}{filter} ORDER BY id",
        text("card"),
        text("gate"),
        text("registered_at"),
        text("admitted_at"),
        text("promoted_at"),
        text("retired_at"),
        store.qualified("model"),
    )
}

fn of(r: &Row) -> Result<Model, StoreError> {
    let json = |s: Option<&str>| s.and_then(|t| serde_json::from_str::<Value>(t).ok());
    let text =
        |i: usize| -> Result<Option<String>, StoreError> { Ok(r.opt_text(i)?.map(str::to_string)) };
    Ok(Model {
        id: r.int(0)?,
        name: r.text(1)?.to_string(),
        version: r.text(2)?.to_string(),
        kind: r.text(3)?.to_string(),
        digest: r.text(4)?.to_string(),
        task: r.text(5)?.to_string(),
        slot: r.text(6)?.to_string(),
        state: r.text(7)?.to_string(),
        card: json(r.opt_text(8)?).unwrap_or(Value::Null),
        encoder_model_id: r.opt_int(9)?,
        encoder_model_ids: Vec::new(),
        trained_on: text(10)?,
        pack_version: text(11)?,
        check: json(r.opt_text(12)?),
        registered_by: r.text(13)?.to_string(),
        registered_at: r.text(14)?.to_string(),
        admitted_by: text(15)?,
        admitted_at: text(16)?,
        promoted_by: text(17)?,
        promoted_at: text(18)?,
        retired_by: text(19)?,
        retired_at: text(20)?,
        review_item: r.opt_int(21)?,
    })
}

/// Fill each model's encoders from `model_encoder`, in their order.
fn with_encoders(store: &mut Store, mut models: Vec<Model>) -> Result<Vec<Model>, StoreError> {
    let heads: Vec<String> = models
        .iter()
        .filter(|m| m.encoder_model_id.is_some())
        .map(|m| m.id.to_string())
        .collect();
    for chunk in heads.chunks(500) {
        let sql = format!(
            "SELECT model_id, encoder_model_id FROM {} WHERE model_id IN ({}) ORDER BY model_id, position",
            store.qualified("model_encoder"),
            chunk.join(", ")
        );
        for r in store.query(&sql, &[])? {
            let (model, encoder) = (r.int(0)?, r.int(1)?);
            if let Some(m) = models.iter_mut().find(|m| m.id == model) {
                m.encoder_model_ids.push(encoder);
            }
        }
    }
    for m in &mut models {
        if m.encoder_model_ids.is_empty()
            && let Some(e) = m.encoder_model_id
        {
            m.encoder_model_ids.push(e);
        }
    }
    Ok(models)
}

fn one_model(store: &mut Store, sql: &str, params: &[Param]) -> Result<Option<Model>, StoreError> {
    let Some(m) = store.query_opt(sql, params)?.map(|r| of(&r)).transpose()? else {
        return Ok(None);
    };
    Ok(with_encoders(store, vec![m])?.pop())
}

/// One model by its id.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Model>, StoreError> {
    let d = store.dialect();
    let sql = select_sql(store, &format!(" WHERE id = {}", d.param(1, Type::Int)));
    one_model(store, &sql, &[Param::Int(id)])
}

/// One model by its digest.
pub fn by_digest(store: &mut Store, digest: &str) -> Result<Option<Model>, StoreError> {
    let d = store.dialect();
    let sql = select_sql(
        store,
        &format!(" WHERE digest = {}", d.param(1, Type::Text)),
    );
    one_model(store, &sql, &[Param::from(digest)])
}

/// A model as a person or a header names it: its id, its digest
/// (`sha256:...`) or `name@version`. None when nothing registered answers.
pub fn resolve(store: &mut Store, reference: &str) -> Result<Option<Model>, StoreError> {
    let reference = reference.trim();
    if let Ok(id) = reference.parse::<i64>() {
        return get(store, id);
    }
    if reference.starts_with("sha256:") {
        return by_digest(store, reference);
    }
    let Some((name, version)) = reference.split_once('@') else {
        return Ok(None);
    };
    let d = store.dialect();
    let sql = select_sql(
        store,
        &format!(
            " WHERE name = {} AND version = {}",
            d.param(1, Type::Text),
            d.param(2, Type::Text)
        ),
    );
    one_model(store, &sql, &[Param::from(name), Param::from(version)])
}

/// What a listing is narrowed to.
#[derive(Debug, Default, Clone)]
pub struct Filter<'a> {
    pub task: Option<&'a str>,
    pub slot: Option<&'a str>,
    pub state: Option<&'a str>,
}

/// The registered models, oldest first.
pub fn list(store: &mut Store, f: &Filter<'_>) -> Result<Vec<Model>, StoreError> {
    let d = store.dialect();
    let mut wheres = Vec::new();
    let mut params = Vec::new();
    for (column, value) in [("task", f.task), ("slot", f.slot), ("state", f.state)] {
        if let Some(v) = value {
            params.push(Param::from(v));
            wheres.push(format!("{column} = {}", d.param(params.len(), Type::Text)));
        }
    }
    let filter = if wheres.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", wheres.join(" AND "))
    };
    let sql = select_sql(store, &filter);
    let models = store
        .query(&sql, &params)?
        .iter()
        .map(of)
        .collect::<Result<Vec<_>, _>>()?;
    with_encoders(store, models)
}

/// A model's transitions, in order.
pub fn events(store: &mut Store, id: i64) -> Result<Vec<Value>, StoreError> {
    let d = store.dialect();
    let t = table("model_event");
    let sql = format!(
        "SELECT transition, principal, {}, {} FROM {} WHERE model_id = {} ORDER BY id",
        d.text_of(t.column("at").expect("at")),
        d.text_of(t.column("detail").expect("detail")),
        store.qualified("model_event"),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(id)])?
        .iter()
        .map(|r| {
            Ok(json!({
                "transition": r.text(0)?,
                "by": r.text(1)?,
                "at": r.text(2)?,
                "detail": r
                    .opt_text(3)?
                    .and_then(|t| serde_json::from_str::<Value>(t).ok()),
            }))
        })
        .collect()
}

fn event(
    store: &mut Store,
    id: i64,
    transition: &str,
    who: &str,
    now: &str,
    detail: Option<Value>,
) -> Result<(), StoreError> {
    store.insert(
        &Insert::new(
            table("model_event"),
            &["model_id", "transition", "principal", "at", "detail"],
        ),
        &[vec![
            Param::Int(id),
            Param::from(transition),
            Param::from(who),
            Param::from(now),
            detail.map_or(Param::Null, |d| Param::from(d.to_string())),
        ]],
    )?;
    Ok(())
}

/// What a model's audit row names it by.
fn scope_of(m: &Model) -> Value {
    json!({
        "model": m.id, "name": m.name, "version": m.version, "digest": m.digest,
        "task": m.task, "slot": m.slot,
    })
}

fn audit_row(
    registry: &mut Registry,
    action: Action,
    who: &str,
    m: &Model,
    details: Value,
) -> Result<(), StoreError> {
    audit::record(
        registry,
        &Entry {
            principal: who,
            action,
            scope: scope_of(m),
            policy: None,
            job_id: None,
            details: Some(details),
        },
    )?;
    Ok(())
}

/// Run `f` inside a transaction of its own, rolled back on an error.
fn in_transaction<T>(
    store: &mut Store,
    f: impl FnOnce(&mut Store) -> Result<T, Error>,
) -> Result<T, Error> {
    store.begin()?;
    match f(store) {
        Ok(v) => {
            store.commit()?;
            Ok(v)
        }
        Err(e) => {
            store.rollback().ok();
            Err(e)
        }
    }
}

/// Check a card against `contracts/model/v1/card.schema.json` as far as the
/// engine relies on it, and answer the slot it names, `site` by default.
fn checked_slot(store: &mut Store, card: &Value) -> Result<String, Error> {
    if !card.is_object() {
        return Err(invalid("a card is a JSON object (contracts/model/v1)"));
    }
    let name = text_field(card, "name")?;
    if !name_ok(name) {
        return Err(invalid(format!(
            "{name} is not a model name: letters, digits, dot, underscore and hyphen, from a letter or digit"
        )));
    }
    let version = text_field(card, "version")?;
    if version.chars().any(char::is_whitespace) {
        return Err(invalid(format!("the version {version:?} holds a space")));
    }
    let kind = text_field(card, "kind")?;
    if !KINDS.contains(&kind) {
        return Err(invalid(format!(
            "{kind} is not a kind of model: {}",
            KINDS.join(", ")
        )));
    }
    if kind == "served" {
        return Err(refused(
            "a served model is Kvasir's to register; the engine registers the models whose answers become registry facts",
        ));
    }
    let digest = text_field(card, "digest")?;
    if !is_digest(digest) {
        return Err(invalid(format!(
            "{digest} is not a digest: sha256: and 64 lowercase hex digits of the artifact"
        )));
    }
    let task = text_field(card, "task")?;
    if !task_ok(task) {
        return Err(invalid(format!(
            "{task} is not a task, such as axis:body_part"
        )));
    }
    let slot = match &card["slot"] {
        Value::Null => "site".to_string(),
        Value::String(s) => s.clone(),
        other => return Err(invalid(format!("slot is a text, not {other}"))),
    };
    if slot != "site" {
        // Record 42 R4: one site-wide model per task, and a cohort's own
        // model in a slot of its own.
        let Some(cohort) = slot.strip_prefix("cohort:").filter(|c| !c.is_empty()) else {
            return Err(invalid(format!(
                "{slot} is not a slot: site, or cohort:<name>"
            )));
        };
        if crate::cohort::by_name(store, cohort)?.is_none() {
            return Err(Error::Unknown(format!(
                "no cohort named {cohort}, so there is no slot {slot}"
            )));
        }
    }
    // Record 43: the threshold its proposals are staged at, a probability.
    match card.get("threshold") {
        None | Some(Value::Null) => {}
        Some(t) => {
            let ok = t.as_f64().is_some_and(|p| p > 0.0 && p <= 1.0);
            if !ok {
                return Err(invalid(format!(
                    "threshold is a probability above 0 and at most 1, not {t}"
                )));
            }
        }
    }
    checked_trained_on(card)?;
    checked_parts_of_a_card(card)?;
    Ok(slot)
}

/// The label set a card says it was trained on is named by its digest.
fn checked_trained_on(card: &Value) -> Result<(), Error> {
    if let Some(t) = card.get("trained_on").filter(|v| !v.is_null()) {
        let set = t["label_set"].as_str().unwrap_or("");
        if !is_digest(set) {
            return Err(invalid(
                "trained_on names its label set by digest: sha256: and 64 lowercase hex digits",
            ));
        }
    }
    Ok(())
}

/// Record 40 R3 and 42 S7: the labels a model was fitted on are a label
/// set this registry wrote or registered, and none drawn from a sealed
/// certification sample, since a model fitted on the sample that certifies
/// it certifies nothing.
fn trained_on_usable(store: &mut Store, digest: &str) -> Result<(), Error> {
    let sets = crate::labels::by_digest(store, digest).map_err(labels_err)?;
    if sets.is_empty() {
        return Err(Error::Unknown(format!(
            "no label set has the digest {digest}; a model is trained on a set this registry wrote (nils labels export)"
        )));
    }
    for set in &sets {
        crate::labels::usable_for_training(store, set.id).map_err(labels_err)?;
    }
    Ok(())
}

/// Record 55 D4 and A5: the card's tested claims, speed, GPU need and
/// equivalence, each as the contract has it where the card gives it.
fn checked_parts_of_a_card(card: &Value) -> Result<(), Error> {
    let text = |v: &Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    match card.get("tested") {
        None | Some(Value::Null) => {}
        Some(Value::Array(claims)) => {
            for (i, c) in claims.iter().enumerate() {
                if !text(&c["claim"]) {
                    return Err(invalid(format!(
                        "tested claim {} says what was measured (claim)",
                        i + 1
                    )));
                }
                if !(c["value"].is_number() || text(&c["value"])) {
                    return Err(invalid(format!(
                        "tested claim {} ({}) gives its value, a number or a text",
                        i + 1,
                        c["claim"].as_str().unwrap_or("")
                    )));
                }
                if !text(&c["evidence"]) {
                    return Err(invalid(format!(
                        "tested claim {} ({}) names the evidence run its number comes from",
                        i + 1,
                        c["claim"].as_str().unwrap_or("")
                    )));
                }
            }
        }
        Some(_) => {
            return Err(invalid(
                "tested is a list of claims, each with its claim, value, bound and evidence",
            ));
        }
    }
    let gpu = match card.get("gpu") {
        None | Some(Value::Null) => None,
        Some(Value::String(g)) if ["none", "optional", "required"].contains(&g.as_str()) => {
            Some(g.as_str())
        }
        Some(other) => {
            return Err(invalid(format!(
                "gpu is none, optional or required, not {other}"
            )));
        }
    };
    match card.get("speed") {
        None | Some(Value::Null) => {}
        Some(Value::Object(s)) => {
            for (side, required) in [("cpu", true), ("gpu", false)] {
                match s.get(side) {
                    None | Some(Value::Null) if !required => {}
                    Some(one @ Value::Object(_)) => {
                        let rate = one["stacks_per_second"].as_f64();
                        if !rate.is_some_and(|r| r.is_finite() && r > 0.0) {
                            return Err(invalid(format!(
                                "speed.{side} gives stacks_per_second, a number above 0"
                            )));
                        }
                        if !text(&one["hardware"]) || !text(&one["evidence"]) {
                            return Err(invalid(format!(
                                "speed.{side} names the hardware it was measured on and its evidence run"
                            )));
                        }
                    }
                    _ => {
                        return Err(invalid(format!(
                            "speed.{side} is stacks_per_second, hardware and evidence"
                        )));
                    }
                }
            }
            if gpu == Some("none") && s.get("gpu").is_some_and(|g| !g.is_null()) {
                return Err(invalid(
                    "a card whose gpu is none states no speed with a GPU",
                ));
            }
        }
        Some(_) => {
            return Err(invalid(
                "speed is an object of cpu and gpu, each stacks_per_second, hardware and evidence",
            ));
        }
    }
    match card.get("equivalence") {
        None | Some(Value::Null) => {}
        Some(e @ Value::Object(_)) => {
            if !e["class"].as_str().is_some_and(|c| class_rank(c).is_some()) {
                return Err(invalid(format!(
                    "equivalence.class is one of {}",
                    CLASSES.join(", ")
                )));
            }
            match &e["tolerance"] {
                Value::Null => {}
                v if v.as_f64().is_some_and(|t| t.is_finite() && t >= 0.0) => {}
                v => {
                    return Err(invalid(format!(
                        "equivalence.tolerance is a number at least 0, not {v}"
                    )));
                }
            }
            if !text(&e["set"]) || !text(&e["evidence"]) {
                return Err(invalid(
                    "equivalence names the set it was measured on and its evidence run",
                ));
            }
        }
        Some(_) => {
            return Err(invalid(
                "equivalence is an object: class, tolerance, set and evidence",
            ));
        }
    }
    Ok(())
}

fn labels_err(e: crate::labels::Error) -> Error {
    match e {
        crate::labels::Error::Store(s) => Error::Store(s),
        crate::labels::Error::Invalid(m) => Error::Invalid(m),
        crate::labels::Error::NotFound(m) => Error::Unknown(m),
        crate::labels::Error::Refused(m) => Error::Refused(m),
    }
}

/// Register a model from its card. Refused when the digest is registered
/// already, or the name and version are taken, or a head names no
/// registered encoder.
pub fn register(registry: &mut Registry, card: &Value, who: &str) -> Result<Model, Error> {
    let slot = checked_slot(registry.store(), card)?;
    let (name, version, kind, digest, task) = (
        text_field(card, "name")?.to_string(),
        text_field(card, "version")?.to_string(),
        text_field(card, "kind")?.to_string(),
        text_field(card, "digest")?.to_string(),
        text_field(card, "task")?.to_string(),
    );
    let store = registry.store();
    if let Some(held) = by_digest(store, &digest)? {
        return Err(refused(format!(
            "{digest} is registered already, as model {} ({})",
            held.id,
            held.label()
        )));
    }
    if let Some(held) = resolve(store, &format!("{name}@{version}"))? {
        return Err(refused(format!(
            "{name}@{version} is taken, by model {} ({}); a new artifact is a new version",
            held.id, held.digest
        )));
    }
    // A head names its encoders, which must be registered: a new encoder
    // makes its heads stale, and the edges are what say which. Record 43:
    // `encoders` lists them in order; `encoder` alone is the one, and
    // given beside the list it is one of them.
    let encoders = encoders_of(store, card, &kind)?;
    let encoder = encoders.first().copied();
    // record 55 E2: a cascade's parts are registered models, none retired
    parts_of(store, card)?;
    let trained_on = card["trained_on"]["label_set"].as_str().map(str::to_string);
    if let Some(digest) = &trained_on {
        trained_on_usable(store, digest)?;
    }
    let pack_version = card["pack_version"].as_str().map(str::to_string);
    let now = now_iso();
    let mut stored = card.clone();
    stored["slot"] = Value::from(slot.as_str());
    let id = in_transaction(store, |store| {
        let id = store
            .insert(
                &Insert::new(
                    table("model"),
                    &[
                        "name",
                        "version",
                        "kind",
                        "digest",
                        "task",
                        "slot",
                        "state",
                        "card",
                        "encoder_model_id",
                        "trained_on",
                        "pack_version",
                        "registered_by",
                        "registered_at",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(name.as_str()),
                    Param::from(version.as_str()),
                    Param::from(kind.as_str()),
                    Param::from(digest.as_str()),
                    Param::from(task.as_str()),
                    Param::from(slot.as_str()),
                    Param::from("registered"),
                    Param::from(stored.to_string()),
                    encoder.map_or(Param::Null, Param::Int),
                    trained_on.as_deref().map_or(Param::Null, Param::from),
                    pack_version.as_deref().map_or(Param::Null, Param::from),
                    Param::from(who),
                    Param::from(now.as_str()),
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the model was not written back".into()))?
            .int(0)?;
        for (position, e) in encoders.iter().enumerate() {
            store.insert(
                &Insert::new(
                    table("model_encoder"),
                    &["model_id", "encoder_model_id", "position"],
                ),
                &[vec![
                    Param::Int(id),
                    Param::Int(*e),
                    Param::Int(position as i64),
                ]],
            )?;
        }
        event(store, id, "registered", who, &now, None)?;
        Ok(id)
    })?;
    let m = get(registry.store(), id)?.expect("the model just written");
    audit_row(
        registry,
        Action::ModelRegister,
        who,
        &m,
        json!({
            "kind": m.kind, "encoder_model_id": m.encoder_model_id,
            "encoder_model_ids": m.encoder_model_ids, "trained_on": m.trained_on,
        }),
    )?;
    Ok(m)
}

/// The encoders a card names, resolved to registered encoders in order:
/// `encoders`, or `encoder` alone. Refused for a digest that is not one,
/// an encoder not registered or not an encoder, one named twice, an
/// `encoder` beside a list that does not hold it, and a head naming none.
fn encoders_of(store: &mut Store, card: &Value, kind: &str) -> Result<Vec<i64>, Error> {
    let digest_of = |e: &Value| -> Result<String, Error> {
        let d = e["digest"].as_str().unwrap_or("");
        if !is_digest(d) {
            return Err(invalid("an encoder is named by its digest"));
        }
        Ok(d.to_string())
    };
    let single = match card.get("encoder").filter(|v| !v.is_null()) {
        Some(e) => Some(digest_of(e)?),
        None => None,
    };
    let listed: Vec<String> = match card.get("encoders").filter(|v| !v.is_null()) {
        None => single.iter().cloned().collect(),
        Some(Value::Array(items)) if !items.is_empty() => {
            let list = items.iter().map(digest_of).collect::<Result<Vec<_>, _>>()?;
            if let Some(one) = &single
                && !list.contains(one)
            {
                return Err(invalid(format!(
                    "encoder {one} is not one of the card's encoders"
                )));
            }
            list
        }
        Some(_) => {
            return Err(invalid(
                "encoders is a list of the encoders a head reads, each by its digest",
            ));
        }
    };
    if listed.is_empty() && kind == "head" {
        return Err(invalid(
            "a head names the encoders whose features it reads (encoders, or encoder.digest)",
        ));
    }
    let mut out = Vec::with_capacity(listed.len());
    for d in &listed {
        let Some(m) = by_digest(store, d)? else {
            return Err(Error::Unknown(format!(
                "the encoder {d} is not registered; register it first"
            )));
        };
        if m.kind != "encoder" {
            return Err(invalid(format!(
                "model {} ({}) is a {}, not an encoder",
                m.id,
                m.label(),
                m.kind
            )));
        }
        if out.contains(&m.id) {
            return Err(invalid(format!("the encoder {d} is named twice")));
        }
        out.push(m.id);
    }
    Ok(out)
}

/// The most parts a cascade's card names.
const MOST_PARTS: usize = 16;

/// Whether a text names a pipeline input, as a descriptor's input ids do:
/// a lowercase letter, then lowercase letters, digits and underscores.
fn part_name_ok(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The parts a card names (record 55 E2), in the card's order: each the
/// name of the pipeline input it fills and a registered model that is not
/// retired. Refused for a `parts` that is not an object of such names and
/// digests, a part that is not registered or is retired, and more than
/// [`MOST_PARTS`]. None for a card without parts.
fn parts_of(store: &mut Store, card: &Value) -> Result<Vec<(String, Model)>, Error> {
    let parts = match card.get("parts") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Object(map)) if !map.is_empty() && map.len() <= MOST_PARTS => map,
        Some(_) => {
            return Err(invalid(format!(
                "parts is an object of at most {MOST_PARTS} parts, each the pipeline input it fills and the digest of a registered model"
            )));
        }
    };
    let own = card["digest"].as_str().unwrap_or("");
    let mut out = Vec::with_capacity(parts.len());
    for (name, digest) in parts {
        if !part_name_ok(name) {
            return Err(invalid(format!(
                "{name} is not a part's name: the pipeline input it fills, such as head"
            )));
        }
        let d = digest.as_str().unwrap_or("");
        if !is_digest(d) {
            return Err(invalid(format!(
                "the part {name} is named by its digest: sha256: and 64 lowercase hex digits"
            )));
        }
        if d == own {
            return Err(invalid(format!("the part {name} is the model itself")));
        }
        let Some(m) = by_digest(store, d)? else {
            return Err(Error::Unknown(format!(
                "the part {name}, {d}, is not registered; register it first"
            )));
        };
        if m.state == "retired" {
            return Err(refused(format!(
                "the part {name} is model {} ({}), which is retired",
                m.id,
                m.label()
            )));
        }
        out.push((name.clone(), m));
    }
    Ok(out)
}

/// The parts a registered model's card names (record 55 E2): each the name
/// of the pipeline input it fills and the model, as they are now (a part
/// may have been retired since). Empty for a model that is no cascade.
pub fn parts(store: &mut Store, m: &Model) -> Result<Vec<(String, Model)>, StoreError> {
    let Some(map) = m.card.get("parts").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(map.len());
    for (name, digest) in map {
        if let Some(part) = digest
            .as_str()
            .map(|d| by_digest(store, d))
            .transpose()?
            .flatten()
        {
            out.push((name.clone(), part));
        }
    }
    Ok(out)
}

/// The admitted or promoted cascades that read `part` (record 55 E2), each
/// with the name of the input the part fills, leaving out those `except`
/// names.
fn holders(
    store: &mut Store,
    part: &Model,
    except: &[i64],
) -> Result<Vec<(String, Model)>, StoreError> {
    Ok(list(store, &Filter::default())?
        .into_iter()
        .filter(|m| matches!(m.state.as_str(), "admitted" | "promoted") && !except.contains(&m.id))
        .filter_map(|m| {
            let name = m
                .card
                .get("parts")?
                .as_object()?
                .iter()
                .find(|(_, d)| d.as_str() == Some(part.digest.as_str()))?
                .0
                .clone();
            Some((name, m))
        })
        .collect())
}

/// Refused for a cascade one of whose parts is retired (record 55 E2): it
/// can be neither admitted nor promoted, since no run would be given it.
fn parts_stand(store: &mut Store, m: &Model, doing: &str) -> Result<(), Error> {
    if let Some((name, gone)) = parts(store, m)?
        .into_iter()
        .find(|(_, p)| p.state == "retired")
    {
        return Err(refused(format!(
            "model {} ({}) reads its part {name}, model {} ({}), which is retired, so it is not {doing}; register a cascade of parts that stand",
            m.id,
            m.label(),
            gone.id,
            gone.label()
        )));
    }
    Ok(())
}

/// Refused where retiring `old` would leave an admitted or promoted
/// cascade (other than those `except` names) without one of its parts
/// (record 55 E2): the cascade names it, and goes first.
fn not_held(store: &mut Store, old: &Model, except: &[i64], why: &str) -> Result<(), Error> {
    if let Some((name, c)) = holders(store, old, except)?.into_iter().next() {
        return Err(refused(format!(
            "{why}: model {} ({}) is the part {name} of cascade {} ({}), which is {}; retire the cascade first",
            old.id,
            old.label(),
            c.id,
            c.label(),
            c.state
        )));
    }
    Ok(())
}

/// Check a check against `contracts/model/v1/lifecycle.schema.json`: a
/// suite, a verdict and the checks behind it, which must agree.
fn checked_check(check: &Value) -> Result<bool, Error> {
    if !check.is_object() {
        return Err(invalid(
            "a check is a JSON object: suite, passed and checks",
        ));
    }
    if check["suite"].as_str().is_none_or(str::is_empty) {
        return Err(invalid("a check names its suite"));
    }
    let Some(passed) = check["passed"].as_bool() else {
        return Err(invalid("a check says whether it passed"));
    };
    let Some(checks) = check["checks"].as_array().filter(|c| !c.is_empty()) else {
        return Err(invalid(
            "a check lists what was checked: checks, each a name and whether it passed",
        ));
    };
    let mut all = true;
    for c in checks {
        if c["name"].as_str().is_none_or(str::is_empty) {
            return Err(invalid("each check has a name"));
        }
        let Some(p) = c["passed"].as_bool() else {
            return Err(invalid(format!(
                "the check {} says whether it passed",
                c["name"]
            )));
        };
        all &= p;
    }
    if all != passed {
        return Err(invalid(format!(
            "the check says passed {passed} while its checks say {all}"
        )));
    }
    Ok(passed)
}

fn known(store: &mut Store, id: i64) -> Result<Model, Error> {
    get(store, id)?.ok_or_else(|| Error::Unknown(format!("no model {id}")))
}

/// Record a check. One that passed admits a registered model and stays on
/// it; one that failed is an event and moves nothing. A promoted or retired
/// model is not checked again: register a new version.
pub fn admit(registry: &mut Registry, id: i64, check: &Value, who: &str) -> Result<Model, Error> {
    let mut passed = checked_check(check)?;
    let store = registry.store();
    let m = known(store, id)?;
    // record 55 A5: a model that takes a GPU is admitted only with its CPU
    // and GPU answers measured on a fixed set, of a class no worse than its
    // card's; the measure is kept as one of the check's checks
    let mut check = check.clone();
    if m.gpu() != "none" {
        let measured = check.get("equivalence").cloned().unwrap_or(Value::Null);
        if let Some(d) = measured["digest"].as_str()
            && d != m.digest
        {
            return Err(invalid(format!(
                "the equivalence was measured on {d}, not on {} ({}): it is tested on the file that ships",
                m.digest,
                m.label()
            )));
        }
        let short = equivalence_short(&m, &measured);
        let row = json!({
            "name": "cpu_gpu_equivalence",
            "passed": short.is_none(),
            "value": measured["class"].as_str().unwrap_or("not measured"),
            "threshold": m.card["equivalence"]["class"].as_str(),
            "why": short,
        });
        if let Some(list) = check["checks"].as_array_mut() {
            list.retain(|c| c["name"] != "cpu_gpu_equivalence");
            list.push(row);
        }
        passed &= short.is_none();
        check["passed"] = Value::Bool(passed);
    }
    let check = &check;
    if matches!(m.state.as_str(), "promoted" | "retired") {
        return Err(refused(format!(
            "model {} ({}) is {}; a check belongs before a promotion, so register a new version",
            m.id,
            m.label(),
            m.state
        )));
    }
    parts_stand(store, &m, "admitted")?;
    let now = now_iso();
    let mut recorded = check.clone();
    if recorded.get("at").is_none() {
        recorded["at"] = Value::from(now.as_str());
    }
    let failed: Vec<Value> = check["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["passed"] == false)
        .map(|c| c["name"].clone())
        .collect();
    let d = store.dialect();
    in_transaction(store, |store| {
        if passed {
            let sql = format!(
                "UPDATE {} SET state = 'admitted', gate = {}, admitted_by = {}, admitted_at = {} WHERE id = {}",
                store.qualified("model"),
                d.param(1, Type::Json),
                d.param(2, Type::Text),
                d.param(3, Type::Timestamp),
                d.param(4, Type::Int),
            );
            store.execute(
                &sql,
                &[
                    Param::from(recorded.to_string()),
                    Param::from(who),
                    Param::from(now.as_str()),
                    Param::Int(id),
                ],
            )?;
        } else if m.state == "registered" {
            let sql = format!(
                "UPDATE {} SET gate = {} WHERE id = {}",
                store.qualified("model"),
                d.param(1, Type::Json),
                d.param(2, Type::Int),
            );
            store.execute(&sql, &[Param::from(recorded.to_string()), Param::Int(id)])?;
        }
        event(
            store,
            id,
            if passed {
                "admitted"
            } else {
                "admission_failed"
            },
            who,
            &now,
            Some(json!({ "check": recorded, "failed": failed })),
        )?;
        Ok(())
    })?;
    let m = known(registry.store(), id)?;
    audit_row(
        registry,
        Action::ModelAdmit,
        who,
        &m,
        json!({ "passed": passed, "suite": check["suite"], "failed": failed }),
    )?;
    Ok(m)
}

/// What a promotion did.
#[derive(Debug, Clone, PartialEq)]
pub struct Promoted {
    pub model: Model,
}

/// Promote a model: refused unless it was admitted by a check that passed,
/// and for a cascade with a retired part (record 55 E2). Since record 55
/// D4 a promotion retires nothing: several models answer a task and slot,
/// and the site chooses the one that does ([`choose`]); a promoted model
/// is a default only where nobody chose and no certificate names one.
pub fn promote(
    registry: &mut Registry,
    id: i64,
    who: &str,
    review_item: Option<i64>,
    why: Option<&str>,
) -> Result<Promoted, Error> {
    let store = registry.store();
    let m = known(store, id)?;
    match m.state.as_str() {
        "admitted" => {}
        "registered" => {
            return Err(refused(format!(
                "model {} ({}) is registered and not admitted: a promotion needs a recorded check that passed (nils model admit)",
                m.id,
                m.label()
            )));
        }
        other => {
            return Err(refused(format!(
                "model {} ({}) is {other} and cannot be promoted",
                m.id,
                m.label()
            )));
        }
    }
    if m.check.as_ref().and_then(|c| c["passed"].as_bool()) != Some(true) {
        return Err(refused(format!(
            "model {} ({}) holds no check that passed",
            m.id,
            m.label()
        )));
    }
    parts_stand(store, &m, "promoted")?;
    let d = store.dialect();
    let now = now_iso();
    in_transaction(store, |store| {
        let sql = format!(
            "UPDATE {} SET state = 'promoted', promoted_by = {}, promoted_at = {}, review_item = {} WHERE id = {}",
            store.qualified("model"),
            d.param(1, Type::Text),
            d.param(2, Type::Timestamp),
            d.param(3, Type::Int),
            d.param(4, Type::Int),
        );
        store.execute(
            &sql,
            &[
                Param::from(who),
                Param::from(now.as_str()),
                review_item.map_or(Param::Null, Param::Int),
                Param::Int(id),
            ],
        )?;
        event(
            store,
            id,
            "promoted",
            who,
            &now,
            Some(json!({ "review_item": review_item, "why": why })),
        )?;
        Ok(())
    })?;
    let model = known(registry.store(), id)?;
    audit_row(
        registry,
        Action::ModelPromote,
        who,
        &model,
        json!({ "review_item": review_item, "why": why }),
    )?;
    Ok(Promoted { model })
}

/// Retire a model. Its decisions stay, and keep naming it. Refused for a
/// part of an admitted or promoted cascade (record 55 E2): the cascade is
/// retired first.
pub fn retire(
    registry: &mut Registry,
    id: i64,
    who: &str,
    why: Option<&str>,
) -> Result<Model, Error> {
    let store = registry.store();
    let m = known(store, id)?;
    if m.state == "retired" {
        return Err(refused(format!(
            "model {} ({}) is retired already",
            m.id,
            m.label()
        )));
    }
    not_held(store, &m, &[], "a served cascade reads it")?;
    let now = now_iso();
    let d = store.dialect();
    in_transaction(store, |store| {
        let sql = format!(
            "UPDATE {} SET state = 'retired', retired_by = {}, retired_at = {} WHERE id = {}",
            store.qualified("model"),
            d.param(1, Type::Text),
            d.param(2, Type::Timestamp),
            d.param(3, Type::Int),
        );
        store.execute(
            &sql,
            &[Param::from(who), Param::from(now.as_str()), Param::Int(id)],
        )?;
        event(store, id, "retired", who, &now, Some(json!({ "why": why })))?;
        Ok(())
    })?;
    let m = known(registry.store(), id)?;
    audit_row(
        registry,
        Action::ModelRetire,
        who,
        &m,
        json!({ "why": why }),
    )?;
    Ok(m)
}

// ------------------------------------------- claims added to a card

/// The claims a registered card may gain (record 55, ruling 31 (a)), in
/// the order they are added: the label set it was trained on, its tested
/// claims and its speed. Nothing else joins a card once it is registered:
/// a card that lacks anything else is a new version of the model.
pub const CLAIMS: [&str; 3] = ["trained_on", "tested", "speed"];

/// What an addition did.
#[derive(Debug, Clone, PartialEq)]
pub struct Added {
    /// The model as it is now, its card holding the claims.
    pub model: Model,
    /// The fields of [`missing`] the claims filled, in the card's order.
    pub fields: Vec<&'static str>,
    /// The claims as they were added, in the card's own shape.
    pub added: Value,
}

/// Whether a card's value states nothing: absent, null, or an empty list.
fn states_nothing(v: &Value) -> bool {
    v.is_null() || v.as_array().is_some_and(Vec::is_empty)
}

/// Refused unless a card lacks `field` ([`missing`] names it) and states
/// nothing at `held`, its value there.
fn may_gain(m: &Model, before: &[&str], field: &str, held: &Value) -> Result<(), Error> {
    if !states_nothing(held) {
        return Err(refused(format!(
            "model {} ({}) states {field} on its card already, and nothing a card states changes (record 55, ruling 31 (a)): another {field} is a new version of the model",
            m.id,
            m.label()
        )));
    }
    if !before.contains(&field) {
        return Err(refused(if field == "speed.gpu" {
            format!(
                "model {} ({}) needs no GPU (its gpu is {}), so its card states no speed with a GPU",
                m.id,
                m.label(),
                m.gpu()
            )
        } else {
            format!(
                "model {} ({}) does not lack {field} on its card",
                m.id,
                m.label()
            )
        }));
    }
    Ok(())
}

/// Add to a registered model's card the claims it lacks (record 55, ruling
/// 31 (a), Nima 2026-10-16: a registered card may gain its missing claims,
/// the label set it was trained on, its tested claims, its speed;
/// additions only, each one audited; nothing already on a card changes).
///
/// `claims` is an object of one or more of [`CLAIMS`], each in the card's
/// own shape (`contracts/model/v1/card.schema.json`), `speed` holding
/// `cpu`, `gpu` or both. A claim is refused where the card states it
/// already, and where [`missing`] does not name it (a speed with a GPU on a
/// card whose gpu is none); any other field is refused, since any other
/// change is a new version of the model. The card with the claims is
/// checked as registration checks a card, and the label set `trained_on`
/// names must be one this registry holds and usable for training under the
/// seals in force (records 40 R3 and 55 E6). `digest`, where given, must be
/// the model's: the claims were written for that artifact.
///
/// The card gains the claims and the model's `trained_on` follows. The
/// addition is an event `claims_added`, holding what was added, the fields
/// filled and what the card lacked before and after, and an audit row
/// `model.add_claims`, both written in the transaction that writes the
/// card, under the card's lock. A model in any state may gain claims: a
/// claim says what a model is and changes nothing it decided.
pub fn add_claims(
    registry: &mut Registry,
    id: i64,
    claims: &Value,
    who: &str,
    why: Option<&str>,
    digest: Option<&str>,
) -> Result<Added, Error> {
    let Some(given) = claims.as_object().filter(|m| !m.is_empty()) else {
        return Err(invalid(
            "the claims are an object of trained_on, tested or speed, each in the card's own shape (contracts/model/v1)",
        ));
    };
    if let Some(other) = given.keys().find(|k| !CLAIMS.contains(&k.as_str())) {
        return Err(refused(format!(
            "{other} is not a claim a registered card gains: only trained_on, tested and speed join a card once it is registered (record 55, ruling 31 (a)); any other change is a new version of the model"
        )));
    }
    if let Some(k) = given.iter().find(|(_, v)| v.is_null()).map(|(k, _)| k) {
        return Err(invalid(format!(
            "{k} is given as null: a claim added states something"
        )));
    }
    let why = why.map(str::trim).filter(|w| !w.is_empty());
    // the label set is read (its labels.tsv, against the seals in force)
    // before the card is locked, as registration reads it before it writes
    let label_set = given
        .get("trained_on")
        .and_then(|t| t["label_set"].as_str())
        .map(str::to_string);
    if let Some(set) = &label_set {
        checked_trained_on(&json!({ "trained_on": given["trained_on"] }))?;
        trained_on_usable(registry.store(), set)?;
    }
    in_transaction(registry.store(), |store| {
        crate::lock::take_for_transaction(store, crate::lock::MODEL_CARD)?;
        let m = known(store, id)?;
        if let Some(d) = digest
            && d != m.digest
        {
            return Err(refused(format!(
                "the claims are for {d}, and model {} ({}) is {}",
                m.id,
                m.label(),
                m.digest
            )));
        }
        if !m.card.is_object() {
            return Err(refused(format!(
                "model {} ({}) holds no card that reads, so nothing is added to it",
                m.id,
                m.label()
            )));
        }
        let before = m.missing();
        let mut card = m.card.clone();
        let mut added = serde_json::Map::new();
        let mut fields: Vec<&'static str> = Vec::new();
        for key in CLAIMS {
            let Some(value) = given.get(key) else {
                continue;
            };
            if key != "speed" {
                if key == "tested" && !value.as_array().is_some_and(|a| !a.is_empty()) {
                    return Err(invalid(
                        "tested adds a list of at least one claim, each with its claim, value and evidence",
                    ));
                }
                may_gain(&m, &before, key, &card[key])?;
                card[key] = value.clone();
                added.insert(key.to_string(), value.clone());
                fields.push(key);
                continue;
            }
            let Some(sides) = value.as_object().filter(|s| !s.is_empty()) else {
                return Err(invalid(
                    "speed is an object of cpu, gpu or both, each stacks_per_second, hardware and evidence",
                ));
            };
            if let Some(other) = sides.keys().find(|s| !matches!(s.as_str(), "cpu" | "gpu")) {
                return Err(invalid(format!("speed holds cpu and gpu, not {other}")));
            }
            if !card["speed"].is_null() && !card["speed"].is_object() {
                return Err(refused(format!(
                    "model {} ({}) states speed on its card already, as something other than cpu and gpu",
                    m.id,
                    m.label()
                )));
            }
            let mut speed = serde_json::Map::new();
            for (side, field) in [("cpu", "speed.cpu"), ("gpu", "speed.gpu")] {
                let Some(one) = sides.get(side) else {
                    continue;
                };
                if one.is_null() {
                    return Err(invalid(format!(
                        "{field} is given as null: a claim added states something"
                    )));
                }
                may_gain(&m, &before, field, &card["speed"][side])?;
                if !card["speed"].is_object() {
                    card["speed"] = json!({});
                }
                card["speed"][side] = one.clone();
                speed.insert(side.to_string(), one.clone());
                fields.push(field);
            }
            added.insert(key.to_string(), Value::Object(speed));
        }
        // checked as registration checks a card (its label set above)
        checked_trained_on(&card)?;
        checked_parts_of_a_card(&card)?;
        let after = missing(&card);
        let added = Value::Object(added);
        let mut sets = vec![("card", Param::from(card.to_string()))];
        if let Some(set) = &label_set {
            sets.push(("trained_on", Param::from(set.as_str())));
        }
        store.update_by_id(table("model"), &sets, "id", id)?;
        let now = now_iso();
        event(
            store,
            id,
            "claims_added",
            who,
            &now,
            Some(json!({
                "fields": fields, "added": added, "missing_before": before,
                "missing_after": after, "why": why,
            })),
        )?;
        let model = known(store, id)?;
        audit::record_in(
            store,
            &Entry {
                principal: who,
                action: Action::ModelAddClaims,
                scope: scope_of(&model),
                policy: None,
                job_id: None,
                details: Some(json!({
                    "fields": fields, "added": added, "missing": after, "why": why,
                })),
            },
        )?;
        Ok(Added {
            model,
            fields,
            added,
        })
    })
}

/// The claims a card gained after it was registered (record 55, ruling 31
/// (a)), oldest first, read from the model's events: when, by whom, the
/// fields filled, what was added and why.
pub fn additions(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|e| e["transition"] == "claims_added")
        .map(|e| {
            json!({
                "at": e["at"], "by": e["by"], "fields": e["detail"]["fields"],
                "added": e["detail"]["added"], "why": e["detail"]["why"],
            })
        })
        .collect()
}

// ------------------------------------------ CPU and GPU equivalence

/// The effective outcome of a proposal at its model's threshold: the value
/// where the model answers, none ("not sure") where it does not. A model
/// whose card names no threshold answers its value.
fn effective(p: &crate::proposals::Proposal, threshold: Option<f64>) -> Option<String> {
    let confidence = p.probabilities.get(&p.value).copied().unwrap_or(0.0);
    match threshold {
        Some(t) if confidence < t => None,
        _ => Some(p.value.clone()),
    }
}

/// Measure the class of the difference between a model's answers on a CPU
/// and on a GPU (record 55 A5, spec 10.5 and 16.3), from the proposals of
/// two runs over the same stacks: each read at its own model's threshold
/// (its card's), keyed by stack and axis. A stack one run answers and the
/// other does not is "not sure" on that side.
///
/// - **I**, the same answer always: no effective outcome and no most
///   probable value changed, and no calibrated probability further apart
///   than the card's tolerance (0.001 where the card gives none for I);
/// - **II**, the same answer within the tolerance on the model's score: no
///   effective outcome changed, and no probability further apart than the
///   card's tolerance for II;
/// - **III**, every change between an answer and not sure: no value turned
///   into another, and the share of changes at most the card's tolerance
///   for III where it gives one;
/// - none, where a value turned into another.
///
/// Answers `{class, tolerance, n, changes, flips, top_changed,
/// max_difference}`; refused when the two runs share no stack.
pub fn equivalence(
    store: &mut Store,
    m: &Model,
    cpu: &[crate::proposals::Proposal],
    gpu: &[crate::proposals::Proposal],
) -> Result<Value, Error> {
    use std::collections::BTreeMap;
    let mut thresholds: BTreeMap<String, Option<f64>> = BTreeMap::new();
    let mut threshold_of = |store: &mut Store,
                            p: &crate::proposals::Proposal|
     -> Result<Option<f64>, Error> {
        let key = p
            .model_id
            .map(|i| i.to_string())
            .or_else(|| p.model_digest.clone())
            .unwrap_or_default();
        if let Some(t) = thresholds.get(&key) {
            return Ok(*t);
        }
        let model = match (p.model_id, p.model_digest.as_deref()) {
            (Some(id), _) => get(store, id)?,
            (None, Some(d)) => by_digest(store, d)?,
            (None, None) => None,
        };
        let Some(model) = model else {
            return Err(Error::Unknown(format!(
                "a proposal on stack {} names a model that is not registered ({key}); the comparison reads each answer at its model's threshold",
                p.stack_id
            )));
        };
        let t = model.threshold();
        thresholds.insert(key, t);
        Ok(t)
    };
    type Side = BTreeMap<(i64, String), (Option<String>, String, BTreeMap<String, f64>)>;
    let mut side =
        |store: &mut Store, list: &[crate::proposals::Proposal]| -> Result<Side, Error> {
            let mut out = Side::new();
            for p in list {
                let t = threshold_of(store, p)?;
                out.insert(
                    (p.stack_id, p.axis.clone()),
                    (effective(p, t), p.value.clone(), p.probabilities.clone()),
                );
            }
            Ok(out)
        };
    let a = side(store, cpu)?;
    let b = side(store, gpu)?;
    let keys: std::collections::BTreeSet<&(i64, String)> = a.keys().chain(b.keys()).collect();
    if a.keys().all(|k| !b.contains_key(k)) {
        return Err(invalid(
            "the two runs answer no stack in common; an equivalence compares one fixed set run twice",
        ));
    }
    let (mut changes, mut flips, mut top_changed) = (0i64, 0i64, 0i64);
    let mut max_difference = 0f64;
    for k in &keys {
        let (ea, ta, pa) = match a.get(*k) {
            Some((e, t, p)) => (e.clone(), Some(t.as_str()), Some(p)),
            None => (None, None, None),
        };
        let (eb, tb, pb) = match b.get(*k) {
            Some((e, t, p)) => (e.clone(), Some(t.as_str()), Some(p)),
            None => (None, None, None),
        };
        match (&ea, &eb) {
            (Some(x), Some(y)) if x != y => flips += 1,
            (Some(_), None) | (None, Some(_)) => changes += 1,
            _ => {}
        }
        if ta != tb {
            top_changed += 1;
        }
        if let (Some(pa), Some(pb)) = (pa, pb) {
            for class in pa.keys().chain(pb.keys()) {
                let d = (pa.get(class).copied().unwrap_or(0.0)
                    - pb.get(class).copied().unwrap_or(0.0))
                .abs();
                if d > max_difference {
                    max_difference = d;
                }
            }
        }
    }
    let n = keys.len() as i64;
    let stated = m.card["equivalence"]["class"].as_str().unwrap_or("");
    let stated_tolerance = m.card["equivalence"]["tolerance"].as_f64();
    // class I's tolerance is the card's where it states class I; class
    // II's where it states I or II; else the speed study's
    let tolerance_i = match stated {
        "I" => stated_tolerance.unwrap_or(CLASS_I_TOLERANCE),
        _ => CLASS_I_TOLERANCE,
    };
    let tolerance_ii = match stated {
        "I" | "II" => stated_tolerance.unwrap_or(CLASS_I_TOLERANCE),
        _ => CLASS_I_TOLERANCE,
    };
    let share_tolerance = if stated == "III" {
        stated_tolerance
    } else {
        None
    };
    let class = if flips == 0 && changes == 0 && top_changed == 0 && max_difference <= tolerance_i {
        Some("I")
    } else if flips == 0 && changes == 0 && max_difference <= tolerance_ii {
        Some("II")
    } else if flips == 0 && share_tolerance.is_none_or(|s| (changes as f64) <= s * n as f64) {
        Some("III")
    } else {
        None
    };
    Ok(json!({
        "class": class,
        "tolerance": stated_tolerance,
        "n": n,
        "changes": changes,
        "flips": flips,
        "top_changed": top_changed,
        "max_difference": max_difference,
    }))
}

/// Whether a measured equivalence meets the class a card states: refused
/// words where it does not (record 55 A5: a model whose class is worse
/// than its card states is not admitted).
fn equivalence_short(m: &Model, measured: &Value) -> Option<String> {
    let stated = m.card["equivalence"]["class"].as_str();
    let Some(stated) = stated.and_then(class_rank).map(|r| CLASSES[r]) else {
        return Some(format!(
            "{} needs a GPU {} and its card states no equivalence class for its CPU and GPU answers",
            m.label(),
            m.gpu()
        ));
    };
    match measured["class"].as_str().and_then(class_rank) {
        None if measured.is_null() || measured.get("class").is_none() => Some(format!(
            "{} takes a GPU ({}), and the check carries no measured CPU and GPU equivalence (nils model admit --cpu run:<id> --gpu run:<id>)",
            m.label(),
            m.gpu()
        )),
        None => Some(format!(
            "on the equivalence set {} of {} answers changed from one value to another between the CPU and the GPU, worse than class III",
            measured["flips"], measured["n"]
        )),
        Some(r) if r > class_rank(stated).unwrap_or(0) => Some(format!(
            "the measured class is {}, worse than class {stated} its card states",
            CLASSES[r]
        )),
        Some(_) => None,
    }
}

// ------------------------------------------------- the chosen model

/// Why a model answers its task and slot (record 55 D4): a site's admin
/// chose it, a recorded certificate names it, or, where neither, it is the
/// one promoted last, or the newest admitted.
pub const CHOSEN_BY: [&str; 4] = ["site", "certificate", "promoted", "admitted"];

/// The model that answers a task and slot, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen {
    pub model: Model,
    /// One of [`CHOSEN_BY`].
    pub by: &'static str,
    /// The certificate that names it, where that is why.
    pub certificate_id: Option<i64>,
    /// Who chose it, when and why, where a person did.
    pub chosen_by: Option<String>,
    pub chosen_at: Option<String>,
    pub why: Option<String>,
}

impl Chosen {
    /// As the doors and `nils model chosen --json` answer it.
    pub fn as_json(&self) -> Value {
        json!({
            "model_id": self.model.id, "name": self.model.name, "version": self.model.version,
            "by": self.by, "certificate_id": self.certificate_id,
            "chosen_by": self.chosen_by, "chosen_at": self.chosen_at, "why": self.why,
        })
    }
}

/// One row of the choice's history.
#[derive(Debug, Clone, PartialEq)]
struct Choice {
    model_id: Option<i64>,
    chosen_by: String,
    chosen_at: String,
    why: Option<String>,
}

/// The choice in force for a task and slot: its newest row.
fn choice_now(store: &mut Store, task: &str, slot: &str) -> Result<Option<Choice>, StoreError> {
    let d = store.dialect();
    let t = table("model_choice");
    let sql = format!(
        "SELECT model_id, chosen_by, {}, why FROM {} WHERE task = {} AND slot = {} ORDER BY id DESC LIMIT 1",
        d.text_of(t.column("chosen_at").expect("chosen_at")),
        store.qualified("model_choice"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    store
        .query_opt(&sql, &[Param::from(task), Param::from(slot)])?
        .map(|r| {
            Ok(Choice {
                model_id: r.opt_int(0)?,
                chosen_by: r.text(1)?.to_string(),
                chosen_at: r.text(2)?.to_string(),
                why: r.opt_text(3)?.map(str::to_string),
            })
        })
        .transpose()
}

/// Whether a model may answer now: admitted or promoted, and, for a
/// cascade, none of its parts retired (record 55 E2).
fn may_answer(store: &mut Store, m: &Model) -> Result<bool, StoreError> {
    if !m.answers() {
        return Ok(false);
    }
    Ok(!parts(store, m)?.iter().any(|(_, p)| p.state == "retired"))
}

/// Whether a model is a cascade (record 55 E2): its card names parts.
pub fn is_cascade(m: &Model) -> bool {
    m.card
        .get("parts")
        .and_then(Value::as_object)
        .is_some_and(|p| !p.is_empty())
}

/// The model that answers a task and slot now (record 55 D4), and why: the
/// one a site's admin chose, while it may answer; else the one the newest
/// recorded certificate names; else the one promoted last; else the newest
/// admitted that is no cascade, since a cascade answers only once a person
/// chose, certified or promoted it. None where no model may answer.
pub fn chosen(store: &mut Store, task: &str, slot: &str) -> Result<Option<Chosen>, StoreError> {
    let choice = choice_now(store, task, slot)?;
    if let Some(c) = &choice
        && let Some(id) = c.model_id
        && let Some(m) = get(store, id)?
        && m.task == task
        && m.slot == slot
        && may_answer(store, &m)?
    {
        return Ok(Some(Chosen {
            model: m,
            by: "site",
            certificate_id: None,
            chosen_by: Some(c.chosen_by.clone()),
            chosen_at: Some(c.chosen_at.clone()),
            why: c.why.clone(),
        }));
    }
    // newest first
    let mut candidates = Vec::new();
    for m in list(
        store,
        &Filter {
            task: Some(task),
            slot: Some(slot),
            ..Default::default()
        },
    )?
    .into_iter()
    .rev()
    {
        if may_answer(store, &m)? {
            candidates.push(m);
        }
    }
    let default = |model: Model, by: &'static str, certificate_id: Option<i64>| Chosen {
        model,
        by,
        certificate_id,
        chosen_by: None,
        chosen_at: None,
        why: None,
    };
    let certificates = crate::labels::certificates(store).map_err(|e| match e {
        crate::labels::Error::Store(s) => s,
        other => StoreError::Message(other.to_string()),
    })?;
    for cert in &certificates {
        if let Some(m) = candidates.iter().find(|m| cert.model_ids.contains(&m.id)) {
            return Ok(Some(default(m.clone(), "certificate", Some(cert.id))));
        }
    }
    // the one promoted last, by its promotion's event
    let sql = format!(
        "SELECT model_id, MAX(id) FROM {} WHERE transition = 'promoted' GROUP BY model_id",
        store.qualified("model_event")
    );
    let mut last: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    for r in store.query(&sql, &[])? {
        last.insert(r.int(0)?, r.int(1)?);
    }
    if let Some(m) = candidates
        .iter()
        .filter(|m| m.state == "promoted")
        .max_by_key(|m| (last.get(&m.id).copied().unwrap_or(0), m.id))
    {
        return Ok(Some(default(m.clone(), "promoted", None)));
    }
    Ok(candidates
        .into_iter()
        .find(|m| !is_cascade(m))
        .map(|m| default(m, "admitted", None)))
}

/// Every task and slot a model is registered for, in order.
pub fn tasks(store: &mut Store) -> Result<Vec<(String, String)>, StoreError> {
    let sql = format!(
        "SELECT DISTINCT task, slot FROM {} ORDER BY task, slot",
        store.qualified("model")
    );
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| Ok((r.text(0)?.to_string(), r.text(1)?.to_string())))
        .collect()
}

/// The model chosen now for every task and slot, by id.
pub fn chosen_ids(store: &mut Store) -> Result<std::collections::BTreeSet<i64>, StoreError> {
    let mut out = std::collections::BTreeSet::new();
    for (task, slot) in tasks(store)? {
        if let Some(c) = chosen(store, &task, &slot)? {
            out.insert(c.model.id);
        }
    }
    Ok(out)
}

/// A model as the doors answer it, with whether it is chosen now.
pub fn to_json_with(m: &Model, chosen: &std::collections::BTreeSet<i64>) -> Value {
    let mut v = m.to_json();
    v["chosen"] = Value::Bool(chosen.contains(&m.id));
    v
}

/// The newest certificate naming each model, by model id (record 48 R2;
/// record 55 ruling 22 b: on a sealed sample or citing its graded run).
pub fn certified(
    store: &mut Store,
) -> Result<std::collections::BTreeMap<i64, crate::labels::Certificate>, StoreError> {
    crate::labels::certified(store).map_err(|e| match e {
        crate::labels::Error::Store(s) => s,
        other => StoreError::Message(other.to_string()),
    })
}

/// A model's certificate as its card shows it: the certificate, with what
/// it certified in a line; null where none names the model.
pub fn certificate_json(
    m: &Model,
    certified: &std::collections::BTreeMap<i64, crate::labels::Certificate>,
) -> Value {
    certified.get(&m.id).map_or(Value::Null, |c| {
        let mut v = c.as_json();
        v["words"] = Value::from(c.words());
        v
    })
}

/// A model as the doors answer it, with whether it is chosen now and the
/// certificate that names it.
pub fn to_json_full(
    m: &Model,
    chosen: &std::collections::BTreeSet<i64>,
    certified: &std::collections::BTreeMap<i64, crate::labels::Certificate>,
) -> Value {
    let mut v = to_json_with(m, chosen);
    v["certificate"] = certificate_json(m, certified);
    v
}

/// Every task and slot with the model chosen there and every model
/// registered there, newest first, retired included (record 55 D4): what
/// `GET /api/models/tasks` and `nils model chosen` answer.
pub fn tasks_json(store: &mut Store, only: Option<&str>) -> Result<Value, StoreError> {
    let mut out = Vec::new();
    let certs = certified(store)?;
    for (task, slot) in tasks(store)? {
        if only.is_some_and(|t| t != task) {
            continue;
        }
        let chosen = chosen(store, &task, &slot)?;
        let ids: std::collections::BTreeSet<i64> = chosen.iter().map(|c| c.model.id).collect();
        let mut models = list(
            store,
            &Filter {
                task: Some(&task),
                slot: Some(&slot),
                ..Default::default()
            },
        )?;
        models.reverse();
        out.push(json!({
            "task": task,
            "slot": slot,
            "chosen": chosen.as_ref().map(Chosen::as_json),
            "models": models.iter().map(|m| to_json_full(m, &ids, &certs)).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({ "tasks": out }))
}

fn write_choice(
    store: &mut Store,
    task: &str,
    slot: &str,
    model_id: Option<i64>,
    who: &str,
    now: &str,
    why: Option<&str>,
) -> Result<(), StoreError> {
    store.insert(
        &Insert::new(
            table("model_choice"),
            &["task", "slot", "model_id", "chosen_by", "chosen_at", "why"],
        ),
        &[vec![
            Param::from(task),
            Param::from(slot),
            model_id.map_or(Param::Null, Param::Int),
            Param::from(who),
            Param::from(now),
            why.map_or(Param::Null, Param::from),
        ]],
    )?;
    Ok(())
}

/// Choose the model that answers its task and slot (record 55 D4): a site
/// admin's act, audited, with an event on the model chosen and on the one
/// a person chose before. Refused for a model that is neither admitted nor
/// promoted, and for a cascade with a retired part.
pub fn choose(
    registry: &mut Registry,
    id: i64,
    who: &str,
    why: Option<&str>,
) -> Result<Chosen, Error> {
    let store = registry.store();
    let m = known(store, id)?;
    if !m.answers() {
        return Err(refused(format!(
            "model {} ({}) is {}; only an admitted or promoted model is chosen to answer {}",
            m.id,
            m.label(),
            m.state,
            m.task
        )));
    }
    parts_stand(store, &m, "chosen")?;
    let before = choice_now(store, &m.task, &m.slot)?.and_then(|c| c.model_id);
    let now = now_iso();
    in_transaction(store, |store| {
        write_choice(store, &m.task, &m.slot, Some(id), who, &now, why)?;
        if let Some(old) = before.filter(|old| *old != id) {
            event(
                store,
                old,
                "unchosen",
                who,
                &now,
                Some(json!({ "replaced_by": id, "why": why })),
            )?;
        }
        event(
            store,
            id,
            "chosen",
            who,
            &now,
            Some(json!({ "task": m.task, "slot": m.slot, "before": before, "why": why })),
        )?;
        Ok(())
    })?;
    audit_row(
        registry,
        Action::ModelChoose,
        who,
        &m,
        json!({ "chosen": id, "before": before, "why": why }),
    )?;
    chosen(registry.store(), &m.task, &m.slot)?
        .filter(|c| c.model.id == id)
        .ok_or_else(|| refused(format!("model {id} was chosen and does not answer")))
}

/// Send a task and slot back to its default (record 55 D4): the model a
/// recorded certificate names, else the one promoted last, else the newest
/// admitted. Audited; the model a person chose before gets an event.
/// Answers the model that answers now, if any.
pub fn unchoose(
    registry: &mut Registry,
    task: &str,
    slot: &str,
    who: &str,
    why: Option<&str>,
) -> Result<Option<Chosen>, Error> {
    if !task_ok(task) {
        return Err(invalid(format!(
            "{task} is not a task, such as axis:body_part"
        )));
    }
    let store = registry.store();
    if !tasks(store)?.iter().any(|(t, s)| t == task && s == slot) {
        return Err(Error::Unknown(format!(
            "no model is registered for {task} in slot {slot}"
        )));
    }
    let before = choice_now(store, task, slot)?.and_then(|c| c.model_id);
    let now = now_iso();
    in_transaction(store, |store| {
        write_choice(store, task, slot, None, who, &now, why)?;
        if let Some(old) = before {
            event(
                store,
                old,
                "unchosen",
                who,
                &now,
                Some(json!({ "replaced_by": null, "why": why })),
            )?;
        }
        Ok(())
    })?;
    let after = chosen(registry.store(), task, slot)?;
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::ModelChoose,
            scope: json!({ "task": task, "slot": slot }),
            policy: None,
            job_id: None,
            details: Some(json!({
                "chosen": null, "before": before, "why": why,
                "answers": after.as_ref().map(|c| c.model.id),
                "by": after.as_ref().map(|c| c.by),
            })),
        },
    )?;
    Ok(after)
}

/// Name the job that fitted a model (record 43: a pipeline run's).
pub fn set_job(store: &mut Store, id: i64, job_id: i64) -> Result<(), StoreError> {
    store.update_by_id(table("model"), &[("job_id", Param::Int(job_id))], "id", id)?;
    Ok(())
}

/// The model a model's answer names, refused unless it is registered and
/// admitted or promoted (record 42 S2).
pub fn author(store: &mut Store, id: i64) -> Result<Model, Error> {
    let Some(m) = get(store, id)? else {
        return Err(Error::Unknown(format!(
            "no model {id} is registered; a model's answer names a registered model (D15)"
        )));
    };
    if !m.answers() {
        return Err(refused(format!(
            "model {} ({}) is {}; only an admitted or promoted model answers",
            m.id,
            m.label(),
            m.state
        )));
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_is_sha256_and_sixty_four_lowercase_hex_digits() {
        let ok = format!("sha256:{}", "a".repeat(64));
        assert!(is_digest(&ok));
        assert!(!is_digest(&format!("sha256:{}", "A".repeat(64))));
        assert!(!is_digest(&format!("sha256:{}", "a".repeat(63))));
        assert!(!is_digest(&"a".repeat(64)));
    }

    #[test]
    fn a_task_is_a_word_and_its_qualifiers() {
        assert!(task_ok("axis:body_part"));
        assert!(task_ok("segment:wmh"));
        assert!(task_ok("chat"));
        assert!(!task_ok("Axis:body_part"));
        assert!(!task_ok("axis:"));
        assert!(!task_ok("axis body"));
        assert!(name_ok("bodypart-head_v1.2"));
        assert!(!name_ok("-x"));
        assert!(!name_ok("a b"));
    }

    #[test]
    fn a_part_is_named_as_a_pipeline_input() {
        for ok in ["head", "coarse", "student", "deferral", "stage_2"] {
            assert!(part_name_ok(ok), "{ok}");
        }
        for bad in ["", "Head", "2nd", "a-b", "a b", "head:x"] {
            assert!(!part_name_ok(bad), "{bad}");
        }
    }
}
