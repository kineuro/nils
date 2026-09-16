// SPDX-License-Identifier: AGPL-3.0-only

//! Cohorts as memberships (record 26 §8 and §9): made, renamed, retired,
//! joined by hand with a reason, fed by a digest of a dataset, and read with
//! their counts and provenance. A cohort is a membership fact and nothing
//! else: the interval log of `cohort_member` says who joined, when, through
//! what, and who closed the interval. Every act here is audited in the
//! same transaction and moves the epoch. Never a row of a person: the
//! documents count, and a code is the one thing a member is named by.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use serde_json::{Value, json};

use crate::audit::{self, Action, Entry};
use crate::home::Registry;
use crate::place::{self, Role};
use crate::schema::{Type, table};
use crate::session::Scheme;
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    /// No cohort by that name.
    NotFound(String),
    /// The name is a cohort's or a selection's already.
    Taken(String),
    /// Codes the registry does not hold; nothing was written.
    Unknown(Vec<String>),
    Message(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::NotFound(n) => write!(f, "no cohort named {n}"),
            Error::Taken(n) => write!(
                f,
                "{n} is in use: a cohort or a selection is named so already"
            ),
            Error::Unknown(codes) => write!(
                f,
                "{} code(s) the registry does not hold: {}; nothing was written",
                codes.len(),
                codes.join(", ")
            ),
            Error::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

impl From<crate::home::HomeError> for Error {
    fn from(e: crate::home::HomeError) -> Error {
        Error::Message(e.to_string())
    }
}

/// One cohort row.
#[derive(Debug, Clone, PartialEq)]
pub struct Cohort {
    pub id: i64,
    pub name: String,
    pub owner: String,
    pub description: Option<String>,
    pub created_at: String,
    pub retired_at: Option<String>,
}

fn text_of(store: &Store, table_name: &str, column: &str) -> String {
    let t = table(table_name);
    let c = t
        .column(column)
        .unwrap_or_else(|| panic!("{table_name}.{column} is not a column"));
    store.dialect().text_of(c)
}

fn cohort_select(store: &Store, filter: &str) -> String {
    format!(
        "SELECT id, name, owner, description, {}, {} FROM {}{filter}",
        text_of(store, "cohort", "created_at"),
        text_of(store, "cohort", "retired_at"),
        store.qualified("cohort")
    )
}

fn cohort_of(r: &crate::store::Row) -> Result<Cohort, StoreError> {
    Ok(Cohort {
        id: r.int(0)?,
        name: r.text(1)?.to_string(),
        owner: r.text(2)?.to_string(),
        description: r.opt_text(3)?.map(str::to_string),
        created_at: r.opt_text(4)?.unwrap_or_default().to_string(),
        retired_at: r.opt_text(5)?.map(str::to_string),
    })
}

/// The cohort of that name, retired or not.
pub fn by_name(store: &mut Store, name: &str) -> Result<Option<Cohort>, StoreError> {
    let d = store.dialect();
    let sql = cohort_select(store, &format!(" WHERE name = {}", d.param(1, Type::Text)));
    store
        .query_opt(&sql, &[Param::from(name)])?
        .map(|r| cohort_of(&r))
        .transpose()
}

/// Every cohort, by name, retired ones included.
pub fn all(store: &mut Store) -> Result<Vec<Cohort>, StoreError> {
    let sql = cohort_select(store, " ORDER BY name");
    store.query(&sql, &[])?.iter().map(cohort_of).collect()
}

/// Whether a selection is named so (Wave 4b §8.2): the two namespaces are
/// one, since `@name` selects either.
pub fn selection_named(store: &mut Store, name: &str) -> Result<bool, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT 1 FROM {} WHERE name = {}",
        store.qualified("selection"),
        d.param(1, Type::Text)
    );
    Ok(store.query_opt(&sql, &[Param::from(name)])?.is_some())
}

fn checked_name(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Message("name: what the cohort is called".into()));
    }
    if name.starts_with('@') || name.contains('/') || name.contains(char::is_whitespace) {
        return Err(Error::Message(format!(
            "{name} is not a cohort name: one word, without @, / or spaces"
        )));
    }
    Ok(name)
}

/// The subject ids with an open interval in the cohort.
pub fn open_members(store: &mut Store, cohort_id: i64) -> Result<BTreeSet<i64>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT subject_id FROM {} WHERE cohort_id = {} AND left_at IS NULL",
        store.qualified("cohort_member"),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(cohort_id)])?
        .iter()
        .map(|r| r.int(0))
        .collect()
}

/// The open members of the cohort of that name; None when there is no
/// cohort so named.
pub fn open_members_of(store: &mut Store, name: &str) -> Result<Option<BTreeSet<i64>>, StoreError> {
    match by_name(store, name)? {
        Some(c) => Ok(Some(open_members(store, c.id)?)),
        None => Ok(None),
    }
}

/// The subject id of each code, and the codes the registry does not hold.
pub fn subjects_by_code(
    store: &mut Store,
    codes: &[String],
) -> Result<(BTreeMap<String, i64>, Vec<String>), StoreError> {
    let mut found = BTreeMap::new();
    let wanted: Vec<String> = codes
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if wanted.is_empty() {
        return Ok((found, Vec::new()));
    }
    let t = table("subject");
    let cols = [
        t.column("id").expect("subject.id"),
        t.column("code").expect("subject.code"),
    ];
    for r in store.select_by_keys(t, &cols, "code", &wanted)? {
        found.insert(r.text(1)?.to_string(), r.int(0)?);
    }
    let mut unknown: Vec<String> = wanted
        .iter()
        .filter(|c| !found.contains_key(*c))
        .cloned()
        .collect();
    unknown.sort();
    unknown.dedup();
    Ok((found, unknown))
}

/// What opened an interval: the provenance columns of `cohort_member`.
#[derive(Debug, Clone, Default)]
pub struct Opened<'a> {
    /// `digest`, `promotion`, `manual` or `import`.
    pub source: &'a str,
    pub batch_id: Option<i64>,
    pub handle_id: Option<i64>,
    pub epoch: Option<i64>,
    pub scheme_digest: Option<&'a str>,
    pub params: Option<Value>,
    pub ask_hash: Option<&'a str>,
    pub selection_version: Option<i64>,
    pub reason: Option<&'a str>,
}

/// Open an interval for every subject without one in the cohort, inside the
/// caller's transaction. Returns how many were opened; the rest were
/// members already.
pub fn join(
    store: &mut Store,
    cohort_id: i64,
    subjects: &[i64],
    actor: &str,
    opened: &Opened<'_>,
) -> Result<usize, StoreError> {
    let open = open_members(store, cohort_id)?;
    let now = now_iso();
    let params = opened.params.as_ref().map(Value::to_string);
    let rows: Vec<Vec<Param>> = subjects
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|s| !open.contains(s))
        .map(|s| {
            vec![
                Param::Int(cohort_id),
                Param::Int(*s),
                Param::from(now.as_str()),
                Param::from(actor),
                Param::from(opened.source),
                opened.batch_id.map_or(Param::Null, Param::Int),
                opened.handle_id.map_or(Param::Null, Param::Int),
                opened.epoch.map_or(Param::Null, Param::Int),
                opened.scheme_digest.map_or(Param::Null, Param::from),
                params.as_deref().map_or(Param::Null, Param::from),
                opened.ask_hash.map_or(Param::Null, Param::from),
                opened.selection_version.map_or(Param::Null, Param::Int),
                opened.reason.map_or(Param::Null, Param::from),
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
                    "batch_id",
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
    Ok(added)
}

/// Insert the cohort row, inside the caller's transaction; the caller has
/// checked the name is free and audits the making.
pub fn insert(
    store: &mut Store,
    name: &str,
    owner: &str,
    description: Option<&str>,
) -> Result<i64, StoreError> {
    let rows = store.insert(
        &Insert::new(
            table("cohort"),
            &["name", "owner", "description", "created_at"],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(name),
            Param::from(owner),
            description.map_or(Param::Null, Param::from),
            Param::from(now_iso()),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| StoreError::Message("the cohort was not written back".into()))?
        .int(0)
}

fn rolled_back<T>(registry: &mut Registry, e: impl Into<Error>) -> Result<T, Error> {
    registry.store().rollback().ok();
    Err(e.into())
}

/// Make a cohort (record 26 §9). Refused when a cohort or a selection is
/// named so, as a promotion refuses it. Audited as `cohort.create`.
pub fn create(
    registry: &mut Registry,
    name: &str,
    owner: &str,
    description: Option<&str>,
    actor: &str,
) -> Result<Cohort, Error> {
    let name = checked_name(name)?;
    // the epoch moves from what the store holds, not from a stale reading
    registry.refresh_meta()?;
    let store = registry.store();
    if by_name(store, name)?.is_some() || selection_named(store, name)? {
        return Err(Error::Taken(name.to_string()));
    }
    store.begin()?;
    let id = match insert(store, name, owner, description) {
        Ok(id) => id,
        Err(e) => return rolled_back(registry, e),
    };
    let recorded = audit::record(
        registry,
        &Entry {
            principal: actor,
            action: Action::CohortCreate,
            scope: json!({"cohort": name, "id": id}),
            policy: None,
            job_id: None,
            details: Some(json!({"by": "hand", "owner": owner})),
        },
    );
    if let Err(e) = recorded {
        return rolled_back(registry, e);
    }
    registry.store().commit()?;
    by_name(registry.store(), name)?.ok_or_else(|| Error::NotFound(name.to_string()))
}

/// What a `PUT` changes: a field not named keeps what it holds.
#[derive(Debug, Clone, Default)]
pub struct Change<'a> {
    pub name: Option<&'a str>,
    pub owner: Option<&'a str>,
    /// `Some(None)` clears the description.
    pub description: Option<Option<&'a str>>,
    pub retired: Option<bool>,
}

/// Rename, re-own, describe, retire or bring back a cohort (record 26 §9).
/// A rename edits one row and keeps the id: the releases and the selections
/// that recorded the old name keep it. Retiring keeps the members and the
/// history; the cohort leaves the lists the ask and the summary read.
/// Audited as `cohort.rename`, `cohort.set`, `cohort.retire` or
/// `cohort.restore`, one row per act.
pub fn set(
    registry: &mut Registry,
    name: &str,
    change: &Change<'_>,
    actor: &str,
) -> Result<Cohort, Error> {
    // the epoch moves from what the store holds, not from a stale reading
    registry.refresh_meta()?;
    let store = registry.store();
    let current = by_name(store, name)?.ok_or_else(|| Error::NotFound(name.to_string()))?;
    let new_name = match change.name {
        Some(n) if n != current.name => {
            let n = checked_name(n)?;
            if by_name(store, n)?.is_some() || selection_named(store, n)? {
                return Err(Error::Taken(n.to_string()));
            }
            Some(n)
        }
        _ => None,
    };
    let t = table("cohort");
    store.begin()?;
    let mut acts: Vec<(Action, Value)> = Vec::new();
    let result = (|| -> Result<(), StoreError> {
        if let Some(n) = new_name {
            store.update_by_id(t, &[("name", Param::from(n))], "id", current.id)?;
            acts.push((Action::CohortRename, json!({"from": current.name, "to": n})));
        }
        let mut set: Vec<(&str, Param)> = Vec::new();
        let mut detail = serde_json::Map::new();
        if let Some(o) = change.owner
            && o != current.owner
        {
            set.push(("owner", Param::from(o)));
            detail.insert("owner".into(), json!({"from": current.owner, "to": o}));
        }
        if let Some(d) = change.description
            && d != current.description.as_deref()
        {
            set.push(("description", d.map_or(Param::Null, Param::from)));
            detail.insert(
                "description".into(),
                json!({"from": current.description, "to": d}),
            );
        }
        if !set.is_empty() {
            store.update_by_id(t, &set, "id", current.id)?;
            acts.push((Action::CohortSet, Value::Object(detail)));
        }
        match (change.retired, current.retired_at.is_some()) {
            (Some(true), false) => {
                let now = now_iso();
                store.update_by_id(
                    t,
                    &[("retired_at", Param::from(now.as_str()))],
                    "id",
                    current.id,
                )?;
                acts.push((Action::CohortRetire, json!({"retired_at": now})));
            }
            (Some(false), true) => {
                store.update_by_id(t, &[("retired_at", Param::Null)], "id", current.id)?;
                acts.push((
                    Action::CohortRestore,
                    json!({"was_retired_at": current.retired_at}),
                ));
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(e) = result {
        return rolled_back(registry, e);
    }
    let final_name = new_name.unwrap_or(current.name.as_str());
    for (action, details) in &acts {
        let recorded = audit::record(
            registry,
            &Entry {
                principal: actor,
                action: *action,
                scope: json!({"cohort": final_name, "id": current.id}),
                policy: None,
                job_id: None,
                details: Some(details.clone()),
            },
        );
        if let Err(e) = recorded {
            return rolled_back(registry, e);
        }
    }
    registry.store().commit()?;
    by_name(registry.store(), final_name)?.ok_or_else(|| Error::NotFound(final_name.to_string()))
}

/// What a hand act on the members did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Members {
    /// Intervals opened.
    pub added: usize,
    /// Codes that were members already.
    pub already: usize,
    /// Intervals closed.
    pub removed: usize,
    /// Codes that were not members.
    pub not_members: usize,
}

/// Add and remove members by code, with a reason (record 26 §9): adding
/// opens intervals with `source = manual`, removing closes them with
/// `left_at`, `left_by` and the reason. A code the registry does not hold
/// is refused before anything is written. Audited as `cohort.member.add`
/// and `cohort.member.remove`, with counts.
pub fn members(
    registry: &mut Registry,
    name: &str,
    add: &[String],
    remove: &[String],
    why: Option<&str>,
    actor: &str,
) -> Result<Members, Error> {
    // the epoch moves from what the store holds, not from a stale reading
    registry.refresh_meta()?;
    let store = registry.store();
    let cohort = by_name(store, name)?.ok_or_else(|| Error::NotFound(name.to_string()))?;
    let mut all: Vec<String> = add.iter().chain(remove).cloned().collect();
    all.sort();
    all.dedup();
    let (found, unknown) = subjects_by_code(store, &all)?;
    if !unknown.is_empty() {
        return Err(Error::Unknown(unknown));
    }
    let adding: Vec<i64> = add
        .iter()
        .filter_map(|c| found.get(c.trim()).copied())
        .collect();
    let removing: Vec<i64> = remove
        .iter()
        .filter_map(|c| found.get(c.trim()).copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut out = Members::default();
    store.begin()?;
    let result = (|| -> Result<(), StoreError> {
        if !adding.is_empty() {
            let distinct: BTreeSet<i64> = adding.iter().copied().collect();
            out.added = join(
                store,
                cohort.id,
                &adding,
                actor,
                &Opened {
                    source: "manual",
                    reason: why,
                    ..Opened::default()
                },
            )?;
            out.already = distinct.len() - out.added;
        }
        if !removing.is_empty() {
            let open = open_members(store, cohort.id)?;
            let leaving: Vec<i64> = removing
                .iter()
                .copied()
                .filter(|s| open.contains(s))
                .collect();
            out.not_members = removing.len() - leaving.len();
            if !leaving.is_empty() {
                let d = store.dialect();
                let ids = leaving
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "UPDATE {} SET left_at = {}, left_by = {}, reason = {} \
                     WHERE cohort_id = {} AND left_at IS NULL AND subject_id IN ({ids})",
                    store.qualified("cohort_member"),
                    d.param(1, Type::Timestamp),
                    d.param(2, Type::Text),
                    d.param(3, Type::Text),
                    d.param(4, Type::Int),
                );
                out.removed = store.execute(
                    &sql,
                    &[
                        Param::from(now_iso()),
                        Param::from(actor),
                        why.map_or(Param::Null, Param::from),
                        Param::Int(cohort.id),
                    ],
                )? as usize;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        return rolled_back(registry, e);
    }
    let acts = [
        (
            Action::CohortMemberAdd,
            !adding.is_empty(),
            json!({"added": out.added, "already": out.already, "asked": adding.len()}),
        ),
        (
            Action::CohortMemberRemove,
            !removing.is_empty(),
            json!({"removed": out.removed, "not_members": out.not_members, "asked": removing.len()}),
        ),
    ];
    for (action, did, counts) in acts {
        if !did {
            continue;
        }
        let recorded = audit::record(
            registry,
            &Entry {
                principal: actor,
                action,
                scope: json!({"cohort": cohort.name, "id": cohort.id}),
                policy: None,
                job_id: None,
                details: Some(json!({"counts": counts, "why": why})),
            },
        );
        if let Err(e) = recorded {
            return rolled_back(registry, e);
        }
    }
    registry.store().commit()?;
    Ok(out)
}

/// What a digest's feeding did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fed {
    pub cohort: String,
    /// The cohort was made on this first use.
    pub created: bool,
    /// Intervals opened: the subjects the batch created, and those it met
    /// whose membership was not open.
    pub subjects: usize,
    /// Subjects the batch created or met, members already or not.
    pub met: usize,
}

/// The subjects a batch created, and those whose files it read.
fn subjects_of_batch(store: &mut Store, batch_id: i64) -> Result<Vec<i64>, StoreError> {
    let d = store.dialect();
    let created = format!(
        "SELECT id FROM {} WHERE first_batch_id = {}",
        store.qualified("subject"),
        d.param(1, Type::Int)
    );
    let met = format!(
        "SELECT DISTINCT se.subject_id FROM {} sf \
         JOIN {} i ON i.id = sf.instance_id \
         JOIN {} se ON se.id = i.series_id \
         WHERE sf.batch_id = {}",
        store.qualified("source_file"),
        store.qualified("instance"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    let mut out: BTreeSet<i64> = BTreeSet::new();
    for sql in [created, met] {
        for r in store.query(&sql, &[Param::Int(batch_id)])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out.into_iter().collect())
}

/// A digest of a dataset feeds its cohort (record 26 §8): every subject the
/// batch created joins, and so does one it met whose membership is not
/// open, so a returning person joins too. The cohort is made on first use,
/// owned by the actor, described as fed by the dataset. Audited as
/// `cohort.create` when made and `cohort.join` with the counts, under the
/// batch's job.
pub fn feed(
    registry: &mut Registry,
    name: &str,
    dataset: &str,
    batch_id: i64,
    job_id: Option<i64>,
    actor: &str,
) -> Result<Fed, Error> {
    let name = checked_name(name)?;
    // the epoch moves from what the store holds, not from a stale reading
    registry.refresh_meta()?;
    let store = registry.store();
    let subjects = subjects_of_batch(store, batch_id)?;
    let existing = by_name(store, name)?;
    if existing.is_none() && selection_named(store, name)? {
        return Err(Error::Taken(name.to_string()));
    }
    store.begin()?;
    let description = format!("fed by the dataset {dataset}");
    let (cohort_id, created) = match &existing {
        Some(c) => (c.id, false),
        None => match insert(store, name, actor, Some(&description)) {
            Ok(id) => (id, true),
            Err(e) => return rolled_back(registry, e),
        },
    };
    let opened = match join(
        store,
        cohort_id,
        &subjects,
        actor,
        &Opened {
            source: "digest",
            batch_id: Some(batch_id),
            ..Opened::default()
        },
    ) {
        Ok(n) => n,
        Err(e) => return rolled_back(registry, e),
    };
    if created {
        let recorded = audit::record(
            registry,
            &Entry {
                principal: actor,
                action: Action::CohortCreate,
                scope: json!({"cohort": name, "id": cohort_id}),
                policy: None,
                job_id,
                details: Some(json!({"by": "digest", "dataset": dataset, "batch": batch_id})),
            },
        );
        if let Err(e) = recorded {
            return rolled_back(registry, e);
        }
    }
    let recorded = audit::record(
        registry,
        &Entry {
            principal: actor,
            action: Action::CohortJoin,
            scope: json!({"cohort": name, "id": cohort_id, "batch": batch_id}),
            policy: None,
            job_id,
            details: Some(json!({"dataset": dataset, "subjects": opened, "met": subjects.len()})),
        },
    );
    if let Err(e) = recorded {
        return rolled_back(registry, e);
    }
    registry.store().commit()?;
    Ok(Fed {
        cohort: name.to_string(),
        created,
        subjects: opened,
        met: subjects.len(),
    })
}

/// One open review item and the subjects it is about, through its stack,
/// its series, its subject or, for a grouped one, its member stacks. An
/// item about a batch is about no subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenItem {
    pub id: i64,
    pub kind: String,
    pub subjects: Vec<i64>,
}

fn ids_in(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The subject of each stack, in chunks.
fn subjects_of_stacks(store: &mut Store, stacks: &[i64]) -> Result<HashMap<i64, i64>, StoreError> {
    let mut out = HashMap::new();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT k.id, se.subject_id FROM {} k JOIN {} se ON se.id = k.series_id WHERE k.id IN ({})",
            store.qualified("stack"),
            store.qualified("series"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?, r.int(1)?);
        }
    }
    Ok(out)
}

fn subjects_of_series(store: &mut Store, series: &[i64]) -> Result<HashMap<i64, i64>, StoreError> {
    let mut out = HashMap::new();
    for chunk in series.chunks(500) {
        let sql = format!(
            "SELECT id, subject_id FROM {} WHERE id IN ({})",
            store.qualified("series"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?, r.int(1)?);
        }
    }
    Ok(out)
}

/// Every open review item with the subjects it is about (record 26 §10).
pub fn open_items(store: &mut Store) -> Result<Vec<OpenItem>, StoreError> {
    items_about(store, Some("open"))
}

/// Every review item of a status, or of every status, with the subjects it
/// is about, newest first.
pub fn items_about(store: &mut Store, status: Option<&str>) -> Result<Vec<OpenItem>, StoreError> {
    let reference = text_of(store, "review_item", "ref");
    let d = store.dialect();
    let filter = match status {
        Some(_) => format!(" WHERE status = {}", d.param(1, Type::Text)),
        None => String::new(),
    };
    let params: Vec<Param> = status.map(Param::from).into_iter().collect();
    let sql = format!(
        "SELECT id, kind, scope, {reference} FROM {}{filter} ORDER BY id DESC",
        store.qualified("review_item")
    );
    struct Raw {
        id: i64,
        kind: String,
        scope: String,
        reference: Value,
    }
    let raw: Vec<Raw> = store
        .query(&sql, &params)?
        .iter()
        .map(|r| {
            Ok(Raw {
                id: r.int(0)?,
                kind: r.text(1)?.to_string(),
                scope: r.text(2)?.to_string(),
                reference: r
                    .opt_text(3)?
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or(Value::Null),
            })
        })
        .collect::<Result<_, StoreError>>()?;
    // the member stacks of the grouped items
    let grouped: Vec<i64> = raw
        .iter()
        .filter(|r| r.scope == "group")
        .map(|r| r.id)
        .collect();
    let mut members: HashMap<i64, Vec<i64>> = HashMap::new();
    for chunk in grouped.chunks(500) {
        let sql = format!(
            "SELECT item_id, stack_id FROM {} WHERE item_id IN ({})",
            store.qualified("review_member"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[])? {
            members.entry(r.int(0)?).or_default().push(r.int(1)?);
        }
    }
    let mut stacks: BTreeSet<i64> = BTreeSet::new();
    let mut series: BTreeSet<i64> = BTreeSet::new();
    for r in &raw {
        if let Some(s) = r.reference["stack_id"].as_i64() {
            stacks.insert(s);
        }
        if let Some(s) = r.reference["series_id"].as_i64() {
            series.insert(s);
        }
        if let Some(m) = members.get(&r.id) {
            stacks.extend(m.iter().copied());
        }
    }
    let stacks: Vec<i64> = stacks.into_iter().collect();
    let series: Vec<i64> = series.into_iter().collect();
    let by_stack = subjects_of_stacks(store, &stacks)?;
    let by_series = subjects_of_series(store, &series)?;
    Ok(raw
        .into_iter()
        .map(|r| {
            let mut subjects: BTreeSet<i64> = BTreeSet::new();
            if let Some(s) = r.reference["subject_id"].as_i64() {
                subjects.insert(s);
            }
            if let Some(s) = r.reference["stack_id"]
                .as_i64()
                .and_then(|k| by_stack.get(&k))
            {
                subjects.insert(*s);
            }
            if let Some(s) = r.reference["series_id"]
                .as_i64()
                .and_then(|k| by_series.get(&k))
            {
                subjects.insert(*s);
            }
            for k in members.get(&r.id).into_iter().flatten() {
                if let Some(s) = by_stack.get(k) {
                    subjects.insert(*s);
                }
            }
            OpenItem {
                id: r.id,
                kind: r.kind,
                subjects: subjects.into_iter().collect(),
            }
        })
        .collect())
}

/// The batches that fed a cohort (record 26 §10): the batches its digest
/// intervals name, and every batch of a source whose dataset feeds it. A
/// quarantined file is about no subject, so the quarantine filters by
/// these. None when there is no cohort so named.
pub fn batches_feeding(store: &mut Store, name: &str) -> Result<Option<Vec<i64>>, StoreError> {
    let Some(c) = by_name(store, name)? else {
        return Ok(None);
    };
    let d = store.dialect();
    let mut out: BTreeSet<i64> = BTreeSet::new();
    let sql = format!(
        "SELECT DISTINCT batch_id FROM {} WHERE cohort_id = {} AND batch_id IS NOT NULL",
        store.qualified("cohort_member"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(c.id)])? {
        out.insert(r.int(0)?);
    }
    let places = place::active(store)?;
    let feeding = feeds_of(&places, name);
    if !feeding.is_empty() {
        let roots: Vec<(String, std::path::PathBuf)> = places
            .iter()
            .filter(|p| feeding.contains(&p.name))
            .filter_map(|p| p.tree_path("anon").map(|t| (p.name.clone(), t)))
            .collect();
        let sql = format!(
            "SELECT b.id, so.root_canonical FROM {} b JOIN {} so ON so.id = b.source_id",
            store.qualified("ingest_batch"),
            store.qualified("source")
        );
        for r in store.query(&sql, &[])? {
            let root = std::path::Path::new(r.text(1)?);
            if roots.iter().any(|(_, t)| {
                let real = std::fs::canonicalize(t).unwrap_or_else(|_| t.clone());
                root.starts_with(&real)
            }) {
                out.insert(r.int(0)?);
            }
        }
    }
    Ok(Some(out.into_iter().collect()))
}

/// The open memberships of every cohort in force: name to subjects.
pub fn open_by_cohort(store: &mut Store) -> Result<BTreeMap<String, BTreeSet<i64>>, StoreError> {
    let sql = format!(
        "SELECT c.name, m.subject_id FROM {} m JOIN {} c ON c.id = m.cohort_id \
         WHERE m.left_at IS NULL AND c.retired_at IS NULL ORDER BY c.name",
        store.qualified("cohort_member"),
        store.qualified("cohort")
    );
    let mut out: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for r in store.query(&sql, &[])? {
        out.entry(r.text(0)?.to_string())
            .or_default()
            .insert(r.int(1)?);
    }
    Ok(out)
}

/// The review summary (record 26 §10): the open items by kind, each cohort
/// in force with its open items, and the open items whose subject is in no
/// cohort, those about no subject among them. With a cohort named, the
/// kinds count that cohort's items only.
pub fn review_summary(store: &mut Store, cohort: Option<&str>) -> Result<Value, StoreError> {
    let items = open_items(store)?;
    let by_cohort = open_by_cohort(store)?;
    let in_any: HashSet<i64> = by_cohort.values().flatten().copied().collect();
    let held =
        |members: &BTreeSet<i64>, it: &OpenItem| it.subjects.iter().any(|s| members.contains(s));
    let chosen: Option<&BTreeSet<i64>> = cohort.and_then(|c| by_cohort.get(c));
    let mut by_kind: BTreeMap<String, i64> = BTreeMap::new();
    let mut none = 0i64;
    for it in &items {
        if let Some(members) = chosen
            && !held(members, it)
        {
            continue;
        }
        if cohort.is_some() && chosen.is_none() {
            continue;
        }
        *by_kind.entry(it.kind.clone()).or_insert(0) += 1;
    }
    for it in &items {
        if !it.subjects.iter().any(|s| in_any.contains(s)) {
            none += 1;
        }
    }
    let cohorts: Vec<Value> = by_cohort
        .iter()
        .map(|(name, members)| {
            let open = items.iter().filter(|it| held(members, it)).count();
            json!({"name": name, "open": open})
        })
        .collect();
    Ok(json!({"by_kind": by_kind, "cohorts": cohorts, "none": none}))
}

/// The place names whose dataset feeds the cohort.
fn feeds_of(places: &[place::Place], name: &str) -> Vec<String> {
    places
        .iter()
        .filter(|p| p.role == Role::Source && p.dataset["cohort"].as_str() == Some(name))
        .map(|p| p.name.clone())
        .collect()
}

/// The source place whose pseudonymised tree holds a digest root, by name;
/// cached per root since a cohort's members come from few sources.
fn place_of_root(
    store: &mut Store,
    cache: &mut HashMap<String, Option<String>>,
    root: &str,
) -> Result<Option<String>, StoreError> {
    if let Some(found) = cache.get(root) {
        return Ok(found.clone());
    }
    let found = place::tree_holding(store, "anon", std::path::Path::new(root))?.map(|p| p.name);
    cache.insert(root.to_string(), found.clone());
    Ok(found)
}

/// The releases whose selection named the cohort, newest first.
fn releases_naming(store: &mut Store, name: &str) -> Result<Vec<Value>, StoreError> {
    let selection = text_of(store, "release", "selection");
    let sql = format!(
        "SELECT r.id, r.name, r.version, r.layout, r.subjects, {selection}, \
                (SELECT COUNT(*) FROM {} h WHERE h.release_id = r.id) \
         FROM {} r WHERE r.finished_at IS NOT NULL AND r.withdrawn_at IS NULL ORDER BY r.id DESC",
        store.qualified("handover"),
        store.qualified("release")
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        let selection: Value = r
            .opt_text(5)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        let named = selection["cohorts"]
            .as_array()
            .is_some_and(|a| a.iter().any(|c| c.as_str() == Some(name)));
        if !named {
            continue;
        }
        out.push(json!({
            "id": r.int(0)?,
            "name": r.text(1)?,
            "version": r.text(2)?,
            "layout": r.opt_text(3)?,
            "subjects": r.opt_int(4)?,
            "handed_over": r.int(6)? > 0,
        }));
    }
    Ok(out)
}

/// One interval as read, for the provenance and the joins.
struct Interval {
    subject: i64,
    source: String,
    batch: Option<i64>,
    handle: Option<i64>,
    actor: Option<String>,
    joined_at: String,
    left_at: Option<String>,
    left_by: Option<String>,
    reason: Option<String>,
}

fn intervals(store: &mut Store, cohort_id: i64) -> Result<Vec<Interval>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT subject_id, source, batch_id, handle_id, actor, {}, {}, left_by, reason \
         FROM {} WHERE cohort_id = {} ORDER BY id",
        text_of(store, "cohort_member", "joined_at"),
        text_of(store, "cohort_member", "left_at"),
        store.qualified("cohort_member"),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(cohort_id)])?
        .iter()
        .map(|r| {
            Ok(Interval {
                subject: r.int(0)?,
                source: r.opt_text(1)?.unwrap_or("import").to_string(),
                batch: r.opt_int(2)?,
                handle: r.opt_int(3)?,
                actor: r.opt_text(4)?.map(str::to_string),
                joined_at: r.opt_text(5)?.unwrap_or_default().to_string(),
                left_at: r.opt_text(6)?.map(str::to_string),
                left_by: r.opt_text(7)?.map(str::to_string),
                reason: r.opt_text(8)?.map(str::to_string),
            })
        })
        .collect()
}

/// The dataset a batch read, by the source place holding its root.
fn dataset_of_batch(
    store: &mut Store,
    cache: &mut HashMap<String, Option<String>>,
    batch: i64,
) -> Result<Option<String>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT so.root_canonical FROM {} b JOIN {} so ON so.id = b.source_id WHERE b.id = {}",
        store.qualified("ingest_batch"),
        store.qualified("source"),
        d.param(1, Type::Int)
    );
    let Some(root) = store
        .query_opt(&sql, &[Param::Int(batch)])?
        .map(|r| r.text(0).map(str::to_string))
        .transpose()?
    else {
        return Ok(None);
    };
    place_of_root(store, cache, &root)
}

/// Where the cohort came from: what opened its first interval.
fn from_of(
    store: &mut Store,
    cache: &mut HashMap<String, Option<String>>,
    cohort: &Cohort,
    first: Option<&Interval>,
) -> Result<Value, StoreError> {
    Ok(match first {
        None => json!({"kind": "manual", "detail": {"actor": cohort.owner}}),
        Some(i) => match i.source.as_str() {
            "digest" => {
                let dataset = match i.batch {
                    Some(b) => dataset_of_batch(store, cache, b)?,
                    None => None,
                };
                json!({"kind": "source", "detail": {"dataset": dataset, "batch": i.batch}})
            }
            "promotion" => {
                json!({"kind": "promotion", "detail": {"handle": i.handle, "actor": i.actor}})
            }
            "manual" => json!({"kind": "manual", "detail": {"actor": i.actor}}),
            _ => json!({"kind": "import", "detail": {"actor": i.actor}}),
        },
    })
}

/// The joins and the removals, grouped by what did them, newest first: a
/// digest's by batch, a promotion's by handle, a hand's by act.
fn joins_of(intervals: &[Interval]) -> Vec<Value> {
    #[derive(Default)]
    struct Group {
        when: String,
        what: String,
        subjects: BTreeSet<i64>,
        by: Option<String>,
        batch: Option<i64>,
        handle: Option<i64>,
        reason: Option<String>,
        open: usize,
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for i in intervals {
        let key = match i.source.as_str() {
            "digest" => format!("digest:{}", i.batch.unwrap_or(0)),
            "promotion" => format!("promotion:{}", i.handle.unwrap_or(0)),
            other => format!(
                "{other}:{}:{}",
                i.joined_at,
                i.actor.as_deref().unwrap_or("")
            ),
        };
        let g = groups.entry(key).or_default();
        if g.when.is_empty() || i.joined_at < g.when {
            g.when = i.joined_at.clone();
        }
        g.what = i.source.clone();
        g.subjects.insert(i.subject);
        g.by = i.actor.clone();
        g.batch = i.batch;
        g.handle = i.handle;
        if g.reason.is_none() {
            g.reason = i.reason.clone();
        }
        if i.left_at.is_none() {
            g.open += 1;
        }
        if let (Some(at), Some(by)) = (&i.left_at, &i.left_by) {
            let r = groups.entry(format!("remove:{at}:{by}")).or_default();
            r.when = at.clone();
            r.what = "remove".to_string();
            r.subjects.insert(i.subject);
            r.by = Some(by.clone());
            if r.reason.is_none() {
                r.reason = i.reason.clone();
            }
        }
    }
    let mut out: Vec<Group> = groups.into_values().collect();
    // newest first; at one stamp a removal before the join it closed
    out.sort_by(|a, b| {
        b.when
            .cmp(&a.when)
            .then_with(|| (b.what == "remove").cmp(&(a.what == "remove")))
    });
    out.into_iter()
        .map(|g| {
            let mut doc = json!({
                "when": g.when,
                "what": g.what,
                "subjects": g.subjects.len(),
                "by": g.by,
                "reason": g.reason,
            });
            match g.what.as_str() {
                "digest" => doc["batch"] = json!(g.batch),
                "promotion" => doc["handle"] = json!(g.handle),
                "remove" => {}
                _ => doc["actor"] = json!(g.by),
            }
            if g.what != "remove" {
                doc["left"] = json!(g.open == 0);
            }
            doc
        })
        .collect()
}

/// The counts of a cohort's open members: subjects, sessions under the
/// default window, stacks.
fn counts_of(store: &mut Store, cohort_id: i64) -> Result<(i64, i64, i64), StoreError> {
    let d = store.dialect();
    let member = store.qualified("cohort_member");
    let current = format!(
        "{member} m WHERE m.cohort_id = {} AND m.left_at IS NULL",
        d.param(1, Type::Int)
    );
    let subjects = store.query(
        &format!("SELECT COUNT(DISTINCT m.subject_id) FROM {current}"),
        &[Param::Int(cohort_id)],
    )?[0]
        .int(0)?;
    let sessions = store.query(
        &format!(
            "SELECT COUNT(DISTINCT sc.id) FROM {} sc WHERE sc.window_days = {} \
                 AND sc.subject_id IN (SELECT m.subject_id FROM {current})",
            store.qualified("session_cache"),
            d.param(2, Type::Int),
        ),
        &[
            Param::Int(cohort_id),
            Param::Int(Scheme::default().window_days),
        ],
    )?[0]
        .int(0)?;
    let stacks = store.query(
        &format!(
            "SELECT COUNT(DISTINCT st.id) FROM {} st JOIN {} se ON se.id = st.series_id \
                 WHERE se.subject_id IN (SELECT m.subject_id FROM {current})",
            store.qualified("stack"),
            store.qualified("series"),
        ),
        &[Param::Int(cohort_id)],
    )?[0]
        .int(0)?;
    Ok((subjects, sessions, stacks))
}

/// The sources holding the members' files, by the place of each member's
/// first batch; members from no place under `null`.
fn sources_holding(
    store: &mut Store,
    cache: &mut HashMap<String, Option<String>>,
    cohort_id: i64,
) -> Result<Vec<Value>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT so.root_canonical, COUNT(DISTINCT m.subject_id) FROM {} m \
         JOIN {} s ON s.id = m.subject_id \
         LEFT JOIN {} b ON b.id = s.first_batch_id \
         LEFT JOIN {} so ON so.id = b.source_id \
         WHERE m.cohort_id = {} AND m.left_at IS NULL GROUP BY so.root_canonical",
        store.qualified("cohort_member"),
        store.qualified("subject"),
        store.qualified("ingest_batch"),
        store.qualified("source"),
        d.param(1, Type::Int)
    );
    let rows: Vec<(Option<String>, i64)> = store
        .query(&sql, &[Param::Int(cohort_id)])?
        .iter()
        .map(|r| Ok((r.opt_text(0)?.map(str::to_string), r.int(1)?)))
        .collect::<Result<_, StoreError>>()?;
    let mut by_place: BTreeMap<Option<String>, i64> = BTreeMap::new();
    for (root, n) in rows {
        let place = match root {
            Some(r) => place_of_root(store, cache, &r)?,
            None => None,
        };
        *by_place.entry(place).or_insert(0) += n;
    }
    Ok(by_place
        .into_iter()
        .map(|(place, subjects)| json!({"place": place, "subjects": subjects}))
        .collect())
}

/// One cohort as the list door shows it, with the detail when asked.
fn document(
    store: &mut Store,
    cache: &mut HashMap<String, Option<String>>,
    places: &[place::Place],
    items: &[OpenItem],
    cohort: &Cohort,
    detail: bool,
) -> Result<Value, StoreError> {
    let (subjects, sessions, stacks) = counts_of(store, cohort.id)?;
    let all = intervals(store, cohort.id)?;
    let open: HashSet<i64> = all
        .iter()
        .filter(|i| i.left_at.is_none())
        .map(|i| i.subject)
        .collect();
    let waiting = items
        .iter()
        .filter(|it| it.subjects.iter().any(|s| open.contains(s)))
        .count();
    let releases = releases_naming(store, &cohort.name)?;
    let from = from_of(store, cache, cohort, all.first())?;
    let last_joined = all.iter().map(|i| i.joined_at.as_str()).max();
    let mut doc = json!({
        "id": cohort.id,
        "name": cohort.name,
        "owner": cohort.owner,
        "description": cohort.description,
        "subjects": subjects,
        "sessions": sessions,
        "stacks": stacks,
        "feeds": feeds_of(places, &cohort.name),
        "from": from,
        "waiting": waiting,
        "releases": releases.len(),
        "created_at": cohort.created_at,
        "last_joined": last_joined,
        "retired_at": cohort.retired_at,
    });
    if detail {
        doc["joins"] = Value::Array(joins_of(&all));
        doc["sources_holding"] = Value::Array(sources_holding(store, cache, cohort.id)?);
        doc["releases"] = Value::Array(releases);
    }
    Ok(doc)
}

/// Every cohort with its counts and provenance (`GET /api/cohorts`),
/// retired ones included with `retired_at` set.
pub fn list(store: &mut Store) -> Result<Vec<Value>, StoreError> {
    let places = place::active(store)?;
    let items = open_items(store)?;
    let mut cache = HashMap::new();
    all(store)?
        .iter()
        .map(|c| document(store, &mut cache, &places, &items, c, false))
        .collect()
}

/// One cohort in full (`GET /api/cohorts/{name}`): the list's fields, the
/// joins, the sources holding the members and the releases naming it.
pub fn show(store: &mut Store, name: &str) -> Result<Option<Value>, StoreError> {
    let Some(c) = by_name(store, name)? else {
        return Ok(None);
    };
    let places = place::active(store)?;
    let items = open_items(store)?;
    let mut cache = HashMap::new();
    Ok(Some(document(
        store, &mut cache, &places, &items, &c, true,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_word() {
        assert!(checked_name("ms-2026").is_ok());
        assert!(checked_name(" study-a ").is_ok());
        assert!(checked_name("").is_err());
        assert!(checked_name("@x").is_err());
        assert!(checked_name("a b").is_err());
        assert!(checked_name("a/b").is_err());
    }

    #[test]
    fn joins_group_a_digest_by_batch_and_a_hand_by_act_and_say_who_left() {
        let at = |s: &str| s.to_string();
        let mk = |subject, source: &str, batch, joined: &str, left: Option<&str>| Interval {
            subject,
            source: source.to_string(),
            batch,
            handle: None,
            actor: Some("anna@lab".into()),
            joined_at: at(joined),
            left_at: left.map(at),
            left_by: left.map(|_| "bo@lab".to_string()),
            reason: left.map(|_| "moved away".to_string()),
        };
        let rows = vec![
            mk(1, "digest", Some(7), "2026-09-01T10:00:00Z", None),
            mk(
                2,
                "digest",
                Some(7),
                "2026-09-01T10:00:00Z",
                Some("2026-09-03T10:00:00Z"),
            ),
            mk(3, "manual", None, "2026-09-02T10:00:00Z", None),
        ];
        let joins = joins_of(&rows);
        assert_eq!(joins.len(), 3, "{joins:?}");
        // newest first: the removal, the hand act, the digest
        assert_eq!(joins[0]["what"], "remove");
        assert_eq!(joins[0]["subjects"], 1);
        assert_eq!(joins[0]["by"], "bo@lab");
        assert_eq!(joins[1]["what"], "manual");
        assert_eq!(joins[1]["left"], false);
        assert_eq!(joins[2]["what"], "digest");
        assert_eq!(joins[2]["batch"], 7);
        assert_eq!(joins[2]["subjects"], 2);
        assert_eq!(joins[2]["left"], false);
    }
}
