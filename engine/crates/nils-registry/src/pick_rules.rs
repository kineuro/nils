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
//! What the rules say is `nils_pack::pick_rules`'s; this keeps them.

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

/// The scope whose picks a dataset or a cohort reads: its own when it has
/// rules saved, else the pack's (none).
pub fn reading(store: &mut Store, scope: &Scope) -> Result<Option<Scope>, StoreError> {
    Ok(latest(store, scope)?.map(|_| scope.clone()))
}

/// The picks of a scope among `pick` rows under `alias`, as a condition:
/// the pack's are those that name no scope.
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

/// What a release records of the rules whose picks it read (the
/// main-scans study, section 4), and the scope it read them in. A release
/// of one cohort reads that cohort's picks when it has rules saved; else a
/// release of one dataset, that dataset's; else the pack's, with the pack's
/// version.
pub fn for_release(
    store: &mut Store,
    cohorts: &[String],
    datasets: &[String],
    pack: &str,
    pack_version: &str,
) -> Result<(Option<Scope>, Value), StoreError> {
    let named = match (cohorts, datasets) {
        ([one], _) => Scope::cohort(store, one)?,
        ([], [one]) => Scope::dataset(store, one)?,
        _ => None,
    };
    if let Some(scope) = named
        && let Some(saved) = latest(store, &scope)?
    {
        let doc = json!({
            "scope": scope.to_json(), "version": saved.version, "digest": saved.digest,
        });
        return Ok((Some(scope), doc));
    }
    Ok((
        None,
        json!({
            "scope": null, "version": 1, "defaults": true,
            "pack": pack, "pack_version": pack_version,
        }),
    ))
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
        assert_eq!(reading(&mut s, &scope).unwrap(), Some(scope.clone()));
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
