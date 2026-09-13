// SPDX-License-Identifier: AGPL-3.0-only

//! Documents under authoring (§10): a document is addressed by handle so
//! `apply` returns a handle and never a document, and the JSON is fetched
//! by handle only when something needs it. The digest covers the whole
//! canonical text with its parameters bound, where the content hash is the
//! core's; the same text posted twice is one row. A document holds the
//! question and never subject data; its rows go ninety days after the last
//! use.

use std::fmt;

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error as StoreError, Row, Store};
use nils_registry::time::{now_iso, secs_of};
use nils_registry::{Insert, Param};
use serde::{Deserialize, Serialize};

use crate::ast::{Ask, canonical_json};

/// Days a document is kept after its last use.
pub const KEEP_DAYS: i64 = 90;

#[derive(Debug)]
pub enum DocumentError {
    Store(StoreError),
    NotFound(i64),
    Message(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentError::Store(e) => write!(f, "{e}"),
            DocumentError::NotFound(id) => write!(f, "no document {id}"),
            DocumentError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for DocumentError {}

impl From<StoreError> for DocumentError {
    fn from(e: StoreError) -> Self {
        DocumentError::Store(e)
    }
}

/// A document as stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: i64,
    pub digest: String,
    pub hash: String,
    pub ask: Ask,
    pub principal: String,
    pub created_at: String,
    pub last_used_at: String,
    pub parent_id: Option<i64>,
}

/// The digest of a whole document: its canonical JSON, parameters bound.
pub fn digest(ask: &Ask) -> String {
    let mut h = Blake2b::<U32>::new();
    let v = serde_json::to_value(ask).unwrap_or(serde_json::Value::Null);
    h.update(canonical_json(&v).as_bytes());
    hex::encode(h.finalize())
}

const COLUMNS: &str = "id, digest, hash, ask, principal, created_at, last_used_at, parent_id";

fn document_of(r: &Row) -> Result<Document, DocumentError> {
    let ask: Ask = serde_json::from_str(r.text(3)?)
        .map_err(|e| DocumentError::Message(format!("a stored document is broken: {e}")))?;
    Ok(Document {
        id: r.int(0)?,
        digest: r.text(1)?.to_string(),
        hash: r.text(2)?.to_string(),
        ask,
        principal: r.text(4)?.to_string(),
        created_at: r.text(5)?.to_string(),
        last_used_at: r.text(6)?.to_string(),
        parent_id: r.opt_int(7)?,
    })
}

/// Store a document and return its handle; the same text is one row, its
/// last use moved.
pub fn put(
    store: &mut Store,
    ask: &Ask,
    hash: &str,
    principal: &str,
    parent: Option<i64>,
) -> Result<Document, DocumentError> {
    let d = digest(ask);
    let now = now_iso();
    let dialect = store.dialect();
    let sql = format!(
        "SELECT {COLUMNS} FROM {} WHERE digest = {}",
        store.qualified("ask_document"),
        dialect.param(1, Type::Text)
    );
    if let Some(r) = store.query_opt(&sql, &[Param::from(d.as_str())])? {
        let existing = document_of(&r)?;
        // A document stored with no parent, such as a draft the assistant
        // proposed, is adopted as the next version of the one it is stored
        // under: once, and never into its own line.
        let adopted = match parent {
            Some(p)
                if existing.parent_id.is_none()
                    && p != existing.id
                    && !descends_from(store, p, existing.id)? =>
            {
                Some(p)
            }
            _ => None,
        };
        let mut sets = vec![("last_used_at", Param::from(now.as_str()))];
        if let Some(p) = adopted {
            sets.push(("parent_id", Param::Int(p)));
        }
        store.update_by_id(table("ask_document"), &sets, "id", existing.id)?;
        return Ok(Document {
            last_used_at: now,
            parent_id: adopted.or(existing.parent_id),
            ..existing
        });
    }
    let text = serde_json::to_string(ask)
        .map_err(|e| DocumentError::Message(format!("the document does not serialise: {e}")))?;
    let rows = store.insert(
        &Insert::new(
            table("ask_document"),
            &[
                "digest",
                "hash",
                "ask",
                "principal",
                "created_at",
                "last_used_at",
                "parent_id",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(d.as_str()),
            Param::from(hash),
            Param::from(text.as_str()),
            Param::from(principal),
            Param::from(now.as_str()),
            Param::from(now.as_str()),
            parent.map_or(Param::Null, Param::Int),
        ]],
    )?;
    let id = rows
        .first()
        .ok_or_else(|| DocumentError::Message("the document was not written back".into()))?
        .int(0)?;
    Ok(Document {
        id,
        digest: d,
        hash: hash.to_string(),
        ask: ask.clone(),
        principal: principal.to_string(),
        created_at: now.clone(),
        last_used_at: now,
        parent_id: parent,
    })
}

/// Whether the document `id` is `ancestor` or follows it through its parents.
/// A chain longer than any a person makes is taken as following, so nothing
/// is adopted into it.
fn descends_from(store: &mut Store, id: i64, ancestor: i64) -> Result<bool, DocumentError> {
    let sql = format!(
        "SELECT parent_id FROM {} WHERE id = {}",
        store.qualified("ask_document"),
        store.dialect().param(1, Type::Int)
    );
    let mut at = Some(id);
    let mut steps = 0;
    while let Some(current) = at {
        if current == ancestor || steps > 1000 {
            return Ok(true);
        }
        steps += 1;
        at = match store.query_opt(&sql, &[Param::Int(current)])? {
            Some(r) => r.opt_int(0)?,
            None => None,
        };
    }
    Ok(false)
}

/// A document by handle; a read moves its last use.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Document>, DocumentError> {
    let dialect = store.dialect();
    let sql = format!(
        "SELECT {COLUMNS} FROM {} WHERE id = {}",
        store.qualified("ask_document"),
        dialect.param(1, Type::Int)
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(id)])? else {
        return Ok(None);
    };
    let now = now_iso();
    store.update_by_id(
        table("ask_document"),
        &[("last_used_at", Param::from(now.as_str()))],
        "id",
        id,
    )?;
    Ok(Some(document_of(&r)?))
}

/// Every document, oldest first, without moving any last use (the list
/// door of Wave 5 §12.1 reads them all to fold them into lineages).
pub fn list(store: &mut Store) -> Result<Vec<Document>, DocumentError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM {} ORDER BY id",
        store.qualified("ask_document")
    );
    store.query(&sql, &[])?.iter().map(document_of).collect()
}

/// Drop every document unused for `keep_days`; returns how many went.
pub fn prune(store: &mut Store, now: &str, keep_days: i64) -> Result<u64, DocumentError> {
    let now_secs =
        secs_of(now).ok_or_else(|| DocumentError::Message(format!("{now} is not a timestamp")))?;
    let cutoff = now_secs.saturating_sub((keep_days.max(0) as u64) * 86_400);
    let cutoff_iso = nils_registry::time::iso_of(cutoff);
    let dialect = store.dialect();
    let sql = format!(
        "DELETE FROM {} WHERE last_used_at < {}",
        store.qualified("ask_document"),
        dialect.param(1, Type::Timestamp)
    );
    Ok(store.execute(&sql, &[Param::from(cutoff_iso.as_str())])?)
}
