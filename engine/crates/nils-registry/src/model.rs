// SPDX-License-Identifier: AGPL-3.0-only

//! The model registry (record 42 S2, D15): the models whose answers become
//! registry facts, identified by the digest of their canonical artifact and
//! described by the card of `contracts/model/v1`.
//!
//! A model passes through four states, the lifecycle Kvasir keeps for the
//! language models it serves: registered with its card; admitted by a check
//! that passed, which stays on it; promoted, which is refused unless it was
//! admitted and retires the model promoted before it in the same task and
//! slot; and retired. A failed check is kept as an event and moves nothing.
//! Every transition is an event row and an audit row, and nothing is
//! deleted, because a retired model still names what it decided.
//!
//! A model's answer is a decision with `author_kind` model and this model's
//! id ([`crate::review::apply`]), refused unless the model is admitted or
//! promoted, and staged until a person commits it (record 42 R6).

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
        })
    }
}

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

/// One model by its id.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Model>, StoreError> {
    let d = store.dialect();
    let sql = select_sql(store, &format!(" WHERE id = {}", d.param(1, Type::Int)));
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| of(&r))
        .transpose()
}

/// One model by its digest.
pub fn by_digest(store: &mut Store, digest: &str) -> Result<Option<Model>, StoreError> {
    let d = store.dialect();
    let sql = select_sql(
        store,
        &format!(" WHERE digest = {}", d.param(1, Type::Text)),
    );
    store
        .query_opt(&sql, &[Param::from(digest)])?
        .map(|r| of(&r))
        .transpose()
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
    store
        .query_opt(&sql, &[Param::from(name), Param::from(version)])?
        .map(|r| of(&r))
        .transpose()
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
    store.query(&sql, &params)?.iter().map(of).collect()
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
            scope: json!({
                "model": m.id, "name": m.name, "version": m.version, "digest": m.digest,
                "task": m.task, "slot": m.slot,
            }),
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
    if let Some(t) = card.get("trained_on").filter(|v| !v.is_null()) {
        let set = t["label_set"].as_str().unwrap_or("");
        if !is_digest(set) {
            return Err(invalid(
                "trained_on names its label set by digest: sha256: and 64 lowercase hex digits",
            ));
        }
    }
    Ok(slot)
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
    // A head names its encoder, which must be registered: a new encoder
    // makes its heads stale, and the edge is what says which.
    let encoder = match card.get("encoder").filter(|v| !v.is_null()) {
        Some(e) => {
            let d = e["digest"].as_str().unwrap_or("");
            if !is_digest(d) {
                return Err(invalid("encoder names its model by digest"));
            }
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
            Some(m.id)
        }
        None if kind == "head" => {
            return Err(invalid(
                "a head names the encoder whose features it reads (encoder.digest)",
            ));
        }
        None => None,
    };
    let trained_on = card["trained_on"]["label_set"].as_str().map(str::to_string);
    // Record 40 R3 and 42 S7: the labels a model was fitted on are a label
    // set this registry wrote, and none drawn from a sealed certification
    // sample, since a model fitted on the sample that certifies it
    // certifies nothing.
    if let Some(digest) = &trained_on {
        let sets = crate::labels::by_digest(store, digest).map_err(labels_err)?;
        if sets.is_empty() {
            return Err(Error::Unknown(format!(
                "no label set has the digest {digest}; a model is trained on a set this registry wrote (nils labels export)"
            )));
        }
        for set in &sets {
            crate::labels::usable_for_training(store, set.id).map_err(labels_err)?;
        }
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
        event(store, id, "registered", who, &now, None)?;
        Ok(id)
    })?;
    let m = get(registry.store(), id)?.expect("the model just written");
    audit_row(
        registry,
        Action::ModelRegister,
        who,
        &m,
        json!({ "kind": m.kind, "encoder_model_id": m.encoder_model_id, "trained_on": m.trained_on }),
    )?;
    Ok(m)
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
    let passed = checked_check(check)?;
    let store = registry.store();
    let m = known(store, id)?;
    if matches!(m.state.as_str(), "promoted" | "retired") {
        return Err(refused(format!(
            "model {} ({}) is {}; a check belongs before a promotion, so register a new version",
            m.id,
            m.label(),
            m.state
        )));
    }
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
    /// The model promoted before it in the same task and slot, retired now.
    pub retired: Option<Model>,
}

/// Promote a model: refused unless it was admitted by a check that passed.
/// The model promoted before it in the same task and slot is retired, so a
/// slot has at most one promoted model.
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
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE task = {} AND slot = {} AND state = 'promoted' AND id <> {}",
        store.qualified("model"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Int),
    );
    let before: Vec<i64> = store
        .query(
            &sql,
            &[
                Param::from(m.task.as_str()),
                Param::from(m.slot.as_str()),
                Param::Int(id),
            ],
        )?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    let now = now_iso();
    in_transaction(store, |store| {
        for old in &before {
            let sql = format!(
                "UPDATE {} SET state = 'retired', retired_by = {}, retired_at = {} WHERE id = {}",
                store.qualified("model"),
                d.param(1, Type::Text),
                d.param(2, Type::Timestamp),
                d.param(3, Type::Int),
            );
            store.execute(
                &sql,
                &[
                    Param::from(who),
                    Param::from(now.as_str()),
                    Param::Int(*old),
                ],
            )?;
            event(
                store,
                *old,
                "retired",
                who,
                &now,
                Some(json!({ "replaced_by": id })),
            )?;
        }
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
            Some(json!({ "retired": before, "review_item": review_item, "why": why })),
        )?;
        Ok(())
    })?;
    let model = known(registry.store(), id)?;
    let retired = match before.first() {
        Some(old) => Some(known(registry.store(), *old)?),
        None => None,
    };
    audit_row(
        registry,
        Action::ModelPromote,
        who,
        &model,
        json!({ "retired": before, "review_item": review_item, "why": why }),
    )?;
    Ok(Promoted { model, retired })
}

/// Retire a model. Its decisions stay, and keep naming it.
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
}
