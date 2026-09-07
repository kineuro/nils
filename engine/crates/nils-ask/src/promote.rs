// SPDX-License-Identifier: AGPL-3.0-only

//! Promotion (§8.3): from ask to fact, one way. A complete, never
//! truncated, subject grain handle opens membership intervals recording
//! the handle id, its epoch, scheme digest and parameters as bound, the
//! ask hash and the selection version, and writes the cohort id onto the
//! selection. Re-promotion appends and never edits; the tool reports when
//! a promoted cohort's source ask has moved.

use std::collections::BTreeSet;
use std::fmt;

use nils_registry::audit::{self, Action, Entry};
use nils_registry::home::Registry;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error as StoreError, Store};
use nils_registry::time::now_iso;
use nils_registry::{Insert, Param};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ast::Grain;
use crate::handle::{self, HandleError};

#[derive(Debug)]
pub enum PromoteError {
    Store(StoreError),
    Handle(HandleError),
    NoSuchCohort(String),
    Message(String),
}

impl fmt::Display for PromoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PromoteError::Store(e) => write!(f, "{e}"),
            PromoteError::Handle(e) => write!(f, "{e}"),
            PromoteError::NoSuchCohort(n) => {
                write!(f, "no cohort named {n}; pass create to open one")
            }
            PromoteError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PromoteError {}

impl From<StoreError> for PromoteError {
    fn from(e: StoreError) -> Self {
        PromoteError::Store(e)
    }
}

impl From<HandleError> for PromoteError {
    fn from(e: HandleError) -> Self {
        PromoteError::Handle(e)
    }
}

/// What a promotion did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Promoted {
    pub handle_id: i64,
    pub cohort_id: i64,
    pub cohort: String,
    pub created: bool,
    /// Intervals opened.
    pub added: usize,
    /// Subjects already members with an open interval.
    pub already: usize,
    pub ask_hash: Option<String>,
    /// The selection whose version holds the handle's ask, when one does.
    pub selection: Option<(String, u64)>,
    /// The selection has a newer version than the one promoted.
    pub source_moved: bool,
    pub epoch: i64,
}

/// The selection version whose hash is the handle's ask hash.
struct Matched {
    name: String,
    version: u64,
    selection_id: i64,
    current_version: u64,
}

struct Applied {
    cohort_id: i64,
    created: bool,
    added: usize,
    already: usize,
    selection: Option<Matched>,
}

fn cohort_by_name(store: &mut Store, name: &str) -> Result<Option<i64>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE name = {}",
        store.qualified("cohort"),
        d.param(1, Type::Text)
    );
    store
        .query_opt(&sql, &[Param::from(name)])?
        .map(|r| r.int(0))
        .transpose()
}

/// Promote a handle's subjects into a cohort, opening one so named when
/// asked. A judgement changing act: the epoch advances.
pub fn promote(
    registry: &mut Registry,
    handle_id: i64,
    cohort: &str,
    actor: &str,
    reason: Option<&str>,
    create: bool,
) -> Result<Promoted, PromoteError> {
    let h = handle::get(registry.store(), handle_id)?.ok_or(HandleError::NotFound(handle_id))?;
    if h.withdrawn_at.is_some() {
        return Err(HandleError::Withdrawn(handle_id).into());
    }
    if h.grain != Grain::Subject {
        return Err(HandleError::Grain {
            id: handle_id,
            grain: h.grain,
        }
        .into());
    }
    if h.truncated {
        return Err(HandleError::Truncated(handle_id).into());
    }
    if !h.has_rows() {
        return Err(HandleError::Expired(handle_id).into());
    }
    let subjects: Vec<i64> = handle::keys(registry.store(), handle_id)?
        .into_iter()
        .map(|(k, s)| s.unwrap_or(k))
        .collect::<BTreeSet<i64>>()
        .into_iter()
        .collect();
    let ask_hash = h.ask_hash();
    let now = now_iso();
    let store = registry.store();
    store.begin()?;
    let result = (|| -> Result<Applied, PromoteError> {
        let (cohort_id, created) = match cohort_by_name(store, cohort)? {
            Some(id) => (id, false),
            None if create => {
                let rows = store.insert(
                    &Insert::new(table("cohort"), &["name", "owner", "created_at"])
                        .returning(&["id"]),
                    &[vec![
                        Param::from(cohort),
                        Param::from(actor),
                        Param::from(now.as_str()),
                    ]],
                )?;
                let id = rows
                    .first()
                    .ok_or_else(|| PromoteError::Message("the cohort was not written back".into()))?
                    .int(0)?;
                (id, true)
            }
            None => return Err(PromoteError::NoSuchCohort(cohort.to_string())),
        };
        let d = store.dialect();
        let sql = format!(
            "SELECT subject_id FROM {} WHERE cohort_id = {} AND left_at IS NULL",
            store.qualified("cohort_member"),
            d.param(1, Type::Int)
        );
        let open: BTreeSet<i64> = store
            .query(&sql, &[Param::Int(cohort_id)])?
            .iter()
            .map(|r| r.int(0))
            .collect::<Result<_, _>>()?;
        // the selection whose version holds this ask, if any
        let selection = match &ask_hash {
            Some(hash) => {
                let sql = format!(
                    "SELECT s.name, sv.version, s.id, s.current_version FROM {} sv JOIN {} s ON s.id = sv.selection_id \
                     WHERE sv.hash = {} ORDER BY sv.id DESC",
                    store.qualified("selection_version"),
                    store.qualified("selection"),
                    d.param(1, Type::Text)
                );
                store
                    .query_opt(&sql, &[Param::from(hash.as_str())])?
                    .map(|r| {
                        Ok::<_, StoreError>(Matched {
                            name: r.text(0)?.to_string(),
                            version: r.int(1)? as u64,
                            selection_id: r.int(2)?,
                            current_version: r.int(3)? as u64,
                        })
                    })
                    .transpose()?
            }
            None => None,
        };
        let params_json = h.params.to_string();
        let rows: Vec<Vec<Param>> = subjects
            .iter()
            .filter(|s| !open.contains(s))
            .map(|s| {
                vec![
                    Param::Int(cohort_id),
                    Param::Int(*s),
                    Param::from(now.as_str()),
                    Param::from(actor),
                    Param::from("promotion"),
                    Param::Int(handle_id),
                    Param::Int(h.epoch),
                    h.scheme_digest.as_deref().map_or(Param::Null, Param::from),
                    Param::from(params_json.as_str()),
                    ask_hash.as_deref().map_or(Param::Null, Param::from),
                    selection
                        .as_ref()
                        .map_or(Param::Null, |m| Param::Int(m.version as i64)),
                    reason.map_or(Param::Null, Param::from),
                ]
            })
            .collect();
        let added = rows.len();
        for chunk in rows.chunks(500) {
            store.insert(
                &Insert::new(
                    table("cohort_member"),
                    &[
                        "cohort_id",
                        "subject_id",
                        "joined_at",
                        "actor",
                        "source",
                        "handle_id",
                        "epoch",
                        "scheme_digest",
                        "params",
                        "ask_hash",
                        "selection_version",
                        "reason",
                    ],
                ),
                chunk,
            )?;
        }
        if let Some(m) = &selection {
            store.update_by_id(
                table("selection"),
                &[("cohort_id", Param::Int(cohort_id))],
                "id",
                m.selection_id,
            )?;
        }
        Ok(Applied {
            cohort_id,
            created,
            added,
            already: subjects.len() - added,
            selection,
        })
    })();
    let Applied {
        cohort_id,
        created,
        added,
        already,
        selection,
    } = match result {
        Ok(v) => v,
        Err(e) => {
            registry.store().rollback().ok();
            return Err(e);
        }
    };
    if created {
        let recorded = audit::record(
            registry,
            &Entry {
                principal: actor,
                action: Action::CohortCreate,
                scope: json!({"cohort": cohort, "id": cohort_id}),
                policy: None,
                job_id: None,
                details: Some(json!({"by": "promotion", "handle": handle_id})),
            },
        );
        if let Err(e) = recorded {
            registry.store().rollback().ok();
            return Err(e.into());
        }
    }
    let recorded = audit::record(
        registry,
        &Entry {
            principal: actor,
            action: Action::CohortPromote,
            scope: json!({"cohort": cohort, "id": cohort_id, "handle": handle_id, "added": added, "already": already}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "reason": reason,
                "ask_hash": ask_hash,
                "selection": selection.as_ref().map(|m| json!({"name": m.name, "version": m.version})),
            })),
        },
    );
    if let Err(e) = recorded {
        registry.store().rollback().ok();
        return Err(e.into());
    }
    registry.store().commit()?;
    let source_moved = selection
        .as_ref()
        .is_some_and(|m| m.current_version > m.version);
    Ok(Promoted {
        handle_id,
        cohort_id,
        cohort: cohort.to_string(),
        created,
        added,
        already,
        ask_hash,
        selection: selection.map(|m| (m.name, m.version)),
        source_moved,
        epoch: registry.meta().epoch,
    })
}
