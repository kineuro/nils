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
//!
//! A source place whose arrival has not been declared is `undeclared`
//! (Wave 7a §5.3): it has no tree, and nothing in it is read until a person
//! says how its files arrive. That is the default everywhere a dataset or a
//! handling is read, so a folder without the layout is never read as
//! de-identified.

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
    /// Null on a place of another role. A source place always has one:
    /// one whose arrival was never declared is `undeclared` and has no tree.
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

    /// Whether a path lies under this place: under its folder, or under one
    /// of its dataset's trees, each side resolved the same way. A tree that
    /// is a symbolic link (derivatives/dcm-anon pointing elsewhere) resolves
    /// outside the folder, and a digest keeps that resolved root, so the
    /// trees are matched too.
    pub fn holds_path(&self, path: &Path) -> bool {
        let theirs = canonical_prefix(path);
        let under = |mine: &Path| theirs.starts_with(canonical_prefix(mine));
        under(Path::new(&self.path))
            || ["originals", "anon"]
                .iter()
                .filter_map(|tree| self.tree_path(tree))
                .any(|tree| under(&tree))
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
        // Wave 7a §5.3: a source place is a dataset from the start, and
        // undeclared until a person says how its files arrive
        if p.role == Role::Source {
            default_dataset(None)
        } else {
            Value::Null
        }
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

/// The ways data arrives in a dataset (record 26 §2), and `undeclared`
/// (Wave 7a §5.3): nobody has said yet, so nothing in it is read.
pub const ARRIVALS: [&str; 4] = [UNDECLARED, "identified", "deidentified", "coded"];

/// What a dataset's pseudonymiser writes into PatientID, and so what its
/// digest reads back from the pseudonymised tree (Wave 7a §5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatientId {
    /// The subject's code, the subject code generator's (`subject-code`).
    SubjectCode,
    /// The subject's value of this id type, from the linkage store
    /// (`id-type:<name>`).
    IdType(String),
}

/// The declaration's word for the subject's code.
pub const PATIENT_ID_CODE: &str = "subject-code";

/// The declaration's prefix for an id type.
pub const PATIENT_ID_TYPE: &str = "id-type:";

impl PatientId {
    /// The declaration as written: `subject-code` or `id-type:<name>`. An
    /// id type of `subject-code` is the code; a personnummer is never what
    /// PatientID holds, since that is the identifier pseudonymisation takes
    /// away.
    pub fn parse(text: &str) -> Result<PatientId, String> {
        let text = text.trim();
        if text == PATIENT_ID_CODE {
            return Ok(PatientId::SubjectCode);
        }
        let Some(name) = text.strip_prefix(PATIENT_ID_TYPE) else {
            return Err(format!(
                "patient_id is {PATIENT_ID_CODE} or {PATIENT_ID_TYPE}<name>, not {text}"
            ));
        };
        if name == crate::schema::SUBJECT_CODE_TYPE {
            return Ok(PatientId::SubjectCode);
        }
        if !crate::linkage::valid_id_type_name(name) {
            return Err(format!(
                "patient_id: {name} is no id type's name (lower case letters, digits and hyphens)"
            ));
        }
        if crate::personnummer::is_type(name) {
            return Err(format!(
                "patient_id: a pseudonymised file never holds a {name}; write the subject's code or another id type"
            ));
        }
        Ok(PatientId::IdType(name.to_string()))
    }

    /// The declaration as stored.
    pub fn as_text(&self) -> String {
        match self {
            PatientId::SubjectCode => PATIENT_ID_CODE.to_string(),
            PatientId::IdType(name) => format!("{PATIENT_ID_TYPE}{name}"),
        }
    }

    /// What a dataset document declares; none where it declares nothing,
    /// which an identified dataset reads as the subject's code and any other
    /// as its identity rule says.
    pub fn of(dataset: &Value) -> Result<Option<PatientId>, String> {
        match &dataset["patient_id"] {
            Value::Null => Ok(None),
            Value::String(s) => PatientId::parse(s).map(Some),
            other => Err(format!("patient_id is a word, not {other}")),
        }
    }
}

/// What a source place is (Wave 7a, Nima 2026-10-08: "NILS should always
/// get the declaration from structure"): a `root` the engine explores, each
/// folder under it a dataset; a `dataset`, one folder whose structure says
/// how its files arrive; or a `legacy` place that names a dataset's
/// pseudonymised tree itself (`…/derivatives/dcm-raw` or `dcm-anon`), read
/// as that tree.
pub const KINDS: [&str; 3] = ["dataset", "root", "legacy"];

/// What a dataset's structure says (Wave 7a): only originals
/// (`identified`), only a pseudonymised tree (`anonymised`), both (`both`,
/// identified with its anonymised copy), or anything else (`unknown`):
/// entries holding DICOM beside `derivatives/`, or no tree at all.
pub const STATES: [&str; 4] = ["identified", "anonymised", "both", "unknown"];

/// How a de-identified or coded dataset's subjects are found (Wave 7a): a
/// map of subject codes to its ids, or codes the subject code generator
/// makes from the ids.
pub const SUBJECTS: [&str; 2] = ["map", "generated"];

/// What names the folder of each pseudonymised copy (Wave 7a): the
/// subject's code, or the id type's value PatientID holds.
pub const FOLDERS: [&str; 2] = ["subject-code", "id-type"];

/// Record 55 H2 (round 4): whether picking main scans follows a sort of
/// the dataset as a pipeline step (`after_sort`, the default) or not
/// (`off`).
pub const PICKS: [&str; 2] = ["after_sort", "off"];

/// Whether a dataset's sorts are followed by a pick run: true unless the
/// dataset says `off`. A dataset stored before the field existed has none
/// and is picked after a sort.
pub fn picks_after_sort(dataset: &Value) -> bool {
    dataset["picks"].as_str() != Some("off")
}

/// Why a dataset may not be read yet, where its declaration is not whole
/// (Wave 7a): undeclared, without a tree, or arriving de-identified or
/// coded without saying what PatientID holds and how its subjects are
/// found. None for a whole declaration.
pub fn incomplete(dataset: &Value) -> Option<String> {
    let Ok(d) = dataset_of(dataset, None) else {
        return Some("its declaration cannot be read".into());
    };
    if d["kind"] == "root" {
        return Some(
            "it is a root: each folder under it is a dataset, read by its own name".into(),
        );
    }
    let arrives = d["arrives"].as_str().unwrap_or(UNDECLARED);
    if arrives == UNDECLARED || d["state"] == "unknown" {
        return Some(
            "its structure is unknown: entries beside derivatives/, or no tree at all; say which tree they go into"
                .into(),
        );
    }
    let legacy = d["kind"] == "legacy";
    if d["trees"]["anon"]
        .as_str()
        .is_none_or(|t| t == "." && !legacy)
    {
        return Some("it has no pseudonymised tree".into());
    }
    if arrives != "identified" {
        let mut missing = Vec::new();
        if d["patient_id"].is_null() {
            missing.push("what PatientID holds (patient_id: subject-code or id-type:<name>)");
        }
        if d["subjects"].is_null() {
            missing.push("how its subjects are found (subjects: map or generated)");
        }
        if !missing.is_empty() {
            return Some(format!(
                "it arrives {arrives} and does not say {}",
                missing.join(", nor ")
            ));
        }
    }
    None
}

/// The arrival of a dataset nobody has declared: the default of
/// [`handling_of`], [`dataset_of`] and [`default_dataset`]. Such a dataset
/// has no tree and is never digested, brought in or pseudonymised.
pub const UNDECLARED: &str = "undeclared";

/// Whether a dataset document says its arrival was never declared; a null
/// document is one.
pub fn is_undeclared(dataset: &Value) -> bool {
    dataset_of(dataset, None)
        .map(|d| d["arrives"] == UNDECLARED)
        .unwrap_or(true)
}

/// How what comes in through a place is handled, as the operator declares it:
/// whether it arrives identified, and what a release does to it on the way
/// out. A key not given takes its default, `undeclared` for the arrival, as
/// [`dataset_of`] has it; a value not known is refused with the choices.
/// `arrives` lives on the dataset since record 26 and is mirrored here for a
/// reader from before.
///
/// The dates are not a choice (record 38 S3): a release writes the real date.
/// `dates: keep`, which a caller from before may send, is taken and dropped;
/// `shift` and `year` are refused in words saying what to do instead.
pub fn handling_of(doc: &Value) -> Result<Value, String> {
    if !(doc.is_object() || doc.is_null()) {
        return Err("handling is an object: {arrives, on_release: {uids, deface}}".into());
    }
    let arrives = pick(doc, None, "arrives", &ARRIVALS, UNDECLARED)?;
    let release = doc.get("on_release").cloned().unwrap_or(Value::Null);
    if !(release.is_object() || release.is_null()) {
        return Err("on_release is an object: {uids, deface}".into());
    }
    match release.get("dates") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if s == "keep" => {}
        Some(other) => return Err(format!("{}, not {other}", DATES_ARE_KEPT)),
    }
    let uids = pick(&release, None, "uids", &["remap", "preserve"], "remap")?;
    let deface = match release.get("deface") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(other) => return Err(format!("deface is true or false, not {other}")),
    };
    Ok(json!({
        "arrives": arrives,
        "on_release": {"uids": uids, "deface": deface},
    }))
}

/// Why a date policy other than `keep` is refused, wherever one is asked for.
pub const DATES_ARE_KEPT: &str = "a release keeps the dates (record 38): the release is \
     pseudonymous, not anonymous, and the date is the key the clinical layer joins on. The \
     shift and year policies were removed; where a date must not show in a path, label the \
     sessions by months since baseline (M00, M06) with a months scheme";

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
/// dataset declared before Wave 7a may read the folder itself, `.`, as its
/// pseudonymised tree, and keeps that declaration; no declaration makes one
/// now, and an undeclared dataset has no tree at all.
pub const ORIGINALS_TREE: &str = "derivatives/dcm-original";
pub const ANON_TREE: &str = "derivatives/dcm-anon";

/// The dataset a source place is, as the operator declares it: what
/// arrives (`undeclared`, `identified`, `deidentified` or `coded`), the two trees as the
/// engine found or made them, the identity rule as `nils digest
/// --identity-rule` reads it or null, what an unmapped identifier does
/// (`hold` or `code`), the cohort every digest of it feeds or null, the tag
/// lists (`keep_demographics`, `remove`, `keep`, each tag as `gggg,eeee`)
/// and what becomes of the originals (`kept`, `vaulted` or `purged`). A key
/// not given keeps what is in force, or takes its default: undeclared, with
/// no tree, no rule, `code` for de-identified and coded arrivals and `hold`
/// otherwise, no cohort, demographics kept, the originals kept. A value not
/// known is refused with the choices.
pub fn dataset_of(doc: &Value, current: Option<&Value>) -> Result<Value, String> {
    if !(doc.is_object() || doc.is_null()) {
        return Err("dataset is an object: {arrives, trees, identity, unmapped, cohort, tags, originals_kept}".into());
    }
    let current = current.filter(|c| c.is_object());
    let arrives = pick(doc, current, "arrives", &ARRIVALS, UNDECLARED)?;
    let undeclared = arrives == UNDECLARED;
    let trees = match doc.get("trees") {
        Some(t) if t.is_object() => t.clone(),
        Some(Value::Null) | None => current
            .map(|c| c["trees"].clone())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({"originals": null, "anon": null})),
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
    // a tree is the engine's to set; one not set is none. The folder itself
    // (`.`) is read from a document written before Wave 7a, which schema 80
    // turns undeclared; nothing makes one now
    let anon = match &trees["anon"] {
        Value::Null => Value::Null,
        Value::String(s) if s == ANON_TREE || s == "." => Value::String(s.clone()),
        other => return Err(format!("trees.anon is {ANON_TREE} or ., not {other}")),
    };
    // an undeclared dataset has no tree: nothing in it is read (§5.3)
    let (originals, anon) = if undeclared {
        (Value::Null, Value::Null)
    } else {
        (originals, anon)
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
        if arrives == "deidentified" || arrives == "coded" {
            "code"
        } else {
            "hold"
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
    // Wave 7a §5.4: what the pseudonymiser writes into PatientID; an
    // identified dataset writes the subject's code unless it says another
    let patient_id = match doc.get("patient_id") {
        Some(Value::Null) => Value::Null,
        Some(Value::String(s)) => Value::String(PatientId::parse(s)?.as_text()),
        Some(other) => return Err(format!("patient_id is a word, not {other}")),
        None => current
            .map(|c| c["patient_id"].clone())
            .filter(|v| !v.is_null())
            .unwrap_or(Value::Null),
    };
    let patient_id = if patient_id.is_null() && arrives == "identified" {
        Value::String(PATIENT_ID_CODE.into())
    } else {
        patient_id
    };
    // Wave 7a (Nima, 2026-10-08): how the subjects of a dataset that
    // arrives de-identified or coded are found: through a map of subject
    // codes to its ids, given or already in the registry (`map`), or made
    // by the subject code generator from each id (`generated`)
    let subjects = match doc.get("subjects") {
        Some(Value::Null) => Value::Null,
        Some(Value::String(s)) if SUBJECTS.contains(&s.as_str()) => Value::String(s.clone()),
        Some(other) => {
            return Err(format!(
                "subjects is one of {}, not {other}",
                SUBJECTS.join(", ")
            ));
        }
        None => current
            .map(|c| c["subjects"].clone())
            .unwrap_or(Value::Null),
    };
    // what the place is and what its structure says, both the engine's
    let kind = pick(doc, current, "kind", &KINDS, KINDS[0])?;
    let state = pick(
        doc,
        current,
        "state",
        &STATES,
        match arrives.as_str() {
            "identified" => "identified",
            "deidentified" | "coded" => "anonymised",
            _ => "unknown",
        },
    )?;
    let root = match doc.get("root") {
        Some(Value::Null) => Value::Null,
        Some(Value::String(s)) => Value::String(s.clone()),
        Some(other) => return Err(format!("root is a place's name or null, not {other}")),
        None => current.map(|c| c["root"].clone()).unwrap_or(Value::Null),
    };
    // the folder of each pseudonymised copy: the subject's code, or the id
    // type's value PatientID holds
    let folder = pick(doc, current, "copy_folder", &FOLDERS, FOLDERS[0])?;
    if folder == "id-type"
        && !matches!(
            PatientId::of(&json!({ "patient_id": patient_id })),
            Ok(Some(PatientId::IdType(_)))
        )
    {
        return Err(
            "copy_folder id-type names each copy's folder by the id type PatientID holds; declare patient_id: id-type:<name> with it".into(),
        );
    }
    let originals_kept = pick(
        doc,
        current,
        "originals_kept",
        &["kept", "vaulted", "purged"],
        "kept",
    )?;
    // The place a vault put the originals in, by name: null until one has,
    // and on a dataset whose originals were purged. [`set_originals`]
    // writes it when the job succeeds; a declaration that does not name it
    // keeps what is in force. The path it went to and when are the audit
    // row's and the job's result; a page wants the place.
    let originals_vault = match doc.get("originals_vault") {
        Some(Value::Null) => Value::Null,
        Some(Value::String(s)) => Value::String(s.clone()),
        Some(other) => {
            return Err(format!(
                "originals_vault is the name of the place a vault put the originals in, or null, not {other}"
            ));
        }
        None => current
            .map(|c| c["originals_vault"].clone())
            .unwrap_or(Value::Null),
    };
    // record 55 H2 (round 4): picking main scans after a sort, or not
    let picks = pick(doc, current, "picks", &PICKS, PICKS[0])?;
    Ok(json!({
        "arrives": arrives,
        "trees": {"originals": originals, "anon": anon},
        "identity": identity,
        "unmapped": unmapped,
        "patient_id": patient_id,
        "subjects": subjects,
        "copy_folder": folder,
        "kind": kind,
        "state": state,
        "root": root,
        "cohort": cohort,
        "tags": tags,
        "originals_kept": originals_kept,
        "originals_vault": originals_vault,
        "picks": picks,
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

/// The dataset a source place is until one is declared: undeclared unless
/// said otherwise, with no tree (Wave 7a §5.3).
pub fn default_dataset(arrives: Option<&str>) -> Value {
    let arrives = arrives
        .filter(|a| ARRIVALS.contains(a))
        .unwrap_or(UNDECLARED);
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

/// Record 26 §1: what became of a dataset's originals, written by the act
/// that did it and by nothing else. `kept` is what every dataset carries
/// until a vault or a purge job succeeds; a vault records where it put
/// them, a purge records nothing. The rest of the dataset is kept as it
/// stands.
pub fn set_originals(
    store: &mut Store,
    id: i64,
    kept: &str,
    vault: Option<&str>,
) -> Result<Place, Error> {
    let current = show(store, id)?.ok_or_else(|| Error::Message(format!("no place {id}")))?;
    if current.role != Role::Source {
        return Err(Error::Message(format!(
            "a dataset is a source place; {} is a {} place",
            current.name,
            current.role.name()
        )));
    }
    let asked = json!({
        "originals_kept": kept,
        "originals_vault": vault,
    });
    let checked = dataset_of(&asked, Some(&current.dataset)).map_err(Error::Message)?;
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
        ANON_TREE, ORIGINALS_TREE, PatientId, SUBJECTS, UNDECLARED, dataset_of, default_dataset,
        default_handling, handling_of, incomplete, is_undeclared,
    };
    use serde_json::json;

    #[test]
    fn a_dataset_not_declared_is_undeclared_and_has_no_tree() {
        assert_eq!(
            default_dataset(None),
            json!({
                "arrives": "undeclared",
                "trees": {"originals": null, "anon": null},
                "identity": null,
                "unmapped": "hold",
                "patient_id": null,
                "subjects": null,
                "copy_folder": "subject-code",
                "kind": "dataset",
                "state": "unknown",
                "root": null,
                "cohort": null,
                "tags": {"keep_demographics": true, "remove": [], "keep": []},
                "originals_kept": "kept",
                "originals_vault": null,
                "picks": "after_sort",
            })
        );
        assert_eq!(dataset_of(&json!({}), None).unwrap(), default_dataset(None));
        // record 55 H2 (round 4): picking after a sort, on unless turned off,
        // and a document from before the field picks after a sort too
        let off = dataset_of(&json!({"picks": "off"}), None).unwrap();
        assert_eq!(off["picks"], "off");
        assert!(!super::picks_after_sort(&off));
        assert_eq!(dataset_of(&json!({}), Some(&off)).unwrap()["picks"], "off");
        assert!(dataset_of(&json!({"picks": "sometimes"}), None).is_err());
        assert!(super::picks_after_sort(&json!({"arrives": "identified"})));
        assert_eq!(
            dataset_of(&json!(null), None).unwrap(),
            default_dataset(None)
        );
        // an arrival the handling of before named is kept; one it could not is not
        assert_eq!(default_dataset(Some("identified"))["arrives"], "identified");
        assert_eq!(default_dataset(Some("maybe"))["arrives"], "undeclared");
        assert!(is_undeclared(&json!(null)));
        assert!(is_undeclared(&default_dataset(None)));
        assert!(!is_undeclared(&default_dataset(Some("deidentified"))));
        // an undeclared dataset has no tree whatever it is given
        let d = dataset_of(
            &json!({"trees": {"originals": ORIGINALS_TREE, "anon": ANON_TREE}}),
            None,
        )
        .unwrap();
        assert_eq!(d["trees"], json!({"originals": null, "anon": null}));
        // a declared dataset has the trees the engine set, and no other
        let d = dataset_of(&json!({"arrives": "deidentified"}), None).unwrap();
        assert_eq!(d["trees"], json!({"originals": null, "anon": null}));
        assert_eq!(d["unmapped"], "code");
    }

    /// Wave 7a §5.4: a dataset declares what PatientID holds; an identified
    /// one writes the subject's code unless it says another; a personnummer
    /// is never what it holds.
    #[test]
    fn a_dataset_declares_what_patient_id_holds() {
        let d = dataset_of(&json!({"arrives": "identified"}), None).unwrap();
        assert_eq!(d["patient_id"], "subject-code");
        let d = dataset_of(&json!({"arrives": "deidentified"}), None).unwrap();
        assert_eq!(d["patient_id"], json!(null));
        let d = dataset_of(
            &json!({"arrives": "identified", "patient_id": "id-type:study-id"}),
            None,
        )
        .unwrap();
        assert_eq!(d["patient_id"], "id-type:study-id");
        assert_eq!(
            PatientId::of(&d).unwrap(),
            Some(PatientId::IdType("study-id".into()))
        );
        // kept by a change that does not name it
        let after = dataset_of(&json!({"cohort": "x"}), Some(&d)).unwrap();
        assert_eq!(after["patient_id"], "id-type:study-id");
        let d = dataset_of(&json!({"patient_id": "id-type:subject-code"}), None).unwrap();
        assert_eq!(d["patient_id"], "subject-code");
        for (bad, what) in [
            ("id-type:personnummer", "never holds"),
            ("id-type:Study ID", "no id type"),
            ("the code", "subject-code or id-type:"),
        ] {
            let why = dataset_of(&json!({"patient_id": bad}), None).unwrap_err();
            assert!(why.contains(what), "{bad}: {why}");
        }
    }

    /// Wave 7a (Nima, 2026-10-08): a dataset is read only on a whole
    /// declaration. One arriving de-identified or coded says what PatientID
    /// holds and how its subjects are found; the copy's folder is the code
    /// unless PatientID holds an id type and the dataset asks for it.
    #[test]
    fn a_dataset_is_read_only_on_a_whole_declaration() {
        let trees = json!({"originals": null, "anon": ANON_TREE});
        assert!(incomplete(&json!(null)).unwrap().contains("unknown"));
        assert!(
            incomplete(&json!({"kind": "root"}))
                .unwrap()
                .contains("root")
        );
        // a legacy place reads the pseudonymised tree it names, once whole
        assert_eq!(
            incomplete(
                &json!({"kind": "legacy", "arrives": "deidentified", "trees": {"anon": "."}, "patient_id": "id-type:site-id", "subjects": "map"})
            ),
            None
        );
        assert!(
            incomplete(&json!({"arrives": "deidentified", "trees": {"anon": "."}}))
                .unwrap()
                .contains("no pseudonymised tree")
        );
        let why = incomplete(&json!({"arrives": "deidentified", "trees": trees})).unwrap();
        assert!(
            why.contains("patient_id") && why.contains("subjects"),
            "{why}"
        );
        let why =
            incomplete(&json!({"arrives": "coded", "trees": trees, "patient_id": "subject-code"}))
                .unwrap();
        assert!(
            !why.contains("patient_id") && why.contains("subjects"),
            "{why}"
        );
        for subjects in SUBJECTS {
            assert_eq!(
                incomplete(
                    &json!({"arrives": "deidentified", "trees": trees, "patient_id": "id-type:site-id", "subjects": subjects})
                ),
                None
            );
        }
        assert_eq!(
            incomplete(
                &json!({"arrives": "identified", "trees": {"originals": ORIGINALS_TREE, "anon": ANON_TREE}})
            ),
            None
        );
        let why = dataset_of(&json!({"subjects": "guessed"}), None).unwrap_err();
        assert!(why.contains("map, generated"), "{why}");
        // the folder by the id type needs PatientID to hold one
        let d = dataset_of(
            &json!({"arrives": "identified", "patient_id": "id-type:site-id", "copy_folder": "id-type"}),
            None,
        )
        .unwrap();
        assert_eq!(d["copy_folder"], "id-type");
        assert_eq!(
            dataset_of(&json!({}), None).unwrap()["copy_folder"],
            "subject-code"
        );
        let why = dataset_of(
            &json!({"arrives": "identified", "copy_folder": "id-type"}),
            None,
        )
        .unwrap_err();
        assert!(why.contains("patient_id: id-type"), "{why}");
    }

    /// Wave 7a §5.3: the handling's arrival and the dataset's agree when
    /// neither was declared.
    #[test]
    fn the_handling_and_the_dataset_default_to_the_same_arrival() {
        assert_eq!(default_handling()["arrives"], UNDECLARED);
        assert_eq!(default_dataset(None)["arrives"], UNDECLARED);
        assert_eq!(
            handling_of(&json!(null)).unwrap()["arrives"],
            dataset_of(&json!(null), None).unwrap()["arrives"]
        );
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
            (
                json!({"originals_vault": {"place": "archive"}}),
                "originals_vault is the name",
            ),
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
        // what a vault recorded of the originals survives a change that
        // does not name it
        let vaulted = dataset_of(
            &json!({"originals_kept": "vaulted", "originals_vault": "archive"}),
            Some(&before),
        )
        .unwrap();
        let after = dataset_of(&json!({"cohort": "ms2"}), Some(&vaulted)).unwrap();
        assert_eq!(after["originals_kept"], "vaulted");
        assert_eq!(after["originals_vault"], "archive");
        // the trees are kept until the engine sets them again
        let after = dataset_of(
            &json!({"arrives": "deidentified"}),
            Some(&json!({"arrives": "identified", "trees": {"originals": ORIGINALS_TREE, "anon": ANON_TREE}})),
        )
        .unwrap();
        assert_eq!(after["trees"]["originals"], ORIGINALS_TREE);
    }

    #[test]
    fn a_handling_not_declared_is_undeclared_and_remaps_uids() {
        assert_eq!(
            default_handling(),
            json!({"arrives": "undeclared", "on_release": {"uids": "remap", "deface": false}})
        );
        assert_eq!(handling_of(&json!({})).unwrap(), default_handling());
    }

    #[test]
    fn a_handling_fills_what_it_does_not_name_and_refuses_what_it_does_not_know() {
        let h = handling_of(
            &json!({"arrives": "deidentified", "on_release": {"dates": "keep", "deface": true}}),
        )
        .unwrap();
        assert_eq!(h["arrives"], "deidentified");
        assert_eq!(h["on_release"], json!({"uids": "remap", "deface": true}));
        let why = handling_of(&json!({"arrives": "maybe"})).unwrap_err();
        assert!(why.contains("identified, deidentified"), "{why}");
        assert!(handling_of(&json!({"on_release": {"deface": "yes"}})).is_err());
        assert!(handling_of(&json!("identified")).is_err());
    }

    #[test]
    fn the_dates_are_kept_and_a_policy_that_moves_them_is_refused() {
        // Record 38 S3: `shift` and `year` are gone, in words saying what to
        // do instead; `keep` from a caller before is taken and not stored.
        for dates in ["shift", "year"] {
            let why =
                handling_of(&json!({"on_release": {"dates": dates, "uids": "remap"}})).unwrap_err();
            assert!(why.contains("record 38"), "{why}");
            assert!(why.contains("M00"), "{why}");
        }
        let kept =
            handling_of(&json!({"on_release": {"dates": "keep", "uids": "preserve"}})).unwrap();
        assert_eq!(
            kept["on_release"],
            json!({"uids": "preserve", "deface": false})
        );
    }
}
