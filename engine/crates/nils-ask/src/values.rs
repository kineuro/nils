// SPDX-License-Identifier: AGPL-3.0-only

//! Uploaded identifier lists (§8.4, §9): an identifier enters only through
//! `values` and is resolved on arrival through the linkage store. What the
//! registry keeps is the list's shape, never a value: a `values_source`
//! row with the upload id, the namespace, a digest, the count and the
//! unresolved count, and one `values_member` per position with the subject
//! it resolved to, or none. The upload itself is gone on resolution.

use std::collections::HashMap;
use std::fmt;

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use nils_registry::home::{HomeError, Registry};
use nils_registry::linkage::{self, Subkeys};
use nils_registry::schema::table;
use nils_registry::store::Error as StoreError;
use nils_registry::time::now_iso;
use nils_registry::{Insert, Param};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum ValuesError {
    Store(StoreError),
    Home(HomeError),
    /// The linkage store has no identifier type of that name.
    UnknownNamespace(String),
    Empty,
}

impl fmt::Display for ValuesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValuesError::Store(e) => write!(f, "{e}"),
            ValuesError::Home(e) => write!(f, "{e}"),
            ValuesError::UnknownNamespace(n) => write!(
                f,
                "the linkage store has no identifier type named {n}; nils linkage id-types lists them"
            ),
            ValuesError::Empty => f.write_str("an upload holds at least one value"),
        }
    }
}

impl std::error::Error for ValuesError {}

impl From<StoreError> for ValuesError {
    fn from(e: StoreError) -> Self {
        ValuesError::Store(e)
    }
}

impl From<HomeError> for ValuesError {
    fn from(e: HomeError) -> Self {
        ValuesError::Home(e)
    }
}

/// What an upload left: its id for `values: {name: {upload: id}}`, the
/// counts, and the positions that resolved to no subject. The values
/// themselves are not here and not in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Upload {
    pub upload_id: String,
    pub source_id: i64,
    pub namespace: String,
    pub digest: String,
    pub n: usize,
    pub unresolved: Vec<usize>,
    /// Distinct subjects the list resolved to.
    pub subjects: usize,
}

/// Resolve a list of identifiers of one namespace through the linkage
/// store and keep its shape by reference.
pub fn upload(
    registry: &mut Registry,
    namespace: &str,
    values: &[String],
    principal: &str,
) -> Result<Upload, ValuesError> {
    if values.is_empty() {
        return Err(ValuesError::Empty);
    }
    let key = registry.pseudonym_key()?;
    let keys = Subkeys::derive(&key);
    let mut lk = registry.open_linkage()?;
    let type_id = linkage::id_type_id(&mut lk, namespace)?
        .ok_or_else(|| ValuesError::UnknownNamespace(namespace.to_string()))?;
    let lookups: Vec<Vec<u8>> = values.iter().map(|v| keys.lookup(namespace, v)).collect();
    let mut by_lookup: HashMap<Vec<u8>, i64> = HashMap::new();
    for i in linkage::identities_by_lookup(&mut lk, &lookups)? {
        if i.id_type_id == type_id {
            by_lookup.insert(i.lookup, i.subject_id);
        }
    }
    drop(lk);
    let mut h = Blake2b::<U32>::new();
    for v in values {
        h.update(v.as_bytes());
        h.update(b"\n");
    }
    let digest = hex::encode(h.finalize());
    let now = now_iso();
    let mut idh = Blake2b::<U32>::new();
    idh.update(digest.as_bytes());
    idh.update(now.as_bytes());
    idh.update(principal.as_bytes());
    let upload_id = format!("u-{}", &hex::encode(idh.finalize())[..16]);
    let members: Vec<Option<i64>> = lookups.iter().map(|l| by_lookup.get(l).copied()).collect();
    let unresolved: Vec<usize> = members
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_none())
        .map(|(i, _)| i)
        .collect();
    let subjects = {
        let mut ids: Vec<i64> = members.iter().flatten().copied().collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    };
    let store = registry.store();
    store.begin()?;
    let written = (|| -> Result<i64, StoreError> {
        let rows = store.insert(
            &Insert::new(
                table("values_source"),
                &[
                    "upload_id",
                    "namespace",
                    "digest",
                    "n",
                    "unresolved",
                    "principal",
                    "created_at",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(upload_id.as_str()),
                Param::from(namespace),
                Param::from(digest.as_str()),
                Param::Int(values.len() as i64),
                Param::Int(unresolved.len() as i64),
                Param::from(principal),
                Param::from(now.as_str()),
            ]],
        )?;
        let source = rows
            .first()
            .ok_or_else(|| StoreError::Message("the values source was not written back".into()))?
            .int(0)?;
        let member_rows: Vec<Vec<Param>> = members
            .iter()
            .enumerate()
            .map(|(i, s)| {
                vec![
                    Param::Int(source),
                    Param::Int(i as i64),
                    s.map_or(Param::Null, Param::Int),
                ]
            })
            .collect();
        for chunk in member_rows.chunks(500) {
            store.insert(
                &Insert::new(
                    table("values_member"),
                    &["source_id", "position", "subject_id"],
                ),
                chunk,
            )?;
        }
        Ok(source)
    })();
    let source_id = match written {
        Ok(id) => {
            store.commit()?;
            id
        }
        Err(e) => {
            store.rollback().ok();
            return Err(e.into());
        }
    };
    Ok(Upload {
        upload_id,
        source_id,
        namespace: namespace.to_string(),
        digest,
        n: values.len(),
        unresolved,
        subjects,
    })
}

/// The shape of a stored upload: its count and the unresolved positions
/// (a sample of at most ten), for a handle's `values_unresolved`.
pub fn shape(
    store: &mut nils_registry::store::Store,
    upload_id: &str,
) -> Result<Option<serde_json::Value>, ValuesError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id, n, unresolved FROM {} WHERE upload_id = {}",
        store.qualified("values_source"),
        d.param(1, nils_registry::schema::Type::Text)
    );
    let Some(r) = store.query_opt(&sql, &[Param::from(upload_id)])? else {
        return Ok(None);
    };
    let (id, n, unresolved) = (r.int(0)?, r.int(1)?, r.int(2)?);
    let sql = format!(
        "SELECT position FROM {} WHERE source_id = {} AND subject_id IS NULL ORDER BY position LIMIT 10",
        store.qualified("values_member"),
        d.param(1, nils_registry::schema::Type::Int)
    );
    let sample: Vec<i64> = store
        .query(&sql, &[Param::Int(id)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    Ok(Some(serde_json::json!({
        "n": n,
        "unresolved": unresolved,
        "sample_positions": sample,
    })))
}
