// SPDX-License-Identifier: AGPL-3.0-only

//! A dataset's or a cohort's own main-scan rules (record 55, decision 6 of
//! 2026-10-10; the main-scans study, section 4).
//!
//! A pick belongs to a **scope**: the pack's (everything, made with version
//! 1 of the rules, the pack's defaults), a dataset's or a cohort's. A
//! dataset or a cohort with rules saved gets picks of its own; without, it
//! reads the pack's. The rules are versioned here, one row per version,
//! never changed or deleted: version 1 is never stored, and a save names the
//! version it was based on, so that two people saving at once do not
//! overwrite each other.
//!
//! What the rules say is `nils_pack::pick_rules`'s; this keeps them. Which
//! picks a reader reads is `crate::pick_truth`'s.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};
use crate::time::now_iso;

/// Which kind of scope keeps rules of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScopeKind {
    Dataset,
    Cohort,
}

impl ScopeKind {
    pub fn name(self) -> &'static str {
        match self {
            ScopeKind::Dataset => "dataset",
            ScopeKind::Cohort => "cohort",
        }
    }

    pub fn parse(word: &str) -> Option<ScopeKind> {
        match word {
            "dataset" => Some(ScopeKind::Dataset),
            "cohort" => Some(ScopeKind::Cohort),
            _ => None,
        }
    }
}

/// A dataset (a source place) or a cohort, by its id, with its name for
/// what a person reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub kind: ScopeKind,
    pub id: i64,
    pub name: String,
}

impl Scope {
    /// The cohort of that name, retired or not.
    pub fn cohort(store: &mut Store, name: &str) -> Result<Option<Scope>, StoreError> {
        Ok(crate::cohort::by_name(store, name)?.map(|c| Scope {
            kind: ScopeKind::Cohort,
            id: c.id,
            name: c.name,
        }))
    }

    /// The dataset of that name, or of that id: a source place in force.
    pub fn dataset(store: &mut Store, name: &str) -> Result<Option<Scope>, StoreError> {
        let name = name.trim_start_matches('@');
        let mut found = crate::place::by_name(store, name)?;
        if found.is_none()
            && let Ok(id) = name.parse::<i64>()
        {
            found = crate::place::show(store, id)?;
        }
        Ok(found
            .filter(|p| p.role == crate::place::Role::Source && p.retired_at.is_none())
            .map(|p| Scope {
                kind: ScopeKind::Dataset,
                id: p.id,
                name: p.name,
            }))
    }

    /// The scope of a kind and an id, with its name, where it still exists.
    pub fn of(store: &mut Store, kind: ScopeKind, id: i64) -> Result<Option<Scope>, StoreError> {
        let name = match kind {
            ScopeKind::Cohort => crate::cohort::all(store)?
                .into_iter()
                .find(|c| c.id == id)
                .map(|c| c.name),
            ScopeKind::Dataset => crate::place::show(store, id)?.map(|p| p.name),
        };
        Ok(name.map(|name| Scope { kind, id, name }))
    }

    /// How a run's report and a job name it: `cohort:NAME`, `dataset:NAME`.
    pub fn label(&self) -> String {
        format!("{}:{}", self.kind.name(), self.name)
    }

    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind.name(), "id": self.id, "name": self.name})
    }
}

/// The stacks a scope holds where it holds some and not every stack of its
/// subjects: a dataset's, those its `source` rows hold a file of (Nima,
/// 2026-10-12: a dataset holds stacks, never a subject's membership); none
/// for a cohort, which holds every stack of its open members. Its pick run
/// chooses among these, and its map reads them.
pub fn held_stacks(store: &mut Store, scope: &Scope) -> Result<Option<BTreeSet<i64>>, StoreError> {
    if scope.kind == ScopeKind::Cohort {
        return Ok(None);
    }
    let sources = crate::place::dataset_sources(store)?
        .into_iter()
        .find(|d| d.id == scope.id)
        .map(|d| d.sources)
        .unwrap_or_default();
    if sources.is_empty() {
        return Ok(Some(BTreeSet::new()));
    }
    let sql = format!(
        "SELECT DISTINCT stack_id FROM {} WHERE source_id IN ({})",
        store.qualified("source_stack"),
        sources
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<BTreeSet<i64>, StoreError>>()
        .map(Some)
}

/// One saved version of a scope's rules.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub version: i64,
    pub rules: Value,
    pub digest: String,
    pub reason: String,
    pub author: String,
    pub created_at: String,
}

impl Saved {
    pub fn to_json(&self) -> Value {
        json!({
            "version": self.version, "saved": true, "reason": self.reason,
            "author": self.author, "at": self.created_at, "digest": self.digest,
            "rules": self.rules,
        })
    }
}

/// The digest of a rules document as it is stored: its JSON text, which
/// the rules write in a fixed order.
pub fn digest(rules: &Value) -> String {
    use blake2::digest::consts::U16;
    use blake2::{Blake2b, Digest};
    let mut hasher = Blake2b::<U16>::new();
    hasher.update(rules.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

/// Why a version was not saved.
#[derive(Debug)]
pub enum SaveError {
    /// It was based on a version that is not the latest any more: someone
    /// saved since.
    Behind {
        latest: i64,
    },
    /// It says no why.
    NoReason,
    Store(StoreError),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Behind { latest } => write!(
                f,
                "the rules were changed since; the latest version is {latest}, so look again and save on it"
            ),
            SaveError::NoReason => write!(f, "a version of the rules says why it was made"),
            SaveError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<StoreError> for SaveError {
    fn from(e: StoreError) -> Self {
        SaveError::Store(e)
    }
}

fn select(store: &Store, filter: &str) -> String {
    let t = table("pick_rules");
    let d = store.dialect();
    format!(
        "SELECT version, {}, digest, reason, author, {} FROM {} WHERE scope_kind = {} AND scope_id = {}{filter}",
        d.text_of(t.column("rules").expect("pick_rules.rules")),
        d.text_of(t.column("created_at").expect("pick_rules.created_at")),
        store.qualified("pick_rules"),
        d.param(1, Type::Text),
        d.param(2, Type::Int),
    )
}

fn saved_of(r: &Row) -> Result<Saved, StoreError> {
    Ok(Saved {
        version: r.int(0)?,
        rules: r
            .opt_text(1)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null),
        digest: r.text(2)?.to_string(),
        reason: r.text(3)?.to_string(),
        author: r.text(4)?.to_string(),
        created_at: r.opt_text(5)?.unwrap_or_default().to_string(),
    })
}

fn key(scope: &Scope) -> [Param; 2] {
    [Param::from(scope.kind.name()), Param::Int(scope.id)]
}

/// Every saved version of a scope's rules, the newest first.
pub fn versions(store: &mut Store, scope: &Scope) -> Result<Vec<Saved>, StoreError> {
    let sql = select(store, " ORDER BY version DESC");
    store
        .query(&sql, &key(scope))?
        .iter()
        .map(saved_of)
        .collect()
}

/// The latest saved version of a scope's rules; none while the scope reads
/// the pack's.
pub fn latest(store: &mut Store, scope: &Scope) -> Result<Option<Saved>, StoreError> {
    let sql = select(store, " ORDER BY version DESC LIMIT 1");
    store
        .query_opt(&sql, &key(scope))?
        .map(|r| saved_of(&r))
        .transpose()
}

/// One saved version.
pub fn version(store: &mut Store, scope: &Scope, n: i64) -> Result<Option<Saved>, StoreError> {
    let d = store.dialect();
    let sql = select(store, &format!(" AND version = {}", d.param(3, Type::Int)));
    let [a, b] = key(scope);
    store
        .query_opt(&sql, &[a, b, Param::Int(n)])?
        .map(|r| saved_of(&r))
        .transpose()
}

/// Save a new version of a scope's rules, `based_on + 1`, where `based_on`
/// is the latest (1 while nothing is saved). The rules are stored as they
/// are given, already checked by the caller.
pub fn save(
    store: &mut Store,
    scope: &Scope,
    rules: &Value,
    reason: &str,
    author: &str,
    based_on: i64,
) -> Result<i64, SaveError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(SaveError::NoReason);
    }
    let latest_now = latest(store, scope)?.map_or(1, |s| s.version);
    if based_on != latest_now {
        return Err(SaveError::Behind { latest: latest_now });
    }
    let version = based_on + 1;
    let written = store.insert(
        &Insert::new(
            table("pick_rules"),
            &[
                "scope_kind",
                "scope_id",
                "version",
                "rules",
                "digest",
                "reason",
                "author",
                "created_at",
            ],
        ),
        &[vec![
            Param::from(scope.kind.name()),
            Param::Int(scope.id),
            Param::Int(version),
            Param::from(rules.to_string()),
            Param::from(digest(rules)),
            Param::from(reason),
            Param::from(author),
            Param::from(now_iso()),
        ]],
    );
    match written {
        Ok(_) => Ok(version),
        // two saves on one version: the unique key let one in
        Err(e) => match latest(store, scope)? {
            Some(s) if s.version >= version => Err(SaveError::Behind { latest: s.version }),
            _ => Err(SaveError::Store(e)),
        },
    }
}

/// Every scope with rules saved, by kind and id.
pub fn scopes(store: &mut Store) -> Result<Vec<(ScopeKind, i64)>, StoreError> {
    let sql = format!(
        "SELECT DISTINCT scope_kind, scope_id FROM {} ORDER BY scope_kind, scope_id",
        store.qualified("pick_rules")
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        if let Some(kind) = ScopeKind::parse(r.text(0)?) {
            out.push((kind, r.int(1)?));
        }
    }
    Ok(out)
}

/// What a reader of a scope's picks reads: the scope and the version of
/// its rules whose run finished over all its subjects, and that run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub scope: Scope,
    pub version: i64,
    pub job: Option<i64>,
    pub at: String,
}

impl Reading {
    pub fn to_json(&self) -> Value {
        json!({"scope": self.scope.to_json(), "version": self.version, "run": self.job, "at": self.at})
    }
}

/// The scope whose picks a dataset or a cohort reads: its own once a run
/// of a saved version finished over all its subjects, the latest such
/// version; else none, the pack's. A version saved since, whose run is
/// queued, running or failed, is not read until its run finishes.
pub fn reading(store: &mut Store, scope: &Scope) -> Result<Option<Reading>, StoreError> {
    let t = table("pick_rules_run");
    let d = store.dialect();
    let sql = format!(
        "SELECT version, job_id, {} FROM {} WHERE scope_kind = {} AND scope_id = {} \
           AND state = 'done' AND whole = 1 ORDER BY version DESC, id DESC LIMIT 1",
        d.text_of(t.column("finished_at").expect("pick_rules_run.finished_at")),
        store.qualified("pick_rules_run"),
        d.param(1, Type::Text),
        d.param(2, Type::Int),
    );
    Ok(match store.query_opt(&sql, &key(scope))? {
        None => None,
        Some(r) => Some(Reading {
            scope: scope.clone(),
            version: r.int(0)?,
            job: r.opt_int(1)?,
            at: r.opt_text(2)?.unwrap_or_default().to_string(),
        }),
    })
}

/// A run of a scope's version begins: answers its row.
pub fn run_started(
    store: &mut Store,
    scope: &Scope,
    version: i64,
    job: Option<i64>,
    whole: bool,
) -> Result<i64, StoreError> {
    let written = store.insert(
        &Insert::new(
            table("pick_rules_run"),
            &[
                "scope_kind",
                "scope_id",
                "version",
                "job_id",
                "state",
                "whole",
                "started_at",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(scope.kind.name()),
            Param::Int(scope.id),
            Param::Int(version),
            job.map_or(Param::Null, Param::Int),
            Param::from("running"),
            Param::Int(i64::from(whole)),
            Param::from(now_iso()),
        ]],
    )?;
    written
        .first()
        .map(|r| r.int(0))
        .transpose()?
        .ok_or_else(|| StoreError::Message("the run was not written back".into()))
}

/// A run of a scope's version ended, done or failed with why.
pub fn run_ended(store: &mut Store, row: i64, error: Option<&str>) -> Result<(), StoreError> {
    store.update_by_id(
        table("pick_rules_run"),
        &[
            (
                "state",
                Param::from(if error.is_some() { "failed" } else { "done" }),
            ),
            ("error", error.map_or(Param::Null, Param::from)),
            ("finished_at", Param::from(now_iso())),
        ],
        "id",
        row,
    )?;
    Ok(())
}

/// What the rules door says of a scope's picks: the version they are read
/// from (`picks_from`, null while the scope reads the pack's) and the
/// latest saved version not read yet with the state of its run (`pending`:
/// `queued`, `running` or `failed`, null when none is).
pub fn state(store: &mut Store, scope: &Scope) -> Result<Value, StoreError> {
    let read = reading(store, scope)?;
    let latest = latest(store, scope)?.map(|s| s.version);
    let pending = match latest {
        Some(v) if read.as_ref().is_none_or(|r| r.version < v) => {
            let d = store.dialect();
            let sql = format!(
                "SELECT state, job_id, error FROM {} WHERE scope_kind = {} AND scope_id = {} \
                   AND version = {} AND whole = 1 ORDER BY id DESC LIMIT 1",
                store.qualified("pick_rules_run"),
                d.param(1, Type::Text),
                d.param(2, Type::Int),
                d.param(3, Type::Int),
            );
            let [a, b] = key(scope);
            match store.query_opt(&sql, &[a, b, Param::Int(v)])? {
                Some(r) => json!({
                    "version": v, "state": r.text(0)?, "run": r.opt_int(1)?, "error": r.opt_text(2)?,
                }),
                None => json!({"version": v, "state": "queued", "run": null, "error": null}),
            }
        }
        _ => Value::Null,
    };
    Ok(json!({
        "picks_from": read.as_ref().map(Reading::to_json),
        "pending": pending,
    }))
}

/// Remove a version just saved, when what it was saved for (its run) could
/// not be queued: the one deletion of a rules row, made at once.
pub fn unsave(store: &mut Store, scope: &Scope, version: i64) -> Result<(), StoreError> {
    let d = store.dialect();
    let sql = format!(
        "DELETE FROM {} WHERE scope_kind = {} AND scope_id = {} AND version = {}",
        store.qualified("pick_rules"),
        d.param(1, Type::Text),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
    );
    let [a, b] = key(scope);
    store.execute(&sql, &[a, b, Param::Int(version)])?;
    Ok(())
}

/// The marker a person's pick made for a scope without rules carries in
/// its `parts`, so that the scope's first save takes it in.
pub fn made_in(scope: &Scope) -> String {
    format!("{}:{}", scope.kind.name(), scope.id)
}

/// A scope's first version saved: the persons' picks made for it while it
/// read the pack's (pack picks marked [`made_in`] it, standing) are copied
/// into the scope, where they stand from now. Answers how many.
pub fn carry_person_picks(store: &mut Store, scope: &Scope) -> Result<i64, StoreError> {
    let t = table("pick");
    let d = store.dialect();
    let parts = d.text_of(t.column("parts").expect("pick.parts"));
    let marker = format!("%\"made_in\":\"{}\"%", made_in(scope));
    let sql = format!(
        "SELECT id FROM {} WHERE author_kind = 'person' AND withdrawn_at IS NULL \
           AND rules_scope_kind IS NULL AND {parts} LIKE {} ORDER BY id",
        store.qualified("pick"),
        d.param(1, Type::Text),
    );
    let ids: Vec<i64> = store
        .query(&sql, &[Param::from(marker)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    let columns = "model, role, subject_id, session_day, scheme, scheme_digest, reference, pack, \
                   pack_version, actor, author_kind, decided_at, why, parts, campaign_id";
    for id in &ids {
        let written = store.query(
            &format!(
                "INSERT INTO {p} ({columns}, rules_scope_kind, rules_scope_id) \
                 SELECT {columns}, {}, {} FROM {p} WHERE id = {} RETURNING id",
                d.param(1, Type::Text),
                d.param(2, Type::Int),
                d.param(3, Type::Int),
                p = store.qualified("pick"),
            ),
            &[
                Param::from(scope.kind.name()),
                Param::Int(scope.id),
                Param::Int(*id),
            ],
        )?;
        let new = written[0].int(0)?;
        store.execute(
            &format!(
                "INSERT INTO {ps} (pick_id, stack_id) SELECT {}, stack_id FROM {ps} WHERE pick_id = {}",
                d.param(1, Type::Int),
                d.param(2, Type::Int),
                ps = store.qualified("pick_stack"),
            ),
            &[Param::Int(new), Param::Int(*id)],
        )?;
    }
    Ok(ids.len() as i64)
}

/// The run picks of a scope among `pick` rows under `alias`, any version,
/// as a condition: the pack's are those that name no scope. For a run,
/// which writes and replaces its own scope's; a reader reads the main
/// scans through [`crate::pick_truth::Of`].
pub fn pick_filter(alias: &str, scope: Option<&Scope>) -> String {
    let a = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    match scope {
        None => format!("{a}rules_scope_kind IS NULL"),
        Some(s) => format!(
            "{a}rules_scope_kind = '{}' AND {a}rules_scope_id = {}",
            s.kind.name(),
            s.id
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let mut s = Store::sqlite_in_memory().unwrap();
        crate::migrate::migrate(&mut s, crate::migrate::Kind::Registry).unwrap();
        s
    }

    #[test]
    fn a_version_is_saved_on_the_latest_and_never_over_another() {
        let mut s = store();
        let scope = Scope {
            kind: ScopeKind::Cohort,
            id: 7,
            name: "trio".into(),
        };
        assert!(latest(&mut s, &scope).unwrap().is_none());
        assert!(reading(&mut s, &scope).unwrap().is_none());
        let rules = json!({"roles": {}, "same_kind_in_one_visit": {}});
        // a why is owed, and version 1 is what a first save is based on
        assert!(matches!(
            save(&mut s, &scope, &rules, "  ", "anna", 1),
            Err(SaveError::NoReason)
        ));
        assert!(matches!(
            save(&mut s, &scope, &rules, "why", "anna", 2),
            Err(SaveError::Behind { latest: 1 })
        ));
        assert_eq!(
            save(&mut s, &scope, &rules, "3D first", "anna", 1).unwrap(),
            2
        );
        // a second save on version 1 is behind: someone saved since
        assert!(matches!(
            save(&mut s, &scope, &rules, "mine", "bo", 1),
            Err(SaveError::Behind { latest: 2 })
        ));
        assert_eq!(save(&mut s, &scope, &rules, "again", "bo", 2).unwrap(), 3);
        let all = versions(&mut s, &scope).unwrap();
        assert_eq!(all.iter().map(|v| v.version).collect::<Vec<_>>(), [3, 2]);
        assert_eq!(all[1].reason, "3D first");
        assert_eq!(all[1].digest, digest(&rules));
        assert_eq!(version(&mut s, &scope, 2).unwrap().unwrap().author, "anna");
        assert!(version(&mut s, &scope, 9).unwrap().is_none());
        // read only once a whole run of a version finished
        assert!(reading(&mut s, &scope).unwrap().is_none());
        let run = run_started(&mut s, &scope, 2, Some(9), true).unwrap();
        assert!(reading(&mut s, &scope).unwrap().is_none());
        assert_eq!(state(&mut s, &scope).unwrap()["pending"]["state"], "queued");
        run_ended(&mut s, run, None).unwrap();
        let r = reading(&mut s, &scope).unwrap().unwrap();
        assert_eq!((r.version, r.job), (2, Some(9)));
        assert_eq!(state(&mut s, &scope).unwrap()["pending"]["version"], 3);
        assert!(
            crate::pick_truth::Of::read(&scope, Some(&r), Vec::new())
                .filter(None, "p")
                .ends_with(
                    "AND (p.rules_scope_kind = 'cohort' AND p.rules_scope_id = 7 \
                     AND (p.rules_version = 2 OR p.author_kind = 'person')))"
                )
        );
        unsave(&mut s, &scope, 3).unwrap();
        assert_eq!(latest(&mut s, &scope).unwrap().unwrap().version, 2);
        assert_eq!(scopes(&mut s).unwrap(), [(ScopeKind::Cohort, 7)]);
        // another scope's versions are its own
        let other = Scope {
            kind: ScopeKind::Dataset,
            id: 7,
            name: "ds".into(),
        };
        assert_eq!(save(&mut s, &other, &rules, "mine", "bo", 1).unwrap(), 2);
        assert_eq!(pick_filter("p", None), "p.rules_scope_kind IS NULL");
        assert_eq!(
            pick_filter("", Some(&other)),
            "rules_scope_kind = 'dataset' AND rules_scope_id = 7"
        );
    }
}
