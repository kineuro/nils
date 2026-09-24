// SPDX-License-Identifier: AGPL-3.0-only

//! Campaigns (record 42 S5 and S6): one mechanism for annotation and for
//! curation. v0 ran five QC products that differed only in their workflow
//! and stored nothing of their own; here one campaign asks one question of
//! a frozen list of items, raters claim items under a lease, answer and
//! release, and the campaign is closed through the one write path the
//! registry already has.
//!
//! - **Every item is backed by a review item.** A campaign over a selection
//!   raises one per item; a campaign over a review-item query adopts the
//!   items it names. Closing an item calls [`review::apply`] on it, so rank,
//!   withdrawal, the audit and the epoch behave as they do everywhere else.
//! - **An answer is never a decision.** `apply` withdraws the earlier
//!   decision on the same key and refuses a lower rank, so two raters
//!   written as decisions would cancel each other. An answer stays an
//!   answer, kept for good, and an item becomes one decision only when the
//!   campaign closes: the value the raters agreed on, or the adjudicator's.
//! - **Five questions.** `axis` (which value of one pack axis), `pick`
//!   (which stacks stand for a role in a session), `form` (a small declared
//!   form), `derivative` (a file answer, a mask, named by its derivative
//!   id; the bytes go through the derivative door) and `free`.
//! - **Agreement.** Exact agreement per item, Cohen's and Fleiss' kappa over
//!   the campaign, or an external metric a caller posts for an item (a Dice
//!   over masks, which something with pixels computed: the engine never
//!   computes one). Disagreement opens a second round assigned to an
//!   adjudicator.
//! - **Absence (D1).** With no campaign nothing changes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::Registry;
use crate::audit::{self, Action, Entry};
use crate::review;
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};
use crate::time::{iso_of, now_iso, secs_of};

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    /// The request is malformed: a question, a source, an answer that does
    /// not fit its question.
    Invalid(String),
    /// No campaign, item or assignment of that id.
    NotFound(String),
    /// The state forbids it: a closed campaign, an expired lease, an
    /// assignment of another rater.
    Refused(String),
    /// The rater policy does not name the principal.
    Forbidden(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Invalid(m) | Error::NotFound(m) | Error::Refused(m) | Error::Forbidden(m) => {
                f.write_str(m)
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn refused(m: impl Into<String>) -> Error {
    Error::Refused(m.into())
}

// ------------------------------------------------------------ the question

/// What a campaign asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Question {
    /// Which value of one pack axis; `values`, when given, is the
    /// vocabulary an answer must come from.
    Axis { axis: String, values: Vec<String> },
    /// Which stacks stand for a role in a session.
    Pick { role: String, scheme: String },
    /// A small declared form: `properties` with a `type` or an `enum` each,
    /// and `required`.
    Form { schema: Value },
    /// A file answer, a mask, by its derivative id; a form beside it when
    /// the question declares one.
    Derivative {
        derivative_kind: String,
        form: Option<Value>,
    },
    /// Free text.
    Free,
}

/// The question kinds, as a campaign names them.
pub const KINDS: [&str; 5] = ["axis", "pick", "form", "derivative", "free"];

impl Question {
    pub fn parse(v: &Value) -> Result<Question, Error> {
        let kind = v["kind"]
            .as_str()
            .ok_or_else(|| invalid("question.kind: axis, pick, form, derivative or free"))?;
        Ok(match kind {
            "axis" => {
                let axis = v["axis"]
                    .as_str()
                    .filter(|a| !a.is_empty())
                    .ok_or_else(|| invalid("an axis question names its axis"))?;
                let values = match &v["values"] {
                    Value::Null => Vec::new(),
                    Value::Array(list) => list
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_string)
                                .ok_or_else(|| invalid("question.values: a list of words"))
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err(invalid("question.values: a list of words")),
                };
                Question::Axis {
                    axis: axis.to_string(),
                    values,
                }
            }
            "pick" => Question::Pick {
                role: v["role"]
                    .as_str()
                    .filter(|r| !r.is_empty())
                    .ok_or_else(|| invalid("a pick question names its role"))?
                    .to_string(),
                scheme: v["scheme"].as_str().unwrap_or("default").to_string(),
            },
            "form" => {
                let schema = v["schema"].clone();
                check_schema(&schema)?;
                Question::Form { schema }
            }
            "derivative" => {
                let form = match &v["form"] {
                    Value::Null => None,
                    f => {
                        check_schema(f)?;
                        Some(f.clone())
                    }
                };
                Question::Derivative {
                    derivative_kind: v["derivative_kind"].as_str().unwrap_or("mask").to_string(),
                    form,
                }
            }
            "free" => Question::Free,
            other => {
                return Err(invalid(format!(
                    "{other} is not a question: axis, pick, form, derivative or free"
                )));
            }
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Question::Axis { .. } => "axis",
            Question::Pick { .. } => "pick",
            Question::Form { .. } => "form",
            Question::Derivative { .. } => "derivative",
            Question::Free => "free",
        }
    }

    /// The question as stored.
    pub fn to_json(&self) -> Value {
        match self {
            Question::Axis { axis, values } => {
                let mut v = json!({"kind": "axis", "axis": axis});
                if !values.is_empty() {
                    v["values"] = json!(values);
                }
                v
            }
            Question::Pick { role, scheme } => {
                json!({"kind": "pick", "role": role, "scheme": scheme})
            }
            Question::Form { schema } => json!({"kind": "form", "schema": schema}),
            Question::Derivative {
                derivative_kind,
                form,
            } => {
                let mut v = json!({"kind": "derivative", "derivative_kind": derivative_kind});
                if let Some(f) = form {
                    v["form"] = f.clone();
                }
                v
            }
            Question::Free => json!({"kind": "free"}),
        }
    }

    /// What a label set calls it: the axis, or the question kind and what
    /// it names.
    pub fn what(&self) -> String {
        match self {
            Question::Axis { axis, .. } => axis.clone(),
            Question::Pick { role, .. } => format!("pick:{role}"),
            Question::Form { .. } => "form".to_string(),
            Question::Derivative {
                derivative_kind, ..
            } => format!("derivative:{derivative_kind}"),
            Question::Free => "free".to_string(),
        }
    }

    /// Whether an answer fits the question.
    fn check(&self, a: &Given<'_>) -> Result<(), Error> {
        let value = a.value.filter(|v| !v.trim().is_empty());
        match self {
            Question::Axis { axis, values } => {
                let v =
                    value.ok_or_else(|| invalid(format!("the answer names a value of {axis}")))?;
                if !values.is_empty() && !values.iter().any(|x| x == v) {
                    return Err(invalid(format!(
                        "{v} is not a value of {axis} this campaign asks for: {}",
                        values.join(", ")
                    )));
                }
            }
            Question::Pick { role, .. } => {
                let v = value.ok_or_else(|| {
                    invalid(format!("the answer names the stacks that stand for {role}"))
                })?;
                pick_stacks(v)?;
            }
            Question::Form { schema } => {
                let form = a
                    .form
                    .ok_or_else(|| invalid("the answer carries the form"))?;
                check_form(schema, form)?;
            }
            Question::Derivative { form, .. } => {
                if a.derivative_id.is_none() {
                    return Err(invalid(
                        "the answer names its derivative (POST /api/derivatives registers the file)",
                    ));
                }
                if let Some(schema) = form {
                    let f = a
                        .form
                        .ok_or_else(|| invalid("the answer carries the form beside its file"))?;
                    check_form(schema, f)?;
                }
            }
            Question::Free => {
                value.ok_or_else(|| invalid("the answer is a text"))?;
            }
        }
        Ok(())
    }

    /// What two answers are compared by for exact agreement and kappa: the
    /// value, the stacks of a pick in order, the form as canonical JSON.
    /// None where there is nothing the engine can compare (a mask).
    fn comparable(&self, a: &Answer) -> Option<String> {
        match self {
            Question::Axis { .. } | Question::Free => a.value.clone(),
            Question::Pick { .. } => a
                .value
                .as_deref()
                .and_then(|v| pick_stacks(v).ok())
                .map(|s| join_ids(&s)),
            Question::Form { .. } => a.form.as_ref().map(canonical),
            Question::Derivative { .. } => None,
        }
    }
}

/// A pick's answer, the stacks as ids: `12` or `12,14`.
pub fn pick_stacks(v: &str) -> Result<Vec<i64>, Error> {
    let mut out: Vec<i64> = v
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<i64>()
                .map_err(|_| invalid(format!("{s} is not a stack id")))
        })
        .collect::<Result<_, _>>()?;
    if out.is_empty() {
        return Err(invalid("a pick names at least one stack"));
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn join_ids(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// JSON with its keys sorted, so two equal forms compare equal.
fn canonical(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let sorted: BTreeMap<&String, Value> =
                    m.iter().map(|(k, v)| (k, sort(v))).collect();
                json!(sorted)
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(v).to_string()
}

const FIELD_TYPES: [&str; 5] = ["string", "number", "integer", "boolean", "array"];

/// A form is declared as a small JSON Schema: an object with `properties`,
/// each with a `type` or an `enum`, and `required`.
fn check_schema(schema: &Value) -> Result<(), Error> {
    let props = schema["properties"]
        .as_object()
        .ok_or_else(|| invalid("a form declares its properties"))?;
    if props.is_empty() {
        return Err(invalid("a form declares at least one property"));
    }
    for (name, p) in props {
        match (p["type"].as_str(), p["enum"].as_array()) {
            (Some(t), _) if FIELD_TYPES.contains(&t) => {}
            (None, Some(values)) if !values.is_empty() => {}
            _ => {
                return Err(invalid(format!(
                    "form property {name}: a type ({}) or an enum",
                    FIELD_TYPES.join(", ")
                )));
            }
        }
    }
    if let Some(req) = schema.get("required") {
        let list = req
            .as_array()
            .ok_or_else(|| invalid("a form's required is a list"))?;
        for r in list {
            let name = r.as_str().unwrap_or_default();
            if !props.contains_key(name) {
                return Err(invalid(format!(
                    "the form requires {name}, which it does not declare"
                )));
            }
        }
    }
    Ok(())
}

fn check_form(schema: &Value, form: &Value) -> Result<(), Error> {
    let given = form
        .as_object()
        .ok_or_else(|| invalid("the form is an object"))?;
    let props = schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for r in schema["required"].as_array().into_iter().flatten() {
        let name = r.as_str().unwrap_or_default();
        if given.get(name).is_none_or(Value::is_null) {
            return Err(invalid(format!("the form's {name} is required")));
        }
    }
    for (name, v) in given {
        let Some(p) = props.get(name) else {
            return Err(invalid(format!("the form declares no {name}")));
        };
        if v.is_null() {
            continue;
        }
        if let Some(values) = p["enum"].as_array()
            && !values.contains(v)
        {
            return Err(invalid(format!(
                "the form's {name} is not one of its values"
            )));
        }
        let fits = match p["type"].as_str() {
            Some("string") => v.is_string(),
            Some("number") => v.is_number(),
            Some("integer") => v.is_i64() || v.is_u64(),
            Some("boolean") => v.is_boolean(),
            Some("array") => v.is_array(),
            _ => true,
        };
        if !fits {
            return Err(invalid(format!(
                "the form's {name} is not a {}",
                p["type"].as_str().unwrap_or("value")
            )));
        }
    }
    Ok(())
}

// ------------------------------------------------------- the adjudication

/// When a second round is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// When the raters disagree, or an external metric falls below its
    /// threshold.
    Disagree,
    /// Every item goes to an adjudicator.
    Always,
    /// Never: a disagreement closes into nothing.
    Never,
}

/// How agreement is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Every rater gave the same answer.
    Exact,
    /// Exact per item, with Cohen's and Fleiss' kappa over the campaign.
    Kappa,
    /// A number a caller posts for an item, held against the threshold.
    External,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Adjudication {
    pub when: When,
    pub metric: Metric,
    pub threshold: f64,
}

impl Adjudication {
    pub fn parse(v: &Value) -> Result<Adjudication, Error> {
        let when = match v["when"].as_str().unwrap_or("disagree") {
            "disagree" => When::Disagree,
            "always" => When::Always,
            "never" => When::Never,
            other => {
                return Err(invalid(format!(
                    "adjudication.when: disagree, always or never, not {other}"
                )));
            }
        };
        let metric = match v["metric"].as_str().unwrap_or("exact") {
            "exact" => Metric::Exact,
            "kappa" => Metric::Kappa,
            "external" => Metric::External,
            other => {
                return Err(invalid(format!(
                    "adjudication.metric: exact, kappa or external, not {other}"
                )));
            }
        };
        let threshold = match &v["threshold"] {
            Value::Null => 0.8,
            t => t
                .as_f64()
                .filter(|t| t.is_finite())
                .ok_or_else(|| invalid("adjudication.threshold is a number"))?,
        };
        Ok(Adjudication {
            when,
            metric,
            threshold,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "when": match self.when { When::Disagree => "disagree", When::Always => "always", When::Never => "never" },
            "metric": match self.metric { Metric::Exact => "exact", Metric::Kappa => "kappa", Metric::External => "external" },
            "threshold": self.threshold,
        })
    }
}

/// What closing writes.
pub const CLOSES_INTO: [&str; 4] = ["decision", "stage", "pick", "none"];

// ------------------------------------------------------------- creating

/// The items a campaign asks about.
#[derive(Debug, Clone)]
pub enum Items {
    /// Stacks, from a frozen selection: a review item is raised for each.
    Stacks(Vec<i64>),
    /// Sessions as (subject, the day the session opened), for a pick.
    Sessions(Vec<(i64, String)>),
    /// Review items already raised, adopted as they are.
    Review(Vec<i64>),
}

/// A campaign to create.
#[derive(Debug, Clone)]
pub struct New<'a> {
    pub name: &'a str,
    pub owner: &'a str,
    pub question: &'a Value,
    /// `{selection: name@v}`, `{handle: id}` or `{review: {...}}`: what the
    /// items came from, recorded as it was given.
    pub source: Value,
    pub items: Items,
    pub handle_id: Option<i64>,
    pub content_hash: Option<&'a str>,
    pub pack_version: Option<&'a str>,
    pub raters_per_item: i64,
    /// Who may rate, and who may adjudicate; empty means anyone who holds
    /// `campaigns:work`.
    pub raters: Vec<String>,
    pub adjudicators: Vec<String>,
    pub adjudication: &'a Value,
    pub closes_into: &'a str,
    pub lease_seconds: i64,
    /// Seeds and pre-segmentations per item, by the item's key.
    pub inputs: BTreeMap<String, Vec<i64>>,
}

/// A campaign as read.
#[derive(Debug, Clone, PartialEq)]
pub struct Campaign {
    pub id: i64,
    pub name: String,
    pub owner: String,
    pub status: String,
    pub question: Value,
    pub grain: String,
    pub source: Value,
    pub handle_id: Option<i64>,
    pub content_hash: Option<String>,
    pub epoch: i64,
    pub pack_version: Option<String>,
    pub raters_per_item: i64,
    pub rater_policy: Value,
    pub adjudication: Value,
    pub closes_into: String,
    pub lease_seconds: i64,
    pub created_at: String,
    pub closed_at: Option<String>,
    pub closed_by: Option<String>,
    pub agreement: Value,
}

impl Campaign {
    pub fn question(&self) -> Result<Question, Error> {
        Question::parse(&self.question)
    }

    pub fn adjudication(&self) -> Result<Adjudication, Error> {
        Adjudication::parse(&self.adjudication)
    }

    fn listed(&self, key: &str) -> Vec<String> {
        self.rater_policy[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    }

    pub fn raters(&self) -> Vec<String> {
        self.listed("raters")
    }

    pub fn adjudicators(&self) -> Vec<String> {
        self.listed("adjudicators")
    }

    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "owner": self.owner,
            "status": self.status,
            "question": self.question,
            "grain": self.grain,
            "source": self.source,
            "handle_id": self.handle_id,
            "content_hash": self.content_hash,
            "epoch": self.epoch,
            "pack_version": self.pack_version,
            "raters_per_item": self.raters_per_item,
            "rater_policy": self.rater_policy,
            "adjudication": self.adjudication,
            "closes_into": self.closes_into,
            "lease_seconds": self.lease_seconds,
            "created_at": self.created_at,
            "closed_at": self.closed_at,
            "closed_by": self.closed_by,
            "agreement": self.agreement,
        })
    }
}

/// The columns of a table, each read as the text the engine wrote.
fn select_list(store: &Store, t: &str, names: &[&str], alias: Option<&str>) -> String {
    let d = store.dialect();
    let t = table(t);
    names
        .iter()
        .map(|n| {
            d.text_of_qualified(
                alias,
                t.column(n)
                    .unwrap_or_else(|| panic!("{}.{n} is not a column", t.name)),
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn json_at(r: &Row, i: usize) -> Result<Value, StoreError> {
    Ok(r.opt_text(i)?
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null))
}

const CAMPAIGN_COLUMNS: [&str; 20] = [
    "id",
    "name",
    "owner",
    "status",
    "question",
    "grain",
    "source",
    "handle_id",
    "content_hash",
    "epoch",
    "pack_version",
    "raters_per_item",
    "rater_policy",
    "adjudication",
    "closes_into",
    "lease_seconds",
    "created_at",
    "closed_at",
    "closed_by",
    "agreement",
];

fn campaign_of(r: &Row) -> Result<Campaign, StoreError> {
    Ok(Campaign {
        id: r.int(0)?,
        name: r.text(1)?.to_string(),
        owner: r.text(2)?.to_string(),
        status: r.text(3)?.to_string(),
        question: json_at(r, 4)?,
        grain: r.text(5)?.to_string(),
        source: json_at(r, 6)?,
        handle_id: r.opt_int(7)?,
        content_hash: r.opt_text(8)?.map(str::to_string),
        epoch: r.int(9)?,
        pack_version: r.opt_text(10)?.map(str::to_string),
        raters_per_item: r.int(11)?,
        rater_policy: json_at(r, 12)?,
        adjudication: json_at(r, 13)?,
        closes_into: r.text(14)?.to_string(),
        lease_seconds: r.int(15)?,
        created_at: r.text(16)?.to_string(),
        closed_at: r.opt_text(17)?.map(str::to_string),
        closed_by: r.opt_text(18)?.map(str::to_string),
        agreement: json_at(r, 19)?,
    })
}

/// One campaign by id.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Campaign>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        select_list(store, "campaign", &CAMPAIGN_COLUMNS, None),
        store.qualified("campaign"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| campaign_of(&r))
        .transpose()?)
}

/// One campaign by name.
pub fn by_name(store: &mut Store, name: &str) -> Result<Option<Campaign>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE name = {}",
        select_list(store, "campaign", &CAMPAIGN_COLUMNS, None),
        store.qualified("campaign"),
        store.dialect().param(1, Type::Text)
    );
    Ok(store
        .query_opt(&sql, &[Param::from(name)])?
        .map(|r| campaign_of(&r))
        .transpose()?)
}

/// A campaign by its id or its name, as a verb or a door names it.
pub fn find(store: &mut Store, which: &str) -> Result<Campaign, Error> {
    let found = match which.parse::<i64>() {
        Ok(id) => get(store, id)?,
        Err(_) => by_name(store, which)?,
    };
    found.ok_or_else(|| Error::NotFound(format!("no campaign {which}")))
}

/// Every campaign, newest first.
pub fn list(store: &mut Store) -> Result<Vec<Campaign>, Error> {
    let sql = format!(
        "SELECT {} FROM {} ORDER BY id DESC",
        select_list(store, "campaign", &CAMPAIGN_COLUMNS, None),
        store.qualified("campaign")
    );
    Ok(store
        .query(&sql, &[])?
        .iter()
        .map(campaign_of)
        .collect::<Result<_, _>>()?)
}

/// The review items a query names, for a campaign that adopts them: open
/// ones, of one kind or a kind prefix (`body_part:` takes every classifier
/// question about the axis), at stack or group scope.
#[derive(Debug, Clone, Default)]
pub struct ReviewQuery {
    pub kind: Option<String>,
    pub kind_prefix: Option<String>,
    pub job_id: Option<i64>,
    pub limit: Option<usize>,
}

impl ReviewQuery {
    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind, "kind_prefix": self.kind_prefix, "job_id": self.job_id, "limit": self.limit})
    }
}

pub fn review_items(store: &mut Store, q: &ReviewQuery) -> Result<Vec<i64>, Error> {
    if q.kind.is_none() && q.kind_prefix.is_none() {
        return Err(invalid(
            "a review source names a kind or a kind prefix (body_part: takes every question about the axis)",
        ));
    }
    let d = store.dialect();
    let mut sql = format!(
        "SELECT id FROM {} WHERE status = 'open' AND scope IN ('stack', 'group')",
        store.qualified("review_item")
    );
    let mut params = Vec::new();
    if let Some(k) = &q.kind {
        params.push(Param::from(k.as_str()));
        sql.push_str(&format!(
            " AND kind = {}",
            d.param(params.len(), Type::Text)
        ));
    }
    if let Some(p) = &q.kind_prefix {
        params.push(Param::from(format!("{p}%")));
        sql.push_str(&format!(
            " AND kind LIKE {}",
            d.param(params.len(), Type::Text)
        ));
    }
    if let Some(j) = q.job_id {
        params.push(Param::Int(j));
        sql.push_str(&format!(
            " AND job_id = {}",
            d.param(params.len(), Type::Int)
        ));
    }
    sql.push_str(" ORDER BY id");
    let mut ids: Vec<i64> = store
        .query(&sql, &params)?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    if let Some(n) = q.limit {
        ids.truncate(n);
    }
    Ok(ids)
}

/// Create a campaign: its row, one item per key, each backed by a review
/// item, in one transaction; then the audit row.
pub fn create(registry: &mut Registry, n: &New<'_>) -> Result<Campaign, Error> {
    let name = n.name.trim();
    if name.is_empty() || name.parse::<i64>().is_ok() {
        return Err(invalid("a campaign's name is a word, not a number"));
    }
    let question = Question::parse(n.question)?;
    let adjudication = Adjudication::parse(n.adjudication)?;
    if !CLOSES_INTO.contains(&n.closes_into) {
        return Err(invalid(format!(
            "closes_into: {}, not {}",
            CLOSES_INTO.join(", "),
            n.closes_into
        )));
    }
    match (n.closes_into, &question) {
        ("decision" | "stage", Question::Axis { .. })
        | ("pick", Question::Pick { .. })
        | ("none", _) => {}
        ("decision" | "stage", _) => {
            return Err(invalid(
                "only an axis question closes into a decision; close the others into none",
            ));
        }
        ("pick", _) => return Err(invalid("only a pick question closes into a pick")),
        _ => unreachable!("checked above"),
    }
    if matches!(question, Question::Derivative { .. })
        && adjudication.when == When::Disagree
        && adjudication.metric != Metric::External
    {
        return Err(invalid(
            "a derivative question is compared by an external metric (a Dice something with pixels computes), or adjudicated always or never; the engine never compares files",
        ));
    }
    if n.raters_per_item < 1 {
        return Err(invalid("raters_per_item is one or more"));
    }
    if n.lease_seconds < 1 {
        return Err(invalid("lease_seconds is one or more"));
    }
    let grain = match (&n.items, &question) {
        (Items::Sessions(_), Question::Axis { .. }) => {
            return Err(invalid("an axis question is asked of stacks, not sessions"));
        }
        (Items::Stacks(_) | Items::Review(_), Question::Pick { .. }) => {
            return Err(invalid(
                "a pick question is asked of sessions; build it from a selection at session grain",
            ));
        }
        (Items::Sessions(_), _) => "session",
        _ => "stack",
    };
    let count = match &n.items {
        Items::Stacks(s) => s.len(),
        Items::Sessions(s) => s.len(),
        Items::Review(r) => r.len(),
    };
    if count == 0 {
        return Err(invalid("the source names no item"));
    }
    let epoch = registry.meta().epoch;
    let store = registry.store();
    if by_name(store, name)?.is_some() {
        return Err(refused(format!("a campaign named {name} exists")));
    }
    // An adopted item is asked by one open campaign at a time.
    let adopted: Vec<review::Item> = match &n.items {
        Items::Review(ids) => {
            let mut out = Vec::new();
            for id in ids {
                let it = review::item(store, *id)
                    .map_err(|e| invalid(e.to_string()))?
                    .ok_or_else(|| Error::NotFound(format!("no review item {id}")))?;
                if it.status != "open" {
                    return Err(refused(format!(
                        "review item {id} is {}, not open",
                        it.status
                    )));
                }
                if !matches!(it.scope.as_str(), "stack" | "group") {
                    return Err(invalid(format!(
                        "review item {id} is about a {}; a campaign adopts stack and group questions",
                        it.scope
                    )));
                }
                if let Question::Axis { axis, .. } = &question
                    && it.evidence["axis"].as_str() != Some(axis.as_str())
                {
                    return Err(invalid(format!(
                        "review item {id} is not a question about {axis}"
                    )));
                }
                out.push(it);
            }
            let d = store.dialect();
            let sql = format!(
                "SELECT i.review_item_id FROM {} i JOIN {} c ON c.id = i.campaign_id WHERE c.status = 'open'",
                store.qualified("campaign_item"),
                store.qualified("campaign")
            );
            let _ = d;
            let held: BTreeSet<i64> = store
                .query(&sql, &[])?
                .iter()
                .map(|r| r.int(0))
                .collect::<Result<_, _>>()?;
            if let Some(id) = ids.iter().find(|id| held.contains(id)) {
                return Err(refused(format!(
                    "review item {id} is asked by an open campaign already"
                )));
            }
            out
        }
        _ => Vec::new(),
    };
    let now = now_iso();
    let policy = json!({"raters": n.raters, "adjudicators": n.adjudicators});
    store.begin()?;
    let written = (|| -> Result<(i64, usize), Error> {
        let id = store
            .insert(
                &Insert::new(
                    table("campaign"),
                    &[
                        "name",
                        "owner",
                        "status",
                        "question",
                        "grain",
                        "source",
                        "handle_id",
                        "content_hash",
                        "epoch",
                        "pack_version",
                        "raters_per_item",
                        "rater_policy",
                        "adjudication",
                        "closes_into",
                        "lease_seconds",
                        "created_at",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(name),
                    Param::from(n.owner),
                    Param::from("open"),
                    Param::from(question.to_json().to_string()),
                    Param::from(grain),
                    Param::from(n.source.to_string()),
                    n.handle_id.map_or(Param::Null, Param::Int),
                    n.content_hash.map_or(Param::Null, Param::from),
                    Param::Int(epoch),
                    n.pack_version.map_or(Param::Null, Param::from),
                    Param::Int(n.raters_per_item),
                    Param::from(policy.to_string()),
                    Param::from(adjudication.to_json().to_string()),
                    Param::from(n.closes_into),
                    Param::Int(n.lease_seconds),
                    Param::from(now.as_str()),
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the campaign was not written back".into()))?
            .int(0)?;
        let evidence = |extra: Value| -> Value {
            let mut e = json!({
                "campaign": id,
                "campaign_name": name,
                "question": question.kind(),
                "campaign_state": "open",
            });
            match &question {
                Question::Axis { axis, .. } => e["axis"] = json!(axis),
                Question::Pick { role, .. } => e["role"] = json!(role),
                _ => {}
            }
            if let (Value::Object(m), Value::Object(x)) = (&mut e, extra) {
                m.extend(x);
            }
            e
        };
        let kind = format!("campaign.{}", question.kind());
        let mut rows: Vec<Vec<Param>> = Vec::new();
        let item_row = |position: usize,
                        review_item: i64,
                        stack: Option<i64>,
                        subject: Option<i64>,
                        day: Option<&str>,
                        key: &str| {
            let inputs = n.inputs.get(key).map(|ids| json!(ids).to_string());
            vec![
                Param::Int(id),
                Param::Int(position as i64),
                Param::Int(review_item),
                stack.map_or(Param::Null, Param::Int),
                subject.map_or(Param::Null, Param::Int),
                day.map_or(Param::Null, Param::from),
                Param::from(key),
                inputs.map_or(Param::Null, Param::from),
                Param::from("open"),
                Param::Int(1),
            ]
        };
        match &n.items {
            Items::Stacks(stacks) => {
                for (i, stack) in stacks.iter().enumerate() {
                    let r = raise(
                        store,
                        &kind,
                        "stack",
                        &json!({"stack_id": stack}),
                        &evidence(json!({"position": i})),
                        &now,
                    )?;
                    rows.push(item_row(
                        i,
                        r,
                        Some(*stack),
                        None,
                        None,
                        &format!("stack:{stack}"),
                    ));
                }
            }
            Items::Sessions(sessions) => {
                for (i, (subject, day)) in sessions.iter().enumerate() {
                    let r = raise(
                        store,
                        &kind,
                        "subject",
                        &json!({"subject_id": subject, "session_day": day}),
                        &evidence(json!({"position": i})),
                        &now,
                    )?;
                    rows.push(item_row(
                        i,
                        r,
                        None,
                        Some(*subject),
                        Some(day),
                        &format!("session:{subject}:{day}"),
                    ));
                }
            }
            Items::Review(_) => {
                for (i, it) in adopted.iter().enumerate() {
                    // the adopted item says which campaign asks it now
                    let mut ev = it.evidence.clone();
                    if let Value::Object(m) = &mut ev {
                        m.insert("campaign".into(), json!(id));
                        m.insert("campaign_name".into(), json!(name));
                        m.insert("campaign_state".into(), json!("open"));
                    }
                    store.update_by_id(
                        table("review_item"),
                        &[("evidence", Param::from(ev.to_string()))],
                        "id",
                        it.id,
                    )?;
                    let stack = it.reference["stack_id"].as_i64();
                    rows.push(item_row(
                        i,
                        it.id,
                        stack,
                        None,
                        None,
                        &format!("review:{}", it.id),
                    ));
                }
            }
        }
        for chunk in rows.chunks(500) {
            store.insert(
                &Insert::new(
                    table("campaign_item"),
                    &[
                        "campaign_id",
                        "position",
                        "review_item_id",
                        "stack_id",
                        "subject_id",
                        "session_day",
                        "key",
                        "input_derivative_ids",
                        "state",
                        "round",
                    ],
                ),
                chunk,
            )?;
        }
        Ok((id, rows.len()))
    })();
    let (id, items) = match written {
        Ok(w) => w,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    };
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal: n.owner,
            action: Action::CampaignCreate,
            scope: json!({"campaign": id, "name": name, "items": items, "handle": n.handle_id}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "question": question.kind(), "closes_into": n.closes_into,
                "raters_per_item": n.raters_per_item, "adjudication": adjudication.to_json(),
            })),
        },
    )?;
    get(registry.store(), id)?.ok_or_else(|| Error::NotFound(format!("no campaign {id}")))
}

fn raise(
    store: &mut Store,
    kind: &str,
    scope: &str,
    reference: &Value,
    evidence: &Value,
    now: &str,
) -> Result<i64, StoreError> {
    store
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
                    "members",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(kind),
                Param::from(scope),
                Param::from(reference.to_string()),
                Param::from(evidence.to_string()),
                Param::from("open"),
                Param::from(now),
                Param::Int(1),
            ]],
        )?
        .first()
        .ok_or_else(|| StoreError::Message("the review item was not written back".into()))?
        .int(0)
}

// ------------------------------------------------------------ the items

/// One item as read.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: i64,
    pub campaign_id: i64,
    pub position: i64,
    pub review_item_id: i64,
    pub stack_id: Option<i64>,
    pub subject_id: Option<i64>,
    pub session_day: Option<String>,
    pub key: String,
    pub input_derivative_ids: Value,
    pub state: String,
    pub round: i64,
    pub agreement: Option<f64>,
    pub metric: Value,
    pub outcome: Value,
    pub decision_id: Option<i64>,
    pub pick_id: Option<i64>,
    pub resolved_at: Option<String>,
}

impl Item {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "campaign_id": self.campaign_id, "position": self.position,
            "review_item_id": self.review_item_id, "stack_id": self.stack_id,
            "subject_id": self.subject_id, "session_day": self.session_day, "key": self.key,
            "input_derivative_ids": self.input_derivative_ids, "state": self.state,
            "round": self.round, "agreement": self.agreement, "metric": self.metric,
            "outcome": self.outcome, "decision_id": self.decision_id, "pick_id": self.pick_id,
            "resolved_at": self.resolved_at,
        })
    }
}

const ITEM_COLUMNS: [&str; 17] = [
    "id",
    "campaign_id",
    "position",
    "review_item_id",
    "stack_id",
    "subject_id",
    "session_day",
    "key",
    "input_derivative_ids",
    "state",
    "round",
    "agreement",
    "metric",
    "outcome",
    "decision_id",
    "pick_id",
    "resolved_at",
];

fn item_of(r: &Row) -> Result<Item, StoreError> {
    Ok(Item {
        id: r.int(0)?,
        campaign_id: r.int(1)?,
        position: r.int(2)?,
        review_item_id: r.int(3)?,
        stack_id: r.opt_int(4)?,
        subject_id: r.opt_int(5)?,
        session_day: r.opt_text(6)?.map(str::to_string),
        key: r.text(7)?.to_string(),
        input_derivative_ids: json_at(r, 8)?,
        state: r.text(9)?.to_string(),
        round: r.int(10)?,
        agreement: r.opt_double(11)?,
        metric: json_at(r, 12)?,
        outcome: json_at(r, 13)?,
        decision_id: r.opt_int(14)?,
        pick_id: r.opt_int(15)?,
        resolved_at: r.opt_text(16)?.map(str::to_string),
    })
}

/// A campaign's items, in position order.
pub fn items(store: &mut Store, campaign: i64) -> Result<Vec<Item>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE campaign_id = {} ORDER BY position",
        select_list(store, "campaign_item", &ITEM_COLUMNS, None),
        store.qualified("campaign_item"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query(&sql, &[Param::Int(campaign)])?
        .iter()
        .map(item_of)
        .collect::<Result<_, _>>()?)
}

pub fn item(store: &mut Store, id: i64) -> Result<Option<Item>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        select_list(store, "campaign_item", &ITEM_COLUMNS, None),
        store.qualified("campaign_item"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| item_of(&r))
        .transpose()?)
}

// ------------------------------------------------------ the assignments

/// A rater or an adjudicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Rater,
    Adjudicator,
}

impl Role {
    pub fn parse(s: &str) -> Result<Role, Error> {
        match s {
            "rater" => Ok(Role::Rater),
            "adjudicator" => Ok(Role::Adjudicator),
            other => Err(invalid(format!(
                "{other} is not a role: rater or adjudicator"
            ))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Rater => "rater",
            Role::Adjudicator => "adjudicator",
        }
    }
}

/// One assignment as read.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub id: i64,
    pub campaign_id: i64,
    pub item_id: i64,
    pub principal: Option<String>,
    pub role: String,
    pub round: i64,
    pub state: String,
    pub created_at: String,
    pub leased_at: Option<String>,
    pub lease_until: Option<String>,
    pub ended_at: Option<String>,
}

impl Assignment {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "campaign_id": self.campaign_id, "item_id": self.item_id,
            "principal": self.principal, "role": self.role, "round": self.round,
            "state": self.state, "created_at": self.created_at, "leased_at": self.leased_at,
            "lease_until": self.lease_until, "ended_at": self.ended_at,
        })
    }
}

const ASSIGNMENT_COLUMNS: [&str; 11] = [
    "id",
    "campaign_id",
    "item_id",
    "principal",
    "role",
    "round",
    "state",
    "created_at",
    "leased_at",
    "lease_until",
    "ended_at",
];

fn assignment_of(r: &Row) -> Result<Assignment, StoreError> {
    Ok(Assignment {
        id: r.int(0)?,
        campaign_id: r.int(1)?,
        item_id: r.int(2)?,
        principal: r.opt_text(3)?.map(str::to_string),
        role: r.text(4)?.to_string(),
        round: r.int(5)?,
        state: r.text(6)?.to_string(),
        created_at: r.text(7)?.to_string(),
        leased_at: r.opt_text(8)?.map(str::to_string),
        lease_until: r.opt_text(9)?.map(str::to_string),
        ended_at: r.opt_text(10)?.map(str::to_string),
    })
}

/// A campaign's assignments, in the order they were made.
pub fn assignments(store: &mut Store, campaign: i64) -> Result<Vec<Assignment>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE campaign_id = {} ORDER BY id",
        select_list(store, "campaign_assignment", &ASSIGNMENT_COLUMNS, None),
        store.qualified("campaign_assignment"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query(&sql, &[Param::Int(campaign)])?
        .iter()
        .map(assignment_of)
        .collect::<Result<_, _>>()?)
}

fn assignment(store: &mut Store, id: i64) -> Result<Option<Assignment>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        select_list(store, "campaign_assignment", &ASSIGNMENT_COLUMNS, None),
        store.qualified("campaign_assignment"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| assignment_of(&r))
        .transpose()?)
}

/// What a claim hands a rater: the assignment, its lease and the item.
#[derive(Debug, Clone, PartialEq)]
pub struct Claimed {
    pub assignment: Assignment,
    pub item: Item,
    /// Whether the principal held this lease already.
    pub held: bool,
}

impl Claimed {
    pub fn as_json(&self) -> Value {
        json!({
            "assignment": self.assignment.as_json(),
            "item": self.item.as_json(),
            "held": self.held,
        })
    }
}

/// Serialise the writers of one campaign: Postgres locks its row for the
/// transaction; SQLite's immediate transaction already holds the database.
fn lock(store: &mut Store, campaign: i64) -> Result<(), StoreError> {
    if matches!(store, Store::Postgres { .. }) {
        let sql = format!(
            "SELECT id FROM {} WHERE id = {} FOR UPDATE",
            store.qualified("campaign"),
            store.dialect().param(1, Type::Int)
        );
        store.query(&sql, &[Param::Int(campaign)])?;
    }
    Ok(())
}

fn plus_seconds(now: &str, seconds: i64) -> String {
    let base = secs_of(now).unwrap_or_else(crate::time::now_secs);
    iso_of(base.saturating_add(seconds.max(0) as u64))
}

/// Leases past their time end: a rater's returns the item to the pool, an
/// adjudicator's is offered again. Answers the leases that expired.
fn expire_in(store: &mut Store, campaign: i64, now: &str) -> Result<u64, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id, item_id, role, round FROM {} WHERE campaign_id = {} AND state = 'leased' AND lease_until < {}",
        store.qualified("campaign_assignment"),
        d.param(1, Type::Int),
        d.param(2, Type::Timestamp)
    );
    let gone: Vec<(i64, i64, String, i64)> = store
        .query(&sql, &[Param::Int(campaign), Param::from(now)])?
        .iter()
        .map(|r| Ok((r.int(0)?, r.int(1)?, r.text(2)?.to_string(), r.int(3)?)))
        .collect::<Result<_, StoreError>>()?;
    for (id, item, role, round) in &gone {
        store.update_by_id(
            table("campaign_assignment"),
            &[
                ("state", Param::from("expired")),
                ("ended_at", Param::from(now)),
            ],
            "id",
            *id,
        )?;
        if role == "adjudicator" {
            offer(store, campaign, *item, None, *round, now)?;
        }
    }
    Ok(gone.len() as u64)
}

/// End every lease of every campaign that is past its time. Answers how
/// many ended.
pub fn expire(registry: &mut Registry, now: &str) -> Result<u64, Error> {
    let store = registry.store();
    let ids: Vec<i64> = store
        .query(
            &format!(
                "SELECT id FROM {} WHERE status = 'open'",
                store.qualified("campaign")
            ),
            &[],
        )?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    let mut n = 0;
    for id in ids {
        store.begin()?;
        let done = lock(store, id).and_then(|()| expire_in(store, id, now));
        match done {
            Ok(k) => {
                store.commit()?;
                n += k;
            }
            Err(e) => {
                store.rollback().ok();
                return Err(e.into());
            }
        }
    }
    Ok(n)
}

fn offer(
    store: &mut Store,
    campaign: i64,
    item: i64,
    principal: Option<&str>,
    round: i64,
    now: &str,
) -> Result<i64, StoreError> {
    store
        .insert(
            &Insert::new(
                table("campaign_assignment"),
                &[
                    "campaign_id",
                    "item_id",
                    "principal",
                    "role",
                    "round",
                    "state",
                    "created_at",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::Int(campaign),
                Param::Int(item),
                principal.map_or(Param::Null, Param::from),
                Param::from("adjudicator"),
                Param::Int(round),
                Param::from("offered"),
                Param::from(now),
            ]],
        )?
        .first()
        .ok_or_else(|| StoreError::Message("the assignment was not written back".into()))?
        .int(0)
}

/// Claim the next item: for a rater, the first open item, by position, that
/// still wants raters and that this principal was never given; for an
/// adjudicator, the first item waiting for one that this principal did not
/// rate. The lease runs for the campaign's `lease_seconds`. A principal
/// who holds a live lease gets it back rather than a second. None when
/// nothing is left for this principal.
pub fn claim(
    registry: &mut Registry,
    campaign: i64,
    principal: &str,
    role: Role,
    now: &str,
) -> Result<Option<Claimed>, Error> {
    let c = get(registry.store(), campaign)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {campaign}")))?;
    if c.status != "open" {
        return Err(refused(format!("campaign {} is {}", c.name, c.status)));
    }
    let listed = match role {
        Role::Rater => c.raters(),
        Role::Adjudicator => c.adjudicators(),
    };
    if !listed.is_empty() && !listed.iter().any(|p| p == principal) {
        return Err(Error::Forbidden(format!(
            "campaign {} names its {}s, and {principal} is not one",
            c.name,
            role.name()
        )));
    }
    let store = registry.store();
    let d = store.dialect();
    store.begin()?;
    let done = (|| -> Result<Option<(i64, bool)>, Error> {
        lock(store, campaign)?;
        expire_in(store, campaign, now)?;
        // a live lease is handed back, never a second
        let sql = format!(
            "SELECT id FROM {} WHERE campaign_id = {} AND principal = {} AND role = {} AND state = 'leased' ORDER BY id LIMIT 1",
            store.qualified("campaign_assignment"),
            d.param(1, Type::Int),
            d.param(2, Type::Text),
            d.param(3, Type::Text)
        );
        if let Some(r) = store.query_opt(
            &sql,
            &[
                Param::Int(campaign),
                Param::from(principal),
                Param::from(role.name()),
            ],
        )? {
            return Ok(Some((r.int(0)?, true)));
        }
        let until = plus_seconds(now, c.lease_seconds);
        let a = store.qualified("campaign_assignment");
        let i = store.qualified("campaign_item");
        match role {
            Role::Rater => {
                let sql = format!(
                    "SELECT i.id FROM {i} i WHERE i.campaign_id = {} AND i.state = 'open' AND i.round = 1 \
                     AND (SELECT COUNT(*) FROM {a} a WHERE a.item_id = i.id AND a.role = 'rater' \
                          AND a.state IN ('leased', 'submitted')) < {} \
                     AND NOT EXISTS (SELECT 1 FROM {a} a WHERE a.item_id = i.id AND a.principal = {}) \
                     ORDER BY i.position LIMIT 1",
                    d.param(1, Type::Int),
                    d.param(2, Type::Int),
                    d.param(3, Type::Text)
                );
                let Some(r) = store.query_opt(
                    &sql,
                    &[
                        Param::Int(campaign),
                        Param::Int(c.raters_per_item),
                        Param::from(principal),
                    ],
                )?
                else {
                    return Ok(None);
                };
                let item = r.int(0)?;
                let id = store
                    .insert(
                        &Insert::new(
                            table("campaign_assignment"),
                            &[
                                "campaign_id",
                                "item_id",
                                "principal",
                                "role",
                                "round",
                                "state",
                                "created_at",
                                "leased_at",
                                "lease_until",
                            ],
                        )
                        .returning(&["id"]),
                        &[vec![
                            Param::Int(campaign),
                            Param::Int(item),
                            Param::from(principal),
                            Param::from("rater"),
                            Param::Int(1),
                            Param::from("leased"),
                            Param::from(now),
                            Param::from(now),
                            Param::from(until.as_str()),
                        ]],
                    )?
                    .first()
                    .ok_or_else(|| {
                        StoreError::Message("the assignment was not written back".into())
                    })?
                    .int(0)?;
                Ok(Some((id, false)))
            }
            Role::Adjudicator => {
                let sql = format!(
                    "SELECT a.id FROM {a} a JOIN {i} i ON i.id = a.item_id \
                     WHERE a.campaign_id = {} AND a.role = 'adjudicator' AND a.state = 'offered' \
                     AND i.state = 'needs_adjudication' \
                     AND (a.principal IS NULL OR a.principal = {}) \
                     AND NOT EXISTS (SELECT 1 FROM {a} b WHERE b.item_id = i.id AND b.principal = {} AND b.id <> a.id) \
                     ORDER BY i.position, a.id LIMIT 1",
                    d.param(1, Type::Int),
                    d.param(2, Type::Text),
                    d.param(3, Type::Text)
                );
                let Some(r) = store.query_opt(
                    &sql,
                    &[
                        Param::Int(campaign),
                        Param::from(principal),
                        Param::from(principal),
                    ],
                )?
                else {
                    return Ok(None);
                };
                let id = r.int(0)?;
                store.update_by_id(
                    table("campaign_assignment"),
                    &[
                        ("principal", Param::from(principal)),
                        ("state", Param::from("leased")),
                        ("leased_at", Param::from(now)),
                        ("lease_until", Param::from(until.as_str())),
                    ],
                    "id",
                    id,
                )?;
                Ok(Some((id, false)))
            }
        }
    })();
    let claimed = match done {
        Ok(c) => c,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    };
    store.commit()?;
    let Some((id, held)) = claimed else {
        return Ok(None);
    };
    let a = assignment(store, id)?.ok_or_else(|| Error::NotFound(format!("no assignment {id}")))?;
    let it = item(store, a.item_id)?
        .ok_or_else(|| Error::NotFound(format!("no campaign item {}", a.item_id)))?;
    if !held {
        audit::record(
            registry,
            &Entry {
                principal,
                action: Action::CampaignClaim,
                scope: json!({"campaign": campaign, "item": it.id, "assignment": id}),
                policy: None,
                job_id: None,
                details: Some(
                    json!({"role": role.name(), "round": a.round, "lease_until": a.lease_until}),
                ),
            },
        )?;
    }
    Ok(Some(Claimed {
        assignment: a,
        item: it,
        held,
    }))
}

/// Give a leased item back unanswered. A rater's item returns to the pool
/// for others (never to the same rater); an adjudicator's is offered again.
pub fn release(
    registry: &mut Registry,
    assignment_id: i64,
    principal: &str,
    now: &str,
) -> Result<Assignment, Error> {
    let store = registry.store();
    let a = assignment(store, assignment_id)?
        .ok_or_else(|| Error::NotFound(format!("no assignment {assignment_id}")))?;
    if a.principal.as_deref() != Some(principal) {
        return Err(Error::Forbidden(format!(
            "assignment {assignment_id} is not {principal}'s"
        )));
    }
    if a.state != "leased" {
        return Err(refused(format!(
            "assignment {assignment_id} is {}, not leased",
            a.state
        )));
    }
    store.begin()?;
    let done = (|| -> Result<(), StoreError> {
        lock(store, a.campaign_id)?;
        store.update_by_id(
            table("campaign_assignment"),
            &[
                ("state", Param::from("released")),
                ("ended_at", Param::from(now)),
            ],
            "id",
            a.id,
        )?;
        if a.role == "adjudicator" {
            offer(store, a.campaign_id, a.item_id, None, a.round, now)?;
        }
        Ok(())
    })();
    if let Err(e) = done {
        store.rollback().ok();
        return Err(e.into());
    }
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal,
            action: Action::CampaignRelease,
            scope: json!({"campaign": a.campaign_id, "item": a.item_id, "assignment": a.id}),
            policy: None,
            job_id: None,
            details: Some(json!({"role": a.role, "round": a.round})),
        },
    )?;
    assignment(registry.store(), assignment_id)?
        .ok_or_else(|| Error::NotFound(format!("no assignment {assignment_id}")))
}

// ------------------------------------------------------------ the answers

/// An answer as given.
#[derive(Debug, Clone)]
pub struct Given<'a> {
    pub assignment: i64,
    pub principal: &'a str,
    /// The verified actor's kind: person, agent or model.
    pub author_kind: &'a str,
    pub value: Option<&'a str>,
    pub form: Option<&'a Value>,
    pub derivative_id: Option<i64>,
    pub why: Option<&'a str>,
}

/// An answer as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub id: i64,
    pub campaign_id: i64,
    pub item_id: i64,
    pub assignment_id: i64,
    pub principal: String,
    pub role: String,
    pub round: i64,
    pub author_kind: String,
    pub value: Option<String>,
    pub form: Option<Value>,
    pub derivative_id: Option<i64>,
    pub why: Option<String>,
    pub actor_detail: Value,
    pub answered_at: String,
}

impl Answer {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "campaign_id": self.campaign_id, "item_id": self.item_id,
            "assignment_id": self.assignment_id, "principal": self.principal, "role": self.role,
            "round": self.round, "author_kind": self.author_kind, "value": self.value,
            "form": self.form, "derivative_id": self.derivative_id, "why": self.why,
            "actor_detail": self.actor_detail, "answered_at": self.answered_at,
        })
    }
}

const ANSWER_COLUMNS: [&str; 14] = [
    "id",
    "campaign_id",
    "item_id",
    "assignment_id",
    "principal",
    "role",
    "round",
    "author_kind",
    "value",
    "form",
    "derivative_id",
    "why",
    "actor_detail",
    "answered_at",
];

fn answer_of(r: &Row) -> Result<Answer, StoreError> {
    let form = json_at(r, 9)?;
    Ok(Answer {
        id: r.int(0)?,
        campaign_id: r.int(1)?,
        item_id: r.int(2)?,
        assignment_id: r.int(3)?,
        principal: r.text(4)?.to_string(),
        role: r.text(5)?.to_string(),
        round: r.int(6)?,
        author_kind: r.text(7)?.to_string(),
        value: r.opt_text(8)?.map(str::to_string),
        form: (!form.is_null()).then_some(form),
        derivative_id: r.opt_int(10)?,
        why: r.opt_text(11)?.map(str::to_string),
        actor_detail: json_at(r, 12)?,
        answered_at: r.text(13)?.to_string(),
    })
}

/// Every answer of a campaign, in the order given.
pub fn answers(store: &mut Store, campaign: i64) -> Result<Vec<Answer>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE campaign_id = {} ORDER BY id",
        select_list(store, "campaign_answer", &ANSWER_COLUMNS, None),
        store.qualified("campaign_answer"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query(&sql, &[Param::Int(campaign)])?
        .iter()
        .map(answer_of)
        .collect::<Result<_, _>>()?)
}

fn answers_of_item(store: &mut Store, item: i64) -> Result<Vec<Answer>, StoreError> {
    let sql = format!(
        "SELECT {} FROM {} WHERE item_id = {} ORDER BY id",
        select_list(store, "campaign_answer", &ANSWER_COLUMNS, None),
        store.qualified("campaign_answer"),
        store.dialect().param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(item)])?
        .iter()
        .map(answer_of)
        .collect()
}

/// What an answer did.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub answer: i64,
    pub item: i64,
    /// The item's state after it.
    pub state: String,
    /// A round-two assignment it opened, when the raters disagreed.
    pub adjudication: Option<i64>,
}

/// Record an answer on a leased assignment, and move the item on: once the
/// last rater answered, it is agreed, or it waits for its metric, or a
/// second round is offered to an adjudicator; an adjudicator's answer
/// settles it. Nothing here is a decision.
pub fn answer(registry: &mut Registry, g: &Given<'_>, now: &str) -> Result<Answered, Error> {
    let store = registry.store();
    let a = assignment(store, g.assignment)?
        .ok_or_else(|| Error::NotFound(format!("no assignment {}", g.assignment)))?;
    if a.principal.as_deref() != Some(g.principal) {
        return Err(Error::Forbidden(format!(
            "assignment {} is not {}'s",
            a.id, g.principal
        )));
    }
    let c = get(store, a.campaign_id)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {}", a.campaign_id)))?;
    if c.status != "open" {
        return Err(refused(format!("campaign {} is {}", c.name, c.status)));
    }
    if a.state != "leased" {
        return Err(refused(format!(
            "assignment {} is {}; claim the item again",
            a.id, a.state
        )));
    }
    if a.lease_until
        .as_deref()
        .is_some_and(|u| secs_of(u).zip(secs_of(now)).is_some_and(|(u, n)| u < n))
    {
        return Err(refused(format!(
            "the lease of assignment {} ran out; claim again",
            a.id
        )));
    }
    let question = c.question()?;
    question.check(g)?;
    let adjudication = c.adjudication()?;
    if !["person", "agent", "model"].contains(&g.author_kind) {
        return Err(invalid(format!(
            "an author is a person, an agent or a model, not {}",
            g.author_kind
        )));
    }
    let actor_detail = crate::actor::current();
    let stored_value = match &question {
        Question::Pick { .. } => g
            .value
            .map(|v| pick_stacks(v).map(|s| join_ids(&s)))
            .transpose()?,
        _ => g.value.map(|v| v.trim().to_string()),
    };
    store.begin()?;
    let done = (|| -> Result<Answered, Error> {
        lock(store, c.id)?;
        let id = store
            .insert(
                &Insert::new(
                    table("campaign_answer"),
                    &[
                        "campaign_id",
                        "item_id",
                        "assignment_id",
                        "principal",
                        "role",
                        "round",
                        "author_kind",
                        "value",
                        "form",
                        "derivative_id",
                        "why",
                        "actor_detail",
                        "answered_at",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::Int(c.id),
                    Param::Int(a.item_id),
                    Param::Int(a.id),
                    Param::from(g.principal),
                    Param::from(a.role.as_str()),
                    Param::Int(a.round),
                    Param::from(g.author_kind),
                    stored_value.clone().map_or(Param::Null, Param::from),
                    g.form.map_or(Param::Null, |f| Param::from(f.to_string())),
                    g.derivative_id.map_or(Param::Null, Param::Int),
                    g.why.map_or(Param::Null, Param::from),
                    Param::from(actor_detail.to_string()),
                    Param::from(now),
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the answer was not written back".into()))?
            .int(0)?;
        store.update_by_id(
            table("campaign_assignment"),
            &[
                ("state", Param::from("submitted")),
                ("ended_at", Param::from(now)),
            ],
            "id",
            a.id,
        )?;
        let it = item(store, a.item_id)?
            .ok_or_else(|| Error::NotFound(format!("no campaign item {}", a.item_id)))?;
        let all = answers_of_item(store, it.id)?;
        let mut opened = None;
        let state = if a.role == "adjudicator" {
            let settled = all.last().expect("the answer just written");
            let agreement = share_agreeing(&question, &all, settled);
            let outcome = outcome_of(&question, std::slice::from_ref(settled));
            set_item(
                store,
                it.id,
                "adjudicated",
                2,
                agreement,
                Some(&outcome),
                now,
            )?;
            "adjudicated".to_string()
        } else {
            let raters: Vec<&Answer> = all.iter().filter(|x| x.role == "rater").collect();
            if (raters.len() as i64) < c.raters_per_item {
                it.state.clone()
            } else {
                let keys: Vec<Option<String>> =
                    raters.iter().map(|x| question.comparable(x)).collect();
                let agree = keys.iter().all(|k| k.is_some() && *k == keys[0]);
                let next = match (adjudication.when, adjudication.metric) {
                    (When::Always, _) => "needs_adjudication",
                    (_, Metric::External) => "awaiting_metric",
                    (_, _) if agree => "agreed",
                    (When::Disagree, _) => "needs_adjudication",
                    (When::Never, _) if keys.iter().all(Option::is_none) => "agreed",
                    (When::Never, _) => "disagreed",
                };
                let owned: Vec<Answer> = raters.iter().map(|x| (*x).clone()).collect();
                match next {
                    "agreed" => {
                        let outcome = outcome_of(&question, &owned);
                        set_item(store, it.id, "agreed", 1, Some(1.0), Some(&outcome), now)?;
                    }
                    "needs_adjudication" => {
                        opened = Some(adjudicate(store, &c, &it, &owned, now)?);
                    }
                    other => set_item(store, it.id, other, 1, None, None, now)?,
                }
                next.to_string()
            }
        };
        Ok(Answered {
            answer: id,
            item: it.id,
            state,
            adjudication: opened,
        })
    })();
    let answered = match done {
        Ok(x) => x,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    };
    store.commit()?;
    // the value of an axis or a pick is a word of the pack or stack ids; a
    // form's text and a free answer stay out of the audit row
    let shown = matches!(question, Question::Axis { .. } | Question::Pick { .. })
        .then(|| stored_value.clone())
        .flatten();
    audit::record(
        registry,
        &Entry {
            principal: g.principal,
            action: Action::CampaignAnswer,
            scope: json!({
                "campaign": c.id, "item": answered.item, "assignment": a.id,
                "answer": answered.answer,
            }),
            policy: None,
            job_id: None,
            details: Some(json!({
                "role": a.role, "round": a.round, "value": shown,
                "derivative": g.derivative_id, "author_kind": g.author_kind,
                "item_state": answered.state, "adjudication": answered.adjudication,
            })),
        },
    )?;
    Ok(answered)
}

/// The share of round-one raters whose answer is the settled one.
fn share_agreeing(question: &Question, all: &[Answer], settled: &Answer) -> Option<f64> {
    let key = question.comparable(settled)?;
    let raters: Vec<&Answer> = all.iter().filter(|a| a.role == "rater").collect();
    if raters.is_empty() {
        return None;
    }
    let same = raters
        .iter()
        .filter(|a| question.comparable(a).as_deref() == Some(key.as_str()))
        .count();
    Some(same as f64 / raters.len() as f64)
}

/// What an item came to: the value (and form) the answers agree on, or the
/// files when the question is a derivative.
fn outcome_of(question: &Question, answers: &[Answer]) -> Value {
    let first = answers.first();
    match question {
        Question::Derivative { .. } => json!({
            "derivatives": answers.iter().filter_map(|a| a.derivative_id).collect::<Vec<_>>(),
            "form": first.and_then(|a| a.form.clone()),
            "answers": answers.iter().map(|a| a.id).collect::<Vec<_>>(),
        }),
        _ => json!({
            "value": first.and_then(|a| a.value.clone()),
            "form": first.and_then(|a| a.form.clone()),
            "answers": answers.iter().map(|a| a.id).collect::<Vec<_>>(),
        }),
    }
}

fn set_item(
    store: &mut Store,
    id: i64,
    state: &str,
    round: i64,
    agreement: Option<f64>,
    outcome: Option<&Value>,
    _now: &str,
) -> Result<(), StoreError> {
    let mut sets = vec![("state", Param::from(state)), ("round", Param::Int(round))];
    if let Some(a) = agreement {
        sets.push(("agreement", Param::Double(a)));
    }
    if let Some(o) = outcome {
        sets.push(("outcome", Param::from(o.to_string())));
    }
    store.update_by_id(table("campaign_item"), &sets, "id", id)?;
    Ok(())
}

/// Say on the review item where the campaign stands with it, so the
/// ordinary queue shows an item waiting for its adjudicator (D7).
fn mark_review_item(store: &mut Store, review_item: i64, state: &str) -> Result<(), Error> {
    let Some(it) = review::item(store, review_item).map_err(|e| invalid(e.to_string()))? else {
        return Ok(());
    };
    let mut ev = it.evidence;
    if let Value::Object(m) = &mut ev {
        m.insert("campaign_state".into(), json!(state));
    }
    store.update_by_id(
        table("review_item"),
        &[("evidence", Param::from(ev.to_string()))],
        "id",
        review_item,
    )?;
    Ok(())
}

/// Open the second round: one adjudicator assignment, offered to the first
/// named adjudicator who did not rate the item, or to any.
fn adjudicate(
    store: &mut Store,
    c: &Campaign,
    it: &Item,
    raters: &[Answer],
    now: &str,
) -> Result<i64, Error> {
    let rated: BTreeSet<&str> = raters.iter().map(|a| a.principal.as_str()).collect();
    let named = c.adjudicators();
    let to = named.iter().find(|p| !rated.contains(p.as_str()));
    let id = offer(store, c.id, it.id, to.map(String::as_str), 2, now)?;
    set_item(store, it.id, "needs_adjudication", 2, None, None, now)?;
    mark_review_item(store, it.review_item_id, "needs_adjudication")?;
    Ok(id)
}

/// Post an external metric for an item whose raters all answered, a Dice
/// over their masks: at or above the campaign's threshold the item is
/// agreed, below it goes to an adjudicator. The engine only compares.
pub fn post_metric(
    registry: &mut Registry,
    item_id: i64,
    principal: &str,
    name: &str,
    value: f64,
    now: &str,
) -> Result<Answered, Error> {
    if !value.is_finite() {
        return Err(invalid("a metric is a number"));
    }
    let store = registry.store();
    let it = item(store, item_id)?
        .ok_or_else(|| Error::NotFound(format!("no campaign item {item_id}")))?;
    let c = get(store, it.campaign_id)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {}", it.campaign_id)))?;
    if c.status != "open" {
        return Err(refused(format!("campaign {} is {}", c.name, c.status)));
    }
    let adj = c.adjudication()?;
    if adj.metric != Metric::External {
        return Err(refused(format!(
            "campaign {} measures agreement itself; it takes no external metric",
            c.name
        )));
    }
    if it.state != "awaiting_metric" {
        return Err(refused(format!(
            "item {item_id} is {}; a metric is posted once its raters have all answered",
            it.state
        )));
    }
    let question = c.question()?;
    store.begin()?;
    let done = (|| -> Result<Answered, Error> {
        lock(store, c.id)?;
        let metric = json!({"name": name, "value": value, "threshold": adj.threshold, "by": principal, "at": now});
        store.update_by_id(
            table("campaign_item"),
            &[("metric", Param::from(metric.to_string()))],
            "id",
            it.id,
        )?;
        let raters: Vec<Answer> = answers_of_item(store, it.id)?
            .into_iter()
            .filter(|a| a.role == "rater")
            .collect();
        let mut opened = None;
        let state = if value >= adj.threshold {
            let outcome = outcome_of(&question, &raters);
            set_item(store, it.id, "agreed", 1, Some(1.0), Some(&outcome), now)?;
            "agreed"
        } else if adj.when == When::Never {
            set_item(store, it.id, "disagreed", 1, None, None, now)?;
            "disagreed"
        } else {
            opened = Some(adjudicate(store, &c, &it, &raters, now)?);
            "needs_adjudication"
        };
        Ok(Answered {
            answer: 0,
            item: it.id,
            state: state.to_string(),
            adjudication: opened,
        })
    })();
    let answered = match done {
        Ok(x) => x,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    };
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal,
            action: Action::CampaignMetric,
            scope: json!({"campaign": c.id, "item": it.id}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "metric": name, "value": value, "threshold": adj.threshold,
                "item_state": answered.state, "adjudication": answered.adjudication,
            })),
        },
    )?;
    Ok(answered)
}

// ------------------------------------------------------------- agreement

/// Agreement over a whole campaign, from the round-one answers of the items
/// every rater answered: the share of items all raters agreed on, Fleiss'
/// kappa, and Cohen's kappa where the same two raters answered every item.
/// Null where the question has nothing to compare (a mask).
pub fn agreement(store: &mut Store, campaign: i64) -> Result<Value, Error> {
    let c =
        get(store, campaign)?.ok_or_else(|| Error::NotFound(format!("no campaign {campaign}")))?;
    let question = c.question()?;
    let all = answers(store, campaign)?;
    let mut by_item: BTreeMap<i64, Vec<&Answer>> = BTreeMap::new();
    for a in all.iter().filter(|a| a.role == "rater") {
        by_item.entry(a.item_id).or_default().push(a);
    }
    let n = c.raters_per_item as usize;
    let complete: Vec<Vec<(String, String)>> = by_item
        .values()
        .filter(|v| v.len() == n)
        .map(|v| {
            v.iter()
                .filter_map(|a| question.comparable(a).map(|k| (a.principal.clone(), k)))
                .collect::<Vec<_>>()
        })
        .filter(|v: &Vec<(String, String)>| v.len() == n)
        .collect();
    if complete.is_empty() || n < 2 {
        return Ok(
            json!({"items": complete.len(), "raters_per_item": n, "exact": null, "fleiss_kappa": null, "cohen_kappa": null}),
        );
    }
    let agreed = complete
        .iter()
        .filter(|v| v.iter().all(|(_, k)| *k == v[0].1))
        .count();
    let ratings: Vec<Vec<String>> = complete
        .iter()
        .map(|v| v.iter().map(|(_, k)| k.clone()).collect())
        .collect();
    let fleiss = fleiss_kappa(&ratings);
    let cohen = if n == 2 {
        let pairs: BTreeSet<(String, String)> = complete
            .iter()
            .map(|v| {
                let mut p = [v[0].0.clone(), v[1].0.clone()];
                p.sort();
                (p[0].clone(), p[1].clone())
            })
            .collect();
        if pairs.len() == 1 {
            let (first, _) = pairs.iter().next().cloned().expect("one pair");
            let labelled: Vec<(String, String)> = complete
                .iter()
                .map(|v| {
                    if v[0].0 == first {
                        (v[0].1.clone(), v[1].1.clone())
                    } else {
                        (v[1].1.clone(), v[0].1.clone())
                    }
                })
                .collect();
            cohen_kappa(&labelled)
        } else {
            None
        }
    } else {
        None
    };
    Ok(json!({
        "items": complete.len(),
        "raters_per_item": n,
        "exact": agreed as f64 / complete.len() as f64,
        "fleiss_kappa": fleiss,
        "cohen_kappa": cohen,
    }))
}

/// Fleiss' kappa over items that each carry the same number of ratings.
/// None when every rating is one category (the chance agreement is one).
pub fn fleiss_kappa(items: &[Vec<String>]) -> Option<f64> {
    let n = items.first()?.len();
    if n < 2 || items.iter().any(|v| v.len() != n) {
        return None;
    }
    let cats: BTreeSet<&String> = items.iter().flatten().collect();
    let total = (items.len() * n) as f64;
    let mut p_bar = 0.0;
    let mut counts_all: BTreeMap<&String, f64> = BTreeMap::new();
    for v in items {
        let mut counts: BTreeMap<&String, f64> = BTreeMap::new();
        for k in v {
            *counts.entry(k).or_default() += 1.0;
            *counts_all.entry(k).or_default() += 1.0;
        }
        let s: f64 = counts.values().map(|c| c * (c - 1.0)).sum();
        p_bar += s / (n as f64 * (n as f64 - 1.0));
    }
    p_bar /= items.len() as f64;
    let p_e: f64 = cats
        .iter()
        .map(|c| {
            let p = counts_all.get(c).copied().unwrap_or(0.0) / total;
            p * p
        })
        .sum();
    if (1.0 - p_e).abs() < 1e-12 {
        return None;
    }
    Some((p_bar - p_e) / (1.0 - p_e))
}

/// Cohen's kappa for two raters, one pair of labels per item.
pub fn cohen_kappa(pairs: &[(String, String)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    let n = pairs.len() as f64;
    let observed = pairs.iter().filter(|(a, b)| a == b).count() as f64 / n;
    let mut first: BTreeMap<&String, f64> = BTreeMap::new();
    let mut second: BTreeMap<&String, f64> = BTreeMap::new();
    for (a, b) in pairs {
        *first.entry(a).or_default() += 1.0;
        *second.entry(b).or_default() += 1.0;
    }
    let expected: f64 = first
        .iter()
        .map(|(k, c)| c / n * second.get(k).copied().unwrap_or(0.0) / n)
        .sum();
    if (1.0 - expected).abs() < 1e-12 {
        return None;
    }
    Some((observed - expected) / (1.0 - expected))
}

// ------------------------------------------------------------ closing

/// A person's pick a pick campaign's item came to: the stacks that stand
/// for the role on the item's occasion.
#[derive(Debug, Clone)]
pub struct PickAsk<'a> {
    pub role: &'a str,
    /// The scheme the campaign's occasions are under, by name.
    pub scheme: &'a str,
    pub subject_id: i64,
    pub session_day: &'a str,
    pub stacks: &'a [i64],
    /// The person whose pick it is: the adjudicator, or the one closing.
    pub who: &'a str,
    /// Why, in words: the campaign, the item and its answers.
    pub why: &'a str,
    pub campaign: i64,
}

/// Writes a person's pick and answers its id, or why it was refused. It is
/// record 42 S3's writer, the one `nils pick set` and `POST /api/picks`
/// use, which a pick run leaves standing; it needs the served pack and the
/// scheme, which this crate does not read, so the engine passes it in.
pub type PickWriter<'a> = &'a dyn Fn(&mut Registry, &PickAsk<'_>) -> Result<i64, String>;

/// Who closes, and what.
#[derive(Clone)]
pub struct Close<'a> {
    pub campaign: i64,
    /// The verified principal closing it, and the kind its actor is.
    pub who: &'a str,
    pub author_kind: &'a str,
    /// The person's pick writer a pick campaign closes through; a pick
    /// campaign closed without one leaves its items unresolved and says so.
    pub picks: Option<PickWriter<'a>>,
}

impl std::fmt::Debug for Close<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Close")
            .field("campaign", &self.campaign)
            .field("who", &self.who)
            .field("author_kind", &self.author_kind)
            .field("picks", &self.picks.is_some())
            .finish()
    }
}

/// What a close did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Closed {
    pub decisions: Vec<i64>,
    pub picks: Vec<i64>,
    /// Items that came to an outcome and were closed with it.
    pub resolved: i64,
    /// Items left without one: unanswered, waiting, or disagreeing with no
    /// adjudication. Their review items stay open in the queue.
    pub unresolved: i64,
    /// Items a decision could not be written for, with why.
    pub refused: Vec<(i64, String)>,
    pub staged: bool,
    pub agreement: Value,
}

impl Closed {
    pub fn as_json(&self) -> Value {
        json!({
            "decisions": self.decisions,
            "picks": self.picks,
            "resolved": self.resolved,
            "unresolved": self.unresolved,
            "refused": self.refused.iter().map(|(i, why)| json!({"item": i, "why": why})).collect::<Vec<_>>(),
            "staged": self.staged,
            "agreement": self.agreement,
        })
    }
}

/// Close a campaign. Every agreed or adjudicated item is written as the
/// campaign says: through [`review::apply`] into one decision, in force or
/// staged; into a person's pick; or into nothing, the review item closed
/// with the outcome. Every answer stays. The rest are left unresolved,
/// their leases ended, and the campaign is closed.
pub fn close(registry: &mut Registry, cl: &Close<'_>, now: &str) -> Result<Closed, Error> {
    let c = get(registry.store(), cl.campaign)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {}", cl.campaign)))?;
    if c.status != "open" {
        return Err(refused(format!("campaign {} is {}", c.name, c.status)));
    }
    let question = c.question()?;
    let staged = c.closes_into == "stage";
    let mut out = Closed {
        staged,
        ..Closed::default()
    };
    let all = items(registry.store(), c.id)?;
    for it in &all {
        if !matches!(it.state.as_str(), "agreed" | "adjudicated") {
            out.unresolved += 1;
            continue;
        }
        let answers = answers_of_item(registry.store(), it.id)?;
        let adjudicator = answers.iter().rev().find(|a| a.role == "adjudicator");
        let raters = answers.iter().filter(|a| a.role == "rater").count();
        let path = match adjudicator {
            Some(a) => format!(
                "campaign {} item {}: adjudicated by {} over {raters} answer(s)",
                c.name, it.position, a.principal
            ),
            None => format!(
                "campaign {} item {}: {raters} answer(s) agreed",
                c.name, it.position
            ),
        };
        // the author of what the item came to: the adjudicator where there
        // was one, else the principal closing it, each as verified
        let (who, kind) = match adjudicator {
            Some(a) => (a.principal.clone(), a.author_kind.clone()),
            None => (cl.who.to_string(), cl.author_kind.to_string()),
        };
        match c.closes_into.as_str() {
            "decision" | "stage" => {
                let value = it.outcome["value"].as_str().map(str::to_string);
                let Some(value) = value else {
                    out.refused
                        .push((it.id, "the item came to no value".into()));
                    out.unresolved += 1;
                    continue;
                };
                let applied = review::apply(
                    registry,
                    &review::Apply {
                        item: it.review_item_id,
                        member: None,
                        scope: "stack",
                        value: Some(&value),
                        author: review::Author {
                            who: &who,
                            kind: &kind,
                            version: None,
                            model: None,
                        },
                        stage: staged,
                        why: Some(&path),
                        campaign: Some(c.id),
                    },
                );
                match applied {
                    Ok(applied) => {
                        let store = registry.store();
                        if !applied.closed.contains(&it.review_item_id) {
                            finish_review_item(
                                store,
                                it.review_item_id,
                                if staged { "staged" } else { "accepted" },
                                &who,
                                &json!({"campaign": c.id, "decision": applied.decision, "value": value}),
                                Some(applied.decision),
                                now,
                            )?;
                        }
                        mark_review_item(store, it.review_item_id, "resolved")?;
                        store.update_by_id(
                            table("campaign_item"),
                            &[
                                ("state", Param::from("resolved")),
                                ("decision_id", Param::Int(applied.decision)),
                                ("resolved_at", Param::from(now)),
                            ],
                            "id",
                            it.id,
                        )?;
                        out.decisions.push(applied.decision);
                        out.resolved += 1;
                    }
                    Err(e) => {
                        out.refused.push((it.id, e.to_string()));
                        out.unresolved += 1;
                    }
                }
            }
            "pick" => {
                let Question::Pick { role, scheme } = &question else {
                    unreachable!("create refuses a pick close of another question");
                };
                let stacks = it.outcome["value"]
                    .as_str()
                    .map(pick_stacks)
                    .transpose()?
                    .unwrap_or_default();
                let (Some(subject), Some(day)) = (it.subject_id, it.session_day.as_deref()) else {
                    out.refused
                        .push((it.id, "the item names no session".into()));
                    out.unresolved += 1;
                    continue;
                };
                // a pick is a person's (record 42 S3): an agent's or a
                // model's answer is evidence, never a pick
                if kind != "person" {
                    out.refused.push((
                        it.id,
                        format!("a pick is a person's; {who} answered as a {kind}"),
                    ));
                    out.unresolved += 1;
                    continue;
                }
                let Some(write) = cl.picks else {
                    out.refused.push((
                        it.id,
                        "a pick campaign closes through the person's pick writer, and none was given"
                            .into(),
                    ));
                    out.unresolved += 1;
                    continue;
                };
                let pick = match write(
                    registry,
                    &PickAsk {
                        role,
                        scheme,
                        subject_id: subject,
                        session_day: day,
                        stacks: &stacks,
                        who: &who,
                        why: &path,
                        campaign: c.id,
                    },
                ) {
                    Ok(pick) => pick,
                    Err(why) => {
                        out.refused.push((it.id, why));
                        out.unresolved += 1;
                        continue;
                    }
                };
                let store = registry.store();
                finish_review_item(
                    store,
                    it.review_item_id,
                    "accepted",
                    &who,
                    &json!({"campaign": c.id, "pick": pick, "stacks": stacks, "role": role}),
                    None,
                    now,
                )?;
                mark_review_item(store, it.review_item_id, "resolved")?;
                store.update_by_id(
                    table("campaign_item"),
                    &[
                        ("state", Param::from("resolved")),
                        ("pick_id", Param::Int(pick)),
                        ("resolved_at", Param::from(now)),
                    ],
                    "id",
                    it.id,
                )?;
                out.picks.push(pick);
                out.resolved += 1;
            }
            _ => {
                let store = registry.store();
                finish_review_item(
                    store,
                    it.review_item_id,
                    "accepted",
                    &who,
                    &json!({"campaign": c.id, "closed_into": "none", "outcome": it.outcome}),
                    None,
                    now,
                )?;
                mark_review_item(store, it.review_item_id, "resolved")?;
                store.update_by_id(
                    table("campaign_item"),
                    &[
                        ("state", Param::from("resolved")),
                        ("resolved_at", Param::from(now)),
                    ],
                    "id",
                    it.id,
                )?;
                out.resolved += 1;
            }
        }
    }
    // The close is one act. Each decision it staged moved the epoch as it
    // was audited, so the earlier ones would read as staged before the
    // registry moved on, by the close's own later writes; what the person
    // closing looked at is the campaign, so every one carries the epoch
    // the close left.
    if staged && !out.decisions.is_empty() {
        let epoch = registry.meta().epoch;
        let store = registry.store();
        store.update_by_ids(
            table("decision"),
            &[("epoch_staged", Param::Int(epoch))],
            "id",
            &out.decisions,
        )?;
    }
    out.agreement = agreement(registry.store(), c.id)?;
    let store = registry.store();
    let d = store.dialect();
    // the leases still out end with the campaign
    store.execute(
        &format!(
            "UPDATE {} SET state = 'released', ended_at = {} WHERE campaign_id = {} AND state IN ('leased', 'offered')",
            store.qualified("campaign_assignment"),
            d.param(1, Type::Timestamp),
            d.param(2, Type::Int)
        ),
        &[Param::from(now), Param::Int(c.id)],
    )?;
    store.update_by_id(
        table("campaign"),
        &[
            ("status", Param::from("closed")),
            ("closed_at", Param::from(now)),
            ("closed_by", Param::from(cl.who)),
            ("agreement", Param::from(out.agreement.to_string())),
        ],
        "id",
        c.id,
    )?;
    audit::record(
        registry,
        &Entry {
            principal: cl.who,
            action: Action::CampaignClose,
            scope: json!({"campaign": c.id, "name": c.name}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "closes_into": c.closes_into, "decisions": out.decisions.len(),
                "picks": out.picks.len(), "resolved": out.resolved,
                "unresolved": out.unresolved, "refused": out.refused.len(),
                "agreement": out.agreement,
            })),
        },
    )?;
    Ok(out)
}

/// Close a review item with what the campaign made of it, when `apply` did
/// not (a question it does not recognise as its own axis, a pick, nothing).
fn finish_review_item(
    store: &mut Store,
    id: i64,
    status: &str,
    who: &str,
    decision: &Value,
    decision_id: Option<i64>,
    now: &str,
) -> Result<(), StoreError> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = {}, decided_at = {}, actor = {}, decision = {}, decision_id = {} \
         WHERE id = {} AND status IN ('open', 'staged')",
        store.qualified("review_item"),
        d.param(1, Type::Text),
        d.param(2, Type::Timestamp),
        d.param(3, Type::Text),
        d.param(4, Type::Json),
        d.param(5, Type::Int),
        d.param(6, Type::Int),
    );
    store.execute(
        &sql,
        &[
            Param::from(status),
            Param::from(now),
            Param::from(who),
            Param::from(decision.to_string()),
            decision_id.map_or(Param::Null, Param::Int),
            Param::Int(id),
        ],
    )?;
    Ok(())
}

/// A campaign's counts by item state, and its assignments by state.
pub fn counts(store: &mut Store, campaign: i64) -> Result<Value, Error> {
    let d = store.dialect();
    let mut states = serde_json::Map::new();
    for r in store.query(
        &format!(
            "SELECT state, COUNT(*) FROM {} WHERE campaign_id = {} GROUP BY state ORDER BY state",
            store.qualified("campaign_item"),
            d.param(1, Type::Int)
        ),
        &[Param::Int(campaign)],
    )? {
        states.insert(r.text(0)?.to_string(), json!(r.int(1)?));
    }
    let mut leases = serde_json::Map::new();
    for r in store.query(
        &format!(
            "SELECT state, COUNT(*) FROM {} WHERE campaign_id = {} GROUP BY state ORDER BY state",
            store.qualified("campaign_assignment"),
            d.param(1, Type::Int)
        ),
        &[Param::Int(campaign)],
    )? {
        leases.insert(r.text(0)?.to_string(), json!(r.int(1)?));
    }
    let answers = store
        .query(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE campaign_id = {}",
                store.qualified("campaign_answer"),
                d.param(1, Type::Int)
            ),
            &[Param::Int(campaign)],
        )?
        .first()
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0);
    Ok(json!({"items": states, "assignments": leases, "answers": answers}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn kappa_is_one_for_perfect_agreement_and_zero_at_chance() {
        let perfect = vec![
            s(&["a", "a"]),
            s(&["b", "b"]),
            s(&["a", "a"]),
            s(&["b", "b"]),
        ];
        assert!((fleiss_kappa(&perfect).unwrap() - 1.0).abs() < 1e-9);
        let pairs: Vec<(String, String)> = perfect
            .iter()
            .map(|v| (v[0].clone(), v[1].clone()))
            .collect();
        assert!((cohen_kappa(&pairs).unwrap() - 1.0).abs() < 1e-9);
        // every combination once: no better than chance
        let chance = [
            s(&["a", "a"]),
            s(&["a", "b"]),
            s(&["b", "a"]),
            s(&["b", "b"]),
        ];
        let pairs: Vec<(String, String)> = chance
            .iter()
            .map(|v| (v[0].clone(), v[1].clone()))
            .collect();
        assert!(cohen_kappa(&pairs).unwrap().abs() < 1e-9);
        // one category only: kappa is undefined, not a number
        assert_eq!(fleiss_kappa(&[s(&["a", "a"]), s(&["a", "a"])]), None);
    }

    #[test]
    fn fleiss_kappa_matches_the_textbook_example() {
        // Fleiss (1971), as Wikipedia works it: ten items, fourteen raters,
        // five categories, kappa 0.210
        let table = [
            [0, 0, 0, 0, 14],
            [0, 2, 6, 4, 2],
            [0, 0, 3, 5, 6],
            [0, 3, 9, 2, 0],
            [2, 2, 8, 1, 1],
            [7, 7, 0, 0, 0],
            [3, 2, 6, 3, 0],
            [2, 5, 3, 2, 2],
            [6, 5, 2, 1, 0],
            [0, 2, 2, 3, 7],
        ];
        let items: Vec<Vec<String>> = table
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .flat_map(|(c, n)| std::iter::repeat_n(c.to_string(), *n))
                    .collect()
            })
            .collect();
        let k = fleiss_kappa(&items).unwrap();
        assert!((k - 0.210).abs() < 0.001, "{k}");
    }

    #[test]
    fn a_question_holds_its_answers_to_its_shape() {
        let q = Question::parse(
            &json!({"kind": "axis", "axis": "body_part", "values": ["brain", "spine"]}),
        )
        .unwrap();
        let given = |value: Option<&'static str>| Given {
            assignment: 1,
            principal: "p",
            author_kind: "person",
            value,
            form: None,
            derivative_id: None,
            why: None,
        };
        q.check(&given(Some("brain"))).unwrap();
        assert!(q.check(&given(Some("chest"))).is_err());
        assert!(q.check(&given(None)).is_err());
        let f = Question::parse(&json!({"kind": "form", "schema": {
            "properties": {"quality": {"enum": ["good", "poor"]}, "note": {"type": "string"}},
            "required": ["quality"]
        }}))
        .unwrap();
        let good = json!({"quality": "good"});
        let bad = json!({"quality": "lovely"});
        let extra = json!({"quality": "good", "who": "x"});
        let with = |form: &'static Value| Given {
            form: Some(form),
            ..given(None)
        };
        let good: &'static Value = Box::leak(Box::new(good));
        let bad: &'static Value = Box::leak(Box::new(bad));
        let extra: &'static Value = Box::leak(Box::new(extra));
        f.check(&with(good)).unwrap();
        assert!(f.check(&with(bad)).is_err());
        assert!(f.check(&with(extra)).is_err());
        let m = Question::parse(&json!({"kind": "derivative", "derivative_kind": "mask"})).unwrap();
        assert!(m.check(&given(None)).is_err());
        m.check(&Given {
            derivative_id: Some(7),
            ..given(None)
        })
        .unwrap();
        assert!(Question::parse(&json!({"kind": "vote"})).is_err());
        assert_eq!(pick_stacks("14, 12,12").unwrap(), vec![12, 14]);
        assert!(pick_stacks("x").is_err());
    }
}
