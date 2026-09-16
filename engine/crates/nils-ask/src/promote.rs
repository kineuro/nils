// SPDX-License-Identifier: AGPL-3.0-only

//! Promotion (§8.3): from ask to fact, one way. A complete, never
//! truncated handle opens membership intervals recording the handle id,
//! its epoch, scheme digest and parameters as bound, the ask hash and the
//! selection version, and writes the cohort id onto the selection.
//! Re-promotion appends and never edits; the tool reports when a promoted
//! cohort's source ask has moved. Record 26 §9: a handle at session or
//! stack grain promotes too, filing the distinct subjects of its rows, the
//! grain and the row count recorded on the intervals.

use std::collections::BTreeSet;
use std::fmt;

use nils_registry::Param;
use nils_registry::audit::{self, Action, Entry};
use nils_registry::cohort::{self, Opened};
use nils_registry::home::Registry;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error as StoreError, Store};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ast::Grain;
use crate::handle::{self, HandleError};

#[derive(Debug)]
pub enum PromoteError {
    Store(StoreError),
    Handle(HandleError),
    NoSuchCohort(String),
    /// The name is a selection's (Wave 4b §8.2), or the cohort is retired.
    Refused(String),
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
            PromoteError::Refused(m) => f.write_str(m),
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
    /// The handle's grain; the subjects of its rows are what joined.
    #[serde(default)]
    pub grain: String,
    /// The handle's rows, of which `added + already` distinct subjects.
    #[serde(default)]
    pub rows: i64,
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

/// Whether a version of the selection named so holds this ask: the
/// selection is then the cohort's own source ask (Wave 4b §8.2).
fn selection_holds(store: &mut Store, name: &str, hash: Option<&str>) -> Result<bool, StoreError> {
    let Some(hash) = hash else {
        return Ok(false);
    };
    let d = store.dialect();
    let sql = format!(
        "SELECT 1 FROM {} sv JOIN {} s ON s.id = sv.selection_id WHERE s.name = {} AND sv.hash = {}",
        store.qualified("selection_version"),
        store.qualified("selection"),
        d.param(1, Type::Text),
        d.param(2, Type::Text)
    );
    Ok(store
        .query_opt(&sql, &[Param::from(name), Param::from(hash)])?
        .is_some())
}

/// Promote a handle's subjects into a cohort, opening one so named when
/// asked. A judgement changing act: the epoch advances. A subject grain
/// handle's keys are its subjects; a session or stack grain handle's rows
/// each name their subject, and the distinct subjects join (record 26 §9).
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
    if !matches!(h.grain, Grain::Subject | Grain::Session | Grain::Stack) {
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
    // a subject handle's keys are its subjects; a session's or a stack's
    // rows each name their subject
    let subjects: Vec<i64> = handle::keys(registry.store(), handle_id)?
        .into_iter()
        .filter_map(|(k, s)| match h.grain {
            Grain::Subject => Some(s.unwrap_or(k)),
            _ => s,
        })
        .collect::<BTreeSet<i64>>()
        .into_iter()
        .collect();
    let ask_hash = h.ask_hash();
    let store = registry.store();
    let existing = cohort::by_name(store, cohort)?;
    if let Some(c) = &existing
        && c.retired_at.is_some()
    {
        return Err(PromoteError::Refused(format!(
            "cohort {cohort} is retired; bring it back before promoting into it"
        )));
    }
    // Wave 4b §8.2: the two namespaces are one, unless the selection is
    // this cohort's own source ask, which is what a promotion of a saved
    // selection into a cohort of its name is.
    if existing.is_none()
        && create
        && cohort::selection_named(store, cohort)?
        && !selection_holds(store, cohort, ask_hash.as_deref())?
    {
        return Err(PromoteError::Refused(format!(
            "{cohort} is a selection's name; a cohort cannot be named so (Wave 4b section 8.2)"
        )));
    }
    store.begin()?;
    let result = (|| -> Result<Applied, PromoteError> {
        let (cohort_id, created) = match &existing {
            Some(c) => (c.id, false),
            None if create => (cohort::insert(store, cohort, actor, None)?, true),
            None => return Err(PromoteError::NoSuchCohort(cohort.to_string())),
        };
        let d = store.dialect();
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
        // record 26 §9: the grain and the row count beside the parameters
        // as bound, so an interval says what kind of answer opened it
        let params = json!({
            "grain": h.grain.name(),
            "rows": h.row_count,
            "ask": h.params,
        });
        let added = cohort::join(
            store,
            cohort_id,
            &subjects,
            actor,
            &Opened {
                source: "promotion",
                handle_id: Some(handle_id),
                epoch: Some(h.epoch),
                scheme_digest: h.scheme_digest.as_deref(),
                params: Some(params),
                ask_hash: ask_hash.as_deref(),
                selection_version: selection.as_ref().map(|m| m.version as i64),
                reason,
                ..Opened::default()
            },
        )?;
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
                "grain": h.grain.name(),
                "rows": h.row_count,
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
        grain: h.grain.name().to_string(),
        rows: h.row_count,
        added,
        already,
        ask_hash,
        selection: selection.map(|m| (m.name, m.version)),
        source_moved,
        epoch: registry.meta().epoch,
    })
}
