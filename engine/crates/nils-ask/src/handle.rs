// SPDX-License-Identifier: AGPL-3.0-only

//! Result handles (§8.4): what a run leaves behind. A handle carries its
//! grain, its columns with types, its row count and content hash, the
//! desugared ask it came from with the parameters as bound and the
//! selection versions pinned, its provenance (who, when, node, pack,
//! epoch, scheme digest), its disclosure, and the keys it named as
//! `handle_member` with its rows paged as `handle_page`. It degrades to
//! the primitive the release and the pipelines consume: a list of keys.
//!
//! Four bounds (§8.4, §14.1): a handle keeps its rows for ninety days after
//! the last read and then keeps its metadata, hash and ask; permanence for
//! a subject set is promotion; a handle may not be named unless its
//! desugared ask is stored with it; and a handle is pinned while a cohort
//! or a selection names it.

use std::collections::BTreeSet;
use std::fmt;

use nils_registry::schema::{Type, table};
use nils_registry::store::{Cell, Error as StoreError, Row, Store};
use nils_registry::time::{now_iso, secs_of};
use nils_registry::{Insert, Param};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ast::{Ask, Grain, Src};
use crate::exec::Answer;

/// Days a handle keeps its rows after the last read (Q7, Q14).
pub const KEEP_DAYS: i64 = 90;

#[derive(Debug)]
pub enum HandleError {
    Store(StoreError),
    /// A handle may not be named unless its desugared ask is stored with it.
    NamedWithoutAsk,
    NotFound(i64),
    Withdrawn(i64),
    /// The rows were dropped by retention; the metadata, hash and ask stay.
    Expired(i64),
    Truncated(i64),
    Grain {
        id: i64,
        grain: Grain,
    },
    Message(String),
}

impl fmt::Display for HandleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandleError::Store(e) => write!(f, "{e}"),
            HandleError::NamedWithoutAsk => {
                f.write_str("a handle may not be named unless its desugared ask is stored with it")
            }
            HandleError::NotFound(id) => write!(f, "no handle {id}"),
            HandleError::Withdrawn(id) => write!(f, "handle {id} was withdrawn"),
            HandleError::Expired(id) => write!(
                f,
                "handle {id} has expired: its rows were dropped by retention and only its metadata, hash and ask remain"
            ),
            HandleError::Truncated(id) => write!(
                f,
                "handle {id} is truncated: a capped answer may be paged and read, never hashed, released, pinned or promoted"
            ),
            HandleError::Grain { id, grain } => write!(f, "handle {id} is a {grain} set"),
            HandleError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for HandleError {}

impl From<StoreError> for HandleError {
    fn from(e: StoreError) -> Self {
        HandleError::Store(e)
    }
}

/// One column of a handle, with the type its cells showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
}

/// A handle as read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handle {
    pub id: i64,
    pub name: Option<String>,
    pub grain: Grain,
    pub columns: Vec<Column>,
    pub row_count: i64,
    pub content_hash: Option<String>,
    pub ast_version: i64,
    /// The desugared ask, with its parameters bound and its selections
    /// pinned; absent only on an unnamed, transient handle.
    pub ask: Option<Ask>,
    pub params: Value,
    pub selection_versions: Value,
    pub principal: String,
    pub created_at: String,
    pub node: String,
    pub pack_version: Option<String>,
    pub epoch: i64,
    pub scheme_digest: Option<String>,
    pub disclosure: String,
    pub suppression: Value,
    pub truncated: bool,
    pub values_unresolved: Value,
    pub last_read_at: Option<String>,
    pub rows_dropped_at: Option<String>,
    pub withdrawn_at: Option<String>,
    pub withdrawn_by: Option<String>,
    pub withdrawn_why: Option<String>,
}

impl Handle {
    /// Whether the rows are still kept.
    pub fn has_rows(&self) -> bool {
        self.rows_dropped_at.is_none()
    }

    /// The hash of the ask the handle came from, the one promotion writes.
    pub fn ask_hash(&self) -> Option<String> {
        self.ask.as_ref().map(crate::hash::content_hash)
    }
}

/// Where a handle came from.
#[derive(Debug, Clone)]
pub struct Provenance<'a> {
    pub principal: &'a str,
    pub node: &'a str,
    pub pack_version: Option<&'a str>,
    pub epoch: i64,
    pub scheme_digest: Option<&'a str>,
    pub disclosure: &'a str,
    pub suppression: Value,
}

/// What to save with an answer.
#[derive(Debug, Clone)]
pub struct Spec<'a> {
    pub name: Option<&'a str>,
    pub grain: Grain,
    /// The desugared ask; a named handle refuses to save without it.
    pub ask: Option<&'a Ask>,
    pub params: Value,
    pub selection_versions: Value,
    pub values_unresolved: Value,
    pub provenance: Provenance<'a>,
    pub page_rows: usize,
}

const COLUMNS: [&str; 25] = [
    "id",
    "name",
    "grain",
    "columns",
    "row_count",
    "content_hash",
    "ast_version",
    "ask",
    "params",
    "selection_versions",
    "principal",
    "created_at",
    "node",
    "pack_version",
    "epoch",
    "scheme_digest",
    "disclosure",
    "suppression",
    "truncated",
    "values_unresolved",
    "last_read_at",
    "rows_dropped_at",
    "withdrawn_at",
    "withdrawn_by",
    "withdrawn_why",
];

fn json_of(r: &Row, i: usize) -> Result<Value, HandleError> {
    Ok(match r.opt_text(i)? {
        Some(t) => serde_json::from_str(t)
            .map_err(|e| HandleError::Message(format!("a handle's JSON column is broken: {e}")))?,
        None => Value::Null,
    })
}

fn handle_of(r: &Row) -> Result<Handle, HandleError> {
    let grain_text = r.text(2)?.to_string();
    let grain: Grain = serde_json::from_value(Value::String(grain_text.clone()))
        .map_err(|_| HandleError::Message(format!("a handle names grain {grain_text}")))?;
    let columns: Vec<Column> = serde_json::from_value(json_of(r, 3)?)
        .map_err(|e| HandleError::Message(format!("a handle's columns are broken: {e}")))?;
    let ask =
        match json_of(r, 7)? {
            Value::Null => None,
            v => Some(serde_json::from_value(v).map_err(|e| {
                HandleError::Message(format!("a handle's stored ask is broken: {e}"))
            })?),
        };
    let opt = |i: usize| -> Result<Option<String>, HandleError> {
        Ok(r.opt_text(i)?.map(str::to_string))
    };
    Ok(Handle {
        id: r.int(0)?,
        name: opt(1)?,
        grain,
        columns,
        row_count: r.int(4)?,
        content_hash: opt(5)?,
        ast_version: r.int(6)?,
        ask,
        params: json_of(r, 8)?,
        selection_versions: json_of(r, 9)?,
        principal: r.text(10)?.to_string(),
        created_at: r.text(11)?.to_string(),
        node: r.text(12)?.to_string(),
        pack_version: opt(13)?,
        epoch: r.int(14)?,
        scheme_digest: opt(15)?,
        disclosure: r.text(16)?.to_string(),
        suppression: json_of(r, 17)?,
        truncated: r.int(18)? != 0,
        values_unresolved: json_of(r, 19)?,
        last_read_at: opt(20)?,
        rows_dropped_at: opt(21)?,
        withdrawn_at: opt(22)?,
        withdrawn_by: opt(23)?,
        withdrawn_why: opt(24)?,
    })
}

/// A cell as JSON, the same on both backends: a double at nine decimals,
/// bytes as hex.
pub fn cell_json(c: &Cell) -> Value {
    match c {
        Cell::Null => Value::Null,
        Cell::Text(t) => Value::String(t.clone()),
        Cell::Int(i) => json!(i),
        Cell::Double(d) => {
            let rounded: f64 = crate::exec::render(c).parse().unwrap_or(*d);
            json!(rounded)
        }
        Cell::Bool(b) => Value::Bool(*b),
        Cell::Bytes(b) => Value::String(hex::encode(b)),
    }
}

fn cell_type(c: &Cell) -> Option<&'static str> {
    match c {
        Cell::Null => None,
        Cell::Text(_) => Some("text"),
        Cell::Int(_) => Some("integer"),
        Cell::Double(_) => Some("number"),
        Cell::Bool(_) => Some("boolean"),
        Cell::Bytes(_) => Some("bytes"),
    }
}

/// The columns of an answer with the type each showed.
pub fn columns_of(answer: &Answer) -> Vec<Column> {
    answer
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| Column {
            name: name.clone(),
            type_: answer
                .rows
                .iter()
                .find_map(|r| r.0.get(i).and_then(cell_type))
                .unwrap_or("null")
                .to_string(),
        })
        .collect()
}

/// Save an answer as a handle: the row, the keys as members when the
/// answer is at record level, the rows as pages. The caller owns the
/// transaction when it holds one.
pub fn save(store: &mut Store, spec: &Spec<'_>, answer: &Answer) -> Result<Handle, HandleError> {
    if spec.name.is_some() && spec.ask.is_none() {
        return Err(HandleError::NamedWithoutAsk);
    }
    let now = now_iso();
    let columns = columns_of(answer);
    let ask_json = match spec.ask {
        Some(a) => serde_json::to_value(a)
            .map_err(|e| HandleError::Message(format!("the ask does not serialise: {e}")))?,
        None => Value::Null,
    };
    let ast_version = spec.ask.map_or(1, |a| i64::from(a.ast_version));
    let p = &spec.provenance;
    let rows = store.insert(
        &Insert::new(
            table("handle"),
            &[
                "name",
                "grain",
                "columns",
                "row_count",
                "content_hash",
                "ast_version",
                "ask",
                "params",
                "selection_versions",
                "principal",
                "created_at",
                "node",
                "pack_version",
                "epoch",
                "scheme_digest",
                "disclosure",
                "suppression",
                "truncated",
                "values_unresolved",
            ],
        )
        .returning(&["id"]),
        &[vec![
            spec.name.map_or(Param::Null, Param::from),
            Param::from(spec.grain.name()),
            Param::from(serde_json::to_string(&columns).unwrap_or_default()),
            Param::Int(answer.rows.len() as i64),
            answer
                .content_hash
                .as_deref()
                .map_or(Param::Null, Param::from),
            Param::Int(ast_version),
            Param::from(ask_json.to_string()),
            Param::from(spec.params.to_string()),
            Param::from(spec.selection_versions.to_string()),
            Param::from(p.principal),
            Param::from(now.as_str()),
            Param::from(p.node),
            p.pack_version.map_or(Param::Null, Param::from),
            Param::Int(p.epoch),
            p.scheme_digest.map_or(Param::Null, Param::from),
            Param::from(p.disclosure),
            Param::from(p.suppression.to_string()),
            Param::Int(i64::from(answer.truncated)),
            Param::from(spec.values_unresolved.to_string()),
        ]],
    )?;
    let id = rows
        .first()
        .ok_or_else(|| HandleError::Message("the handle row was not written back".into()))?
        .int(0)?;
    // the keys, when the answer names them
    if answer.columns.first().map(String::as_str) == Some("_key") {
        let members: Vec<Vec<Param>> = answer
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let key = r.opt_int(0).ok().flatten()?;
                let subject = r.opt_int(1).ok().flatten();
                Some(vec![
                    Param::Int(id),
                    Param::Int(i as i64),
                    Param::Int(key),
                    subject.map_or(Param::Null, Param::Int),
                ])
            })
            .collect();
        for chunk in members.chunks(500) {
            store.insert(
                &Insert::new(
                    table("handle_member"),
                    &["handle_id", "position", "key", "subject_id"],
                ),
                chunk,
            )?;
        }
    }
    // the pages
    let page_rows = spec.page_rows.max(1);
    let pages: Vec<Vec<Param>> = answer
        .rows
        .chunks(page_rows)
        .enumerate()
        .map(|(n, rows)| {
            let page: Vec<Value> = rows
                .iter()
                .map(|r| Value::Array(r.0.iter().map(cell_json).collect()))
                .collect();
            vec![
                Param::Int(id),
                Param::Int(n as i64),
                Param::from(Value::Array(page).to_string()),
            ]
        })
        .collect();
    for chunk in pages.chunks(200) {
        store.insert(
            &Insert::new(table("handle_page"), &["handle_id", "page", "rows"]),
            chunk,
        )?;
    }
    get(store, id)?.ok_or(HandleError::NotFound(id))
}

/// One handle by id.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Handle>, HandleError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        COLUMNS.join(", "),
        store.qualified("handle"),
        d.param(1, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| handle_of(&r))
        .transpose()
}

/// The handles named this, newest first.
pub fn by_name(store: &mut Store, name: &str) -> Result<Vec<Handle>, HandleError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT {} FROM {} WHERE name = {} ORDER BY id DESC",
        COLUMNS.join(", "),
        store.qualified("handle"),
        d.param(1, Type::Text)
    );
    store
        .query(&sql, &[Param::from(name)])?
        .iter()
        .map(handle_of)
        .collect()
}

/// Every handle, newest first, withdrawn ones included when asked.
pub fn list(store: &mut Store, withdrawn: bool) -> Result<Vec<Handle>, HandleError> {
    let filter = if withdrawn {
        ""
    } else {
        " WHERE withdrawn_at IS NULL"
    };
    let sql = format!(
        "SELECT {} FROM {}{filter} ORDER BY id DESC",
        COLUMNS.join(", "),
        store.qualified("handle")
    );
    store.query(&sql, &[])?.iter().map(handle_of).collect()
}

/// The keys a handle named, in position order, each with its subject.
pub fn keys(store: &mut Store, id: i64) -> Result<Vec<(i64, Option<i64>)>, HandleError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT key, subject_id FROM {} WHERE handle_id = {} ORDER BY position",
        store.qualified("handle_member"),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(id)])?
        .iter()
        .map(|r| Ok((r.int(0)?, r.opt_int(1)?)))
        .collect()
}

/// How many pages a handle holds.
pub fn page_count(store: &mut Store, id: i64) -> Result<i64, HandleError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE handle_id = {}",
        store.qualified("handle_page"),
        d.param(1, Type::Int)
    );
    Ok(store.query(&sql, &[Param::Int(id)])?[0].int(0)?)
}

/// One page of a handle's rows, as stored; a read moves `last_read_at`.
/// `None` past the last page; an expired handle's pages are gone.
pub fn page(store: &mut Store, id: i64, page: i64) -> Result<Option<Vec<Vec<Value>>>, HandleError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT rows FROM {} WHERE handle_id = {} AND page = {}",
        store.qualified("handle_page"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(id), Param::Int(page)])? else {
        return Ok(None);
    };
    let rows: Vec<Vec<Value>> = serde_json::from_str(r.text(0)?)
        .map_err(|e| HandleError::Message(format!("a page is broken: {e}")))?;
    touch(store, id)?;
    Ok(Some(rows))
}

/// Record a read.
pub fn touch(store: &mut Store, id: i64) -> Result<(), HandleError> {
    let now = now_iso();
    store.update_by_id(
        table("handle"),
        &[("last_read_at", Param::from(now.as_str()))],
        "id",
        id,
    )?;
    Ok(())
}

/// Withdraw a handle with a reason; its rows go, its record stays.
pub fn withdraw(store: &mut Store, id: i64, by: &str, why: &str) -> Result<(), HandleError> {
    let h = get(store, id)?.ok_or(HandleError::NotFound(id))?;
    if h.withdrawn_at.is_some() {
        return Err(HandleError::Withdrawn(id));
    }
    let now = now_iso();
    store.update_by_id(
        table("handle"),
        &[
            ("withdrawn_at", Param::from(now.as_str())),
            ("withdrawn_by", Param::from(by)),
            ("withdrawn_why", Param::from(why)),
        ],
        "id",
        id,
    )?;
    drop_rows(store, id, &now)
}

fn drop_rows(store: &mut Store, id: i64, now: &str) -> Result<(), HandleError> {
    let d = store.dialect();
    for t in ["handle_member", "handle_page"] {
        let sql = format!(
            "DELETE FROM {} WHERE handle_id = {}",
            store.qualified(t),
            d.param(1, Type::Int)
        );
        store.execute(&sql, &[Param::Int(id)])?;
    }
    store.update_by_id(
        table("handle"),
        &[("rows_dropped_at", Param::from(now))],
        "id",
        id,
    )?;
    Ok(())
}

/// Who pins a handle: a cohort promoted from it, a selection whose ask
/// reads it. (A release or a job naming a handle is a later wave's column.)
pub fn pinned_by(store: &mut Store, id: i64) -> Result<Vec<String>, HandleError> {
    let d = store.dialect();
    let mut out = Vec::new();
    let sql = format!(
        "SELECT DISTINCT c.name FROM {} cm JOIN {} c ON c.id = cm.cohort_id WHERE cm.handle_id = {} ORDER BY c.name",
        store.qualified("cohort_member"),
        store.qualified("cohort"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)])? {
        out.push(format!("cohort {}", r.text(0)?));
    }
    let sql = format!(
        "SELECT s.name, sv.version, sv.ask FROM {} sv JOIN {} s ON s.id = sv.selection_id ORDER BY s.name, sv.version",
        store.qualified("selection_version"),
        store.qualified("selection")
    );
    let wanted = id.to_string();
    for r in store.query(&sql, &[])? {
        let Ok(ask) = serde_json::from_str::<Ask>(r.text(2)?) else {
            continue;
        };
        let reads = ask
            .sets
            .values()
            .any(|s| matches!(&s.from, Some(Src::Handle { id, .. }) if *id == wanted));
        if reads {
            out.push(format!("selection {}@{}", r.text(0)?, r.int(1)?));
        }
    }
    Ok(out)
}

/// What a prune did.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pruned {
    pub dropped: Vec<i64>,
    pub pinned: Vec<(i64, Vec<String>)>,
}

/// Drop the rows of every handle unread for `keep_days`, unless something
/// names it (§14.1): `nils ask handles prune`.
pub fn prune(store: &mut Store, now: &str, keep_days: i64) -> Result<Pruned, HandleError> {
    let now_secs =
        secs_of(now).ok_or_else(|| HandleError::Message(format!("{now} is not a timestamp")))?;
    let mut out = Pruned::default();
    let mut candidates: BTreeSet<i64> = BTreeSet::new();
    for h in list(store, false)? {
        if !h.has_rows() {
            continue;
        }
        let last = h.last_read_at.as_deref().unwrap_or(&h.created_at);
        let Some(last_secs) = secs_of(last) else {
            continue;
        };
        if last_secs.saturating_add((keep_days.max(0) as u64) * 86_400) <= now_secs {
            candidates.insert(h.id);
        }
    }
    for id in candidates {
        let pins = pinned_by(store, id)?;
        if pins.is_empty() {
            drop_rows(store, id, now)?;
            out.dropped.push(id);
        } else {
            out.pinned.push((id, pins));
        }
    }
    Ok(out)
}

/// Record that identifiers were projected through a handle (§9): who,
/// which handle, which columns, how many rows, at which epoch; never a
/// value.
/// How many times a handle's rows were read or exported (Wave 4c §6.1).
pub fn read_count(store: &mut Store, id: i64) -> Result<i64, HandleError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE handle_id = {}",
        store.qualified("handle_read_audit"),
        d.param(1, Type::Int)
    );
    Ok(store.query(&sql, &[Param::Int(id)])?[0].int(0)?)
}

pub fn read_audit(
    store: &mut Store,
    principal: &str,
    handle_id: i64,
    columns: &[String],
    rows: usize,
    purpose: Option<&str>,
    epoch: i64,
) -> Result<i64, HandleError> {
    let now = now_iso();
    let written = store.insert(
        &Insert::new(
            table("handle_read_audit"),
            &[
                "principal",
                "handle_id",
                "read_at",
                "columns",
                "rows",
                "purpose",
                "epoch",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(principal),
            Param::Int(handle_id),
            Param::from(now.as_str()),
            Param::from(serde_json::to_string(columns).unwrap_or_default()),
            Param::Int(rows as i64),
            purpose.map_or(Param::Null, Param::from),
            Param::Int(epoch),
        ]],
    )?;
    Ok(written.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
}
