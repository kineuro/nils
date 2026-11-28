// SPDX-License-Identifier: AGPL-3.0-only

//! How an acquisition family is filed: the pack's reference for whoever
//! drafts a question.
//!
//! An acquisition often leaves several images in a session (a magnitude,
//! a phase, a reconstruction made from both), and the axes file each
//! under values of their own. What the values mean is on the axes
//! (`description`, `terms`); how one family's images are found together
//! is here. A pack may hold a `families` folder beside its manifest, one
//! file a family, named by the family:
//!
//! ```yaml
//! family: two-echo                # the file's name without .yml
//! label: 'Two-echo imaging'
//! terms: ['two echo', 'dual echo']
//! summary: 'what the family is, and what one acquisition leaves'
//! products:
//!   - id: first-echo
//!     what: 'what this image is'
//!     acquired: true
//!     where:
//!       - ['=', {}, ['axis', {}, 'kind'], 'a']
//!     note: 'optional'
//! ask:
//!   - words: 'what a person asks'
//!     sets: |
//!       first:
//!         grain: stack
//!         where: [['=', {}, ['axis', {}, 'kind'], 'a']]
//!     note: 'optional'
//! raw:
//!   - field: text_series_description
//!     what: 'what the header text tells that the axes do not'
//! ```
//!
//! The files decide nothing: no rule and no sort reads them. The ask's
//! catalog serves each family whole, so a family is added or changed by a
//! release of the pack and by no code.
//!
//! The folder is found by its name and not through the manifest, so a
//! pack that carries one loads in an engine that does not know it, which
//! reads no folder the manifest does not name.
//!
//! **What this loader holds a file to**, each refusal with its path: every
//! key is one it knows; `family` is the file's name; the family's and its
//! products' identities are lower case letters, digits and hyphens, a
//! product's unique in its family; `terms` is not empty; there is a
//! product and a worked example; every clause is a clause in shape
//! (`[op, {options}, ...arguments]`); an axis a clause names is one the
//! pack declares, and a value it holds an axis against is a value of that
//! axis, by any of its names; no clause carries the option `exact`, which
//! is a question's own; and a `sets` text is YAML that maps a set's name
//! to the set, with the clauses of its `where` held the same way.
//!
//! What a clause can only be held to with the ask's own reading is held
//! where the catalog is built, which has the language and the fields: that
//! every clause parses, that a field or a derived field it names is one
//! the catalog lists, that a `sets` text parses as the sets of a document,
//! and that `raw` names a text field of a stack.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, R};
use crate::rules::Axis;
use crate::yaml::{self, File};

/// One acquisition family, as its file says it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Family {
    pub family: String,
    pub label: String,
    /// The words a person uses for the family.
    pub terms: Vec<String>,
    pub summary: String,
    /// What one acquisition of the family leaves, each as it is filed.
    pub products: Vec<Product>,
    /// Worked examples.
    pub ask: Vec<Example>,
    /// Header text for what the axes do not split.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<Raw>,
}

/// One image an acquisition of a family leaves.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Product {
    pub id: String,
    pub what: String,
    /// Whether the scanner acquired it, or it was computed or made from
    /// others.
    pub acquired: bool,
    /// The clauses that find it, as a document writes them: all hold.
    #[serde(rename = "where")]
    pub where_: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A worked example: what a person asks, and the sets that answer it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Example {
    pub words: String,
    /// A fragment of a document, the `sets` of it, as text.
    pub sets: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A header text field that tells what the axes do not.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Raw {
    pub field: String,
    pub what: String,
}

/// The families of the pack in `dir`, in the order of their files' names;
/// none where the pack has no `families` folder.
pub(crate) fn load(dir: &Path, axes: &[Axis]) -> R<Vec<Family>> {
    let folder = dir.join("families");
    let unread = |at: &Path, e: std::io::Error| Error {
        file: Some(at.to_path_buf()),
        line: None,
        path: String::new(),
        message: format!("cannot be read: {e}"),
    };
    // the pack's own folder is listed, so a pack kept in memory is built
    // again when a `families` folder is added to it
    let beside = crate::cache::read_dir(dir).map_err(|e| unread(dir, e))?;
    if !beside.contains(&folder) || !folder.is_dir() {
        return Ok(Vec::new());
    }
    let files: Vec<std::path::PathBuf> = crate::cache::read_dir(&folder)
        .map_err(|e| unread(&folder, e))?
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
        .collect();
    let mut out = Vec::with_capacity(files.len());
    for path in files {
        let f = File::read(&path)?;
        let named = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        out.push(f.blame(family_of(&f.value, named, axes))?);
    }
    Ok(out)
}

/// Lower case letters, digits and hyphens, and at least one.
fn is_id(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The keys of a mapping held to those a family file may have there.
fn known(m: &serde_json::Map<String, Value>, at: &str, keys: &[&str]) -> R<()> {
    match m.keys().find(|k| !keys.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => Err(Error::at(
            if at.is_empty() {
                k.clone()
            } else {
                format!("{at}.{k}")
            },
            format!("{k} is not a key here; they are {}", keys.join(", ")),
        )),
    }
}

/// A text that says something.
fn said(m: &serde_json::Map<String, Value>, key: &str, at: &str) -> R<String> {
    let path = if at.is_empty() {
        key.to_string()
    } else {
        format!("{at}.{key}")
    };
    match yaml::get(m, key, at)? {
        Value::String(s) if !s.trim().is_empty() => Ok(s.clone()),
        Value::String(_) => Err(Error::at(path, "is empty")),
        _ => Err(Error::at(path, "expected a string")),
    }
}

fn family_of(v: &Value, file: &str, axes: &[Axis]) -> R<Family> {
    let m = yaml::obj(v, "family")?;
    known(
        m,
        "",
        &[
            "family", "label", "terms", "summary", "products", "ask", "raw",
        ],
    )?;
    let family = said(m, "family", "")?;
    if family != file {
        return Err(Error::at(
            "family",
            format!("the family is {family} and its file is {file}: a family is named as its file"),
        ));
    }
    if !is_id(&family) {
        return Err(Error::at(
            "family",
            format!("{family} is not lower case letters, digits and hyphens"),
        ));
    }
    let terms = yaml::texts(yaml::get(m, "terms", "")?, "terms")?;
    if terms.is_empty() || terms.iter().any(|t| t.trim().is_empty()) {
        return Err(Error::at(
            "terms",
            "a family has at least one word a person uses for it, and none is empty",
        ));
    }
    let mut products: Vec<Product> = Vec::new();
    for (i, p) in yaml::arr(yaml::get(m, "products", "")?, "products")?
        .iter()
        .enumerate()
    {
        let at = format!("products[{i}]");
        let pm = yaml::obj(p, &at)?;
        known(pm, &at, &["id", "what", "acquired", "where", "note"])?;
        let id = said(pm, "id", &at)?;
        if !is_id(&id) {
            return Err(Error::at(
                format!("{at}.id"),
                format!("{id} is not lower case letters, digits and hyphens"),
            ));
        }
        if products.iter().any(|q| q.id == id) {
            return Err(Error::at(
                format!("{at}.id"),
                format!("{id} is a product of {family} already"),
            ));
        }
        let Value::Bool(acquired) = yaml::get(pm, "acquired", &at)? else {
            return Err(Error::at(
                format!("{at}.acquired"),
                "expected true (the scanner acquired it) or false (computed, or made from others)",
            ));
        };
        let where_at = format!("{at}.where");
        let where_ = yaml::arr(yaml::get(pm, "where", &at)?, &where_at)?;
        if where_.is_empty() {
            return Err(Error::at(
                where_at,
                "a product is found by at least one clause",
            ));
        }
        for (j, c) in where_.iter().enumerate() {
            clause(c, &format!("{where_at}[{j}]"), axes)?;
        }
        products.push(Product {
            id,
            what: said(pm, "what", &at)?,
            acquired: *acquired,
            where_: where_.clone(),
            note: match pm.get("note") {
                Some(_) => Some(said(pm, "note", &at)?),
                None => None,
            },
        });
    }
    if products.is_empty() {
        return Err(Error::at(
            "products",
            "a family leaves at least one product",
        ));
    }
    let mut ask: Vec<Example> = Vec::new();
    for (i, e) in yaml::arr(yaml::get(m, "ask", "")?, "ask")?
        .iter()
        .enumerate()
    {
        let at = format!("ask[{i}]");
        let em = yaml::obj(e, &at)?;
        known(em, &at, &["words", "sets", "note"])?;
        let sets = said(em, "sets", &at)?;
        sets_of(&sets, &format!("{at}.sets"), axes)?;
        ask.push(Example {
            words: said(em, "words", &at)?,
            sets,
            note: match em.get("note") {
                Some(_) => Some(said(em, "note", &at)?),
                None => None,
            },
        });
    }
    if ask.is_empty() {
        return Err(Error::at("ask", "a family has at least one worked example"));
    }
    let mut raw: Vec<Raw> = Vec::new();
    if let Some(list) = m.get("raw") {
        for (i, r) in yaml::arr(list, "raw")?.iter().enumerate() {
            let at = format!("raw[{i}]");
            let rm = yaml::obj(r, &at)?;
            known(rm, &at, &["field", "what"])?;
            raw.push(Raw {
                field: said(rm, "field", &at)?,
                what: said(rm, "what", &at)?,
            });
        }
    }
    Ok(Family {
        label: said(m, "label", "")?,
        summary: said(m, "summary", "")?,
        family,
        terms,
        products,
        ask,
        raw,
    })
}

/// A `sets` text: YAML that maps each set's name to the set, the clauses
/// of each set's `where` held as a product's are.
fn sets_of(text: &str, at: &str, axes: &[Axis]) -> R<()> {
    let parsed = File::from_text(at, text)
        .map_err(|e| Error::at(at, format!("the sets are not YAML: {}", e.message)))?;
    let sets = yaml::obj(&parsed.value, at)
        .map_err(|_| Error::at(at, "the sets map each set's name to the set"))?;
    if sets.is_empty() {
        return Err(Error::at(at, "the sets hold at least one set"));
    }
    for (name, set) in sets {
        let set_at = format!("{at}.{name}");
        let sm = yaml::obj(set, &set_at)?;
        if let Some(w) = sm.get("where") {
            let where_at = format!("{set_at}.where");
            for (j, c) in yaml::arr(w, &where_at)?.iter().enumerate() {
                clause(c, &format!("{where_at}[{j}]"), axes)?;
            }
        }
    }
    Ok(())
}

/// A value as a clause writes one: a text, or a number read as its digits.
fn literal(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A clause in shape, with the axes it names and the values it holds them
/// against held to the pack's own: `[op, {options}, ...arguments]`, an
/// argument a clause, a literal or a list of literals.
fn clause(v: &Value, at: &str, axes: &[Axis]) -> R<()> {
    let shape = || {
        Error::at(
            at,
            "a clause is [op, {options}, ...arguments], the options there even when empty",
        )
    };
    let items = v.as_array().ok_or_else(shape)?;
    let (Some(Value::String(op)), Some(Value::Object(options))) = (items.first(), items.get(1))
    else {
        return Err(shape());
    };
    // A family's clause never says `exact`. The option keeps a fine value
    // alone where a question means it, and a question is read before it
    // is told to: a family that wrote it would teach a first draft the
    // one clause a draft is asked not to open with.
    if options.contains_key("exact") {
        return Err(Error::at(
            at,
            "a family's clause carries no exact: the option is a question's own, written once \
             the question was read; write the clause without it",
        ));
    }
    let args = &items[2..];
    let axis_of = |a: &Value| -> Option<String> {
        let inner = a.as_array()?;
        (inner.first()?.as_str()? == "axis" && inner.get(1)?.is_object())
            .then(|| inner.get(2).and_then(Value::as_str).map(str::to_string))?
    };
    if op == "axis" {
        let name = args.first().and_then(Value::as_str).unwrap_or_default();
        if !axes.iter().any(|a| a.name == name) {
            return Err(Error::at(at, format!("no axis named {name} in the pack")));
        }
        return Ok(());
    }
    // an axis held against values it writes out
    if matches!(op.as_str(), "=" | "<>" | "in" | "not_in" | "has")
        && let [left, right] = args
        && let Some(name) = axis_of(left)
        && let Some(axis) = axes.iter().find(|a| a.name == name)
    {
        let written: Vec<&Value> = match right {
            Value::Array(list)
                if !list.first().is_some_and(|x| {
                    x.is_string() && list.get(1).is_some_and(Value::is_object)
                }) =>
            {
                list.iter().collect()
            }
            other => vec![other],
        };
        for w in written {
            if let Some(text) = literal(w)
                && axis.value_named(&text).is_none()
            {
                return Err(Error::at(
                    at,
                    format!("{text} is no value of {name}, by any of its names"),
                ));
            }
        }
    }
    for (i, a) in args.iter().enumerate() {
        match a {
            Value::Object(_) => {
                return Err(Error::at(
                    format!("{at}[{}]", i + 2),
                    "a mapping is no argument of a clause",
                ));
            }
            Value::Array(list)
                if list.first().is_some_and(Value::is_string)
                    && list.get(1).is_some_and(Value::is_object) =>
            {
                clause(a, &format!("{at}[{}]", i + 2), axes)?;
            }
            _ => {}
        }
    }
    Ok(())
}
