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
//! - **Six questions.** `axis` (which value of one pack axis), `axes`
//!   (several axes of a stack at once, record 45: the answer is held to the
//!   pack's legal combinations and closes into one decision per axis),
//!   `pick` (which stacks stand for a role in a session), `form` (a small
//!   declared form), `derivative` (a file answer, a mask, named by its
//!   derivative id; the bytes go through the derivative door) and `free`.
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
    /// Several axes of one stack at once (record 45 R2): the answer names a
    /// value, or a set of them for a multi-valued axis, for every axis
    /// asked, and is refused unless the pack's constraints, frozen into the
    /// question when the campaign was made, allow the combination
    /// (`nils_pack::legal`). It closes into one decision per axis.
    Axes {
        axes: Vec<String>,
        constraints: Value,
        /// Record 48, after the first real read: the axes the rater is not
        /// asked, which the engine computes from the answer through the
        /// pack's rules (`nils_pack::derive`) and keeps beside it, marked
        /// derived. Empty where the question derives nothing.
        derive: Vec<String>,
    },
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
pub const KINDS: [&str; 6] = ["axis", "axes", "pick", "form", "derivative", "free"];

impl Question {
    pub fn parse(v: &Value) -> Result<Question, Error> {
        let kind = v["kind"]
            .as_str()
            .ok_or_else(|| invalid("question.kind: axis, axes, pick, form, derivative or free"))?;
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
                if values.iter().any(|v| v == CANT_TELL) {
                    return Err(invalid(format!(
                        "question.values names {CANT_TELL}, the rater's can't tell on an axes question, never a value"
                    )));
                }
                Question::Axis {
                    axis: axis.to_string(),
                    values,
                }
            }
            "axes" => {
                let axes: Vec<String> = v["axes"]
                    .as_array()
                    .filter(|a| !a.is_empty())
                    .ok_or_else(|| invalid("an axes question names its axes, a list"))?
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .ok_or_else(|| invalid("question.axes: a list of axis names"))
                    })
                    .collect::<Result<_, _>>()?;
                if axes.iter().collect::<BTreeSet<_>>().len() != axes.len() {
                    return Err(invalid("question.axes names an axis twice"));
                }
                let constraints = v["constraints"].clone();
                check_constraints(&axes, &constraints)?;
                let derive: Vec<String> = match &v["derive"] {
                    Value::Null => Vec::new(),
                    Value::Array(list) => list
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                                .ok_or_else(|| invalid("question.derive: a list of axis names"))
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err(invalid("question.derive: a list of axis names")),
                };
                if derive.iter().collect::<BTreeSet<_>>().len() != derive.len() {
                    return Err(invalid("question.derive names an axis twice"));
                }
                if let Some(a) = derive.iter().find(|d| axes.contains(d)) {
                    return Err(invalid(format!(
                        "{a} is asked and derived at once: an asked axis is the rater's to answer"
                    )));
                }
                Question::Axes {
                    axes,
                    constraints,
                    derive,
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
                    "{other} is not a question: axis, axes, pick, form, derivative or free"
                )));
            }
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Question::Axis { .. } => "axis",
            Question::Axes { .. } => "axes",
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
            Question::Axes {
                axes,
                constraints,
                derive,
            } => {
                let mut v = json!({
                    "kind": "axes", "axes": axes, "values": constraints["values"],
                    "constraints": constraints,
                });
                if !derive.is_empty() {
                    v["derive"] = json!(derive);
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
            Question::Axes { axes, .. } => format!("axes:{}", axes.join("+")),
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
                // can't tell is an axes answer's word (record 48); on an axis
                // question it would close into a decision that says it
                if v.trim() == CANT_TELL {
                    return Err(invalid(format!(
                        "{CANT_TELL} is never a value of {axis}; an axes question takes it as an answer"
                    )));
                }
                if !values.is_empty() && !values.iter().any(|x| x == v) {
                    return Err(invalid(format!(
                        "{v} is not a value of {axis} this campaign asks for: {}",
                        values.join(", ")
                    )));
                }
            }
            Question::Axes {
                axes, constraints, ..
            } => {
                let v = value.ok_or_else(|| {
                    invalid(format!(
                        "the answer names a value of each of {}, as {{axis: value}}",
                        axes.join(", ")
                    ))
                })?;
                let joint = answer_joint_of(axes, constraints, v)?;
                legal(constraints, &joint).map_err(invalid)?;
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
            // an answer kept from before a requestion is compared on the
            // axes asked now
            Question::Axes {
                axes, constraints, ..
            } => a.value.as_deref().map(|v| {
                stored_joint_of(axes, constraints, v)
                    .map(|j| canonical_joint(constraints, &j))
                    .unwrap_or_else(|_| v.to_string())
            }),
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

// ------------------------------------------------------- the axes question

/// A joint answer: each asked axis with its values, none for "the axis has
/// no value here", one for a single-valued axis, a set for a multi-valued
/// one, in identities. In a rater's answer an axis may hold [`CANT_TELL`]
/// alone instead ([`answer_joint_of`]).
pub type Joint = BTreeMap<String, Vec<String>>;

/// The rater's "can't tell" on one axis of an axes answer (record 48, how
/// the reference is read): the data give no clue, so the rater does not
/// guess. It is an answer, distinct from an axis left out (refused) and
/// from null (the axis has no value here), and distinct from a pack's own
/// fallback value such as `Unknown`, which is the pipeline's. It is never a
/// value of the pack: a question over a pack that names it is refused, the
/// pack's constraints read such an axis as unnamed, and a close writes no
/// decision on it. Two raters who both say it agree on the axis.
pub const CANT_TELL: &str = "cant_tell";

/// Whether an axis of a joint answer is the rater's can't tell.
pub fn cant_tell(values: &[String]) -> bool {
    values.len() == 1 && values[0] == CANT_TELL
}

/// The axes a joint answer says can't tell of, in order.
pub fn cant_tell_axes(a: &Joint) -> Vec<String> {
    a.iter()
        .filter(|(_, v)| cant_tell(v))
        .map(|(k, _)| k.clone())
        .collect()
}

/// A joint answer without its can't-tell axes: what the pack's constraints
/// and a close read.
pub fn told(a: &Joint) -> Joint {
    a.iter()
        .filter(|(_, v)| !cant_tell(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The constraints an axes question carries are the shape
/// `nils_pack::legal::constraints` writes: every asked axis's vocabulary,
/// the multi-valued axes, the exclusion groups and the implications.
fn check_constraints(axes: &[String], c: &Value) -> Result<(), Error> {
    if !c.is_object() {
        return Err(invalid(
            "an axes question carries the pack's constraints, which the engine fills in from the served pack when the campaign is made",
        ));
    }
    for a in axes {
        if !c["values"][a].is_array() {
            return Err(invalid(format!(
                "the constraints name no values of {a}, an axis the question asks"
            )));
        }
        if words(&c["values"][a]).iter().any(|v| v == CANT_TELL) {
            return Err(invalid(format!(
                "{a} names {CANT_TELL} as a value, and {CANT_TELL} is the rater's can't tell, never a value"
            )));
        }
    }
    if !c["multi"].is_array() || !c["implications"].is_array() {
        return Err(invalid(
            "the constraints are {values, multi, groups, implications}",
        ));
    }
    Ok(())
}

fn words(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// An axes answer, `{axis: value | [values] | null}`, read against the
/// question: every asked axis and no other, each value one the question
/// asks for, a single-valued axis one value at most. Values only: a
/// suggestion, System 1's candidates and the pack's own reads never say
/// [`CANT_TELL`], and are refused where they do.
pub fn joint_of(axes: &[String], constraints: &Value, text: &str) -> Result<Joint, Error> {
    read_joint(axes, constraints, text, false)
}

/// A rater's axes answer: as [`joint_of`], and any axis may be
/// [`CANT_TELL`], alone. Every asked axis is still named, so an axis is
/// never skipped by accident.
pub fn answer_joint_of(axes: &[String], constraints: &Value, text: &str) -> Result<Joint, Error> {
    read_joint(axes, constraints, text, true)
}

/// A kept answer read against the question as it stands: an answer given
/// before `nils campaign requestion` moved an axis from asked to derived
/// still names that axis, and is read on the axes asked now. The answer
/// itself is never rewritten.
pub fn stored_joint_of(axes: &[String], constraints: &Value, text: &str) -> Result<Joint, Error> {
    let trimmed = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(mut m)) => {
            m.retain(|k, _| axes.contains(k));
            Value::Object(m).to_string()
        }
        _ => text.to_string(),
    };
    read_joint(axes, constraints, &trimmed, true)
}

fn read_joint(
    axes: &[String],
    constraints: &Value,
    text: &str,
    may_cant_tell: bool,
) -> Result<Joint, Error> {
    let v: Value = serde_json::from_str(text)
        .map_err(|_| invalid("an axes answer is an object, {axis: value | [values]}"))?;
    let given = v
        .as_object()
        .ok_or_else(|| invalid("an axes answer is an object, {axis: value | [values]}"))?;
    for k in given.keys() {
        if !axes.contains(k) {
            return Err(invalid(format!(
                "the answer names {k}, which the question does not ask; it asks {}",
                axes.join(", ")
            )));
        }
    }
    let multi = words(&constraints["multi"]);
    let mut out = Joint::new();
    for axis in axes {
        let raw = given.get(axis).ok_or_else(|| {
            invalid(format!(
                "the answer names every axis the question asks, and {axis} is missing (null says it has no value)"
            ))
        })?;
        let mut values: Vec<String> = match raw {
            Value::Null => Vec::new(),
            Value::String(s) if s.trim().is_empty() => Vec::new(),
            Value::String(s) => vec![s.trim().to_string()],
            Value::Array(list) => list
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(|s| s.trim().to_string())
                        .ok_or_else(|| invalid(format!("{axis}: a value or a list of values")))
                })
                .collect::<Result<_, _>>()?,
            _ => return Err(invalid(format!("{axis}: a value or a list of values"))),
        };
        values.sort();
        values.dedup();
        if values.iter().any(|v| v == CANT_TELL) {
            if !may_cant_tell {
                return Err(invalid(format!(
                    "{CANT_TELL} is a rater's answer on {axis}, never a value of it"
                )));
            }
            if values.len() > 1 {
                return Err(invalid(format!(
                    "{axis}: {CANT_TELL} stands alone, never beside a value"
                )));
            }
            out.insert(axis.clone(), values);
            continue;
        }
        if values.len() > 1 && !multi.contains(axis) {
            return Err(invalid(format!(
                "{axis} holds one value, and the answer names {}",
                values.join(", ")
            )));
        }
        let allowed = words(&constraints["values"][axis]);
        if let Some(bad) = values.iter().find(|x| !allowed.contains(x)) {
            return Err(invalid(format!(
                "{bad} is not a value of {axis} this campaign asks for: {}",
                allowed.join(", ")
            )));
        }
        out.insert(axis.clone(), values);
    }
    Ok(out)
}

/// An assignment as the one text two equal answers share: the axes in
/// order, a single-valued axis as its value or null, a multi-valued one as
/// its sorted list.
pub fn canonical_joint(constraints: &Value, a: &Joint) -> String {
    let multi = words(&constraints["multi"]);
    let mut m = serde_json::Map::new();
    for (axis, values) in a {
        let v = if cant_tell(values) {
            json!(CANT_TELL)
        } else if multi.contains(axis) {
            json!(values)
        } else {
            values.first().map_or(Value::Null, |v| json!(v))
        };
        m.insert(axis.clone(), v);
    }
    Value::Object(m).to_string()
}

/// A condition of the constraint language on an answer: true, false, or
/// unknown when it reads an axis the answer does not name.
fn holds(e: &Value, a: &Joint) -> Option<bool> {
    match e {
        Value::Bool(b) => Some(*b),
        Value::Object(m) => {
            if let Some(axis) = m.get("axis").and_then(Value::as_str) {
                let held = a.get(axis)?;
                if let Some(v) = m.get("is").and_then(Value::as_str) {
                    return Some(held.iter().any(|x| x == v));
                }
                if let Some(v) = m.get("missing_or").and_then(Value::as_str) {
                    return Some(held.is_empty() || held.iter().any(|x| x == v));
                }
                return None;
            }
            if let Some(list) = m.get("all").and_then(Value::as_array) {
                let each: Vec<Option<bool>> = list.iter().map(|x| holds(x, a)).collect();
                if each.contains(&Some(false)) {
                    return Some(false);
                }
                return each.iter().all(|x| *x == Some(true)).then_some(true);
            }
            if let Some(list) = m.get("any").and_then(Value::as_array) {
                let each: Vec<Option<bool>> = list.iter().map(|x| holds(x, a)).collect();
                if each.contains(&Some(true)) {
                    return Some(true);
                }
                return each.iter().all(|x| *x == Some(false)).then_some(false);
            }
            if let Some(x) = m.get("not") {
                return holds(x, a).map(|b| !b);
            }
            None
        }
        _ => None,
    }
}

/// Whether the pack allows an assignment: at most one member of each
/// exclusion group, and what an implication whose condition holds sets.
/// The words say which rule or group forbids it.
pub fn legal(constraints: &Value, a: &Joint) -> Result<(), String> {
    // a can't-tell axis is read as one the answer does not name: no group
    // holds it and no implication reads or sets it
    let a = &told(a);
    for (axis, groups) in constraints["groups"].as_object().into_iter().flatten() {
        let Some(held) = a.get(axis) else { continue };
        for (group, members) in groups.as_object().into_iter().flatten() {
            let members = words(members);
            let both: Vec<&String> = held.iter().filter(|v| members.contains(v)).collect();
            if both.len() > 1 {
                return Err(format!(
                    "{} are all in the exclusion group {group} of {axis}, and at most one of them holds (the pack, {})",
                    both.iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(" and "),
                    constraints["pack"].as_str().unwrap_or("its pack")
                ));
            }
        }
    }
    for imp in constraints["implications"].as_array().into_iter().flatten() {
        if holds(&imp["when"], a) != Some(true) {
            continue;
        }
        for t in imp["then"].as_array().into_iter().flatten() {
            if !t["when"].is_null() && holds(&t["when"], a) != Some(true) {
                continue;
            }
            let (Some(axis), Some(value)) = (t["axis"].as_str(), t["value"].as_str()) else {
                continue;
            };
            let Some(held) = a.get(axis) else { continue };
            if !held.iter().any(|v| v == value) {
                return Err(format!(
                    "the pack's rule {} sets {axis} to {value} when {}, and the answer says {axis} is {}",
                    imp["rule"].as_str().unwrap_or("?"),
                    said(&imp["when"]),
                    if held.is_empty() {
                        "nothing".to_string()
                    } else {
                        held.join(", ")
                    }
                ));
            }
        }
    }
    Ok(())
}

/// A condition in words, for a refusal.
fn said(e: &Value) -> String {
    match e {
        Value::Bool(b) => b.to_string(),
        Value::Object(m) => {
            if let Some(axis) = m.get("axis").and_then(Value::as_str) {
                if let Some(v) = m.get("is").and_then(Value::as_str) {
                    return format!("{axis} is {v}");
                }
                if let Some(v) = m.get("missing_or").and_then(Value::as_str) {
                    return format!("{axis} is {v} or nothing");
                }
            }
            for (key, join) in [("all", " and "), ("any", " or ")] {
                if let Some(list) = m.get(key).and_then(Value::as_array) {
                    return list.iter().map(said).collect::<Vec<_>>().join(join);
                }
            }
            if let Some(x) = m.get("not") {
                return format!("not ({})", said(x));
            }
            e.to_string()
        }
        _ => e.to_string(),
    }
}

/// What one axis of an axes answer says, as text, for agreement per axis.
fn axis_of_answer(text: Option<&str>, axis: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text?).ok()?;
    v.get(axis).map(Value::to_string)
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
    /// Record 48 R1: the share of each batch held back to be read alone,
    /// from [`HOLD_BACK_MIN`] to one; [`HOLD_BACK_MIN`] when none is given.
    pub hold_back: Option<f64>,
}

/// Record 48 R1: the least share of a batch held back to be read alone.
pub const HOLD_BACK_MIN: f64 = 0.1;

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
    /// Record 48 R1: the share of each batch held back to be read alone.
    pub hold_back: f64,
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

    /// The question as served: as stored, and what an answer to it may say
    /// beside a value (record 48): `unsure` on any answer, and for an axes
    /// question `cant_tell`, the word that says it on an axis.
    pub fn served_question(&self) -> Value {
        let mut q = self.question.clone();
        if q.is_object() {
            q["unsure"] = json!(true);
            if q["kind"] == "axes" {
                q["cant_tell"] = json!(CANT_TELL);
            }
        }
        q
    }

    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "owner": self.owner,
            "status": self.status,
            "question": self.served_question(),
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
            "hold_back": self.hold_back,
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

const CAMPAIGN_COLUMNS: [&str; 21] = [
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
    "hold_back",
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
        hold_back: r.opt_double(20)?.unwrap_or(HOLD_BACK_MIN),
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
        ("decision" | "stage", Question::Axis { .. } | Question::Axes { .. })
        | ("pick", Question::Pick { .. })
        | ("none", _) => {}
        ("decision" | "stage", _) => {
            return Err(invalid(
                "only an axis or an axes question closes into decisions; close the others into none",
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
    let hold_back = n.hold_back.unwrap_or(HOLD_BACK_MIN);
    if !(HOLD_BACK_MIN..=1.0).contains(&hold_back) {
        return Err(invalid(format!(
            "hold_back is the share of a batch read alone, from {HOLD_BACK_MIN} to 1"
        )));
    }
    let grain = match (&n.items, &question) {
        (Items::Sessions(_), Question::Axis { .. } | Question::Axes { .. }) => {
            return Err(invalid(
                "an axis or an axes question is asked of stacks, not sessions",
            ));
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
                if let Question::Axes { axes, .. } = &question
                    && !it.evidence["axis"]
                        .as_str()
                        .is_some_and(|a| axes.iter().any(|x| x == a))
                {
                    return Err(invalid(format!(
                        "review item {id} is not a question about {}, the axes the question asks",
                        axes.join(", ")
                    )));
                }
                out.push(it);
            }
            let held = held_review_items(store)?;
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
                        "hold_back",
                        "hold_back_seed",
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
                    Param::Double(hold_back),
                    Param::from(uuid::Uuid::new_v4().to_string()),
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
                Question::Axes { axes, .. } => e["axes"] = json!(axes),
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
            // Record 45: an axes question asks a stack once, whatever the
            // adopted items it answers. One item per stack, backed by a
            // review item of its own that names the adopted ones (`asks`),
            // which its close answers axis by axis.
            Items::Review(_) if matches!(question, Question::Axes { .. }) => {
                let mut by_stack: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
                let mut order: Vec<i64> = Vec::new();
                for it in &adopted {
                    let stacks: Vec<i64> = if it.scope == "group" {
                        review::members(store, it.id)
                            .map_err(|e| invalid(e.to_string()))?
                            .into_iter()
                            .filter(|m| m.decided_at.is_none())
                            .map(|m| m.stack_id)
                            .collect()
                    } else {
                        it.reference["stack_id"].as_i64().into_iter().collect()
                    };
                    for s in stacks {
                        if !by_stack.contains_key(&s) {
                            order.push(s);
                        }
                        by_stack.entry(s).or_default().push(it.id);
                    }
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
                }
                for (i, stack) in order.iter().enumerate() {
                    let r = raise(
                        store,
                        &kind,
                        "stack",
                        &json!({"stack_id": stack}),
                        &evidence(json!({"position": i, "asks": by_stack[stack]})),
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
            Items::Review(_) => {
                let mut position = 0usize;
                for it in &adopted {
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
                    // a group is asked stack by stack: one item per member
                    // not yet decided, so a rater answers a stack, never a
                    // whole confidence band (wave 43's proof: 3 items for
                    // 239 stacks)
                    if it.scope == "group" {
                        for m in
                            review::members(store, it.id).map_err(|e| invalid(e.to_string()))?
                        {
                            if m.decided_at.is_some() {
                                continue;
                            }
                            rows.push(item_row(
                                position,
                                it.id,
                                Some(m.stack_id),
                                None,
                                None,
                                &format!("review:{}:stack:{}", it.id, m.stack_id),
                            ));
                            position += 1;
                        }
                        continue;
                    }
                    let stack = it.reference["stack_id"].as_i64();
                    rows.push(item_row(
                        position,
                        it.id,
                        stack,
                        None,
                        None,
                        &format!("review:{}", it.id),
                    ));
                    position += 1;
                }
                if rows.is_empty() {
                    return Err(invalid("the review items name no stack left to decide"));
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

/// The review items an open campaign asks: the ones its items stand on,
/// and those an axes item answers (`asks`, record 45), so one item is asked
/// by one open campaign at a time.
/// The open campaign that asks a review item, by id and name: one whose
/// items stand on it, or whose axes item answers it (`asks`, record 45).
/// Such an item is answered in the campaign and closed by its close, not
/// at Review's apply doors.
pub fn holder(store: &mut Store, review_item: i64) -> Result<Option<(i64, String)>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT c.id, c.name FROM {} i JOIN {} c ON c.id = i.campaign_id \
         WHERE c.status IN ('open', 'closing') AND i.review_item_id = {} ORDER BY c.id",
        store.qualified("campaign_item"),
        store.qualified("campaign"),
        d.param(1, Type::Int),
    );
    if let Some(r) = store.query(&sql, &[Param::Int(review_item)])?.first() {
        return Ok(Some((r.int(0)?, r.text(1)?.to_string())));
    }
    let t = table("review_item");
    let sql = format!(
        "SELECT c.id, c.name, {} FROM {} i JOIN {} c ON c.id = i.campaign_id JOIN {} r ON r.id = i.review_item_id \
         WHERE c.status IN ('open', 'closing') AND r.kind = 'campaign.axes' ORDER BY c.id",
        d.text_of(t.column("evidence").expect("evidence")),
        store.qualified("campaign_item"),
        store.qualified("campaign"),
        store.qualified("review_item"),
    );
    for r in store.query(&sql, &[])? {
        let ev: Value = r
            .opt_text(2)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        if ev["asks"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|x| x.as_i64() == Some(review_item))
        {
            return Ok(Some((r.int(0)?, r.text(1)?.to_string())));
        }
    }
    Ok(None)
}

fn held_review_items(store: &mut Store) -> Result<BTreeSet<i64>, Error> {
    let sql = format!(
        "SELECT i.review_item_id FROM {} i JOIN {} c ON c.id = i.campaign_id WHERE c.status IN ('open', 'closing')",
        store.qualified("campaign_item"),
        store.qualified("campaign")
    );
    let backing: Vec<i64> = store
        .query(&sql, &[])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    let mut held: BTreeSet<i64> = backing.iter().copied().collect();
    let t = table("review_item");
    for chunk in backing.chunks(500) {
        let sql = format!(
            "SELECT {} FROM {} WHERE kind = 'campaign.axes' AND id IN ({})",
            store
                .dialect()
                .text_of(t.column("evidence").expect("evidence")),
            store.qualified("review_item"),
            join_ids(chunk)
        );
        for r in store.query(&sql, &[])? {
            let ev: Value = r
                .opt_text(0)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            held.extend(
                ev["asks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64),
            );
        }
    }
    Ok(held)
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
    /// Record 48 R1: a batch held the item back to be read alone.
    pub held_back: bool,
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
            "resolved_at": self.resolved_at, "held_back": self.held_back,
        })
    }
}

const ITEM_COLUMNS: [&str; 18] = [
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
    "held_back",
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
        held_back: r.opt_int(17)?.unwrap_or(0) != 0,
    })
}

/// Record 48 R1: the items a rater could still be given, in position
/// order: open in round one, wanting raters, never given to this rater,
/// and not held back by a batch to be read alone.
pub fn open_for(
    store: &mut Store,
    campaign: &Campaign,
    principal: &str,
) -> Result<Vec<Item>, Error> {
    let d = store.dialect();
    let a = store.qualified("campaign_assignment");
    // a lease past its end holds its item for no one, as a claim finds it
    let sql = format!(
        "SELECT {} FROM {} i WHERE i.campaign_id = {} AND i.state = 'open' AND i.round = 1 \
         AND (i.held_back IS NULL OR i.held_back = 0) \
         AND (SELECT COUNT(*) FROM {a} a WHERE a.item_id = i.id AND a.role = 'rater' \
              AND (a.state = 'submitted' OR (a.state = 'leased' AND a.lease_until >= {}))) < {} \
         AND NOT EXISTS (SELECT 1 FROM {a} a WHERE a.item_id = i.id AND a.principal = {}) \
         ORDER BY i.position",
        select_list(store, "campaign_item", &ITEM_COLUMNS, Some("i")),
        store.qualified("campaign_item"),
        d.param(1, Type::Int),
        d.param(2, Type::Timestamp),
        d.param(3, Type::Int),
        d.param(4, Type::Text),
    );
    Ok(store
        .query(
            &sql,
            &[
                Param::Int(campaign.id),
                Param::from(now_iso().as_str()),
                Param::Int(campaign.raters_per_item),
                Param::from(principal),
            ],
        )?
        .iter()
        .map(item_of)
        .collect::<Result<_, _>>()?)
}

/// Record 48 R1: the seed the engine drew when the campaign was made, from
/// which the items a batch holds back are chosen. Never returned by a door.
pub fn hold_back_seed(store: &mut Store, campaign: i64) -> Result<String, Error> {
    let sql = format!(
        "SELECT hold_back_seed FROM {} WHERE id = {}",
        store.qualified("campaign"),
        store.dialect().param(1, Type::Int)
    );
    if let Some(seed) = store
        .query_opt(&sql, &[Param::Int(campaign)])?
        .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
    {
        return Ok(seed);
    }
    // none drawn yet: draw one now and keep it
    let seed = uuid::Uuid::new_v4().to_string();
    store.update_by_id(
        table("campaign"),
        &[("hold_back_seed", Param::from(seed.as_str()))],
        "id",
        campaign,
    )?;
    Ok(seed)
}

/// Record 48 R1: hold items of a campaign back from every batch, to be
/// read alone.
pub fn hold_back(store: &mut Store, campaign: i64, items: &[i64]) -> Result<(), Error> {
    for chunk in items.chunks(500) {
        let sql = format!(
            "UPDATE {} SET held_back = 1 WHERE campaign_id = {} AND id IN ({})",
            store.qualified("campaign_item"),
            store.dialect().param(1, Type::Int),
            join_ids(chunk)
        );
        store.execute(&sql, &[Param::Int(campaign)])?;
    }
    Ok(())
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
    claim_in(registry, campaign, principal, role, Order::Position, now)
}

/// The order a rater's claims take the items in (record 48 R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// By position, as the items were listed.
    #[default]
    Position,
    /// By value: first the items where the two systems or the rules
    /// disagree, then the least confident, then by position ([`worth`]).
    Value,
}

impl Order {
    pub fn parse(s: &str) -> Result<Order, Error> {
        match s {
            "position" => Ok(Order::Position),
            "value" => Ok(Order::Value),
            other => Err(invalid(format!("order: position or value, not {other}"))),
        }
    }
}

/// What makes an item worth a person's look first (record 48 R1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Worth {
    /// The two systems disagree (System 1's `agree` leaves an axis out), or
    /// the rules that voted on an axis said different values.
    pub disagree: bool,
    /// System 1's first candidate's probability where it asked about the
    /// stack; else the lowest confidence the rules resolved an axis with;
    /// one where nothing is known.
    pub confidence: f64,
    /// Whether System 1 asked about the stack.
    pub asked: bool,
}

impl Worth {
    pub fn as_json(&self) -> Value {
        json!({"disagree": self.disagree, "confidence": self.confidence, "asked": self.asked})
    }
}

/// The axes a question is about, for [`worth`]: its axis or axes, or none
/// (every axis) for the other kinds.
pub fn axes_of(question: &Question) -> Vec<String> {
    match question {
        Question::Axis { axis, .. } => vec![axis.clone()],
        Question::Axes { axes, .. } => axes.clone(),
        _ => Vec::new(),
    }
}

/// How much each stack is worth a look (record 48 R1): from the open
/// `classify.asked` item where System 1 asked (its confidence, and whether
/// both systems agree on every axis asked), else from the rules (the lowest
/// confidence an axis resolved with, and whether the rules that voted on an
/// axis said different values; a clause that only restates another axis is
/// no witness). `axes` narrows it to the axes asked; empty is every axis.
pub fn worth(
    store: &mut Store,
    stacks: &[i64],
    axes: &[String],
) -> Result<BTreeMap<i64, Worth>, Error> {
    let wanted = |a: &str| axes.is_empty() || axes.iter().any(|x| x == a);
    let mut out: BTreeMap<i64, Worth> = stacks
        .iter()
        .map(|s| {
            (
                *s,
                Worth {
                    disagree: false,
                    confidence: 1.0,
                    asked: false,
                },
            )
        })
        .collect();
    let list: Vec<i64> = out.keys().copied().collect();
    let d = store.dialect();
    let t = table("review_item");
    // System 1's questions, one open per stack
    for chunk in list.chunks(500) {
        let keys = chunk
            .iter()
            .map(|s| format!("'{}'", crate::asked::key(*s)))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT group_key, {} FROM {} WHERE kind = '{}' AND status = 'open' AND group_key IN ({keys})",
            d.text_of(t.column("evidence").expect("evidence")),
            store.qualified("review_item"),
            crate::asked::KIND,
        );
        for r in store.query(&sql, &[])? {
            let Some(stack) = r
                .opt_text(0)?
                .and_then(|k| k.rsplit(':').next())
                .and_then(|n| n.parse::<i64>().ok())
            else {
                continue;
            };
            let ev: Value = r
                .opt_text(1)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            let asked: Vec<String> = ev["axes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|a| wanted(a))
                .map(str::to_string)
                .collect();
            let agree: BTreeSet<&str> = ev["agree"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if let Some(w) = out.get_mut(&stack) {
                w.asked = true;
                w.confidence = ev["confidence"].as_f64().unwrap_or(1.0);
                w.disagree = asked.iter().any(|a| !agree.contains(a.as_str()));
            }
        }
    }
    // the rules: the confidence each axis resolved with
    for chunk in list.chunks(500) {
        let sql = format!(
            "SELECT stack_id, axis, confidence FROM {} WHERE stack_id IN ({})",
            store.qualified("classification_axis"),
            join_ids(chunk)
        );
        for r in store.query(&sql, &[])? {
            let (stack, axis) = (r.int(0)?, r.text(1)?.to_string());
            if !wanted(&axis) {
                continue;
            }
            let c = r.double(2)?;
            if let Some(w) = out.get_mut(&stack)
                && !w.asked
                && c < w.confidence
            {
                w.confidence = c;
            }
        }
    }
    // and whether the rules that voted on an axis disagreed
    let mut voters: BTreeMap<i64, (String, bool)> = BTreeMap::new();
    let sql = format!(
        "SELECT id, axis, restates FROM {}",
        store.qualified("classification_voter")
    );
    for r in store.query(&sql, &[])? {
        voters.insert(r.int(0)?, (r.text(1)?.to_string(), r.int(2)? != 0));
    }
    for chunk in list.chunks(500) {
        let sql = format!(
            "SELECT stack_id, votes FROM {} WHERE stack_id IN ({})",
            store.qualified("classification_vote"),
            join_ids(chunk)
        );
        let mut said: BTreeMap<(i64, String), BTreeSet<String>> = BTreeMap::new();
        for r in store.query(&sql, &[])? {
            let stack = r.int(0)?;
            let pairs: Vec<(i64, String)> = serde_json::from_str(r.text(1)?).unwrap_or_default();
            for (voter, value) in pairs {
                if let Some((axis, restates)) = voters.get(&voter)
                    && !restates
                    && wanted(axis)
                {
                    said.entry((stack, axis.clone())).or_default().insert(value);
                }
            }
        }
        for ((stack, _), values) in said {
            if values.len() > 1
                && let Some(w) = out.get_mut(&stack)
                && !w.asked
            {
                w.disagree = true;
            }
        }
    }
    Ok(out)
}

/// A place from 0 to a million drawn from a seed and an item: where a
/// sealed item falls among the confidences, the same for one seed.
fn drawn(seed: &str, item: i64) -> i64 {
    let d = ring::digest::digest(&ring::digest::SHA256, format!("{seed}:{item}").as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d.as_ref()[..8]);
    (u64::from_be_bytes(b) % 1_000_000) as i64
}

/// The key [`Order::Value`] sorts by: disagreement first, then the least
/// confident, then the position.
fn by_value(w: Option<&Worth>, position: i64) -> (u8, i64, i64) {
    match w {
        Some(w) => (
            u8::from(!w.disagree),
            (w.confidence.clamp(0.0, 1.0) * 1_000_000.0).round() as i64,
            position,
        ),
        None => (1, 1_000_000, position),
    }
}

/// [`claim`], taking a rater's next item in the order given. An
/// adjudicator's order is always the position.
pub fn claim_in(
    registry: &mut Registry,
    campaign: i64,
    principal: &str,
    role: Role,
    order: Order,
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
        let leased_ms = crate::time::millis_at(now);
        let a = store.qualified("campaign_assignment");
        let i = store.qualified("campaign_item");
        match role {
            Role::Rater => {
                let sql = format!(
                    "SELECT i.id, i.position, i.stack_id FROM {i} i WHERE i.campaign_id = {} AND i.state = 'open' AND i.round = 1 \
                     AND (SELECT COUNT(*) FROM {a} a WHERE a.item_id = i.id AND a.role = 'rater' \
                          AND a.state IN ('leased', 'submitted')) < {} \
                     AND NOT EXISTS (SELECT 1 FROM {a} a WHERE a.item_id = i.id AND a.principal = {}) \
                     ORDER BY i.position{}",
                    d.param(1, Type::Int),
                    d.param(2, Type::Int),
                    d.param(3, Type::Text),
                    if order == Order::Position {
                        " LIMIT 1"
                    } else {
                        ""
                    }
                );
                let open: Vec<(i64, i64, Option<i64>)> = store
                    .query(
                        &sql,
                        &[
                            Param::Int(campaign),
                            Param::Int(c.raters_per_item),
                            Param::from(principal),
                        ],
                    )?
                    .iter()
                    .map(|r| Ok((r.int(0)?, r.int(1)?, r.opt_int(2)?)))
                    .collect::<Result<_, StoreError>>()?;
                let item = match order {
                    Order::Position => open.first().map(|(id, ..)| *id),
                    Order::Value => {
                        let stacks: Vec<i64> = open.iter().filter_map(|(.., s)| *s).collect();
                        let worth = worth(store, &stacks, &axes_of(&c.question()?))?;
                        // record 48 R2: a stack of a sample sealed now is
                        // never ranked by what the systems said of it; it
                        // takes a place drawn from the campaign's seed
                        let (sealed, _) = crate::labels::sealed_now(store, &stacks, &[])
                            .map_err(|e| StoreError::Message(e.to_string()))?;
                        let seed = hold_back_seed(store, c.id)?;
                        open.iter()
                            .min_by_key(|(id, position, stack)| match stack {
                                Some(s) if sealed.contains(s) => (1, drawn(&seed, *id), *position),
                                _ => by_value(stack.and_then(|s| worth.get(&s)), *position),
                            })
                            .map(|(id, ..)| *id)
                    }
                };
                let Some(item) = item else {
                    return Ok(None);
                };
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
                                "leased_ms",
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
                            leased_ms.map_or(Param::Null, |m| Param::Int(m as i64)),
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
                        (
                            "leased_ms",
                            leased_ms.map_or(Param::Null, |m| Param::Int(m as i64)),
                        ),
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

/// Lengthen a lease its holder still holds (record 45): the lease runs the
/// campaign's `lease_seconds` from now, as a claim's does. A lease that ran
/// out is not renewed, since its item may be another rater's by now; the
/// rater claims again. A heartbeat, so it writes no audit row: the claim
/// and the answer are the acts.
pub fn renew(
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
    store.begin()?;
    let done = (|| -> Result<(), Error> {
        lock(store, a.campaign_id)?;
        // read again under the lock: a close, an answer or an expiry may
        // have come first
        let c = get(store, a.campaign_id)?
            .ok_or_else(|| Error::NotFound(format!("no campaign {}", a.campaign_id)))?;
        if c.status != "open" {
            return Err(refused(format!("campaign {} is {}", c.name, c.status)));
        }
        let a = assignment(store, a.id)?
            .ok_or_else(|| Error::NotFound(format!("no assignment {assignment_id}")))?;
        if a.state != "leased" {
            return Err(refused(format!(
                "assignment {assignment_id} is {}, not leased; claim again",
                a.state
            )));
        }
        if a.lease_until
            .as_deref()
            .is_some_and(|u| secs_of(u).zip(secs_of(now)).is_some_and(|(u, n)| u < n))
        {
            return Err(refused(format!(
                "the lease of assignment {assignment_id} ran out; claim again"
            )));
        }
        store.update_by_id(
            table("campaign_assignment"),
            &[(
                "lease_until",
                Param::from(plus_seconds(now, c.lease_seconds).as_str()),
            )],
            "id",
            a.id,
        )?;
        Ok(())
    })();
    if let Err(e) = done {
        store.rollback().ok();
        return Err(e);
    }
    store.commit()?;
    assignment(registry.store(), assignment_id)?
        .ok_or_else(|| Error::NotFound(format!("no assignment {assignment_id}")))
}

/// Give a leased item back unanswered. A rater's item returns to the pool
/// for others (never to the same rater); an adjudicator's is offered again.
/// Only its holder gives it back; [`release_as`] lets an operator give back
/// another's.
pub fn release(
    registry: &mut Registry,
    assignment_id: i64,
    principal: &str,
    now: &str,
) -> Result<Assignment, Error> {
    release_as(registry, assignment_id, principal, false, now)
}

/// Whether a lease ran out by `now`: an expired lease holds nothing, even
/// before the next claim writes it `expired`.
fn ran_out(a: &Assignment, now: &str) -> bool {
    a.state == "leased"
        && a.lease_until
            .as_deref()
            .is_some_and(|u| secs_of(u).zip(secs_of(now)).is_some_and(|(u, n)| u < n))
}

/// [`release`], where `oversees` says the principal may give back another
/// person's claim: a holder of review:work, which the caller checks. The
/// campaign's owner may as well, which this checks. A lease that already
/// ran out is no one's claim: it is written `expired`, as the next claim
/// would write it, whoever asks. Every release is audited with the holder
/// it took the item from.
pub fn release_as(
    registry: &mut Registry,
    assignment_id: i64,
    principal: &str,
    oversees: bool,
    now: &str,
) -> Result<Assignment, Error> {
    let store = registry.store();
    let a = assignment(store, assignment_id)?
        .ok_or_else(|| Error::NotFound(format!("no assignment {assignment_id}")))?;
    let expired = ran_out(&a, now);
    let own = a.principal.as_deref() == Some(principal);
    if !own && !expired && !oversees {
        let owner = get(store, a.campaign_id)?.is_some_and(|c| c.owner == principal);
        if !owner {
            return Err(Error::Forbidden(format!(
                "assignment {assignment_id} is not {principal}'s; the campaign's owner or a holder of review:work gives back another's claim"
            )));
        }
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
        if expired {
            expire_in(store, a.campaign_id, now)?;
            return Ok(());
        }
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
    let mut details = json!({"role": a.role, "round": a.round});
    if !own {
        details["holder"] = json!(a.principal);
    }
    if expired {
        details["expired"] = json!(true);
    }
    audit::record(
        registry,
        &Entry {
            principal,
            action: Action::CampaignRelease,
            scope: json!({"campaign": a.campaign_id, "item": a.item_id, "assignment": a.id}),
            policy: None,
            job_id: None,
            details: Some(details),
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
    /// The registered model, when a model answers (record 42 S2).
    pub model: Option<i64>,
    pub value: Option<&'a str>,
    pub form: Option<&'a Value>,
    pub derivative_id: Option<i64>,
    pub why: Option<&'a str>,
    /// Record 48: the rater answered and wants a second look at the stack.
    /// Kept and exported with the answer; it never bears on whether the
    /// answer fits.
    pub unsure: bool,
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
    /// The registered model, when a model answered.
    pub model_id: Option<i64>,
    /// Record 48 R1: seconds from the claim to the answer (none for an
    /// answer given to a batch), the answer the engine suggested, whether
    /// this one differs from it, and how it came (`claim` or `batch`).
    pub seconds: Option<f64>,
    pub suggested: Option<String>,
    pub changed: Option<bool>,
    pub via: Option<String>,
    /// Record 48: the rater marked the stack unsure.
    pub unsure: bool,
    /// Record 48: the axes derived from the answer through the pack.
    pub derived: Option<Value>,
}

impl Answer {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "campaign_id": self.campaign_id, "item_id": self.item_id,
            "assignment_id": self.assignment_id, "principal": self.principal, "role": self.role,
            "round": self.round, "author_kind": self.author_kind, "value": self.value,
            "form": self.form, "derivative_id": self.derivative_id, "why": self.why,
            "actor_detail": self.actor_detail, "answered_at": self.answered_at,
            "model_id": self.model_id, "seconds": self.seconds, "suggested": self.suggested,
            "changed": self.changed, "via": self.via, "unsure": self.unsure,
            "derived": self.derived,
        })
    }
}

const ANSWER_COLUMNS: [&str; 21] = [
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
    "model_id",
    "seconds",
    "suggested",
    "changed",
    "via",
    "unsure",
    "derived",
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
        model_id: r.opt_int(14)?,
        seconds: r.opt_double(15)?,
        suggested: r.opt_text(16)?.map(str::to_string),
        changed: r.opt_int(17)?.map(|c| c != 0),
        via: r.opt_text(18)?.map(str::to_string),
        unsure: r.opt_int(19)?.is_some_and(|u| u != 0),
        derived: {
            let d = json_at(r, 20)?;
            (!d.is_null()).then_some(d)
        },
    })
}

/// Record 48: keep what the engine derived from a kept answer that has
/// none yet (an answer given to a batch, or before its question derived
/// anything). What an answer derived is never changed once kept; the
/// answer's own value is never touched. True where it was written.
pub fn set_derived(store: &mut Store, answer: i64, derived: &Value) -> Result<bool, Error> {
    let d = store.dialect();
    let n = store.execute(
        &format!(
            "UPDATE {} SET derived = {} WHERE id = {} AND derived IS NULL",
            store.qualified("campaign_answer"),
            d.param(1, Type::Json),
            d.param(2, Type::Int)
        ),
        &[Param::from(derived.to_string()), Param::Int(answer)],
    )?;
    Ok(n > 0)
}

/// What [`requestion`] did.
#[derive(Debug, Clone)]
pub struct Requestioned {
    pub campaign: i64,
    pub asked_before: Vec<String>,
    pub asked: Vec<String>,
    pub derive: Vec<String>,
    /// The answers kept, which are read on the axes asked now.
    pub answers: usize,
}

/// Record 48, after the first real read: move an open axes campaign to a
/// question that asks fewer axes and derives the rest, keeping its items
/// and every answer given. Every axis asked now was asked before, so each
/// kept answer answers it; an answer's axes that are derived now stay in
/// its value as the rater gave them and are read no more, and the answer is
/// never rewritten. Refused while an item is leased, since the rater holds
/// the old form, and for any campaign that is not open or does not ask
/// axes. The caller fills what each kept answer derives
/// ([`set_derived`]).
pub fn requestion(
    registry: &mut Registry,
    which: &str,
    question: &Value,
    who: &str,
    now: &str,
) -> Result<Requestioned, Error> {
    let c = find(registry.store(), which)?;
    if c.status != "open" {
        return Err(refused(format!("campaign {} is {}", c.name, c.status)));
    }
    let Question::Axes { axes: before, .. } = c.question()? else {
        return Err(refused(format!(
            "campaign {} does not ask axes; only an axes question moves axes to derived",
            c.name
        )));
    };
    let next = Question::parse(question)?;
    let Question::Axes {
        axes,
        constraints,
        derive,
    } = &next
    else {
        return Err(invalid("the new question asks axes"));
    };
    if let Some(a) = axes.iter().find(|a| !before.contains(a)) {
        return Err(refused(format!(
            "{a} was not asked before, so the answers kept do not answer it; a requestion only moves asked axes to derived"
        )));
    }
    let store = registry.store();
    store.begin()?;
    let done = (|| -> Result<usize, Error> {
        lock(store, c.id)?;
        // a lease past its end holds nothing: it ends here, as a claim
        // would end it, and does not stand in the way
        expire_in(store, c.id, now)?;
        let held = assignments(store, c.id)?
            .into_iter()
            .filter(|a| matches!(a.state.as_str(), "leased" | "offered"))
            .count();
        if held > 0 {
            return Err(refused(format!(
                "{held} item(s) of {} are leased under the question as it stands; wait for them to be answered or released (nils campaign release <assignment>)",
                c.name
            )));
        }
        // every kept answer still reads on the axes asked now
        let kept = answers(store, c.id)?;
        for a in &kept {
            if let Some(v) = a.value.as_deref() {
                let j = stored_joint_of(axes, constraints, v)?;
                legal(constraints, &j).map_err(|e| {
                    refused(format!(
                        "answer {} does not read on the axes asked now: {e}",
                        a.id
                    ))
                })?;
            }
        }
        store.update_by_id(
            table("campaign"),
            &[("question", Param::from(next.to_json().to_string()))],
            "id",
            c.id,
        )?;
        // the items' own review items say which axes they ask
        for it in items(store, c.id)? {
            if let Some(ri) =
                crate::review::item(store, it.review_item_id).map_err(|e| invalid(e.to_string()))?
                && ri.evidence.get("axes").is_some()
            {
                let mut ev = ri.evidence.clone();
                ev["axes"] = json!(axes);
                store.update_by_id(
                    table("review_item"),
                    &[("evidence", Param::from(ev.to_string()))],
                    "id",
                    ri.id,
                )?;
            }
        }
        Ok(kept.len())
    })();
    let kept = match done {
        Ok(n) => n,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    };
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::CampaignRequestion,
            scope: json!({"campaign": c.id, "name": c.name}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "asked_before": before, "asked": axes, "derive": derive,
                "answers_kept": kept, "at": now,
            })),
        },
    )?;
    Ok(Requestioned {
        campaign: c.id,
        asked_before: before,
        asked: axes.clone(),
        derive: derive.clone(),
        answers: kept,
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

/// A derivative answer names a file registered through the derivative door
/// (record 42 S4): a derivative of the kind the question asks for, not
/// withdrawn, of the item's subject and, where both name one, its stack.
fn derivative_answers(store: &mut Store, item_id: i64, id: i64, kind: &str) -> Result<(), Error> {
    let d = crate::derivative::get(store, id)?.ok_or_else(|| {
        invalid(format!(
            "no derivative {id}; POST /api/derivatives registers the file first"
        ))
    })?;
    if d.kind != kind {
        return Err(invalid(format!(
            "derivative {id} is a {}, and the question asks for a {kind}",
            d.kind
        )));
    }
    if d.withdrawn_at.is_some() {
        return Err(invalid(format!("derivative {id} is withdrawn")));
    }
    let it = item(store, item_id)?
        .ok_or_else(|| Error::NotFound(format!("no campaign item {item_id}")))?;
    if it.subject_id.is_some() && d.subject_id != it.subject_id {
        return Err(invalid(format!(
            "derivative {id} belongs to another subject than item {}",
            it.position
        )));
    }
    if let (Some(stack), Some(of)) = (it.stack_id, d.stack_id)
        && stack != of
    {
        return Err(invalid(format!(
            "derivative {id} was made from stack {of}, and item {} asks about stack {stack}",
            it.position
        )));
    }
    Ok(())
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
    answer_with(registry, g, &Timing::default(), now)
}

/// What the reader knew about an answer beside it (record 48 R1): the
/// answer the engine suggested, as an answer to the question would say it,
/// and whether the answer was given to a whole batch rather than after a
/// claim of its own.
#[derive(Debug, Clone, Default)]
pub struct Timing<'a> {
    pub suggested: Option<&'a str>,
    pub batch: bool,
    /// Record 48: the axes the engine derived from this answer through the
    /// pack, `{axis: value | [values] | null | "cant_tell"}`, kept beside
    /// it and marked derived. The engine computes them; a caller's own are
    /// never taken.
    pub derived: Option<&'a Value>,
}

/// An answer's text as it is kept: a pick's stacks as ids, an axes answer
/// in its canonical order, any other value trimmed.
fn kept_value(question: &Question, v: &str) -> Result<String, Error> {
    Ok(match question {
        Question::Pick { .. } => join_ids(&pick_stacks(v)?),
        Question::Axes {
            axes, constraints, ..
        } => canonical_joint(constraints, &answer_joint_of(axes, constraints, v)?),
        _ => v.trim().to_string(),
    })
}

/// [`answer`], with what the reader knew beside it: the time from the
/// claim to the answer, the suggestion and whether the answer changed it
/// are kept on the answer (record 48 R1).
pub fn answer_with(
    registry: &mut Registry,
    g: &Given<'_>,
    t: &Timing<'_>,
    now: &str,
) -> Result<Answered, Error> {
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
    // A repeat of an answer already given on this assignment is answered
    // again; the lease and the state are held to it inside the transaction.
    if a.state != "leased" && a.state != "submitted" {
        return Err(refused(format!(
            "assignment {} is {}; claim the item again",
            a.id, a.state
        )));
    }
    let question = c.question()?;
    question.check(g)?;
    if let Question::Derivative {
        derivative_kind, ..
    } = &question
        && let Some(id) = g.derivative_id
    {
        derivative_answers(store, a.item_id, id, derivative_kind)?;
    }
    let adjudication = c.adjudication()?;
    if !["person", "agent", "model"].contains(&g.author_kind) {
        return Err(invalid(format!(
            "an author is a person, an agent or a model, not {}",
            g.author_kind
        )));
    }
    // Record 42 S2 (D15): a model's answer names the registered model, and
    // only a model's does; the close carries it onto the decision.
    match (g.author_kind, g.model) {
        ("model", None) => {
            return Err(invalid(
                "a model's answer names the registered model that answered (D15)",
            ));
        }
        ("model", Some(id)) => {
            if crate::model::get(store, id)?.is_none() {
                return Err(Error::NotFound(format!("no registered model {id}")));
            }
        }
        (kind, Some(_)) => {
            return Err(invalid(format!(
                "a {kind} is not a model; only a model's answer names a model"
            )));
        }
        _ => {}
    }
    let actor_detail = crate::actor::current();
    // one text for one joint answer, whatever order it came in
    let stored_value = g.value.map(|v| kept_value(&question, v)).transpose()?;
    // a suggestion that does not read as an answer is no suggestion
    let suggested = t
        .suggested
        .filter(|v| !v.trim().is_empty())
        .and_then(|v| kept_value(&question, v).ok());
    let changed = suggested
        .as_ref()
        .map(|s| stored_value.as_deref() != Some(s.as_str()));
    store.begin()?;
    let mut replayed = false;
    let done = (|| -> Result<Answered, Error> {
        lock(store, c.id)?;
        // What the checks above read may have moved while this waited for
        // the campaign: read it again under the lock.
        let status = get(store, c.id)?.map(|x| x.status).unwrap_or_default();
        if status != "open" {
            return Err(refused(format!("campaign {} is {status}", c.name)));
        }
        let a = assignment(store, a.id)?
            .ok_or_else(|| Error::NotFound(format!("no assignment {}", a.id)))?;
        // One answer per item, rater and round: the same answer on the same
        // assignment again is the one given, anything else is refused.
        if let Some(held) = answers_of_item(store, a.item_id)?
            .into_iter()
            .find(|x| x.principal == g.principal && x.round == a.round)
        {
            let same = held.assignment_id == a.id
                && held.value == stored_value
                && held.form.as_ref() == g.form
                && held.derivative_id == g.derivative_id
                && held.unsure == g.unsure;
            if !same {
                return Err(refused(format!(
                    "{} answered item {} in round {} already, as answer {}",
                    g.principal, a.item_id, a.round, held.id
                )));
            }
            let it = item(store, a.item_id)?
                .ok_or_else(|| Error::NotFound(format!("no campaign item {}", a.item_id)))?;
            replayed = true;
            return Ok(Answered {
                answer: held.id,
                item: it.id,
                state: it.state,
                adjudication: None,
            });
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
        write_answer(
            store,
            &c,
            &Written {
                question: &question,
                adjudication: &adjudication,
                assignment: &a,
                given: g,
                stored_value: stored_value.as_deref(),
                suggested: suggested.as_deref(),
                changed,
                batch: t.batch,
                actor_detail: &actor_detail,
                derived: t.derived,
            },
            now,
        )
    })();
    let answered = match done {
        Ok(x) => x,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    };
    store.commit()?;
    if replayed {
        return Ok(answered);
    }
    // the value of an axis or a pick is a word of the pack or stack ids; a
    // form's text and a free answer stay out of the audit row
    let shown = matches!(
        question,
        Question::Axis { .. } | Question::Axes { .. } | Question::Pick { .. }
    )
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
                "via": if t.batch { "batch" } else { "claim" }, "changed": changed,
                "unsure": g.unsure,
            })),
        },
    )?;
    Ok(answered)
}

/// What [`write_answer`] writes.
struct Written<'a> {
    question: &'a Question,
    adjudication: &'a Adjudication,
    assignment: &'a Assignment,
    given: &'a Given<'a>,
    stored_value: Option<&'a str>,
    suggested: Option<&'a str>,
    changed: Option<bool>,
    batch: bool,
    actor_detail: &'a Value,
    /// Record 48: what the engine derived from the answer, kept beside it.
    derived: Option<&'a Value>,
}

/// Write an answer on a leased assignment and move its item on, inside the
/// caller's transaction under the campaign's lock.
fn write_answer(
    store: &mut Store,
    c: &Campaign,
    w: &Written<'_>,
    now: &str,
) -> Result<Answered, Error> {
    let a = w.assignment;
    // to the millisecond where the lease kept its instant, else to the
    // second of its stamp
    let seconds = if w.batch {
        None
    } else {
        let from_ms = store
            .query_opt(
                &format!(
                    "SELECT leased_ms FROM {} WHERE id = {}",
                    store.qualified("campaign_assignment"),
                    store.dialect().param(1, Type::Int)
                ),
                &[Param::Int(a.id)],
            )?
            .and_then(|r| r.opt_int(0).ok().flatten())
            .map(|m| m as u64)
            .or_else(|| a.leased_at.as_deref().and_then(secs_of).map(|s| s * 1000));
        from_ms
            .zip(crate::time::millis_at(now))
            .map(|(from, to)| to.saturating_sub(from) as f64 / 1000.0)
    };
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
                    "model_id",
                    "seconds",
                    "suggested",
                    "changed",
                    "via",
                    "unsure",
                    "derived",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::Int(c.id),
                Param::Int(a.item_id),
                Param::Int(a.id),
                Param::from(w.given.principal),
                Param::from(a.role.as_str()),
                Param::Int(a.round),
                Param::from(w.given.author_kind),
                w.stored_value
                    .map(str::to_string)
                    .map_or(Param::Null, Param::from),
                w.given
                    .form
                    .map_or(Param::Null, |f| Param::from(f.to_string())),
                w.given.derivative_id.map_or(Param::Null, Param::Int),
                w.given.why.map_or(Param::Null, Param::from),
                Param::from(w.actor_detail.to_string()),
                Param::from(now),
                w.given.model.map_or(Param::Null, Param::Int),
                seconds.map_or(Param::Null, Param::Double),
                w.suggested
                    .map(str::to_string)
                    .map_or(Param::Null, Param::from),
                w.changed.map_or(Param::Null, |c| Param::Int(i64::from(c))),
                Param::from(if w.batch { "batch" } else { "claim" }),
                Param::Int(i64::from(w.given.unsure)),
                w.derived
                    .map_or(Param::Null, |d| Param::from(d.to_string())),
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
        let agreement = share_agreeing(w.question, &all, settled);
        let mut outcome = outcome_of(w.question, std::slice::from_ref(settled));
        per_axis(w.question, &all, settled, &mut outcome);
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
                raters.iter().map(|x| w.question.comparable(x)).collect();
            let agree = keys.iter().all(|k| k.is_some() && *k == keys[0]);
            let next = match (w.adjudication.when, w.adjudication.metric) {
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
                    let mut outcome = outcome_of(w.question, &owned);
                    per_axis(w.question, &all, &owned[0], &mut outcome);
                    set_item(store, it.id, "agreed", 1, Some(1.0), Some(&outcome), now)?;
                }
                "needs_adjudication" => {
                    opened = Some(adjudicate(store, c, &it, &owned, now)?);
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
}

/// Record 48 R1: answer one item of a batch a rater accepted in one move
/// ([`accept_many`] with one item); refused where the item was.
pub fn accept(
    registry: &mut Registry,
    campaign: i64,
    item_id: i64,
    g: &Given<'_>,
    suggested: Option<&str>,
    now: &str,
) -> Result<Answered, Error> {
    let mut done = accept_many(registry, campaign, &[item_id], g, suggested, now)?;
    if let Some((_, why)) = done.refused.pop() {
        return Err(refused(why));
    }
    done.accepted
        .pop()
        .ok_or_else(|| refused(format!("item {item_id} was not accepted")))
}

/// What [`accept_many`] did: the answers written, and the items it left
/// with why.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Accepted {
    pub accepted: Vec<Answered>,
    pub refused: Vec<(i64, String)>,
}

/// Record 48 R1: answer the items of a batch a rater accepted in one move,
/// in one transaction under the campaign's lock. Each item still wanting a
/// rater, never given to this one, is leased to the rater as a claim would
/// lease it and answered at once with `g`'s value, marked as given to a
/// batch; it is still its own item, closed into its own decision. An item
/// that no longer asks raters, has its raters or was the rater's already is
/// left, with why. `g.assignment` is not read.
pub fn accept_many(
    registry: &mut Registry,
    campaign: i64,
    items: &[i64],
    g: &Given<'_>,
    suggested: Option<&str>,
    now: &str,
) -> Result<Accepted, Error> {
    let c = get(registry.store(), campaign)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {campaign}")))?;
    if c.status != "open" {
        return Err(refused(format!("campaign {} is {}", c.name, c.status)));
    }
    let listed = c.raters();
    if !listed.is_empty() && !listed.iter().any(|p| p == g.principal) {
        return Err(Error::Forbidden(format!(
            "campaign {} names its raters, and {} is not one",
            c.name, g.principal
        )));
    }
    if !["person", "agent", "model"].contains(&g.author_kind) {
        return Err(invalid(format!(
            "an author is a person, an agent or a model, not {}",
            g.author_kind
        )));
    }
    if (g.author_kind == "model") != g.model.is_some() {
        return Err(invalid(
            "a model's answer names the registered model that answered, and only a model's does (D15)",
        ));
    }
    let question = c.question()?;
    question.check(g)?;
    let adjudication = c.adjudication()?;
    let stored_value = g.value.map(|v| kept_value(&question, v)).transpose()?;
    let suggested = suggested
        .filter(|v| !v.trim().is_empty())
        .and_then(|v| kept_value(&question, v).ok());
    let changed = suggested
        .as_ref()
        .map(|s| stored_value.as_deref() != Some(s.as_str()));
    let actor_detail = crate::actor::current();
    let until = plus_seconds(now, c.lease_seconds);
    let leased_ms = crate::time::millis_at(now);
    let store = registry.store();
    let d = store.dialect();
    store.begin()?;
    let done = (|| -> Result<(Accepted, Vec<i64>), Error> {
        lock(store, campaign)?;
        let status = get(store, c.id)?.map(|x| x.status).unwrap_or_default();
        if status != "open" {
            return Err(refused(format!("campaign {} is {status}", c.name)));
        }
        expire_in(store, campaign, now)?;
        let mut out = Accepted::default();
        let mut assignments = Vec::new();
        let a_t = store.qualified("campaign_assignment");
        for &item_id in items {
            let Some(it) = item(store, item_id)?.filter(|it| it.campaign_id == campaign) else {
                out.refused.push((
                    item_id,
                    format!("campaign {} has no item {item_id}", c.name),
                ));
                continue;
            };
            if it.state != "open" || it.round != 1 {
                out.refused.push((
                    item_id,
                    format!("item {item_id} is {}, and no longer asks raters", it.state),
                ));
                continue;
            }
            let sql = format!(
                "SELECT SUM(CASE WHEN role = 'rater' AND state IN ('leased', 'submitted') THEN 1 ELSE 0 END), \
                        SUM(CASE WHEN principal = {} THEN 1 ELSE 0 END) \
                 FROM {a_t} WHERE item_id = {}",
                d.param(1, Type::Text),
                d.param(2, Type::Int),
            );
            let r = store.query_opt(&sql, &[Param::from(g.principal), Param::Int(item_id)])?;
            let count = |i: usize| -> i64 {
                r.as_ref()
                    .and_then(|r| {
                        r.opt_int(i)
                            .ok()
                            .flatten()
                            .or_else(|| r.opt_double(i).ok().flatten().map(|x| x as i64))
                            .or_else(|| {
                                r.opt_text(i)
                                    .ok()
                                    .flatten()
                                    .and_then(|t| t.split('.').next()?.parse().ok())
                            })
                    })
                    .unwrap_or(0)
            };
            if count(1) > 0 {
                out.refused.push((
                    item_id,
                    format!("{} was given item {item_id} already", g.principal),
                ));
                continue;
            }
            if count(0) >= c.raters_per_item {
                out.refused
                    .push((item_id, format!("item {item_id} has the raters it wants")));
                continue;
            }
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
                            "leased_ms",
                        ],
                    )
                    .returning(&["id"]),
                    &[vec![
                        Param::Int(campaign),
                        Param::Int(item_id),
                        Param::from(g.principal),
                        Param::from("rater"),
                        Param::Int(1),
                        Param::from("leased"),
                        Param::from(now),
                        Param::from(now),
                        Param::from(until.as_str()),
                        leased_ms.map_or(Param::Null, |m| Param::Int(m as i64)),
                    ]],
                )?
                .first()
                .ok_or_else(|| StoreError::Message("the assignment was not written back".into()))?
                .int(0)?;
            let a = assignment(store, id)?
                .ok_or_else(|| Error::NotFound(format!("no assignment {id}")))?;
            let answered = write_answer(
                store,
                &c,
                &Written {
                    question: &question,
                    adjudication: &adjudication,
                    assignment: &a,
                    given: g,
                    stored_value: stored_value.as_deref(),
                    suggested: suggested.as_deref(),
                    changed,
                    batch: true,
                    actor_detail: &actor_detail,
                    derived: None,
                },
                now,
            )?;
            assignments.push(id);
            out.accepted.push(answered);
        }
        Ok((out, assignments))
    })();
    let (out, assignments) = match done {
        Ok(x) => x,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    };
    store.commit()?;
    let shown = matches!(question, Question::Axis { .. } | Question::Axes { .. })
        .then(|| stored_value.clone())
        .flatten();
    for (answered, assignment) in out.accepted.iter().zip(&assignments) {
        audit::record(
            registry,
            &Entry {
                principal: g.principal,
                action: Action::CampaignAnswer,
                scope: json!({
                    "campaign": c.id, "item": answered.item, "assignment": assignment,
                    "answer": answered.answer,
                }),
                policy: None,
                job_id: None,
                details: Some(json!({
                    "role": "rater", "round": 1, "value": shown,
                    "author_kind": g.author_kind, "item_state": answered.state,
                    "adjudication": answered.adjudication, "via": "batch", "changed": changed,
                })),
            },
        )?;
    }
    Ok(out)
}

/// The median of some numbers, none when there are none.
fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// The ninetieth percentile of some numbers, by the nearest rank; none
/// when there are none.
fn p90(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((0.9 * v.len() as f64).ceil() as usize).clamp(1, v.len());
    Some(v[rank - 1])
}

/// Record 48 R1: how fast a campaign is read, per rater and over all: the
/// answers, those read one by one and those given to a batch, the median
/// and the ninetieth percentile of the seconds from claim to answer over
/// the ones read one by one, to a tenth of a second, and of the
/// answers that had a suggestion, how many changed it and the share.
/// Counts and times only: never a value.
pub fn stats(store: &mut Store, campaign: i64) -> Result<Value, Error> {
    let all = answers(store, campaign)?;
    let mut by: BTreeMap<String, Vec<&Answer>> = BTreeMap::new();
    for a in &all {
        by.entry(a.principal.clone()).or_default().push(a);
    }
    let summary = |list: &[&Answer]| -> Value {
        let read: Vec<&&Answer> = list
            .iter()
            .filter(|a| a.via.as_deref() != Some("batch"))
            .collect();
        let batched = list.len() - read.len();
        let seconds: Vec<f64> = read.iter().filter_map(|a| a.seconds).collect();
        let suggested = list.iter().filter(|a| a.changed.is_some()).count();
        let changed = list.iter().filter(|a| a.changed == Some(true)).count();
        let tenth = |v: Option<f64>| v.map(|x| (x * 10.0).round() / 10.0);
        json!({
            "answers": list.len(),
            "read": read.len(),
            "batched": batched,
            "timed": seconds.len(),
            "median_seconds": tenth(median(seconds.clone())),
            "p90_seconds": tenth(p90(seconds)),
            "suggested": suggested,
            "changed": changed,
            "share_changed": (suggested > 0).then(|| changed as f64 / suggested as f64),
        })
    };
    let raters: Vec<Value> = by
        .iter()
        .map(|(p, list)| {
            let mut v = summary(list);
            v["principal"] = json!(p);
            v
        })
        .collect();
    let every: Vec<&Answer> = all.iter().collect();
    Ok(json!({"campaign": campaign, "all": summary(&every), "raters": raters}))
}

/// For an axes item, the share of round-one raters whose answer on each
/// axis is the settled one's: agreement per axis beside the whole.
fn per_axis(question: &Question, all: &[Answer], settled: &Answer, outcome: &mut Value) {
    let Question::Axes { axes, .. } = question else {
        return;
    };
    let raters: Vec<&Answer> = all.iter().filter(|a| a.role == "rater").collect();
    let mut m = serde_json::Map::new();
    for axis in axes {
        let want = axis_of_answer(settled.value.as_deref(), axis);
        let same = raters
            .iter()
            .filter(|a| want.is_some() && axis_of_answer(a.value.as_deref(), axis) == want)
            .count();
        m.insert(
            axis.clone(),
            if raters.is_empty() {
                Value::Null
            } else {
                json!(same as f64 / raters.len() as f64)
            },
        );
    }
    outcome["per_axis"] = Value::Object(m);
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
/// files when the question is a derivative. An axes item says, beside it,
/// how far the round-one raters agreed on each axis (record 45).
fn outcome_of(question: &Question, answers: &[Answer]) -> Value {
    let first = answers.first();
    match question {
        Question::Axes { .. } => json!({
            "value": first.and_then(|a| a.value.clone()),
            "form": null,
            "answers": answers.iter().map(|a| a.id).collect::<Vec<_>>(),
        }),
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
        // read again under the lock: another metric, or a close, may have
        // come first
        let status = get(store, c.id)?.map(|x| x.status).unwrap_or_default();
        if status != "open" {
            return Err(refused(format!("campaign {} is {status}", c.name)));
        }
        let state = item(store, it.id)?.map(|x| x.state).unwrap_or_default();
        if state != "awaiting_metric" {
            return Err(refused(format!(
                "item {item_id} is {state}; a metric is posted once its raters have all answered"
            )));
        }
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
    let n = c.raters_per_item as usize;
    let mut out = measure(&all, n, |a| question.comparable(a));
    let raters: Vec<&Answer> = all.iter().filter(|a| a.role == "rater").collect();
    // record 48: the stacks raters marked unsure are counted, to be read
    // twice and reported apart
    out["unsure"] = json!(raters.iter().filter(|a| a.unsure).count());
    // record 45: an axes campaign is measured whole and per axis; record
    // 48: with the count of can't-tell answers on each, a finding of its own
    if let Question::Axes { axes, .. } = &question {
        let said = json!(CANT_TELL).to_string();
        let mut per = serde_json::Map::new();
        for axis in axes {
            let mut m = measure(&all, n, |a| axis_of_answer(a.value.as_deref(), axis));
            m["cant_tell"] = json!(
                raters
                    .iter()
                    .filter(|a| axis_of_answer(a.value.as_deref(), axis).as_deref()
                        == Some(said.as_str()))
                    .count()
            );
            per.insert(axis.clone(), m);
        }
        out["per_axis"] = Value::Object(per);
    }
    Ok(out)
}

/// Agreement over the items every rater answered, by what `key` compares.
fn measure(all: &[Answer], n: usize, key: impl Fn(&Answer) -> Option<String>) -> Value {
    let mut by_item: BTreeMap<i64, Vec<&Answer>> = BTreeMap::new();
    for a in all.iter().filter(|a| a.role == "rater") {
        by_item.entry(a.item_id).or_default().push(a);
    }
    let complete: Vec<Vec<(String, String)>> = by_item
        .values()
        .filter(|v| v.len() == n)
        .map(|v| {
            v.iter()
                .filter_map(|a| key(a).map(|k| (a.principal.clone(), k)))
                .collect::<Vec<_>>()
        })
        .filter(|v: &Vec<(String, String)>| v.len() == n)
        .collect();
    if complete.is_empty() || n < 2 {
        return json!({"items": complete.len(), "raters_per_item": n, "exact": null, "fleiss_kappa": null, "cohen_kappa": null});
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
    json!({
        "items": complete.len(),
        "raters_per_item": n,
        "exact": agreed as f64 / complete.len() as f64,
        "fleiss_kappa": fleiss,
        "cohen_kappa": cohen,
    })
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
    /// The registered model, when a model closes it (record 42 S2): the
    /// decisions it writes are that model's answers, staged (R6).
    pub model: Option<i64>,
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
            .field("model", &self.model)
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
    /// Items asked of a group's member that someone decided on their own
    /// while the campaign was open: that decision stands and the item is
    /// skipped, with why.
    pub skipped: Vec<(i64, String)>,
    /// Whether it staged anything: every decision of a campaign that closes
    /// into `stage`, and any a model's answer settled or an agent or a
    /// model closed (record 42 R6).
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
            "skipped": self.skipped.iter().map(|(i, why)| json!({"item": i, "why": why})).collect::<Vec<_>>(),
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
///
/// Record 42 R6, extended to agents: a decision goes in force as it closes
/// only when the answers behind it and the closer are all persons. An item
/// a model's or an agent's answer settled is staged, and keeps the model as its author when that
/// model's answers were all there was; an agent's or a model's close is
/// staged whoever rated. A person commits them.
pub fn close(registry: &mut Registry, cl: &Close<'_>, now: &str) -> Result<Closed, Error> {
    let c = get(registry.store(), cl.campaign)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {}", cl.campaign)))?;
    // A close that died part way left the campaign closing; a person runs
    // it again, and it writes what the first did not.
    let again = c.status == "closing" && cl.author_kind == "person";
    if c.status != "open" && !again {
        return Err(refused(format!(
            "campaign {} is {}{}",
            c.name,
            c.status,
            if c.status == "closing" {
                "; a person runs its close again"
            } else {
                ""
            }
        )));
    }
    let question = c.question()?;
    // a lease past its end ends as expired, not as released by the close
    {
        let store = registry.store();
        store.begin()?;
        match lock(store, c.id).and_then(|()| expire_in(store, c.id, now)) {
            Ok(_) => store.commit()?,
            Err(e) => {
                store.rollback().ok();
                return Err(e.into());
            }
        }
    }
    // The close is one writer: it takes the campaign from open to closing in
    // one statement before it writes anything, so a second close, and every
    // claim, answer and metric, finds it no longer open. Each item is then
    // written in a transaction of its own, the decision with the item's link
    // to it; a close that fails part way opens the campaign again, one that
    // died leaves it closing for a person to run again, and a close run
    // again counts the items already resolved and writes the rest.
    let store = registry.store();
    let d = store.dialect();
    let took = store.execute(
        &format!(
            "UPDATE {} SET status = 'closing' WHERE id = {} AND status = {}",
            store.qualified("campaign"),
            d.param(1, Type::Int),
            d.param(2, Type::Text)
        ),
        &[Param::Int(c.id), Param::from(c.status.as_str())],
    )?;
    if took != 1 {
        let now_is = get(store, c.id)?.map(|x| x.status).unwrap_or_default();
        return Err(refused(format!("campaign {} is {now_is}", c.name)));
    }
    match close_items(registry, cl, &c, &question, now) {
        Ok(out) => Ok(out),
        Err(e) => {
            let store = registry.store();
            store
                .execute(
                    &format!(
                        "UPDATE {} SET status = 'open' WHERE id = {} AND status = 'closing'",
                        store.qualified("campaign"),
                        store.dialect().param(1, Type::Int)
                    ),
                    &[Param::Int(c.id)],
                )
                .ok();
            Err(e)
        }
    }
}

fn close_items(
    registry: &mut Registry,
    cl: &Close<'_>,
    c: &Campaign,
    question: &Question,
    now: &str,
) -> Result<Closed, Error> {
    let staged = c.closes_into == "stage";
    let mut out = Closed::default();
    let mut staged_ones: Vec<i64> = Vec::new();
    let all = items(registry.store(), c.id)?;
    // the items this close resolved, for a group asked member by member
    let mut resolved_here: BTreeSet<i64> = BTreeSet::new();
    for it in &all {
        // a close run again after one that died counts what it resolved
        if it.state == "resolved" {
            out.resolved += 1;
            continue;
        }
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
        // The answers the outcome is: the adjudicator's, or the raters' who
        // agreed.
        let settled: BTreeSet<i64> = it.outcome["answers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .collect();
        let behind: Vec<&Answer> = answers.iter().filter(|a| settled.contains(&a.id)).collect();
        let models: BTreeSet<i64> = behind.iter().filter_map(|a| a.model_id).collect();
        // The author of what the item came to: the adjudicator where there
        // was one; one model where every answer behind it was that model's
        // (record 42 R6: it keeps the model as author); else the principal
        // closing it. Each is as verified.
        let one_model = !behind.is_empty()
            && models.len() == 1
            && behind
                .iter()
                .all(|a| a.author_kind == "model" && a.model_id.is_some());
        let (who, kind, model) = match adjudicator {
            Some(a) => (a.principal.clone(), a.author_kind.clone(), a.model_id),
            None if one_model => (
                behind[0].principal.clone(),
                "model".to_string(),
                models.first().copied(),
            ),
            None => (cl.who.to_string(), cl.author_kind.to_string(), cl.model),
        };
        // R6, which the ruling on wave 42 extends to agents: a model's or an
        // agent's answer is evidence until a person commits it, so an item
        // either answered is staged; so is anything an agent or a model
        // closes, or authors, whoever rated. Only raters and a
        // closer who are all persons put an item in force as it closes.
        let persons_only = cl.author_kind == "person"
            && kind == "person"
            && behind.iter().all(|a| a.author_kind == "person");
        let stage = staged || !persons_only;
        let path = if models.is_empty() {
            path
        } else {
            format!(
                "{path}; model(s) {} answered",
                models
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        // An item asked of one member of a group decides that member
        // alone; the group closes when its last member is decided.
        let member = match it.stack_id {
            Some(stack)
                if review::item(registry.store(), it.review_item_id)
                    .map_err(|e| invalid(e.to_string()))?
                    .is_some_and(|r| r.scope == "group") =>
            {
                Some(stack)
            }
            _ => None,
        };
        // A member someone decided on their own while the campaign was
        // open keeps that decision: the close skips it and says so, as it
        // refuses a stack item whose review item was closed meanwhile.
        if let Some(stack) = member
            && matches!(c.closes_into.as_str(), "decision" | "stage")
            && review::members(registry.store(), it.review_item_id)
                .map_err(|e| invalid(e.to_string()))?
                .iter()
                .any(|m| m.stack_id == stack && m.decided_at.is_some())
        {
            out.skipped.push((
                it.id,
                format!(
                    "stack {stack} of review item {} was decided while the campaign was open; that decision stands",
                    it.review_item_id
                ),
            ));
            continue;
        }
        match c.closes_into.as_str() {
            "decision" | "stage" if matches!(question, Question::Axes { .. }) => {
                let Question::Axes {
                    axes, constraints, ..
                } = question
                else {
                    unreachable!("matched above");
                };
                let joint = it.outcome["value"]
                    .as_str()
                    .map(|v| stored_joint_of(axes, constraints, v));
                let Some(Ok(joint)) = joint else {
                    out.refused
                        .push((it.id, "the item came to no joint answer".into()));
                    out.unresolved += 1;
                    continue;
                };
                // Every decision of the item, its review items and the
                // item's link are one transaction: the item closes whole,
                // one decision per axis, or not at all.
                registry.store().begin()?;
                let written = close_axes(
                    registry,
                    c,
                    it,
                    axes,
                    &joint,
                    &Author {
                        who: &who,
                        kind: &kind,
                        model,
                    },
                    stage,
                    &path,
                    now,
                );
                match written {
                    Ok(Ok((decided, skipped))) => {
                        registry.store().commit()?;
                        if !skipped.is_empty() {
                            out.skipped.push((
                                it.id,
                                format!(
                                    "{} of stack {} decided while the campaign was open; that decision stands",
                                    skipped.join(", "),
                                    it.stack_id.unwrap_or_default()
                                ),
                            ));
                        }
                        for (id, staged) in decided {
                            if staged {
                                staged_ones.push(id);
                            }
                            out.decisions.push(id);
                        }
                        out.resolved += 1;
                    }
                    Ok(Err(why)) => {
                        registry.store().rollback().ok();
                        registry.refresh_meta().ok();
                        out.refused.push((it.id, why));
                        out.unresolved += 1;
                    }
                    Err(e) => {
                        registry.store().rollback().ok();
                        registry.refresh_meta().ok();
                        return Err(e);
                    }
                }
            }
            "decision" | "stage" => {
                let value = it.outcome["value"].as_str().map(str::to_string);
                let Some(value) = value else {
                    out.refused
                        .push((it.id, "the item came to no value".into()));
                    out.unresolved += 1;
                    continue;
                };
                // The decision, its review item and the item's link to it
                // are one transaction, so the link never lags the decision
                // (a commit reads who answered through it).
                registry.store().begin()?;
                let written = (|| -> Result<Result<review::Applied, String>, Error> {
                    let applied = match review::apply_within(
                        registry,
                        &review::Apply {
                            item: it.review_item_id,
                            member,
                            scope: "stack",
                            value: Some(&value),
                            author: review::Author {
                                who: &who,
                                kind: &kind,
                                version: None,
                                model,
                            },
                            stage,
                            why: Some(&path),
                            campaign: Some(c.id),
                        },
                    ) {
                        Ok(a) => a,
                        Err(review::Error::Store(e)) => return Err(e.into()),
                        Err(e) => return Ok(Err(e.to_string())),
                    };
                    let store = registry.store();
                    // a group with members still undecided stays open in
                    // the queue for them
                    let whole = member.is_none() || applied.closed.contains(&it.review_item_id);
                    if member.is_none() && !applied.closed.contains(&it.review_item_id) {
                        finish_review_item(
                            store,
                            it.review_item_id,
                            if applied.staged { "staged" } else { "accepted" },
                            &who,
                            &json!({"campaign": c.id, "decision": applied.decision, "value": value}),
                            Some(applied.decision),
                            now,
                        )?;
                    }
                    if whole {
                        mark_review_item(store, it.review_item_id, "resolved")?;
                    }
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
                    Ok(Ok(applied))
                })();
                match written {
                    Ok(Ok(applied)) => {
                        registry.store().commit()?;
                        if applied.staged {
                            staged_ones.push(applied.decision);
                        }
                        out.decisions.push(applied.decision);
                        out.resolved += 1;
                    }
                    Ok(Err(why)) => {
                        registry.store().rollback().ok();
                        registry.refresh_meta().ok();
                        out.refused.push((it.id, why));
                        out.unresolved += 1;
                    }
                    Err(e) => {
                        registry.store().rollback().ok();
                        registry.refresh_meta().ok();
                        return Err(e);
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
                // model's answer is evidence, never a pick, and a pick is
                // written in force, so it is never staged (R6)
                if kind != "person" {
                    out.refused.push((
                        it.id,
                        format!("a pick is a person's; {who} answered as a {kind}"),
                    ));
                    out.unresolved += 1;
                    continue;
                }
                if !persons_only {
                    out.refused.push((
                        it.id,
                        format!(
                            "a pick is a person's and is written in force; a model answered item {}, or {} closes as a {}: a person adjudicates it or closes the campaign",
                            it.position, cl.who, cl.author_kind
                        ),
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
                // a group asked member by member is closed with its last
                // member's item
                let last = member.is_none()
                    || all.iter().all(|o| {
                        o.id == it.id
                            || o.review_item_id != it.review_item_id
                            || o.state == "resolved"
                            || resolved_here.contains(&o.id)
                    });
                if last {
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
                }
                store.update_by_id(
                    table("campaign_item"),
                    &[
                        ("state", Param::from("resolved")),
                        ("resolved_at", Param::from(now)),
                    ],
                    "id",
                    it.id,
                )?;
                resolved_here.insert(it.id);
                out.resolved += 1;
            }
        }
    }
    // The close is one act. Each decision it staged moved the epoch as it
    // was audited, so the earlier ones would read as staged before the
    // registry moved on, by the close's own later writes; what the person
    // closing looked at is the campaign, so every one carries the epoch
    // the close left.
    out.staged = staged || !staged_ones.is_empty();
    if !staged_ones.is_empty() {
        let epoch = registry.meta().epoch;
        let store = registry.store();
        store.update_by_ids(
            table("decision"),
            &[("epoch_staged", Param::Int(epoch))],
            "id",
            &staged_ones,
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
                "unresolved": out.unresolved, "refused": out.refused.len(), "skipped": out.skipped.len(),
                "agreement": out.agreement,
            })),
        },
    )?;
    Ok(out)
}

/// Who an item's decisions are by.
struct Author<'a> {
    who: &'a str,
    kind: &'a str,
    model: Option<i64>,
}

/// What an axes item's close wrote: each decision with whether it is
/// staged, and the axes it skipped.
type AxesClosed = (Vec<(i64, bool)>, Vec<String>);

/// Close one axes item inside the transaction the caller holds (record 45
/// E4): one decision per axis, each through [`review::apply_within`], so
/// rank, withdrawal, the audit and the epoch are the spine's. The decision
/// on an axis answers the adopted item that asks it: a group's member, or a
/// stack's own question, which the apply closes; with none, it is written
/// on the item's own review item for that axis. An axis whose adopted
/// question on this stack someone decided while the campaign was open keeps
/// that decision and is skipped, as a group's member item is (wave 43's
/// fix). Answers each decision and whether it is staged, with the axes
/// skipped, or why the item was refused (the caller rolls back).
#[allow(clippy::too_many_arguments)]
fn close_axes(
    registry: &mut Registry,
    c: &Campaign,
    it: &Item,
    axes: &[String],
    joint: &Joint,
    by: &Author<'_>,
    stage: bool,
    path: &str,
    now: &str,
) -> Result<Result<AxesClosed, String>, Error> {
    let Some(stack) = it.stack_id else {
        return Ok(Err("an axes item names its stack".into()));
    };
    let own = review::item(registry.store(), it.review_item_id)
        .map_err(|e| invalid(e.to_string()))?
        .ok_or_else(|| Error::NotFound(format!("no review item {}", it.review_item_id)))?;
    // the adopted items this one answers, still waiting for an answer, and
    // the axes whose adopted question on this stack was decided meanwhile
    let mut adopted: Vec<review::Item> = Vec::new();
    let mut decided_meanwhile: BTreeSet<String> = BTreeSet::new();
    for id in own.evidence["asks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
    {
        let Some(x) = review::item(registry.store(), id).map_err(|e| invalid(e.to_string()))?
        else {
            continue;
        };
        let waits = matches!(x.status.as_str(), "open" | "staged");
        let member_decided = x.scope == "group"
            && review::members(registry.store(), x.id)
                .map_err(|e| invalid(e.to_string()))?
                .iter()
                .any(|m| m.stack_id == stack && m.decided_at.is_some());
        if (!waits || member_decided)
            && let Some(axis) = x.evidence["axis"].as_str()
        {
            decided_meanwhile.insert(axis.to_string());
        }
        if waits {
            adopted.push(x);
        }
    }
    let mut decided: Vec<(i64, bool)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut by_axis = serde_json::Map::new();
    // record 48: an axis the item came to can't tell on is left out, with
    // no decision; its adopted question stays open
    let unknown = cant_tell_axes(joint);
    for axis in axes {
        if unknown.contains(axis) {
            continue;
        }
        if decided_meanwhile.contains(axis) {
            skipped.push(axis.clone());
            continue;
        }
        let values = joint.get(axis).cloned().unwrap_or_default();
        let value = (!values.is_empty()).then(|| values.join(","));
        // the adopted question about this axis on this stack, if any: a
        // group whose member the stack is and still undecided, else the
        // stack's own
        let mut target: (i64, Option<i64>, Option<&str>) = (own.id, None, Some(axis.as_str()));
        for x in adopted
            .iter()
            .filter(|x| x.evidence["axis"].as_str() == Some(axis.as_str()))
        {
            if x.scope == "group" {
                let open = review::members(registry.store(), x.id)
                    .map_err(|e| invalid(e.to_string()))?
                    .iter()
                    .any(|m| m.stack_id == stack && m.decided_at.is_none());
                if open {
                    target = (x.id, Some(stack), None);
                    break;
                }
            } else if x.reference["stack_id"].as_i64() == Some(stack) {
                target = (x.id, None, None);
            }
        }
        let apply = review::Apply {
            item: target.0,
            member: target.1,
            scope: "stack",
            value: value.as_deref(),
            author: review::Author {
                who: by.who,
                kind: by.kind,
                version: None,
                model: by.model,
            },
            stage,
            why: Some(path),
            campaign: Some(c.id),
        };
        let applied = match target.2 {
            Some(a) => review::apply_axis_within(registry, &apply, a),
            None => review::apply_within(registry, &apply),
        };
        let applied = match applied {
            Ok(a) => a,
            Err(review::Error::Store(e)) => return Err(e.into()),
            Err(e) => return Ok(Err(format!("{axis}: {e}"))),
        };
        by_axis.insert(axis.clone(), json!(applied.decision));
        decided.push((applied.decision, applied.staged));
    }
    let staged = decided.iter().any(|(_, s)| *s);
    let first = decided.first().map(|(id, _)| *id);
    let store = registry.store();
    finish_review_item(
        store,
        own.id,
        if staged { "staged" } else { "accepted" },
        by.who,
        &json!({"campaign": c.id, "decisions": by_axis, "value": told(joint), "cant_tell": unknown}),
        first,
        now,
    )?;
    mark_review_item(store, own.id, "resolved")?;
    let mut outcome = it.outcome.clone();
    outcome["decisions"] = Value::Object(by_axis);
    if !unknown.is_empty() {
        outcome["cant_tell"] = json!(unknown);
    }
    if !skipped.is_empty() {
        outcome["skipped"] = json!(skipped);
    }
    store.update_by_id(
        table("campaign_item"),
        &[
            ("state", Param::from("resolved")),
            ("decision_id", first.map_or(Param::Null, Param::Int)),
            ("outcome", Param::from(outcome.to_string())),
            ("resolved_at", Param::from(now)),
        ],
        "id",
        it.id,
    )?;
    Ok(Ok((decided, skipped)))
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
            model: None,
            value,
            form: None,
            derivative_id: None,
            why: None,
            unsure: false,
        };
        q.check(&given(Some("brain"))).unwrap();
        assert!(q.check(&given(Some("chest"))).is_err());
        assert!(q.check(&given(None)).is_err());
        // record 48: can't tell is an axes answer's word, never an axis value
        let open = Question::parse(&json!({"kind": "axis", "axis": "body_part"})).unwrap();
        open.check(&given(Some("brain"))).unwrap();
        assert!(open.check(&given(Some("cant_tell"))).is_err());
        assert!(
            Question::parse(&json!({"kind": "axis", "axis": "x", "values": ["a", "cant_tell"]}))
                .is_err()
        );
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
