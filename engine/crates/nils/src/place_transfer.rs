// SPDX-License-Identifier: AGPL-3.0-only

//! A site's places carried to a new registry (Wave 7a §15.3). `nils place
//! export` writes every place in force to one JSON document: its name, role
//! and path, the guarantees the operator declared, its handling, and for a
//! source place the dataset it was declared as. `nils place import` declares
//! them in another registry as they were written: no folder is looked at,
//! nothing under a place is moved or renamed, and no probe file is made, so
//! what the engine measured is left for `nils place list --probe`.
//!
//! A place the registry has already, by the same name, role and path, is
//! kept as it is (a refresh made by `nils setup --new-registry` declares the
//! setup's own places first, as the setup record has them). Anything else
//! the import cannot take is said for every place before one is written: a
//! name taken by another place, a backup place or a root that is neither
//! there nor imported, a dataset that does not check, an ID type the new
//! registry lacks.

use std::collections::BTreeSet;
use std::path::Path;

use nils_registry::Registry;
use nils_registry::place::{self, Role};
use nils_registry::time::now_iso;
use serde_json::{Value, json};

/// What the document says it is.
pub(crate) const FORMAT: &str = "nils-places";
/// Its version: a document of another is refused rather than read wrongly.
pub(crate) const VERSION: i64 = 1;

/// Every place in force, as `nils place export` writes it. Retired places
/// are history of the registry they were declared in, and are counted, not
/// written.
pub(crate) fn export(registry: &mut Registry) -> Result<Value, String> {
    let registry_id = registry.meta().registry_id.clone();
    let all = place::list(registry.store()).map_err(|e| e.to_string())?;
    let retired = all.iter().filter(|p| p.retired_at.is_some()).count();
    let places: Vec<Value> = all
        .iter()
        .filter(|p| p.retired_at.is_none())
        .map(|p| {
            json!({
                "name": p.name,
                "role": p.role.name(),
                "path": p.path,
                "guarantees": p.guarantees,
                "handling": p.handling,
                "dataset": if p.role == Role::Source {
                    place::dataset_of(&p.dataset, None).unwrap_or_else(|_| p.dataset.clone())
                } else {
                    Value::Null
                },
            })
        })
        .collect();
    Ok(json!({
        "format": FORMAT,
        "version": VERSION,
        "registry": registry_id,
        "exported_at": now_iso(),
        "retired_left_out": retired,
        "places": places,
    }))
}

/// One place of a document, read and checked.
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    role: Role,
    path: String,
    guarantees: Value,
    handling: Value,
    dataset: Value,
}

impl Entry {
    fn backup(&self) -> Option<&str> {
        self.guarantees["backup"].as_str().filter(|b| !b.is_empty())
    }

    fn root(&self) -> Option<&str> {
        self.dataset["root"].as_str().filter(|r| !r.is_empty())
    }
}

/// What an import does with one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Declared in this registry by the import.
    Added,
    /// Already declared here by the same name, role and path, and kept as
    /// it is.
    Kept,
    /// Named with `--skip`.
    Skipped,
}

impl Outcome {
    fn word(&self) -> &'static str {
        match self {
            Outcome::Added => "added",
            Outcome::Kept => "kept",
            Outcome::Skipped => "skipped",
        }
    }
}

/// One line of an import's plan.
#[derive(Debug, Clone)]
pub(crate) struct Planned {
    pub(crate) name: String,
    pub(crate) role: &'static str,
    pub(crate) path: String,
    pub(crate) outcome: Outcome,
    /// The id it has here, once added or where it was kept.
    pub(crate) id: Option<i64>,
}

impl Planned {
    pub(crate) fn as_json(&self) -> Value {
        json!({
            "name": self.name,
            "role": self.role,
            "path": self.path,
            "outcome": self.outcome.word(),
            "id": self.id,
        })
    }
}

fn entries_of(doc: &Value) -> Result<Vec<Entry>, String> {
    if doc["format"].as_str() != Some(FORMAT) {
        return Err(format!(
            "this is not a document nils place export wrote: its format is not {FORMAT}"
        ));
    }
    if doc["version"].as_i64() != Some(VERSION) {
        return Err(format!(
            "the document is version {}, and this engine reads version {VERSION}",
            doc["version"]
        ));
    }
    let Some(list) = doc["places"].as_array() else {
        return Err("the document has no list of places".into());
    };
    let mut out = Vec::with_capacity(list.len());
    let mut names = BTreeSet::new();
    for (i, p) in list.iter().enumerate() {
        let name = p["name"]
            .as_str()
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| format!("place {} of the document has no name", i + 1))?
            .to_string();
        if !names.insert(name.clone()) {
            return Err(format!("{name} is in the document twice"));
        }
        let role = p["role"]
            .as_str()
            .and_then(Role::parse)
            .ok_or_else(|| format!("{name}: its role {} is not a role", p["role"]))?;
        let path = p["path"]
            .as_str()
            .filter(|s| Path::new(s).is_absolute())
            .ok_or_else(|| {
                format!(
                    "{name}: its path {} is not an absolute directory",
                    p["path"]
                )
            })?
            .to_string();
        let guarantees = match &p["guarantees"] {
            Value::Object(_) => p["guarantees"].clone(),
            Value::Null => json!({}),
            other => return Err(format!("{name}: guarantees is an object, not {other}")),
        };
        let handling = match &p["handling"] {
            Value::Null => Value::Null,
            h => place::handling_of(h).map_err(|e| format!("{name}: {e}"))?,
        };
        let dataset = match (&p["dataset"], role) {
            (Value::Null, _) => Value::Null,
            (d, Role::Source) => place::dataset_of(d, None).map_err(|e| format!("{name}: {e}"))?,
            (_, other) => {
                return Err(format!(
                    "{name}: a dataset is a source place, and this one is a {} place",
                    other.name()
                ));
            }
        };
        out.push(Entry {
            name,
            role,
            path,
            guarantees,
            handling,
            dataset,
        });
    }
    Ok(out)
}

/// The order a registry can take the places in: a place another names as
/// its backup before the place that names it, the places of other roles
/// before the source places, and a root before the datasets under it.
fn ordered(entries: Vec<Entry>) -> Vec<Entry> {
    let backs: BTreeSet<String> = entries
        .iter()
        .filter_map(|e| e.backup().map(str::to_string))
        .collect();
    let rank = |e: &Entry| -> u8 {
        if backs.contains(&e.name) {
            0
        } else if e.role != Role::Source {
            1
        } else if e.root().is_none() {
            2
        } else {
            3
        }
    };
    let mut out = entries;
    // stable, so the document's own order holds within a rank
    out.sort_by_key(rank);
    out
}

/// What importing `doc` into `registry` would do, checked whole: every
/// reason it cannot is said at once, and nothing is written. `skip` names
/// places of the document to leave out; a name it does not hold is refused,
/// so a misspelt skip never imports what it meant to leave.
fn plan(
    registry: &mut Registry,
    doc: &Value,
    skip: &[String],
) -> Result<Vec<(Entry, Planned)>, String> {
    let entries = ordered(entries_of(doc)?);
    for s in skip {
        if !entries.iter().any(|e| &e.name == s) {
            return Err(format!(
                "--skip {s}: the document has no place of that name"
            ));
        }
    }
    let mut refused = Vec::new();
    let mut out = Vec::new();
    for e in entries {
        let mut planned = Planned {
            name: e.name.clone(),
            role: e.role.name(),
            path: e.path.clone(),
            outcome: Outcome::Added,
            id: None,
        };
        if skip.contains(&e.name) {
            planned.outcome = Outcome::Skipped;
            out.push((e, planned));
            continue;
        }
        let there = place::by_name(registry.store(), &e.name)
            .map_err(|err| err.to_string())?
            .filter(|p| p.retired_at.is_none());
        if let Some(p) = there {
            if p.role == e.role && p.path == e.path {
                planned.outcome = Outcome::Kept;
                planned.id = Some(p.id);
            } else {
                refused.push(format!(
                    "{}: this registry has a place by that name, a {} place at {}, and the \
                     document's is a {} place at {}",
                    e.name,
                    p.role.name(),
                    p.path,
                    e.role.name(),
                    e.path
                ));
            }
        }
        out.push((e, planned));
    }
    // what the places name must be here, or come with them
    let present = |registry: &mut Registry, out: &[(Entry, Planned)], name: &str| {
        out.iter()
            .any(|(e, p)| e.name == name && p.outcome != Outcome::Skipped)
            || place::by_name(registry.store(), name)
                .ok()
                .flatten()
                .is_some_and(|p| p.retired_at.is_none())
    };
    for (e, p) in &out {
        if p.outcome != Outcome::Added {
            continue;
        }
        if let Some(b) = e.backup()
            && !present(registry, &out, b)
        {
            refused.push(format!(
                "{}: its backup place {b} is neither in this registry nor imported",
                e.name
            ));
        }
        if let Some(r) = e.root()
            && !present(registry, &out, r)
        {
            refused.push(format!(
                "{}: its root {r} is neither in this registry nor imported",
                e.name
            ));
        }
        if e.role == Role::Source
            && let Some(why) = crate::dataset::id_types_refused(registry, &e.dataset)?
        {
            refused.push(format!("{}: {why}", e.name));
        }
    }
    if refused.is_empty() {
        Ok(out)
    } else {
        Err(format!("nothing was imported:\n  {}", refused.join("\n  ")))
    }
}

/// Import `doc` into `registry`: what [`plan`] says, then, unless `print`,
/// each place to add declared as the document has it, one audit row each.
/// What was planned or done comes back, a line a place.
pub(crate) fn import(
    registry: &mut Registry,
    doc: &Value,
    skip: &[String],
    print: bool,
) -> Result<Vec<Planned>, String> {
    let planned = plan(registry, doc, skip)?;
    let mut out = Vec::with_capacity(planned.len());
    for (e, mut p) in planned {
        if p.outcome == Outcome::Added && !print {
            let id = place::add(
                registry.store(),
                &place::New {
                    name: &e.name,
                    role: e.role,
                    path: &e.path,
                    guarantees: e.guarantees.clone(),
                    // nothing measured here: `nils place list --probe` does
                    probed: json!({}),
                    handling: e.handling.clone(),
                    dataset: e.dataset.clone(),
                },
            )
            .map_err(|err| format!("{}: {err}", e.name))?;
            crate::audit(
                registry,
                nils_registry::audit::Action::PlaceAdd,
                json!({"place": id, "name": e.name, "role": e.role.name()}),
                Some(json!({"imported": true})),
            )
            .map_err(|err| format!("{}: {}", e.name, err.message))?;
            p.id = Some(id);
        }
        out.push(p);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_registry::{Backend, Home, InitOptions, Registry, Scheme};
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nils-place-transfer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A registry in `dir`, on SQLite or, with a DSN, in a schema of its own.
    fn registry(dir: &Path, postgres: Option<(&str, &str)>) -> Registry {
        let home = Home::new(dir);
        home.keys(None).add("nils", b"a test passphrase").unwrap();
        home.init(&InitOptions {
            backend: if postgres.is_some() {
                Backend::Postgres
            } else {
                Backend::Sqlite
            },
            dsn: postgres.map(|(dsn, _)| dsn.to_string()),
            schema: postgres.map(|(_, schema)| schema.to_string()),
            scheme: Scheme::DEFAULT,
            key: "nils".into(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap()
    }

    fn drop_schemas(dsn: &str, schema: &str) {
        nils_registry::store::Store::connect_postgres(dsn, schema)
            .unwrap()
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .unwrap();
    }

    fn add(
        r: &mut Registry,
        name: &str,
        role: Role,
        path: &Path,
        guarantees: Value,
        dataset: Value,
    ) -> i64 {
        std::fs::create_dir_all(path).unwrap();
        place::add(
            r.store(),
            &place::New {
                name,
                role,
                path: &path.display().to_string(),
                guarantees,
                probed: json!({}),
                handling: Value::Null,
                dataset,
            },
        )
        .unwrap()
    }

    /// Every entry on disk under `dir`, so a test can say nothing moved.
    fn tree(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                out.push(p.strip_prefix(dir).unwrap().display().to_string());
                if p.is_dir() {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }

    /// The site of one test: a backup place, a working place, a source
    /// root, a dataset under it declared de-identified, a test dataset and a
    /// retired place, in an old registry; then a new registry that has the
    /// setup's own places already, as `nils setup --new-registry` leaves it.
    fn exported_then_imported(postgres: Option<&str>) {
        let dir = scratch(if postgres.is_some() { "pg" } else { "sqlite" });
        let data = dir.join("data");
        let (old_schema, new_schema) = ("nils_transfer_old", "nils_transfer_new");
        if let Some(dsn) = postgres {
            drop_schemas(dsn, old_schema);
            drop_schemas(dsn, new_schema);
        }
        let mut old = registry(&dir.join("old"), postgres.map(|d| (d, old_schema)));
        add(
            &mut old,
            "archives",
            Role::Backup,
            &data.join("archives"),
            json!({"protected": true}),
            Value::Null,
        );
        add(
            &mut old,
            "registry",
            Role::Registry,
            &dir.join("old"),
            json!({"backup": "archives", "snapshots": true}),
            Value::Null,
        );
        add(
            &mut old,
            "scratch-nvme",
            Role::Working,
            &data.join("scratch"),
            json!({"fast": true}),
            Value::Null,
        );
        add(
            &mut old,
            "source",
            Role::Source,
            &data.join("source"),
            json!({}),
            Value::Null,
        );
        let study = data.join("source").join("study-a");
        std::fs::create_dir_all(study.join("derivatives").join("dcm-anon")).unwrap();
        std::fs::write(
            study.join("derivatives").join("dcm-anon").join("f.dcm"),
            b"x",
        )
        .unwrap();
        add(
            &mut old,
            "study-a",
            Role::Source,
            &study,
            json!({"protected": true}),
            json!({"arrives": "deidentified", "trees": {"originals": null, "anon": place::ANON_TREE},
                   "root": "source", "subjects": "generated", "cohort": "a"}),
        );
        add(
            &mut old,
            "test-data",
            Role::Source,
            &data.join("test"),
            json!({}),
            Value::Null,
        );
        let gone = add(
            &mut old,
            "gone",
            Role::Working,
            &data.join("gone"),
            json!({}),
            Value::Null,
        );
        place::retire(old.store(), gone).unwrap();

        let doc = export(&mut old).unwrap();
        assert_eq!(doc["format"], FORMAT);
        assert_eq!(doc["retired_left_out"], 1);
        let names: Vec<&str> = doc["places"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "archives",
                "registry",
                "scratch-nvme",
                "source",
                "study-a",
                "test-data"
            ],
            "every place in force, and no retired one"
        );
        let study_doc = doc["places"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "study-a")
            .unwrap();
        assert_eq!(study_doc["dataset"]["arrives"], "deidentified");
        assert_eq!(study_doc["dataset"]["cohort"], "a");

        // the new registry, with the setup's own places declared already
        let new_dir = dir.join("new");
        let mut new = registry(&new_dir, postgres.map(|d| (d, new_schema)));
        add(
            &mut new,
            "archives",
            Role::Backup,
            &data.join("archives"),
            json!({"protected": true}),
            Value::Null,
        );
        add(
            &mut new,
            "source",
            Role::Source,
            &data.join("source"),
            json!({}),
            Value::Null,
        );
        let before = tree(&data);

        // a skip that names nothing is refused, and writes nothing
        let wrong = import(&mut new, &doc, &["no-such".into()], false).unwrap_err();
        assert!(wrong.contains("--skip no-such"), "{wrong}");
        // the registry place is the old registry's folder: refused by name
        // conflict nowhere, since the new one has none by that name; a print
        // writes nothing
        let printed = import(&mut new, &doc, &["test-data".into()], true).unwrap();
        let said =
            |p: &[Planned], name: &str| p.iter().find(|x| x.name == name).unwrap().outcome.clone();
        assert_eq!(said(&printed, "archives"), Outcome::Kept);
        assert_eq!(said(&printed, "source"), Outcome::Kept);
        assert_eq!(said(&printed, "scratch-nvme"), Outcome::Added);
        assert_eq!(said(&printed, "study-a"), Outcome::Added);
        assert_eq!(said(&printed, "test-data"), Outcome::Skipped);
        assert_eq!(
            place::active(new.store()).unwrap().len(),
            2,
            "--print declared a place"
        );

        let done = import(&mut new, &doc, &["test-data".into()], false).unwrap();
        assert!(
            done.iter()
                .all(|p| p.outcome == Outcome::Skipped || p.id.is_some())
        );
        let active = place::active(new.store()).unwrap();
        let mut got: Vec<&str> = active.iter().map(|p| p.name.as_str()).collect();
        got.sort();
        assert_eq!(
            got,
            ["archives", "registry", "scratch-nvme", "source", "study-a"]
        );
        let study_here = place::by_name(new.store(), "study-a").unwrap().unwrap();
        assert_eq!(study_here.dataset["arrives"], "deidentified");
        assert_eq!(study_here.dataset["root"], "source");
        assert_eq!(study_here.dataset["trees"]["anon"], place::ANON_TREE);
        let registry_here = place::by_name(new.store(), "registry").unwrap().unwrap();
        assert_eq!(registry_here.guarantees["backup"], "archives");
        // nothing on disk was looked into, moved or probed
        assert_eq!(tree(&data), before, "the import changed the folders");
        // one audit row a place added
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE action = 'place.add'",
            new.store().qualified("audit")
        );
        let rows = new
            .store()
            .query_opt(&sql, &[])
            .unwrap()
            .unwrap()
            .int(0)
            .unwrap();
        assert_eq!(rows, 3);

        // again: every place is there now, and nothing more is added
        let again = import(&mut new, &doc, &["test-data".into()], false).unwrap();
        assert!(
            again
                .iter()
                .all(|p| matches!(p.outcome, Outcome::Kept | Outcome::Skipped)),
            "{again:?}"
        );

        // a name taken by another kind of place is refused before anything
        let mut other = doc.clone();
        other["places"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name": "source", "role": "working", "path": "/elsewhere"}));
        let twice = import(&mut new, &other, &[], false).unwrap_err();
        assert!(twice.contains("twice"), "{twice}");
        let mut clash = doc.clone();
        for p in clash["places"].as_array_mut().unwrap() {
            if p["name"] == "scratch-nvme" {
                p["path"] = json!("/somewhere/else");
            }
        }
        let refused = import(&mut new, &clash, &[], false).unwrap_err();
        assert!(refused.contains("scratch-nvme"), "{refused}");
        assert!(refused.contains("nothing was imported"), "{refused}");
        assert!(
            place::by_name(new.store(), "test-data").unwrap().is_none(),
            "a refused import declared a place"
        );

        if let Some(dsn) = postgres {
            drop_schemas(dsn, old_schema);
            drop_schemas(dsn, new_schema);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn places_exported_from_one_registry_are_declared_in_another_as_they_were() {
        exported_then_imported(None);
    }

    #[test]
    fn places_exported_and_imported_on_postgres() {
        match std::env::var("NILS_TEST_POSTGRES_DSN") {
            Ok(dsn) if !dsn.is_empty() => exported_then_imported(Some(&dsn)),
            _ => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres test is skipped"),
        }
    }

    #[test]
    fn a_document_of_another_kind_or_version_is_refused() {
        let dir = scratch("refused");
        let mut r = registry(&dir.join("reg"), None);
        let not_ours = import(&mut r, &json!({"places": []}), &[], false).unwrap_err();
        assert!(not_ours.contains(FORMAT), "{not_ours}");
        let later = import(
            &mut r,
            &json!({"format": FORMAT, "version": VERSION + 1, "places": []}),
            &[],
            false,
        )
        .unwrap_err();
        assert!(later.contains("version"), "{later}");
        let relative = import(
            &mut r,
            &json!({"format": FORMAT, "version": VERSION,
                    "places": [{"name": "x", "role": "working", "path": "rel/dir"}]}),
            &[],
            false,
        )
        .unwrap_err();
        assert!(relative.contains("absolute"), "{relative}");
        let dataset_on_working = import(
            &mut r,
            &json!({"format": FORMAT, "version": VERSION,
                    "places": [{"name": "x", "role": "working", "path": "/x", "dataset": {"arrives": "identified"}}]}),
            &[],
            false,
        )
        .unwrap_err();
        assert!(
            dataset_on_working.contains("source place"),
            "{dataset_on_working}"
        );
        let no_backup = import(
            &mut r,
            &json!({"format": FORMAT, "version": VERSION,
                    "places": [{"name": "reg", "role": "registry", "path": "/r", "guarantees": {"backup": "nowhere"}}]}),
            &[],
            false,
        )
        .unwrap_err();
        assert!(no_backup.contains("nowhere"), "{no_backup}");
        assert!(place::active(r.store()).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
