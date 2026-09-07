// SPDX-License-Identifier: AGPL-3.0-only

//! Saved asks (§8.2): a selection is a question with a version log,
//! addressed as `selection:<name>@<version>`. A version is immutable; an
//! edit makes the next one. `save` refuses a name a cohort holds unless the
//! selection is that cohort's own source ask, so `member_of X` and
//! `from: selection:X` are never two numbers under one word. `inline`
//! replaces a pinned selection source with the stored ask's sets, renamed
//! under a prefix, so the compiler sees one graph.

use std::collections::BTreeMap;
use std::fmt;

use nils_registry::audit::{self, Action, Entry};
use nils_registry::home::Registry;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error as StoreError, Row, Store};
use nils_registry::time::now_iso;
use nils_registry::{Insert, Param};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ast::{Arg, Ask, Clause, Grain, Src};

#[derive(Debug)]
pub enum SelectionError {
    Store(StoreError),
    /// The name is a cohort's, and the selection is not its source ask.
    NameIsACohort(String),
    NotFound(String, Option<u64>),
    /// A selection source must be pinned to a version before it is inlined.
    NotPinned(String),
    /// The stored ask's answer is at another grain than the set reading it.
    GrainMismatch {
        set: String,
        wanted: Grain,
        stored: Grain,
    },
    /// Selections reading selections eight deep, or a cycle.
    TooDeep,
    Message(String),
}

impl fmt::Display for SelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectionError::Store(e) => write!(f, "{e}"),
            SelectionError::NameIsACohort(n) => write!(
                f,
                "{n} is a cohort; a selection may take a cohort's name only as that cohort's own source ask"
            ),
            SelectionError::NotFound(n, Some(v)) => write!(f, "no selection {n}@{v}"),
            SelectionError::NotFound(n, None) => write!(f, "no selection named {n}"),
            SelectionError::NotPinned(n) => write!(f, "selection {n} is not pinned to a version"),
            SelectionError::GrainMismatch {
                set,
                wanted,
                stored,
            } => write!(
                f,
                "sets.{set} is a {wanted} set but the selection answers at {stored} grain"
            ),
            SelectionError::TooDeep => {
                f.write_str("selections read selections more than eight deep, or in a cycle")
            }
            SelectionError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SelectionError {}

impl From<StoreError> for SelectionError {
    fn from(e: StoreError) -> Self {
        SelectionError::Store(e)
    }
}

/// What a save returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub id: i64,
    pub name: String,
    pub version: u64,
    pub hash: String,
    pub epoch: i64,
}

/// One version as read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub selection_id: i64,
    pub name: String,
    pub version: u64,
    pub current_version: u64,
    pub ask: Ask,
    pub hash: String,
    pub created_at: String,
    pub actor: String,
    pub note: Option<String>,
    pub cohort_id: Option<i64>,
}

/// One selection in a listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listed {
    pub id: i64,
    pub name: String,
    pub owner: String,
    pub created_at: String,
    pub current_version: u64,
    pub cohort_id: Option<i64>,
    pub description: Option<String>,
}

fn cohort_id(store: &mut Store, name: &str) -> Result<Option<i64>, StoreError> {
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

/// Save a desugared ask as the next version of a selection; a judgement
/// changing act, so the epoch advances.
pub fn save(
    registry: &mut Registry,
    name: &str,
    ask: &Ask,
    hash: &str,
    actor: &str,
    note: Option<&str>,
    description: Option<&str>,
) -> Result<Saved, SelectionError> {
    let ask_json = serde_json::to_string(ask)
        .map_err(|e| SelectionError::Message(format!("the ask does not serialise: {e}")))?;
    let now = now_iso();
    let store = registry.store();
    store.begin()?;
    let result = (|| -> Result<(i64, u64), SelectionError> {
        let d = store.dialect();
        let sql = format!(
            "SELECT id, current_version, cohort_id FROM {} WHERE name = {}",
            store.qualified("selection"),
            d.param(1, Type::Text)
        );
        let existing = store.query_opt(&sql, &[Param::from(name)])?;
        let cohort = cohort_id(store, name)?;
        let (id, version) = match existing {
            Some(r) => {
                let (id, current, linked) = (r.int(0)?, r.int(1)? as u64, r.opt_int(2)?);
                if let Some(c) = cohort
                    && linked != Some(c)
                {
                    return Err(SelectionError::NameIsACohort(name.to_string()));
                }
                let next = current + 1;
                store.update_by_id(
                    table("selection"),
                    &[("current_version", Param::Int(next as i64))],
                    "id",
                    id,
                )?;
                (id, next)
            }
            None => {
                if cohort.is_some() {
                    return Err(SelectionError::NameIsACohort(name.to_string()));
                }
                let rows = store.insert(
                    &Insert::new(
                        table("selection"),
                        &[
                            "name",
                            "owner",
                            "created_at",
                            "current_version",
                            "description",
                        ],
                    )
                    .returning(&["id"]),
                    &[vec![
                        Param::from(name),
                        Param::from(actor),
                        Param::from(now.as_str()),
                        Param::Int(1),
                        description.map_or(Param::Null, Param::from),
                    ]],
                )?;
                let id = rows
                    .first()
                    .ok_or_else(|| {
                        SelectionError::Message("the selection was not written back".into())
                    })?
                    .int(0)?;
                (id, 1)
            }
        };
        store.insert(
            &Insert::new(
                table("selection_version"),
                &[
                    "selection_id",
                    "version",
                    "ask",
                    "hash",
                    "created_at",
                    "actor",
                    "note",
                ],
            ),
            &[vec![
                Param::Int(id),
                Param::Int(version as i64),
                Param::from(ask_json.as_str()),
                Param::from(hash),
                Param::from(now.as_str()),
                Param::from(actor),
                note.map_or(Param::Null, Param::from),
            ]],
        )?;
        Ok((id, version))
    })();
    let (id, version) = match result {
        Ok(v) => v,
        Err(e) => {
            registry.store().rollback().ok();
            return Err(e);
        }
    };
    let recorded = audit::record(
        registry,
        &Entry {
            principal: actor,
            action: Action::SelectionSave,
            scope: json!({"selection": name, "version": version, "hash": hash}),
            policy: None,
            job_id: None,
            details: note.map(|n| json!({"note": n})),
        },
    );
    if let Err(e) = recorded {
        registry.store().rollback().ok();
        return Err(e.into());
    }
    registry.store().commit()?;
    Ok(Saved {
        id,
        name: name.to_string(),
        version,
        hash: hash.to_string(),
        epoch: registry.meta().epoch,
    })
}

fn version_of(r: &Row) -> Result<Version, SelectionError> {
    let ask: Ask = serde_json::from_str(r.text(4)?)
        .map_err(|e| SelectionError::Message(format!("a stored ask is broken: {e}")))?;
    Ok(Version {
        selection_id: r.int(0)?,
        name: r.text(1)?.to_string(),
        version: r.int(2)? as u64,
        current_version: r.int(3)? as u64,
        ask,
        hash: r.text(5)?.to_string(),
        created_at: r.text(6)?.to_string(),
        actor: r.text(7)?.to_string(),
        note: r.opt_text(8)?.map(str::to_string),
        cohort_id: r.opt_int(9)?,
    })
}

/// One version of a selection, the current one when none is named.
pub fn get(
    store: &mut Store,
    name: &str,
    version: Option<u64>,
) -> Result<Option<Version>, SelectionError> {
    let d = store.dialect();
    let mut sql = format!(
        "SELECT s.id, s.name, sv.version, s.current_version, sv.ask, sv.hash, sv.created_at, sv.actor, sv.note, s.cohort_id \
         FROM {} sv JOIN {} s ON s.id = sv.selection_id WHERE s.name = {}",
        store.qualified("selection_version"),
        store.qualified("selection"),
        d.param(1, Type::Text)
    );
    let mut params = vec![Param::from(name)];
    match version {
        Some(v) => {
            sql.push_str(&format!(" AND sv.version = {}", d.param(2, Type::Int)));
            params.push(Param::Int(v as i64));
        }
        None => sql.push_str(" AND sv.version = s.current_version"),
    }
    store
        .query_opt(&sql, &params)?
        .map(|r| version_of(&r))
        .transpose()
}

/// Every selection, by name.
pub fn list(store: &mut Store) -> Result<Vec<Listed>, SelectionError> {
    let sql = format!(
        "SELECT id, name, owner, created_at, current_version, cohort_id, description FROM {} ORDER BY name",
        store.qualified("selection")
    );
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| {
            Ok(Listed {
                id: r.int(0)?,
                name: r.text(1)?.to_string(),
                owner: r.text(2)?.to_string(),
                created_at: r.text(3)?.to_string(),
                current_version: r.int(4)? as u64,
                cohort_id: r.opt_int(5)?,
                description: r.opt_text(6)?.map(str::to_string),
            })
        })
        .collect()
}

/// One selection source replaced by the stored ask's sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inlined {
    pub set: String,
    pub name: String,
    pub version: u64,
    /// The set the stored ask answered with, under its new name.
    pub answer: String,
}

fn prefix_of(name: &str, version: u64) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{safe}__v{version}__")
}

fn rename_set(name: &mut String, sets: &BTreeMap<String, String>) {
    if let Some(n) = sets.get(name.as_str()) {
        *name = n.clone();
    }
}

fn rename_in_clause(
    c: &mut Clause,
    sets: &BTreeMap<String, String>,
    params: &BTreeMap<String, String>,
) {
    for key in ["set", "over"] {
        if let Some(Value::String(s)) = c.opts.get_mut(key)
            && let Some(n) = sets.get(s.as_str())
        {
            *s = n.clone();
        }
    }
    // an option holding a clause, a param ref among them
    for v in c.opts.values_mut() {
        if let Ok(mut inner) = crate::ast::clause_of(v)
            && inner.op == "param"
        {
            rename_in_clause(&mut inner, sets, params);
            *v = serde_json::to_value(&inner).unwrap_or(Value::Null);
        }
    }
    if c.op == "param"
        && let Some(Arg::Text(name)) = c.args.first_mut()
        && let Some(n) = params.get(name.as_str())
    {
        *name = n.clone();
    }
    for a in c.args.iter_mut() {
        if let Arg::Clause(inner) = a {
            rename_in_clause(inner, sets, params);
        }
    }
}

/// Fetches a stored, desugared ask by name and version.
pub type Loader<'a> = &'a mut dyn FnMut(&str, u64) -> Result<Option<Ask>, SelectionError>;

/// Replace every pinned selection source with the stored ask's sets under
/// a prefix; `load` fetches a stored, desugared ask by name and version.
pub fn inline(ask: &mut Ask, load: Loader<'_>) -> Result<Vec<Inlined>, SelectionError> {
    let mut out = Vec::new();
    for _round in 0..8 {
        let pending: Vec<(String, String, u64)> = ask
            .sets
            .iter()
            .filter_map(|(set, s)| match &s.from {
                Some(Src::Selection {
                    name,
                    version: Some(v),
                }) => Some((set.clone(), name.clone(), *v)),
                _ => None,
            })
            .collect();
        if pending.is_empty() {
            return Ok(out);
        }
        for (set_name, name, version) in pending {
            if let Some(Src::Selection { version: None, .. }) = ask.sets[&set_name].from {
                return Err(SelectionError::NotPinned(name));
            }
            let stored = load(&name, version)?
                .ok_or_else(|| SelectionError::NotFound(name.clone(), Some(version)))?;
            let wanted = ask.sets[&set_name].grain;
            let stored_grain = stored
                .sets
                .get(&stored.out.set)
                .map(|s| s.grain)
                .ok_or_else(|| {
                    SelectionError::Message(format!(
                        "selection {name}@{version} answers with no set"
                    ))
                })?;
            if stored_grain != wanted {
                return Err(SelectionError::GrainMismatch {
                    set: set_name,
                    wanted,
                    stored: stored_grain,
                });
            }
            let prefix = prefix_of(&name, version);
            let set_map: BTreeMap<String, String> = stored
                .sets
                .keys()
                .map(|k| (k.clone(), format!("{prefix}{k}")))
                .collect();
            let param_map: BTreeMap<String, String> = stored
                .params
                .keys()
                .map(|k| (k.clone(), format!("{prefix}{k}")))
                .collect();
            let values_map: BTreeMap<String, String> = stored
                .values
                .keys()
                .map(|k| (k.clone(), format!("{prefix}{k}")))
                .collect();
            for (old, mut s) in stored.sets {
                match &mut s.from {
                    Some(Src::Set(x)) => rename_set(x, &set_map),
                    Some(Src::Values(v)) => rename_set(v, &values_map),
                    _ => {}
                }
                if let Some(o) = &mut s.of {
                    rename_set(o, &set_map);
                }
                if let Some(a) = &mut s.algebra {
                    for x in a.sets.iter_mut() {
                        rename_set(x, &set_map);
                    }
                }
                if let Some(g) = &mut s.group {
                    rename_set(&mut g.of, &set_map);
                    for c in g.by.iter_mut() {
                        rename_in_clause(c, &set_map, &param_map);
                    }
                }
                for n in s.near.iter_mut() {
                    rename_set(&mut n.set, &set_map);
                    for o in n.order.iter_mut() {
                        rename_in_clause(&mut o.0, &set_map, &param_map);
                    }
                }
                for a in s.attach.iter_mut() {
                    rename_set(&mut a.set, &set_map);
                }
                for h in s.has.iter_mut() {
                    rename_set(&mut h.set, &set_map);
                }
                for x in s.same.iter_mut() {
                    rename_set(&mut x.over, &set_map);
                    for c in x.by.iter_mut() {
                        rename_in_clause(c, &set_map, &param_map);
                    }
                }
                for e in s.every.iter_mut() {
                    rename_set(&mut e.of, &set_map);
                    rename_set(&mut e.in_, &set_map);
                }
                for (_, c) in s.bind.0.iter_mut() {
                    rename_in_clause(c, &set_map, &param_map);
                }
                for c in s.where_.iter_mut() {
                    rename_in_clause(c, &set_map, &param_map);
                }
                if let Some(pk) = &mut s.pick {
                    for o in pk.by.iter_mut() {
                        rename_in_clause(&mut o.0, &set_map, &param_map);
                    }
                }
                ask.sets.insert(set_map[&old].clone(), s);
            }
            for (old, decl) in stored.params {
                ask.params.insert(param_map[&old].clone(), decl);
            }
            for (old, decl) in stored.values {
                ask.values.insert(values_map[&old].clone(), decl);
            }
            let answer = set_map[&stored.out.set].clone();
            let target = ask.sets.get_mut(&set_name).expect("the reading set");
            target.from = Some(Src::Set(answer.clone()));
            out.push(Inlined {
                set: set_name,
                name,
                version,
                answer,
            });
        }
    }
    Err(SelectionError::TooDeep)
}
