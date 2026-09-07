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
use crate::validate::{Code, Issue, Names, Scope, validate};
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
