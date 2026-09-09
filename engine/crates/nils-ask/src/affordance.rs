// SPDX-License-Identifier: AGPL-3.0-only

//! The affordances over the store (§10): `options` on a document, `apply`
//! by move id returning a document handle and never a document, `preview`
//! at the answer's level, `draft` (parse with repair, then diagnose), and
//! `describe`. Keyed by (document hash, set, epoch, scheme digest, scope):
//! `apply` refuses a stale options list with `stale_options`.

use std::fmt;

use nils_registry::home::Registry;
use nils_registry::session::Scheme;
use nils_registry::store::{Error as StoreError, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ast::{Ask, Level};
use crate::describe::{Description, describe as describe_pure, set_sentence};
use crate::diagnose::{Diagnosis, diagnose};
use crate::document::{self, Document, DocumentError};
use crate::exec::Bounds;
use crate::handle::cell_json;
use crate::moves::{self, MoveCall, MoveError, Options};
use crate::repair::Repair;
use crate::run::{RunError, Runner};
use crate::validate::{Class, Code, Issue, Names, Scope, validate};
use crate::{Error as AskError, parse_repaired, prepare};

#[derive(Debug)]
pub enum AffordanceError {
    Ask(AskError),
    Document(DocumentError),
    Move(MoveError),
    Run(RunError),
    Store(StoreError),
    /// The options list the moves came from is not the document's current
    /// one: the document, the epoch or the scope moved.
    StaleOptions(Issue),
    Message(String),
}

impl fmt::Display for AffordanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AffordanceError::Ask(e) => write!(f, "{e}"),
            AffordanceError::Document(e) => write!(f, "{e}"),
            AffordanceError::Move(e) => write!(f, "{e}"),
            AffordanceError::Run(e) => write!(f, "{e}"),
            AffordanceError::Store(e) => write!(f, "{e}"),
            AffordanceError::StaleOptions(i) => write!(f, "{i}"),
            AffordanceError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for AffordanceError {}

macro_rules! from_error {
    ($($t:ty => $v:ident),* $(,)?) => {
        $(impl From<$t> for AffordanceError {
            fn from(e: $t) -> Self {
                AffordanceError::$v(e)
            }
        })*
    };
}

from_error!(
    AskError => Ask,
    DocumentError => Document,
    MoveError => Move,
    RunError => Run,
    StoreError => Store,
);

/// What every affordance needs beside the document.
pub struct Setting<'a> {
    pub names: &'a dyn Names,
    pub scope: &'a Scope,
    pub scheme: &'a Scheme,
    pub principal: &'a str,
    pub bounds: Bounds,
    /// The cap on values listed as fillers (`options_values`).
    pub values_cap: usize,
}

/// The options of one set of a document. No query: the document and the
/// catalog only.
pub fn options(
    epoch: i64,
    ask: &Ask,
    set: Option<&str>,
    s: &Setting<'_>,
) -> Result<Options, AffordanceError> {
    let prepared = prepare(ask.clone(), s.names, s.scope)?;
    let set = set.unwrap_or(ask.out.set.as_str());
    let digest = s.scheme.digest();
    moves::options(
        &prepared.ask,
        &prepared.validated,
        s.names,
        s.scope,
        &prepared.hash,
        epoch,
        &digest,
        set,
        s.values_cap,
    )
    .map_err(|i| AffordanceError::Ask(AskError::Invalid(vec![i])))
}

/// Store a document and return its handle.
pub fn post(
    registry: &mut Registry,
    ask: &Ask,
    s: &Setting<'_>,
) -> Result<Document, AffordanceError> {
    let prepared = prepare(ask.clone(), s.names, s.scope)?;
    let issues = compile_issues(registry, ask, s);
    if !issues.is_empty() {
        return Err(AffordanceError::Ask(AskError::Invalid(issues)));
    }
    Ok(document::put(
        registry.store(),
        ask,
        &prepared.hash,
        s.principal,
        None,
    )?)
}

/// What `apply` returns: a handle, never a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Applied {
    pub document: i64,
    pub parent: i64,
    pub hash: String,
    pub epoch: i64,
    /// The sets touched, each with its new sentence.
    pub changed: Vec<(String, String)>,
    pub options: Options,
}

/// Apply moves to a document by handle, atomically, under the options
/// list they came from.
pub fn apply(
    registry: &mut Registry,
    document_id: i64,
    epoch: i64,
    token: &str,
    set: &str,
    calls: &[MoveCall],
    s: &Setting<'_>,
) -> Result<Applied, AffordanceError> {
    let doc = document::get(registry.store(), document_id)?
        .ok_or(DocumentError::NotFound(document_id))?;
    let current = registry.meta().epoch;
    let stale = |why: String| {
        AffordanceError::StaleOptions(Issue {
            code: Code::StaleOptions,
            path: "moves".into(),
            message: why,
            next: format!(
                "POST /api/ask/options for document {document_id} and apply against its token"
            ),
        })
    };
    if epoch != current {
        return Err(stale(format!(
            "the options were taken at epoch {epoch}; the registry is at {current}"
        )));
    }
    let opts = options(current, &doc.ask, Some(set), s)?;
    if opts.token != token {
        return Err(stale(format!(
            "the token names another document, set or scope than {document_id}, {set}"
        )));
    }
    let mut ask = doc.ask.clone();
    let changed = moves::apply_moves(&mut ask, &opts, calls)?;
    // strict validation of the result; an invalid document is not stored
    let prepared = prepare(ask.clone(), s.names, s.scope)?;
    let stored = document::put(
        registry.store(),
        &ask,
        &prepared.hash,
        s.principal,
        Some(doc.id),
    )?;
    let set_after = if ask.sets.contains_key(set) {
        set.to_string()
    } else {
        ask.out.set.clone()
    };
    let fresh = options(current, &ask, Some(&set_after), s)?;
    let sentences: Vec<(String, String)> = changed
        .iter()
        .filter_map(|c| ask.sets.get(c).map(|x| (c.clone(), set_sentence(c, x))))
        .collect();
    Ok(Applied {
        document: stored.id,
        parent: doc.id,
        hash: prepared.hash,
        epoch: current,
        changed: sentences,
        options: fresh,
    })
}

/// Ten rows or the count, by level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub level: Level,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
}

/// A preview of the answer: `rows` rows at record or aggregate level, the
/// one row at count or boolean level.
pub fn preview(
    registry: &mut Registry,
    ask: &Ask,
    rows: u64,
    s: &Setting<'_>,
    reader: Option<&mut Store>,
) -> Result<Preview, AffordanceError> {
    let prepared = prepare(ask.clone(), s.names, s.scope)?;
    let mut ask = prepared.ask;
    let inlined = crate::run::inline_selections(registry, &mut ask)?;
    let validated = if inlined.is_empty() {
        prepared.validated
    } else {
        validate(&ask, s.names, s.scope).map_err(AskError::Invalid)?
    };
    let mut runner = Runner {
        names: s.names,
        scheme: s.scheme,
        bounds: s.bounds,
        reader,
    };
    let limit = match ask.out.level {
        Level::Boolean | Level::Count => None,
        _ => Some(rows.max(1)),
    };
    let (_, answer) = runner.answer(registry, &ask, &validated, None, limit)?;
    Ok(Preview {
        level: ask.out.level,
        columns: answer.columns.clone(),
        rows: answer
            .rows
            .iter()
            .map(|r| r.0.iter().map(cell_json).collect())
            .collect(),
        truncated: answer.truncated,
    })
}

/// What `draft` returns: the repairs, the diagnosis, and the document's
/// handle when it validated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drafted {
    pub repairs: Vec<Repair>,
    pub diagnosis: Diagnosis,
    pub document: Option<i64>,
    pub hash: Option<String>,
}

/// What the compiler would refuse, as issues, before a document is stored
/// or called valid (Wave 4c, kineuro/nils#93): validate and run agree.
pub fn compile_issues(registry: &mut Registry, ask: &Ask, s: &Setting<'_>) -> Vec<Issue> {
    match crate::run::explain(registry, ask.clone(), s.names, s.scope, s.scheme) {
        Err(RunError::Message(text)) => {
            let (path, message) = text
                .split_once(": ")
                .map(|(p, m)| (p.to_string(), m.to_string()))
                .unwrap_or_else(|| ("document".to_string(), text.clone()));
            vec![Issue {
                code: Code::NotCompilable,
                path,
                message,
                next: "change the document as the message says; validate compiles what run would"
                    .to_string(),
            }]
        }
        _ => Vec::new(),
    }
}

/// Parse an authored text with add only repair, diagnose it, and store it
/// when it validates.
pub fn draft(
    registry: &mut Registry,
    text: &str,
    s: &Setting<'_>,
    reader: Option<&mut Store>,
) -> Result<Drafted, AffordanceError> {
    let (ask, repairs) = parse_repaired(text)?;
    let diagnosis = diagnose(
        registry,
        ask.clone(),
        repairs.clone(),
        s.names,
        s.scope,
        s.scheme,
        s.bounds,
        false,
        reader,
    )?;
    let mut diagnosis = diagnosis;
    if diagnosis.valid {
        let issues = compile_issues(registry, &ask, s);
        if !issues.is_empty() {
            diagnosis.valid = false;
            diagnosis.issues.extend(issues);
        }
    }
    let (document, hash) = if diagnosis.valid {
        let prepared = prepare(ask.clone(), s.names, s.scope)?;
        let d = document::put(registry.store(), &ask, &prepared.hash, s.principal, None)?;
        (Some(d.id), Some(prepared.hash))
    } else {
        (None, None)
    };
    Ok(Drafted {
        repairs,
        diagnosis,
        document,
        hash,
    })
}

/// Describe a document: pure over the desugared document.
pub fn describe(ask: &Ask, s: &Setting<'_>) -> Result<Description, AffordanceError> {
    let prepared = prepare(ask.clone(), s.names, s.scope)?;
    Ok(describe_pure(
        &prepared.ask,
        &prepared.validated.order,
        s.names,
        s.scope,
        s.scheme,
    ))
}

/// Wave 4c §6.4: what a field holds, sampled under the caller's scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub level: String,
    pub field: String,
    /// `values` for a field whose values may be listed, `shapes` for one
    /// whose values may not: free text, a quasi-identifying or a sensitive
    /// field, where digits read 9, lower case a and upper case A.
    pub kind: String,
    pub items: Vec<(String, i64)>,
    /// How many distinct values the field holds in all.
    pub distinct: i64,
    pub truncated: bool,
}

fn shape_of(v: &str) -> String {
    let mut out = String::new();
    for c in v.chars().take(40) {
        out.push(match c {
            '0'..='9' => '9',
            'a'..='z' => 'a',
            'A'..='Z' => 'A',
            other => other,
        });
    }
    if v.chars().count() > 40 {
        out.push('~');
    }
    out
}

/// Sample the values of one field at one level, as a grouped count under
/// the caller's own scope and bounds: values for a technical or clinical
/// field, shapes for the rest, the distinct count either way.
pub fn values(
    registry: &mut Registry,
    level: &str,
    field: &str,
    s: &Setting<'_>,
    cap: usize,
    reader: Option<&mut Store>,
) -> Result<Sample, AffordanceError> {
    let info = s.names.field(level, field).ok_or_else(|| {
        AffordanceError::Message(format!("no field {field} at {level} in this scope"))
    })?;
    let shapes = matches!(
        info.class,
        Class::QuasiIdentifying | Class::Sensitive | Class::Identifying
    );
    let fetch = if shapes {
        cap.saturating_mul(20).max(cap)
    } else {
        cap.saturating_add(1)
    };
    let text = serde_json::json!({
        "ast_version": 1,
        "name": format!("values of {level}.{field}"),
        "sets": {
            "v": {"grain": level},
            "g": {"grain": "group", "group": {"of": "v", "by": [["field", {}, field]]}}
        },
        "keep": ["g"],
        "out": {
            "set": "g",
            "level": "record",
            "columns": [["field", {}, field], ["field", {}, "_rows"]],
            "order": [[["field", {}, "_rows"], "desc"]],
            "limit": fetch
        }
    });
    let ask: Ask = crate::parse(&text.to_string())?;
    let distinct_text = serde_json::json!({
        "ast_version": 1,
        "name": format!("distinct of {level}.{field}"),
        "sets": {
            "v": {"grain": level},
            "g": {"grain": "group", "group": {"of": "v", "by": [["field", {}, field]]}}
        },
        "keep": ["g"],
        "out": {"set": "g", "level": "count"}
    });
    let distinct_ask: Ask = crate::parse(&distinct_text.to_string())?;
    let prepared = prepare(ask, s.names, s.scope)?;
    let counted = prepare(distinct_ask, s.names, s.scope)?;
    let mut runner = Runner {
        names: s.names,
        scheme: s.scheme,
        bounds: s.bounds,
        reader,
    };
    let (_, rows) = runner.answer(
        registry,
        &prepared.ask,
        &prepared.validated,
        None,
        Some(fetch as u64),
    )?;
    let (_, count) = runner.answer(registry, &counted.ask, &counted.validated, None, None)?;
    // the answer carries the grain's own keys first; the field and the
    // count are found by name
    let at = |name: &str| rows.columns.iter().position(|c| c == name);
    let vi = at(field).unwrap_or(rows.columns.len().saturating_sub(2));
    let ni = at("_rows").unwrap_or(rows.columns.len().saturating_sub(1));
    let distinct = count
        .rows
        .first()
        .and_then(|r| r.0.first())
        .map(|c| match cell_json(c) {
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
            _ => 0,
        })
        .unwrap_or(0);
    let mut items: Vec<(String, i64)> = Vec::new();
    if shapes {
        let mut by_shape: std::collections::BTreeMap<String, i64> =
            std::collections::BTreeMap::new();
        for r in &rows.rows {
            let v =
                r.0.get(vi)
                    .map(cell_json)
                    .unwrap_or(serde_json::Value::Null);
            let n =
                r.0.get(ni)
                    .map(cell_json)
                    .and_then(|c| c.as_i64())
                    .unwrap_or(0);
            let text = match v {
                serde_json::Value::Null => "null".to_string(),
                serde_json::Value::String(t) => shape_of(&t),
                other => shape_of(&other.to_string()),
            };
            *by_shape.entry(text).or_insert(0) += n;
        }
        items = by_shape.into_iter().collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    } else {
        for r in rows.rows.iter().take(cap) {
            let v =
                r.0.get(vi)
                    .map(cell_json)
                    .unwrap_or(serde_json::Value::Null);
            let n =
                r.0.get(ni)
                    .map(cell_json)
                    .and_then(|c| c.as_i64())
                    .unwrap_or(0);
            let text = match v {
                serde_json::Value::Null => "null".to_string(),
                serde_json::Value::String(t) => t,
                other => other.to_string(),
            };
            items.push((text, n));
        }
    }
    let truncated = items.len() > cap || rows.rows.len() > cap || rows.truncated;
    items.truncate(cap);
    Ok(Sample {
        level: level.to_string(),
        field: field.to_string(),
        kind: if shapes {
            "shapes".into()
        } else {
            "values".into()
        },
        items,
        distinct,
        truncated,
    })
}
