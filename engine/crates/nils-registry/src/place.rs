// SPDX-License-Identifier: AGPL-3.0-only

//! A place as a registry object (Wave 5 §12.5, §10.2, D74).
//!
//! A place is a named location with a role and the guarantees behind it.
//! Every path the engine takes is bound to a place by role rather than
//! typed, and the rules are checked, not documented: a release writes only
//! to an `export` place, a question's subset only to a `share` place, the
//! `registry` role is refused on a place without a backup, and a `source`
//! place is never written. What the operator declares is `guarantees`;
//! what the engine measured is `probed`.

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
            Role::Source => "the original data, read only for the engine",
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
            Role::Source => "protected storage with snapshots; never written by the engine",
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
}

impl Place {
    pub fn as_json(&self) -> Value {
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
            "handling": handling_of(&self.handling).unwrap_or_else(|_| default_handling()),
            "handling_declared": self.handling.is_object(),
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

/// How what comes in through a place is handled, as the operator declares it:
/// whether it arrives identified, and what a release does to it on the way
/// out. A key not given takes its default; a value not known is refused with
/// the choices. Moving dates while preserving UIDs is refused, as a release's
/// policy refuses it.
pub fn handling_of(doc: &Value) -> Result<Value, String> {
    fn pick(v: &Value, key: &str, choices: &[&str], default: &str) -> Result<String, String> {
        match v.get(key) {
            None | Some(Value::Null) => Ok(default.to_string()),
            Some(Value::String(s)) if choices.contains(&s.as_str()) => Ok(s.clone()),
            Some(other) => Err(format!(
                "{key} is one of {}, not {other}",
                choices.join(", ")
            )),
        }
    }
    if !(doc.is_object() || doc.is_null()) {
        return Err("handling is an object: {arrives, on_release: {dates, uids, deface}}".into());
    }
    let arrives = pick(
        doc,
        "arrives",
        &["identified", "deidentified"],
        "identified",
    )?;
    let release = doc.get("on_release").cloned().unwrap_or(Value::Null);
    if !(release.is_object() || release.is_null()) {
        return Err("on_release is an object: {dates, uids, deface}".into());
    }
    let dates = pick(&release, "dates", &["keep", "shift", "year"], "keep")?;
    let uids = pick(&release, "uids", &["remap", "preserve"], "remap")?;
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
    let d = store.dialect();
    let now = now_iso();
    store.execute(
        &format!(
            "UPDATE {} SET handling = {}, updated_at = {} WHERE id = {}",
            store.qualified("place"),
            d.param(1, Type::Json),
            d.param(2, Type::Timestamp),
            d.param(3, Type::Int)
        ),
        &[
            Param::from(checked.to_string()),
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
        "SELECT id, name, role, path, {}, {}, {}, {}, {}, {}, {} FROM {}{filter} ORDER BY id",
        text("guarantees"),
        text("probed"),
        text("probed_at"),
        text("created_at"),
        text("updated_at"),
        text("retired_at"),
        text("handling"),
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
    })
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
    use super::{default_handling, handling_of};
    use serde_json::json;

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
