// SPDX-License-Identifier: AGPL-3.0-only

//! A place as a registry object (Wave 5 §12.5, §10.2, D74).
//!
//! A place is a named location with a role and the guarantees behind it.
//! Every path the engine takes is bound to a place by role rather than
//! typed, and the rules are checked, not documented: a release writes only
//! to an `export` place, a question's subset only to a `share` place, the
//! `registry` role is refused on a place without a backup, and a `source`
//! place is written by the pseudonymiser only, inside the dataset's own
//! trees. What the operator declares is `guarantees`; what the engine
//! measured is `probed`.
//!
//! A source place is a dataset (record 26): one folder with one name, what
//! arrives in it, its two trees, the identity rule its files are read
//! under, what an unmapped identifier does, the cohort it feeds, its tag
//! lists and what becomes of the originals. The `dataset` column holds
//! that; [`dataset_of`] checks it and fills what it does not name.

use std::path::Path;

use serde_json::{Value, json};

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Store};
use crate::time::now_iso;

/// The seven roles of §10.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Source,
    Registry,
    Working,
    Export,
    Share,
    Exchange,
    Backup,
}

impl Role {
    pub const ALL: [Role; 7] = [
        Role::Source,
        Role::Registry,
        Role::Working,
        Role::Export,
        Role::Share,
        Role::Exchange,
        Role::Backup,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Role::Source => "source",
            Role::Registry => "registry",
            Role::Working => "working",
            Role::Export => "export",
            Role::Share => "share",
            Role::Exchange => "exchange",
            Role::Backup => "backup",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == text)
    }

    /// What the role holds, for a listing.
    pub fn holds(self) -> &'static str {
        match self {
            Role::Source => {
                "a dataset: the originals, and the pseudonymised tree the registry reads"
            }
            Role::Registry => "the registry, the linkage store, the keys",
            Role::Working => "digests in flight, viewing pyramids, rehearsals",
            Role::Export => "releases and BIDS trees",
            Role::Share => "subsets a question wrote out, results handed to a colleague",
            Role::Exchange => "what leaves or arrives through a bridge",
            Role::Backup => "the engine's archives",
        }
    }

    /// What the role must guarantee, for a listing.
    pub fn must(self) -> &'static str {
        match self {
            Role::Source => {
                "protected storage with snapshots; written by the pseudonymiser only, inside its own trees"
            }
            Role::Registry => "protected storage and a routine backup elsewhere",
            Role::Working => "fast; may be lost",
            Role::Export => "protected storage with snapshots",
            Role::Share => "reachable by the group",
            Role::Exchange => "its own rules",
            Role::Backup => "protected, elsewhere from the registry",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Place {
    pub id: i64,
    pub name: String,
    pub role: Role,
    pub path: String,
    /// What the operator declared: `{snapshots, backup, protected, fast}`.
    pub guarantees: Value,
    /// What the engine measured last: `{exists, directory, writable, free_bytes, mount, snapshots_seen}`.
    pub probed: Value,
    pub probed_at: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub retired_at: Option<String>,
    /// How what comes in through it is handled, as the operator declared it;
    /// null until declared, which reads as the defaults.
    pub handling: Value,
    /// The dataset a source place is (record 26): what arrives, the two
    /// trees, the identity rule, what an unmapped identifier does, the
    /// cohort it feeds, the tag lists and what becomes of the originals.
    /// Null on a place of another role, and on a source place from before
    /// it was declared, which reads as the defaults: the folder itself is
    /// the pseudonymised tree.
    pub dataset: Value,
}

impl Place {
    pub fn as_json(&self) -> Value {
        let dataset = (self.role == Role::Source).then(|| self.dataset_doc());
        let mut handling = handling_of(&self.handling).unwrap_or_else(|_| default_handling());
        // `arrives` lives on the dataset now; the handling mirrors it for a
        // reader from before
        if let Some(d) = &dataset {
            handling["arrives"] = d["arrives"].clone();
        }
        json!({
            "id": self.id,
            "name": self.name,
            "role": self.role.name(),
            "path": self.path,
            "guarantees": self.guarantees,
            "probed": self.probed,
            "probed_at": self.probed_at,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "retired_at": self.retired_at,
            "retired": self.retired_at.is_some(),
            "handling": handling,
            "handling_declared": self.handling.is_object(),
            "dataset": dataset,
        })
    }

    /// The dataset as a page reads it: the stored fields, with each tree as
    /// its path under the place and the counts the last probe kept
    /// (`probed.trees`), null where none was measured.
    pub fn dataset_doc(&self) -> Value {
        let mut doc = dataset_of(&self.dataset, None).unwrap_or_else(|_| default_dataset(None));
        let counts = &self.probed["trees"];
        let tree = |rel: Option<&str>, counted: &Value| -> Value {
            let Some(rel) = rel else {
                return Value::Null;
            };
            let path = if rel == "." {
                Path::new(&self.path).to_path_buf()
            } else {
                Path::new(&self.path).join(rel)
            };
            json!({
                "path": path.display().to_string(),
                "files": counted["files"],
                "bytes": counted["bytes"],
                "last_written": counted["last_written"],
                "partial": counted["partial"],
            })
        };
        let trees = doc["trees"].clone();
        doc["trees"] = json!({
            "originals": tree(trees["originals"].as_str(), &counts["originals"]),
            "anon": tree(trees["anon"].as_str(), &counts["anon"]),
        });
        doc
    }

    /// The path of one of a dataset's trees under the place, as stored:
    /// `originals` or `anon`; none for a tree the dataset has not got.
    pub fn tree_path(&self, tree: &str) -> Option<std::path::PathBuf> {
        let doc = dataset_of(&self.dataset, None).ok()?;
        let rel = doc["trees"][tree].as_str()?;
        Some(if rel == "." {
            Path::new(&self.path).to_path_buf()
        } else {
            Path::new(&self.path).join(rel)
        })
    }

    /// Whether a path lies under this place.
    pub fn holds_path(&self, path: &Path) -> bool {
        let mine = Path::new(&self.path);
        let mine = std::fs::canonicalize(mine).unwrap_or_else(|_| mine.to_path_buf());
        let theirs = canonical_prefix(path);
        theirs.starts_with(&mine)
    }
}

/// The longest existing prefix of a path, canonicalised, with the rest
/// appended: a release target that does not exist yet is still compared
/// under its real parent.
fn canonical_prefix(path: &Path) -> std::path::PathBuf {
    let mut rest = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(c) = std::fs::canonicalize(&cur) {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (cur.file_name().map(|f| f.to_os_string()), cur.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cur = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Why a place is refused, as a sentence naming the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The rule on the `registry` role (§10.2): refused on a place whose
/// guarantees name no backup place, or name one that is not a `backup`
/// place.
pub fn check_registry_rule(store: &mut Store, role: Role, guarantees: &Value) -> Result<(), Error> {
    if role != Role::Registry {
        return Ok(());
    }
    let Some(backup) = guarantees["backup"]
        .as_str()
        .filter(|b| !b.trim().is_empty())
    else {
        return Err(Error::Message(Refusal(
            "the registry role is refused on a place without a backup: name a backup place in guarantees.backup (Wave 5 section 10.2)".into(),
        ).0));
    };
    match by_name(store, backup)? {
        Some(p) if p.role == Role::Backup && p.retired_at.is_none() => Ok(()),
        Some(p) => Err(Error::Message(format!(
            "the registry role is refused: {backup} is a {} place, not a backup place (Wave 5 section 10.2)",
            p.role.name()
        ))),
        None => Err(Error::Message(format!(
            "the registry role is refused: no place is named {backup}; add the backup place first (Wave 5 section 10.2)"
        ))),
    }
}

pub struct New<'a> {
    pub name: &'a str,
    pub role: Role,
    pub path: &'a str,
    pub guarantees: Value,
    pub probed: Value,
    /// Null takes the defaults; an object is checked by `handling_of`.
    pub handling: Value,
    /// The dataset a source place is; null reads as the defaults. An object
    /// is checked by `dataset_of`, and refused on any other role.
    pub dataset: Value,
}

pub fn add(store: &mut Store, p: &New<'_>) -> Result<i64, Error> {
    if p.name.trim().is_empty() || p.name.contains('/') || p.name.contains(char::is_whitespace) {
        return Err(Error::Message(
            "a place's name is one word without a slash".into(),
        ));
    }
    if by_name(store, p.name)?.is_some() {
        return Err(Error::Message(format!(
            "a place is already named {}",
            p.name
        )));
    }
    check_registry_rule(store, p.role, &p.guarantees)?;
    let handling = if p.handling.is_null() {
        Value::Null
    } else {
        handling_of(&p.handling).map_err(Error::Message)?
    };
    let dataset = if p.dataset.is_null() {
        Value::Null
    } else if p.role != Role::Source {
        return Err(Error::Message(format!(
            "a dataset is a source place; {} is a {} place",
            p.name,
            p.role.name()
        )));
    } else {
        dataset_of(&p.dataset, None).map_err(Error::Message)?
    };
    let now = now_iso();
    let rows = store.insert(
        &Insert::new(
            table("place"),
            &[
                "name",
                "role",
                "path",
                "guarantees",
                "probed",
                "probed_at",
                "created_at",
                "handling",
                "dataset",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(p.name),
            Param::from(p.role.name()),
            Param::from(p.path),
            Param::from(p.guarantees.to_string()),
            Param::from(p.probed.to_string()),
            Param::from(now.as_str()),
            Param::from(now.as_str()),
            if handling.is_null() {
                Param::Null
            } else {
                Param::from(handling.to_string())
            },
            if dataset.is_null() {
                Param::Null
            } else {
                Param::from(dataset.to_string())
            },
        ]],
    )?;
    rows.first()
        .ok_or(Error::Message("no id returned".into()))?
        .int(0)
}

/// Change a place's path or guarantees, or record a fresh probe.
pub fn set(
    store: &mut Store,
    id: i64,
    path: Option<&str>,
    guarantees: Option<&Value>,
    probed: Option<&Value>,
) -> Result<Place, Error> {
    let current = show(store, id)?.ok_or_else(|| Error::Message(format!("no place {id}")))?;
    if let Some(g) = guarantees {
        check_registry_rule(store, current.role, g)?;
    }
    let d = store.dialect();
    let now = now_iso();
    let mut sets = Vec::new();
    let mut params: Vec<Param> = Vec::new();
    let mut n = 0;
    let mut bind =
        |sets: &mut Vec<String>, params: &mut Vec<Param>, column: &str, ty: Type, value: Param| {
            n += 1;
            sets.push(format!("{column} = {}", d.param(n, ty)));
            params.push(value);
        };
    if let Some(p) = path {
        bind(&mut sets, &mut params, "path", Type::Text, Param::from(p));
    }
    if let Some(g) = guarantees {
        bind(
            &mut sets,
            &mut params,
            "guarantees",
            Type::Json,
            Param::from(g.to_string()),
        );
    }
    if let Some(p) = probed {
        bind(
            &mut sets,
            &mut params,
            "probed",
            Type::Json,
            Param::from(p.to_string()),
        );
        bind(
            &mut sets,
            &mut params,
            "probed_at",
            Type::Timestamp,
            Param::from(now.as_str()),
        );
    }
    if path.is_some() || guarantees.is_some() {
        bind(
            &mut sets,
            &mut params,
            "updated_at",
            Type::Timestamp,
            Param::from(now.as_str()),
        );
    }
    if sets.is_empty() {
        return Ok(current);
    }
    n += 1;
    params.push(Param::Int(id));
    store.execute(
        &format!(
            "UPDATE {} SET {} WHERE id = {}",
            store.qualified("place"),
            sets.join(", "),
            d.param(n, Type::Int)
        ),
        &params,
    )?;
    show(store, id)?.ok_or_else(|| Error::Message(format!("no place {id}")))
}

/// A key of a document: the value given, else the one in force, else the
/// default; a value that is not one of the choices is refused with them.
fn pick(
    v: &Value,
    current: Option<&Value>,
    key: &str,
    choices: &[&str],
    default: &str,
) -> Result<String, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(current
            .and_then(|c| c[key].as_str())
            .filter(|s| choices.contains(s))
            .unwrap_or(default)
            .to_string()),
        Some(Value::String(s)) if choices.contains(&s.as_str()) => Ok(s.clone()),
        Some(other) => Err(format!(
            "{key} is one of {}, not {other}",
            choices.join(", ")
        )),
    }
}

/// The three ways data arrives in a dataset (record 26 §2).
pub const ARRIVALS: [&str; 3] = ["identified", "deidentified", "coded"];

/// How what comes in through a place is handled, as the operator declares it:
/// whether it arrives identified, and what a release does to it on the way
/// out. A key not given takes its default; a value not known is refused with
/// the choices. Moving dates while preserving UIDs is refused, as a release's
/// policy refuses it. `arrives` lives on the dataset since record 26 and is
/// mirrored here for a reader from before.
pub fn handling_of(doc: &Value) -> Result<Value, String> {
    if !(doc.is_object() || doc.is_null()) {
        return Err("handling is an object: {arrives, on_release: {dates, uids, deface}}".into());
    }
    let arrives = pick(doc, None, "arrives", &ARRIVALS, "identified")?;
    let release = doc.get("on_release").cloned().unwrap_or(Value::Null);
    if !(release.is_object() || release.is_null()) {
        return Err("on_release is an object: {dates, uids, deface}".into());
    }
    let dates = pick(&release, None, "dates", &["keep", "shift", "year"], "keep")?;
    let uids = pick(&release, None, "uids", &["remap", "preserve"], "remap")?;
    let deface = match release.get("deface") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(other) => return Err(format!("deface is true or false, not {other}")),
    };
    if dates != "keep" && uids == "preserve" {
        return Err(
            "dates that move cannot keep the original UIDs: preserve UIDs only with dates kept"
                .into(),
        );
    }
    Ok(json!({
        "arrives": arrives,
        "on_release": {"dates": dates, "uids": uids, "deface": deface},
    }))
}

/// The handling a place has until one is declared.
pub fn default_handling() -> Value {
    handling_of(&Value::Null).expect("the defaults are a handling")
}

/// Declare how what comes in through a place is handled; the value is checked
/// and stored whole.
pub fn set_handling(store: &mut Store, id: i64, handling: &Value) -> Result<Place, Error> {
    let checked = handling_of(handling).map_err(Error::Message)?;
    set_json(store, id, "handling", &checked)
}

/// The two trees of a dataset, under the place's path (record 26 §1). A
/// dataset with no such layout reads the folder itself, `.`, as its
/// pseudonymised tree.
pub const ORIGINALS_TREE: &str = "derivatives/dcm-original";
pub const ANON_TREE: &str = "derivatives/dcm-anon";

/// The dataset a source place is, as the operator declares it: what
/// arrives (`identified`, `deidentified` or `coded`), the two trees as the
/// engine found or made them, the identity rule as `nils digest
/// --identity-rule` reads it or null, what an unmapped identifier does
/// (`hold` or `code`), the cohort every digest of it feeds or null, the tag
/// lists (`keep_demographics`, `remove`, `keep`, each tag as `gggg,eeee`)
/// and what becomes of the originals (`kept`, `vaulted` or `purged`). A key
/// not given keeps what is in force, or takes its default: de-identified,
/// the folder itself as the pseudonymised tree, no rule, `hold` for
/// identified arrivals and `code` otherwise, no cohort, demographics kept,
/// the originals kept. A value not known is refused with the choices.
pub fn dataset_of(doc: &Value, current: Option<&Value>) -> Result<Value, String> {
    if !(doc.is_object() || doc.is_null()) {
        return Err("dataset is an object: {arrives, trees, identity, unmapped, cohort, tags, originals_kept}".into());
    }
    let current = current.filter(|c| c.is_object());
    let arrives = pick(doc, current, "arrives", &ARRIVALS, "deidentified")?;
    let trees = match doc.get("trees") {
        Some(t) if t.is_object() => t.clone(),
        Some(Value::Null) | None => current
            .map(|c| c["trees"].clone())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({"originals": null, "anon": "."})),
        Some(other) => {
            return Err(format!(
                "trees is an object {{originals, anon}}, not {other}"
            ));
        }
    };
    let originals = match &trees["originals"] {
        Value::Null => Value::Null,
        Value::String(s) if s == ORIGINALS_TREE => Value::String(s.clone()),
        other => {
            return Err(format!(
                "trees.originals is {ORIGINALS_TREE} or null, not {other}"
            ));
        }
    };
    let anon = match &trees["anon"] {
        Value::Null => Value::String(".".into()),
        Value::String(s) if s == ANON_TREE || s == "." => Value::String(s.clone()),
        other => return Err(format!("trees.anon is {ANON_TREE} or ., not {other}")),
    };
    let identity = match doc.get("identity") {
        Some(Value::Null) => Value::Null,
        Some(v) if v.is_object() => v.clone(),
        Some(other) => {
            return Err(format!(
                "identity is the rule's identity block as an object, or null, not {other}"
            ));
        }
        None => current
            .map(|c| c["identity"].clone())
            .unwrap_or(Value::Null),
    };
    let unmapped = pick(
        doc,
        current,
        "unmapped",
        &["hold", "code"],
        if arrives == "identified" {
            "hold"
        } else {
            "code"
        },
    )?;
    let cohort = match doc.get("cohort") {
        Some(Value::Null) => Value::Null,
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() || s.contains(char::is_whitespace) || s.contains('/') {
                return Err("cohort is one word without a slash, or null".into());
            }
            Value::String(s.to_string())
        }
        Some(other) => return Err(format!("cohort is a name or null, not {other}")),
        None => current.map(|c| c["cohort"].clone()).unwrap_or(Value::Null),
    };
    let tags = tags_of(doc.get("tags"), current.map(|c| &c["tags"]))?;
    let originals_kept = pick(
        doc,
        current,
        "originals_kept",
        &["kept", "vaulted", "purged"],
        "kept",
    )?;
    Ok(json!({
        "arrives": arrives,
        "trees": {"originals": originals, "anon": anon},
        "identity": identity,
        "unmapped": unmapped,
        "cohort": cohort,
        "tags": tags,
        "originals_kept": originals_kept,
    }))
}

/// A dataset's tag lists: whether sex, weight and size are kept, the tags
/// removed beside the four groups the pseudonymiser always removes, and the
/// tags kept out of them. A tag is `gggg,eeee` in hex, upper-cased here; a
/// tag on both lists is refused.
fn tags_of(doc: Option<&Value>, current: Option<&Value>) -> Result<Value, String> {
    let current = current.filter(|c| c.is_object());
    let doc = match doc {
        None | Some(Value::Null) => current.cloned().unwrap_or(Value::Null),
        Some(v) if v.is_object() => v.clone(),
        Some(other) => {
            return Err(format!(
                "tags is an object {{keep_demographics, remove, keep}}, not {other}"
            ));
        }
    };
    let keep_demographics = match doc.get("keep_demographics") {
        None | Some(Value::Null) => current
            .and_then(|c| c["keep_demographics"].as_bool())
            .unwrap_or(true),
        Some(Value::Bool(b)) => *b,
        Some(other) => {
            return Err(format!(
                "tags.keep_demographics is true or false, not {other}"
            ));
        }
    };
    let list = |key: &str| -> Result<Vec<String>, String> {
        let given = match doc.get(key) {
            None | Some(Value::Null) => {
                return Ok(current
                    .and_then(|c| c[key].as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect());
            }
            Some(Value::Array(list)) => list,
            Some(other) => return Err(format!("tags.{key} is a list of gggg,eeee, not {other}")),
        };
        let mut out = Vec::with_capacity(given.len());
        for v in given {
            let tag = v
                .as_str()
                .map(str::trim)
                .filter(|t| {
                    t.len() == 9
                        && t.as_bytes()[4] == b','
                        && t.bytes()
                            .enumerate()
                            .all(|(i, b)| i == 4 || b.is_ascii_hexdigit())
                })
                .ok_or_else(|| format!("tags.{key}: {v} is not a tag as gggg,eeee"))?
                .to_ascii_uppercase();
            if !out.contains(&tag) {
                out.push(tag);
            }
        }
        Ok(out)
    };
    let remove = list("remove")?;
    let keep = list("keep")?;
    if let Some(both) = remove.iter().find(|t| keep.contains(t)) {
        return Err(format!("tags: {both} is on both remove and keep"));
    }
    Ok(json!({"keep_demographics": keep_demographics, "remove": remove, "keep": keep}))
}

/// The dataset a source place is until one is declared: de-identified
/// unless said otherwise, reading the folder itself.
pub fn default_dataset(arrives: Option<&str>) -> Value {
    let arrives = arrives
        .filter(|a| ARRIVALS.contains(a))
        .unwrap_or("deidentified");
    dataset_of(&json!({"arrives": arrives}), None).expect("the defaults are a dataset")
}

/// Declare the dataset a source place is; the value is checked and stored
/// whole, and refused on a place of another role.
pub fn set_dataset(store: &mut Store, id: i64, dataset: &Value) -> Result<Place, Error> {
    let current = show(store, id)?.ok_or_else(|| Error::Message(format!("no place {id}")))?;
    if current.role != Role::Source {
        return Err(Error::Message(format!(
            "a dataset is a source place; {} is a {} place",
            current.name,
            current.role.name()
        )));
    }
    let checked = dataset_of(dataset, None).map_err(Error::Message)?;
    set_json(store, id, "dataset", &checked)
}

/// Write one JSON column of a place and stamp `updated_at`.
fn set_json(store: &mut Store, id: i64, column: &str, value: &Value) -> Result<Place, Error> {
    let d = store.dialect();
    let now = now_iso();
    store.execute(
        &format!(
            "UPDATE {} SET {column} = {}, updated_at = {} WHERE id = {}",
            store.qualified("place"),
            d.param(1, Type::Json),
            d.param(2, Type::Timestamp),
            d.param(3, Type::Int)
        ),
        &[
            Param::from(value.to_string()),
            Param::from(now.as_str()),
            Param::Int(id),
        ],
    )?;
    show(store, id)?.ok_or_else(|| Error::Message(format!("no place {id}")))
}

/// Retire a place: it stays as history, binds nothing, and its name is free
/// to refuse a clash with, never to reuse.
pub fn retire(store: &mut Store, id: i64) -> Result<Place, Error> {
    let d = store.dialect();
    let now = now_iso();
    store.execute(
        &format!(
            "UPDATE {} SET retired_at = {} WHERE id = {} AND retired_at IS NULL",
            store.qualified("place"),
            d.param(1, Type::Timestamp),
            d.param(2, Type::Int)
        ),
        &[Param::from(now.as_str()), Param::Int(id)],
    )?;
    show(store, id)?.ok_or_else(|| Error::Message(format!("no place {id}")))
}

fn select_sql(store: &Store, filter: &str) -> String {
    let d = store.dialect();
    let t = table("place");
    let text = |c: &str| d.text_of(t.column(c).expect("place column"));
    format!(
        "SELECT id, name, role, path, {}, {}, {}, {}, {}, {}, {}, {} FROM {}{filter} ORDER BY id",
        text("guarantees"),
        text("probed"),
        text("probed_at"),
        text("created_at"),
        text("updated_at"),
        text("retired_at"),
        text("handling"),
        text("dataset"),
        store.qualified("place"),
    )
}

fn of(r: &crate::store::Row) -> Result<Place, Error> {
    let json = |s: Option<&str>| {
        s.and_then(|t| serde_json::from_str::<Value>(t).ok())
            .unwrap_or(Value::Null)
    };
    let role_text = r.text(2)?;
    Ok(Place {
        id: r.int(0)?,
        name: r.text(1)?.to_string(),
        role: Role::parse(role_text)
            .ok_or_else(|| Error::Message(format!("{role_text} is not a role of a place")))?,
        path: r.text(3)?.to_string(),
        guarantees: json(r.opt_text(4)?),
        probed: json(r.opt_text(5)?),
        probed_at: r.opt_text(6)?.map(str::to_string),
        created_at: r.text(7)?.to_string(),
        updated_at: r.opt_text(8)?.map(str::to_string),
        retired_at: r.opt_text(9)?.map(str::to_string),
        handling: json(r.opt_text(10)?),
        dataset: json(r.opt_text(11)?),
    })
}

/// The source place whose tree of that name (`originals` or `anon`) holds
/// a path; none when no active dataset's tree does. A dataset reading its
/// folder itself has no originals, and its `anon` tree is the folder.
pub fn tree_holding(store: &mut Store, tree: &str, path: &Path) -> Result<Option<Place>, Error> {
    Ok(active(store)?
        .into_iter()
        .filter(|p| p.role == Role::Source)
        .find(|p| {
            p.tree_path(tree).is_some_and(|t| {
                let real = std::fs::canonicalize(&t).unwrap_or(t);
                canonical_prefix(path).starts_with(&real)
            })
        }))
}

/// Every place, retired ones included.
pub fn list(store: &mut Store) -> Result<Vec<Place>, Error> {
    let sql = select_sql(store, "");
    store.query(&sql, &[])?.iter().map(of).collect()
}

/// The places in force: not retired.
pub fn active(store: &mut Store) -> Result<Vec<Place>, Error> {
    Ok(list(store)?
        .into_iter()
        .filter(|p| p.retired_at.is_none())
        .collect())
}

pub fn show(store: &mut Store, id: i64) -> Result<Option<Place>, Error> {
    let d = store.dialect();
    let sql = select_sql(store, &format!(" WHERE id = {}", d.param(1, Type::Int)));
    match store.query_opt(&sql, &[Param::Int(id)])? {
        Some(r) => Ok(Some(of(&r)?)),
        None => Ok(None),
    }
}

pub fn by_name(store: &mut Store, name: &str) -> Result<Option<Place>, Error> {
    let d = store.dialect();
    let sql = select_sql(store, &format!(" WHERE name = {}", d.param(1, Type::Text)));
    match store.query_opt(&sql, &[Param::from(name)])? {
        Some(r) => Ok(Some(of(&r)?)),
        None => Ok(None),
    }
}

/// The place in force that holds a path, by role; None when no active
/// place of that role holds it.
pub fn holding(store: &mut Store, role: Role, path: &Path) -> Result<Option<Place>, Error> {
    Ok(active(store)?
        .into_iter()
        .filter(|p| p.role == role)
        .find(|p| p.holds_path(path)))
}

/// Any place in force that holds a path, whatever its role.
pub fn any_holding(store: &mut Store, path: &Path) -> Result<Option<Place>, Error> {
    Ok(active(store)?.into_iter().find(|p| p.holds_path(path)))
}

#[cfg(test)]
mod tests {
    use super::{
        ANON_TREE, ORIGINALS_TREE, dataset_of, default_dataset, default_handling, handling_of,
    };
    use serde_json::json;

    #[test]
    fn a_dataset_not_declared_arrives_deidentified_and_reads_its_folder() {
        assert_eq!(
            default_dataset(None),
            json!({
                "arrives": "deidentified",
                "trees": {"originals": null, "anon": "."},
                "identity": null,
                "unmapped": "code",
                "cohort": null,
                "tags": {"keep_demographics": true, "remove": [], "keep": []},
                "originals_kept": "kept",
            })
        );
        assert_eq!(dataset_of(&json!({}), None).unwrap(), default_dataset(None));
        assert_eq!(
            dataset_of(&json!(null), None).unwrap(),
            default_dataset(None)
        );
        // an arrival the handling of before named is kept; one it could not is not
        assert_eq!(default_dataset(Some("identified"))["arrives"], "identified");
        assert_eq!(default_dataset(Some("maybe"))["arrives"], "deidentified");
    }

    #[test]
    fn a_dataset_fills_what_it_does_not_name_and_refuses_what_it_does_not_know() {
        let d = dataset_of(
            &json!({
                "arrives": "identified",
                "trees": {"originals": ORIGINALS_TREE, "anon": ANON_TREE},
                "identity": {"id_type": "study-id", "from": [{"field": "PatientID"}]},
                "cohort": "ms",
                "tags": {"remove": ["0010,1010", "0010,1010"], "keep": ["0008,0080"]},
            }),
            None,
        )
        .unwrap();
        // identified arrivals hold an unmapped identifier unless told otherwise
        assert_eq!(d["unmapped"], "hold");
        assert_eq!(d["originals_kept"], "kept");
        assert_eq!(d["tags"]["keep_demographics"], true);
        assert_eq!(d["tags"]["remove"], json!(["0010,1010"]));
        assert_eq!(d["identity"]["id_type"], "study-id");
        for (doc, what) in [
            (
                json!({"arrives": "maybe"}),
                "identified, deidentified, coded",
            ),
            (json!({"trees": {"anon": "elsewhere"}}), ANON_TREE),
            (
                json!({"trees": {"originals": "derivatives"}}),
                ORIGINALS_TREE,
            ),
            (json!({"identity": "a rule"}), "identity block"),
            (json!({"unmapped": "ask"}), "hold, code"),
            (json!({"cohort": "two words"}), "one word"),
            (json!({"tags": {"remove": ["0010-1010"]}}), "gggg,eeee"),
            (
                json!({"tags": {"remove": ["0010,1010"], "keep": ["0010,1010"]}}),
                "both remove and keep",
            ),
            (
                json!({"tags": {"keep_demographics": "yes"}}),
                "true or false",
            ),
            (json!({"originals_kept": "lost"}), "kept, vaulted, purged"),
        ] {
            let why = dataset_of(&doc, None).unwrap_err();
            assert!(why.contains(what), "{doc}: {why}");
        }
        assert!(dataset_of(&json!("identified"), None).is_err());
    }

    #[test]
    fn a_dataset_changed_keeps_what_the_change_does_not_name() {
        let before = dataset_of(
            &json!({"arrives": "identified", "cohort": "ms", "tags": {"remove": ["0010,1010"]}, "identity": {"id_type": "study-id", "from": []}}),
            None,
        )
        .unwrap();
        let after =
            dataset_of(&json!({"cohort": null, "unmapped": "code"}), Some(&before)).unwrap();
        assert_eq!(after["arrives"], "identified");
        assert_eq!(after["cohort"], json!(null));
        assert_eq!(after["unmapped"], "code");
        assert_eq!(after["tags"]["remove"], json!(["0010,1010"]));
        assert_eq!(after["identity"], before["identity"]);
        // a tag list given replaces the one in force; one not given stays
        let after = dataset_of(&json!({"tags": {"keep": ["0008,0080"]}}), Some(&before)).unwrap();
        assert_eq!(after["tags"]["remove"], json!(["0010,1010"]));
        assert_eq!(after["tags"]["keep"], json!(["0008,0080"]));
        // the trees are kept until the engine sets them again
        let after = dataset_of(
            &json!({"arrives": "deidentified"}),
            Some(&json!({"arrives": "identified", "trees": {"originals": ORIGINALS_TREE, "anon": ANON_TREE}})),
        )
        .unwrap();
        assert_eq!(after["trees"]["originals"], ORIGINALS_TREE);
    }

    #[test]
    fn a_handling_not_declared_arrives_identified_keeps_dates_and_remaps_uids() {
        assert_eq!(
            default_handling(),
            json!({"arrives": "identified", "on_release": {"dates": "keep", "uids": "remap", "deface": false}})
        );
        assert_eq!(handling_of(&json!({})).unwrap(), default_handling());
    }

    #[test]
    fn a_handling_fills_what_it_does_not_name_and_refuses_what_it_does_not_know() {
        let h = handling_of(
            &json!({"arrives": "deidentified", "on_release": {"dates": "shift", "deface": true}}),
        )
        .unwrap();
        assert_eq!(h["arrives"], "deidentified");
        assert_eq!(
            h["on_release"],
            json!({"dates": "shift", "uids": "remap", "deface": true})
        );
        let why = handling_of(&json!({"arrives": "maybe"})).unwrap_err();
        assert!(why.contains("identified, deidentified"), "{why}");
        assert!(handling_of(&json!({"on_release": {"deface": "yes"}})).is_err());
        assert!(handling_of(&json!("identified")).is_err());
    }

    #[test]
    fn dates_that_move_cannot_keep_the_original_uids() {
        for dates in ["shift", "year"] {
            let why = handling_of(&json!({"on_release": {"dates": dates, "uids": "preserve"}}))
                .unwrap_err();
            assert!(why.contains("preserve UIDs only with dates kept"), "{why}");
        }
        assert!(handling_of(&json!({"on_release": {"dates": "keep", "uids": "preserve"}})).is_ok());
    }
}
