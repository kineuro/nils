// SPDX-License-Identifier: AGPL-3.0-only

//! The main scans, from one place. Nima, 2026-10-11: "chat and the page
//! should not disagree and we have to have one place of truth".
//!
//! A pick belongs to a scope ([`crate::pick_rules`]): the pack's, a
//! dataset's or a cohort's. Which picks are *the* main scans of a stack and
//! a role is decided here alone, and every reader of picks asks this
//! module: the ask's `picked` and its role sets, the viewer, releases and
//! the pipeline runs that read them, the pipelines' pre-flight, the Main
//! scans page and its doors, the picks summary (and the dataset summary,
//! which counts with it), the timeline and the command line.
//!
//! - **A reader about a scope** (a dataset or a cohort it was opened from,
//!   or named): that scope's picks once the scope is read, that is once a
//!   run of a saved version of its rules finished over all its subjects
//!   ([`crate::pick_rules::reading`]), the latest such version and the
//!   persons' picks of the scope; else the pack's. Either way, of the
//!   scope's subjects alone: a cohort's open members, a dataset's subjects
//!   with a stack its tree holds a file of, as every reader counts a
//!   dataset's subjects ([`crate::dataset_subjects::scanned`]).
//! - **A reader about no scope**: each subject's own dataset's picks by the
//!   same rule, else the pack's. A subject's dataset is one whose tree holds
//!   a file of one of its stacks (record 55's duplicate policy: a dataset
//!   holds every scan its tree has a file of); a subject of two datasets
//!   with rules read reads the one with the lower id. A cohort's rules are
//!   read only by a reader about that cohort.
//!
//! Whether a scope is read is asked in the statement itself, so a reader
//! never holds an answer that a finished run has since changed. A release
//! records what it read ([`Of::pinned`], [`Of::record`]), and a pipeline
//! run over the release reads that again ([`Of::of_record`]).
//!
//! The picks are a relation, [`Of::relation`]: the live picks (none
//! withdrawn) of the scope the rule names, which a reader joins in place of
//! the `pick` table. Nothing else names the table but the run that writes
//! picks and the few readers the test at the end of this file names, each
//! with why.

use serde_json::{Value, json};

use crate::dialect::qualified;
use crate::pick_rules::{self, Reading, Scope, ScopeKind};
use crate::store::{Error as StoreError, Store};

/// Which version of a scope's picks is read: the one read now (the latest
/// whose run finished over all its subjects, asked in the statement), one
/// a release recorded, or none, the pack's picks of the scope's subjects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    Now,
    At(i64),
    Pack,
}

/// A scope's id as a statement names it: the id, or a cohort by its name
/// (the ask names one so, and compiles without the store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Id {
    Id(i64),
    CohortNamed(String),
}

/// A dataset a subject can be of, for a reader about no scope: its place
/// id, its name, and the `source` rows that are its own
/// ([`crate::place::dataset_sources`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub id: i64,
    pub name: String,
    pub sources: Vec<i64>,
    pub version: Version,
}

/// Whose picks are the main scans for a reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Of {
    /// The pack's alone: what a release recorded when it read no scope's.
    Pack,
    /// A scope the reader is about: its own once read, else the pack's;
    /// of its subjects alone either way (a cohort's open members, a
    /// dataset's subjects with a stack its `sources` hold a file of).
    Scope {
        kind: ScopeKind,
        id: Id,
        name: String,
        version: Version,
        sources: Vec<i64>,
    },
    /// No scope named: each subject's own dataset's once read, else the
    /// pack's.
    Own(Vec<Holder>),
}

fn text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl Of {
    /// The main scans of a scope a reader is about.
    pub fn scope(store: &mut Store, scope: &Scope) -> Result<Of, StoreError> {
        let sources = match scope.kind {
            ScopeKind::Cohort => Vec::new(),
            ScopeKind::Dataset => crate::place::dataset_sources(store)?
                .into_iter()
                .find(|d| d.id == scope.id)
                .map(|d| d.sources)
                .unwrap_or_default(),
        };
        Ok(Of::Scope {
            kind: scope.kind,
            id: Id::Id(scope.id),
            name: scope.name.clone(),
            version: Version::Now,
            sources,
        })
    }

    /// The main scans of a dataset by its id and its `source` rows, where
    /// the reader holds no store (the ask's compiler).
    pub fn dataset(id: i64, name: &str, sources: Vec<i64>) -> Of {
        Of::Scope {
            kind: ScopeKind::Dataset,
            id: Id::Id(id),
            name: name.to_string(),
            version: Version::Now,
            sources,
        }
    }

    /// The main scans of a cohort by its id.
    pub fn cohort(id: i64, name: &str) -> Of {
        Of::Scope {
            kind: ScopeKind::Cohort,
            id: Id::Id(id),
            name: name.to_string(),
            version: Version::Now,
            sources: Vec::new(),
        }
    }

    /// The main scans of a cohort named, where the reader holds no store
    /// (the ask's compiler).
    pub fn cohort_named(name: &str) -> Of {
        Of::Scope {
            kind: ScopeKind::Cohort,
            id: Id::CohortNamed(name.to_string()),
            name: name.to_string(),
            version: Version::Now,
            sources: Vec::new(),
        }
    }

    /// The main scans of no scope, over these datasets: each subject's own
    /// dataset's once read, else the pack's. A dataset that holds no source
    /// holds no subject.
    pub fn own_of(datasets: impl IntoIterator<Item = Holder>) -> Of {
        let mut holders: Vec<Holder> = datasets
            .into_iter()
            .filter(|h| !h.sources.is_empty())
            .collect();
        holders.sort_by_key(|h| h.id);
        Of::Own(holders)
    }

    /// The main scans of no scope: every dataset with rules of its own
    /// saved is one a subject may read its own picks from.
    pub fn own(store: &mut Store) -> Result<Of, StoreError> {
        let saved: Vec<i64> = pick_rules::scopes(store)?
            .into_iter()
            .filter(|(k, _)| *k == ScopeKind::Dataset)
            .map(|(_, id)| id)
            .collect();
        if saved.is_empty() {
            return Ok(Of::Own(Vec::new()));
        }
        Ok(Of::own_of(
            crate::place::dataset_sources(store)?
                .into_iter()
                .filter(|d| saved.contains(&d.id))
                .map(|d| Holder {
                    id: d.id,
                    name: d.name,
                    sources: d.sources,
                    version: Version::Now,
                }),
        ))
    }

    /// The main scans of a scope a reader is about, or of none.
    pub fn of_scope(store: &mut Store, scope: Option<&Scope>) -> Result<Of, StoreError> {
        match scope {
            Some(s) => Of::scope(store, s),
            None => Of::own(store),
        }
    }

    /// What a scope's reading read: its own picks at the version read, or
    /// of none read, the pack's of its subjects.
    pub fn read(scope: &Scope, reading: Option<&Reading>, sources: Vec<i64>) -> Of {
        Of::Scope {
            kind: scope.kind,
            id: Id::Id(scope.id),
            name: scope.name.clone(),
            version: reading.map_or(Version::Pack, |r| Version::At(r.version)),
            sources,
        }
    }

    /// The latest version of a scope read, as a statement asks it.
    fn version_now(schema: Option<&str>, kind: ScopeKind, id: &str) -> String {
        format!(
            "(SELECT MAX(rr.version) FROM {} rr WHERE rr.scope_kind = '{}' AND rr.scope_id = {id} \
             AND rr.state = 'done' AND rr.whole = 1)",
            qualified(schema, "pick_rules_run"),
            kind.name()
        )
    }

    fn id_sql(schema: Option<&str>, id: &Id) -> String {
        match id {
            Id::Id(n) => n.to_string(),
            Id::CohortNamed(name) => format!(
                "(SELECT cn.id FROM {} cn WHERE cn.name = {})",
                qualified(schema, "cohort"),
                text(name)
            ),
        }
    }

    /// The condition that keeps the picks this reader reads, over `pick`
    /// rows under `alias` (the table's own name where none), withdrawn ones
    /// included: [`Of::relation`] is what a reader joins.
    pub fn filter(&self, schema: Option<&str>, alias: &str) -> String {
        let a = if alias.is_empty() { "pick" } else { alias };
        match self {
            Of::Pack => format!("{a}.rules_scope_kind IS NULL"),
            Of::Scope {
                kind,
                id,
                version,
                sources,
                ..
            } => {
                let id = Of::id_sql(schema, id);
                let mine = |v: &str| {
                    format!(
                        "({a}.rules_scope_kind = '{}' AND {a}.rules_scope_id = {id} \
                         AND ({a}.rules_version = {v} OR {a}.author_kind = 'person'))",
                        kind.name()
                    )
                };
                // the scope's subjects: a cohort's open members, a
                // dataset's subjects with a stack its tree holds a file of
                // (`dataset_subjects`)
                let members = match kind {
                    ScopeKind::Cohort => format!(
                        "{a}.subject_id IN (SELECT cm.subject_id FROM {} cm \
                         WHERE cm.cohort_id = {id} AND cm.left_at IS NULL)",
                        qualified(schema, "cohort_member")
                    ),
                    ScopeKind::Dataset if sources.is_empty() => "1 = 0".to_string(),
                    ScopeKind::Dataset => format!(
                        "{a}.subject_id IN ({})",
                        crate::dataset_subjects::scanned(schema, sources)
                    ),
                };
                let read = match version {
                    Version::At(n) => mine(&n.to_string()),
                    Version::Pack => format!("{a}.rules_scope_kind IS NULL"),
                    Version::Now => {
                        let v = Of::version_now(schema, *kind, &id);
                        format!(
                            "(CASE WHEN {v} IS NULL THEN {a}.rules_scope_kind IS NULL ELSE {} END)",
                            mine(&v)
                        )
                    }
                };
                format!("({members} AND {read})")
            }
            Of::Own(holders) if holders.is_empty() => format!("{a}.rules_scope_kind IS NULL"),
            Of::Own(holders) => {
                let read = format!(
                    "(SELECT rr.scope_id FROM {} rr WHERE rr.scope_kind = 'dataset' \
                     AND rr.state = 'done' AND rr.whole = 1)",
                    qualified(schema, "pick_rules_run")
                );
                let mut whens = String::new();
                let mut versions = String::new();
                let mut sources: Vec<i64> = Vec::new();
                for h in holders {
                    let (is_read, v) = match h.version {
                        Version::Now => (
                            format!(" AND {} IN {read}", h.id),
                            Of::version_now(schema, ScopeKind::Dataset, &h.id.to_string()),
                        ),
                        Version::At(n) => (String::new(), n.to_string()),
                        Version::Pack => continue,
                    };
                    whens.push_str(&format!(
                        " WHEN gss.source_id IN ({}){is_read} THEN {}",
                        list(&h.sources),
                        h.id
                    ));
                    versions.push_str(&format!(" WHEN {} THEN {v}", h.id));
                    sources.extend(&h.sources);
                }
                if sources.is_empty() {
                    return format!("{a}.rules_scope_kind IS NULL");
                }
                // the dataset whose picks the subject reads: the lowest
                // read one whose tree holds a file of one of its stacks;
                // none, the pack's
                let own = format!(
                    "(SELECT MIN(CASE{whens} END) FROM {} gs {} \
                     JOIN {} ge ON ge.id = gs.series_id \
                     WHERE ge.subject_id = {a}.subject_id AND gss.source_id IN ({}))",
                    qualified(schema, "stack"),
                    crate::dataset_subjects::holders(schema, "gs", "gss"),
                    qualified(schema, "series"),
                    list(&sources)
                );
                format!(
                    "(COALESCE({own}, -1) = (CASE WHEN {a}.rules_scope_kind IS NULL THEN -1 \
                     WHEN {a}.rules_scope_kind = 'dataset' THEN {a}.rules_scope_id ELSE -2 END) \
                     AND ({a}.rules_scope_kind IS NULL OR {a}.author_kind = 'person' \
                     OR {a}.rules_version = (CASE {a}.rules_scope_id{versions} END)))"
                )
            }
        }
    }

    /// The picks this reader reads, as a relation with the `pick` table's
    /// columns to join in its place: the live ones (none withdrawn) of the
    /// scope the rule names.
    pub fn relation(&self, schema: Option<&str>) -> String {
        format!(
            "(SELECT * FROM {} mp WHERE mp.withdrawn_at IS NULL AND {})",
            qualified(schema, "pick"),
            self.filter(schema, "mp")
        )
    }

    /// [`Of::relation`] in a store's schema.
    pub fn picks(&self, store: &Store) -> String {
        self.relation(store.schema())
    }

    /// What this reader reads now, with every version the rule asks for
    /// resolved: a scope read at its version or the pack's, and of no
    /// scope, the datasets read and at which version.
    pub fn pinned(&self, store: &mut Store) -> Result<Of, StoreError> {
        Ok(match self {
            Of::Pack => Of::Pack,
            Of::Scope {
                version: Version::At(_) | Version::Pack,
                ..
            } => self.clone(),
            Of::Scope {
                kind,
                id,
                name,
                version: Version::Now,
                sources,
            } => {
                let scope = match id {
                    Id::Id(n) => Scope::of(store, *kind, *n)?,
                    Id::CohortNamed(c) => Scope::cohort(store, c)?,
                }
                .unwrap_or(Scope {
                    kind: *kind,
                    id: -1,
                    name: name.clone(),
                });
                let reading = pick_rules::reading(store, &scope)?;
                Of::read(&scope, reading.as_ref(), sources.clone())
            }
            Of::Own(holders) => {
                let mut read = Vec::new();
                for h in holders {
                    let version = match h.version {
                        Version::At(n) => Some(n),
                        Version::Pack => None,
                        Version::Now => {
                            let scope = Scope {
                                kind: ScopeKind::Dataset,
                                id: h.id,
                                name: h.name.clone(),
                            };
                            pick_rules::reading(store, &scope)?.map(|r| r.version)
                        }
                    };
                    if let Some(n) = version {
                        read.push(Holder {
                            version: Version::At(n),
                            ..h.clone()
                        });
                    }
                }
                Of::Own(read)
            }
        })
    }

    /// In words, for an answer that says which picks it read.
    pub fn describe(&self) -> String {
        match self {
            Of::Pack => "the pack's main scans".to_string(),
            Of::Scope {
                kind,
                name,
                version,
                ..
            } => match version {
                Version::Now => format!(
                    "{} {name}'s main scans under its own rules once a run of them finished, else the pack's",
                    kind.name()
                ),
                Version::At(n) => format!(
                    "{} {name}'s main scans under its own rules, version {n}",
                    kind.name()
                ),
                Version::Pack => format!("the pack's main scans of {} {name}", kind.name()),
            },
            Of::Own(_) => "each subject's own dataset's main scans under its own rules once a run of them finished, else the pack's".to_string(),
        }
    }

    /// What a release records of the picks it read ([`Of::pinned`] first),
    /// as `release.pick_rules`: a scope read, `{scope, version, digest,
    /// run}`; else the pack's defaults with the pack's version, and, of no
    /// scope, the datasets whose own picks their subjects read
    /// (`datasets`, each `{scope, version, digest, run, sources}`).
    pub fn record(
        &self,
        store: &mut Store,
        pack: &str,
        pack_version: &str,
    ) -> Result<Value, StoreError> {
        let defaults = json!({
            "scope": null, "version": 1, "defaults": true,
            "pack": pack, "pack_version": pack_version,
        });
        let of_version = |store: &mut Store, scope: &Scope, n: i64| -> Result<Value, StoreError> {
            let digest = pick_rules::version(store, scope, n)?.map(|s| s.digest);
            let run = pick_rules::reading(store, scope)?
                .filter(|r| r.version == n)
                .and_then(|r| r.job);
            Ok(json!({"scope": scope.to_json(), "version": n, "digest": digest, "run": run}))
        };
        Ok(match self.pinned(store)? {
            Of::Scope {
                kind,
                id: Id::Id(id),
                name,
                version: Version::At(n),
                sources,
            } => {
                let mut doc = of_version(store, &Scope { kind, id, name }, n)?;
                if kind == ScopeKind::Dataset {
                    doc["sources"] = json!(sources);
                }
                doc
            }
            Of::Own(holders) => {
                let mut out = defaults;
                let mut datasets = Vec::new();
                for h in holders {
                    let Version::At(n) = h.version else { continue };
                    let scope = Scope {
                        kind: ScopeKind::Dataset,
                        id: h.id,
                        name: h.name.clone(),
                    };
                    let mut doc = of_version(store, &scope, n)?;
                    doc["sources"] = json!(h.sources);
                    datasets.push(doc);
                }
                out["datasets"] = Value::Array(datasets);
                out
            }
            _ => defaults,
        })
    }

    /// What a release recorded it read ([`Of::record`]), for a reader of the
    /// release's picks after it (a pipeline run's input): a scope at its
    /// version, the datasets its subjects read at theirs, or the pack's.
    pub fn of_record(doc: &Value) -> Of {
        let s = &doc["scope"];
        if let (Some(kind), Some(id), Some(version)) = (
            s["kind"].as_str().and_then(ScopeKind::parse),
            s["id"].as_i64(),
            doc["version"].as_i64(),
        ) {
            return Of::Scope {
                kind,
                id: Id::Id(id),
                name: s["name"].as_str().unwrap_or_default().to_string(),
                version: Version::At(version),
                sources: doc["sources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                    .collect(),
            };
        }
        let holders: Vec<Holder> = doc["datasets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| {
                Some(Holder {
                    id: d["scope"]["id"].as_i64()?,
                    name: d["scope"]["name"].as_str().unwrap_or_default().to_string(),
                    sources: d["sources"]
                        .as_array()?
                        .iter()
                        .filter_map(Value::as_i64)
                        .collect(),
                    version: Version::At(d["version"].as_i64()?),
                })
            })
            .collect();
        if holders.is_empty() {
            Of::Pack
        } else {
            Of::own_of(holders)
        }
    }
}

/// What a release reads of the picks and records of them (the main-scans
/// study, section 4, as ruled on 2026-10-11): a release of one cohort reads
/// that cohort's; else of one dataset, that dataset's; else each subject's
/// own dataset's. Each is the scope's own once read, else the pack's.
pub fn for_release(
    store: &mut Store,
    cohorts: &[String],
    datasets: &[String],
    pack: &str,
    pack_version: &str,
) -> Result<(Of, Value), StoreError> {
    let named = match (cohorts, datasets) {
        ([one], _) => Scope::cohort(store, one)?,
        ([], [one]) => Scope::dataset(store, one)?,
        _ => None,
    };
    let of = Of::of_scope(store, named.as_ref())?;
    let pinned = of.pinned(store)?;
    let doc = pinned.record(store, pack, pack_version)?;
    Ok((pinned, doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_reads_a_scope_once_read_and_each_subject_its_own_dataset() {
        let scope = Of::dataset(7, "ds", vec![3]);
        let f = scope.filter(None, "p");
        assert!(f.starts_with(
            "(p.subject_id IN (SELECT hse.subject_id FROM stack hst JOIN source_stack hss"
        ));
        assert!(f.contains("WHERE hss.source_id IN (3)) AND (CASE WHEN (SELECT MAX(rr.version)"));
        assert!(f.contains("THEN p.rules_scope_kind IS NULL ELSE (p.rules_scope_kind = 'dataset'"));
        // a dataset nothing has read holds no subject
        assert!(
            Of::dataset(7, "ds", Vec::new())
                .filter(None, "p")
                .starts_with("(1 = 0 AND")
        );
        let at = Of::Scope {
            kind: ScopeKind::Cohort,
            id: Id::Id(7),
            name: "trio".into(),
            version: Version::At(2),
            sources: Vec::new(),
        };
        assert_eq!(
            at.filter(None, "p"),
            "(p.subject_id IN (SELECT cm.subject_id FROM cohort_member cm \
             WHERE cm.cohort_id = 7 AND cm.left_at IS NULL) AND (p.rules_scope_kind = 'cohort' \
             AND p.rules_scope_id = 7 AND (p.rules_version = 2 OR p.author_kind = 'person')))"
        );
        assert_eq!(
            Of::Pack.filter(Some("s"), ""),
            "pick.rules_scope_kind IS NULL"
        );
        assert_eq!(
            Of::Own(Vec::new()).filter(None, "p"),
            "p.rules_scope_kind IS NULL"
        );
        // a dataset with no source holds no subject
        assert_eq!(
            Of::own_of([Holder {
                id: 3,
                name: "none".into(),
                sources: Vec::new(),
                version: Version::Now,
            }]),
            Of::Own(Vec::new())
        );
        let named = Of::cohort_named("o'hare").filter(Some("s"), "p");
        assert!(named.contains("(SELECT cn.id FROM s.cohort cn WHERE cn.name = 'o''hare')"));
        assert!(
            Of::Pack
                .relation(Some("s"))
                .starts_with("(SELECT * FROM s.pick mp WHERE mp.withdrawn_at IS NULL")
        );
    }

    #[test]
    fn a_release_records_what_it_read_and_a_run_reads_it_again() {
        let doc = json!({
            "scope": null, "version": 1, "defaults": true, "pack": "mri", "pack_version": "1",
            "datasets": [{"scope": {"kind": "dataset", "id": 4, "name": "a"}, "version": 3,
                          "digest": "d", "run": 9, "sources": [1, 2]}],
        });
        assert_eq!(
            Of::of_record(&doc),
            Of::Own(vec![Holder {
                id: 4,
                name: "a".into(),
                sources: vec![1, 2],
                version: Version::At(3),
            }])
        );
        let scoped = json!({"scope": {"kind": "cohort", "id": 7, "name": "trio"}, "version": 2});
        assert_eq!(
            Of::of_record(&scoped),
            Of::Scope {
                kind: ScopeKind::Cohort,
                id: Id::Id(7),
                name: "trio".into(),
                version: Version::At(2),
                sources: Vec::new(),
            }
        );
        assert_eq!(
            Of::of_record(&json!({"scope": null, "defaults": true})),
            Of::Pack
        );
    }

    /// One place of truth: every SQL statement that names the `pick` table
    /// is the resolver's, or one of the named exceptions below, each with
    /// why and how many times it names it. A new reader that reads `pick`
    /// itself, instead of joining [`Of::relation`], fails this test.
    #[test]
    fn no_reader_reads_the_pick_table_but_through_the_resolver() {
        // a line that puts the table into a statement: by its name in a
        // store's schema, by the ask's and the viewer's `q`, or written out
        let names = |line: &str| -> bool {
            (line.contains("qualified(") && line.contains("\"pick\")"))
                || line.contains("q(\"pick\")")
                || line.contains("FROM pick ")
                || line.contains("JOIN pick ")
        };
        const EXCEPTIONS: [(&str, usize, &str); 9] = [
            ("nils-registry/src/pick_truth.rs", 3, "the resolver"),
            (
                "nils-registry/src/pick_rules.rs",
                2,
                "a scope's first save copies in the persons' picks made for it: a writer",
            ),
            ("nils-registry/src/migrate.rs", 1, "migrations"),
            (
                "nils-classify/src/picking.rs",
                13,
                "the run that writes picks, a person's pick, keep and withdraw",
            ),
            (
                "nils-classify/src/effect.rs",
                1,
                "a rules change's effect re-runs the pack's run, which leaves the pack's persons' picks standing",
            ),
            (
                "nils-session/src/lib.rs",
                2,
                "a pick follows its session's day: a writer",
            ),
            (
                "nils/src/campaigns.rs",
                1,
                "a pick question is a pick.border of the pack's run, and Keep writes into the pack's scope",
            ),
            (
                "nils/src/timeline.rs",
                1,
                "the history of every pick of a stack, each named by its scope; whether one is a main scan now comes from the resolver",
            ),
            (
                "nils/src/main.rs",
                1,
                "pick explain reads one pick by its id",
            ),
        ];
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crates folder");
        let mut found: Vec<(String, usize)> = Vec::new();
        let mut dirs = vec![crates.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("a folder of the crates") {
                let path = entry.expect("an entry").path();
                let rel = path
                    .strip_prefix(crates)
                    .expect("under the crates")
                    .to_string_lossy()
                    .replace('\\', "/");
                if path.is_dir() {
                    // the sources; tests write their fixtures as they need
                    let first = rel.split('/').nth(1);
                    if first.is_none() || first == Some("src") {
                        dirs.push(path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let body = std::fs::read_to_string(&path).expect("a source file");
                let n = body.lines().filter(|l| names(l)).count();
                if n > 0 {
                    found.push((rel, n));
                }
            }
        }
        found.sort();
        let mut wrong = Vec::new();
        for (file, n) in &found {
            match EXCEPTIONS.iter().find(|(f, _, _)| f == file) {
                Some((_, want, _)) if want == n => {}
                Some((_, want, why)) => wrong.push(format!(
                    "{file} names the pick table {n} times, its exception ({why}) {want}"
                )),
                None => wrong.push(format!(
                    "{file} names the pick table {n} times: read the main scans through pick_truth::Of"
                )),
            }
        }
        for (file, _, why) in EXCEPTIONS {
            if !found.iter().any(|(f, _)| f == file) {
                wrong.push(format!(
                    "{file} is an exception ({why}) and names the pick table no more"
                ));
            }
        }
        assert!(found.len() > 3, "the sources were not read: {found:?}");
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
