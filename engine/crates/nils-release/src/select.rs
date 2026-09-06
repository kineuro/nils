// SPDX-License-Identifier: AGPL-3.0-only

//! A selection at the four grains (Wave 4a §8): a cohort by name, subjects
//! by code or by any identifier the registry resolves, a subject's session
//! by the label the scheme gives it, and stacks by id or by what the pack
//! says they are. A release takes one and does not compute one: everything
//! else a person means by "the cohort" is a query, and the query is the
//! question wave's.
//!
//! What this module does is read the items as a person or a query wrote
//! them and resolve the ones the registry can resolve up front, so that a
//! release names what it did not find rather than releasing less than it
//! was asked for, and so that `nils select` can show a cohort before it
//! leaves.

use std::collections::{BTreeMap, HashMap};

use nils_registry::Registry;
use nils_registry::linkage::{self, Subkeys};
use nils_registry::schema::Type;
use nils_registry::store::Param;

use crate::run::{Error, Selection};

/// One item, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// `@name`: every current member of the cohort.
    Cohort(String),
    /// A code, or an identifier of any type the registry resolves.
    Subject(String),
    /// `<subject>:<label>`; the subject half resolves like a subject, the
    /// label is matched once the sessions are derived.
    Session(String, String),
    /// A stack, by id.
    Stack(i64),
    /// `<axis>=<value>`: every stack holding that value on that axis.
    Axis(String, String),
}

impl Item {
    /// One line of a selection file: a number is a stack, `@name` a cohort,
    /// `axis=value` an axis, `subject:label` a session, anything else a
    /// subject. A line that is empty or starts with `#` is nothing.
    pub fn parse(line: &str) -> Result<Option<Item>, String> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return Ok(None);
        }
        if let Ok(id) = line.parse::<i64>() {
            return Ok(Some(Item::Stack(id)));
        }
        if let Some(name) = line.strip_prefix('@') {
            let name = name.trim();
            if name.is_empty() {
                return Err("@ names no cohort".to_string());
            }
            return Ok(Some(Item::Cohort(name.to_string())));
        }
        if let Some((axis, value)) = line.split_once('=') {
            let (axis, value) = (axis.trim(), value.trim());
            if axis.is_empty() || value.is_empty() {
                return Err(format!("{line} is not an axis; those are <axis>=<value>"));
            }
            return Ok(Some(Item::Axis(axis.to_string(), value.to_string())));
        }
        if let Some((who, which)) = line.split_once(':') {
            let (who, which) = (who.trim(), which.trim());
            if who.is_empty() || which.is_empty() {
                return Err(format!(
                    "{line} is not a session; those are <subject>:<label>, as `nils session list` prints them"
                ));
            }
            return Ok(Some(Item::Session(who.to_string(), which.to_string())));
        }
        Ok(Some(Item::Subject(line.to_string())))
    }

    /// A whole file: JSON when it is JSON, which is what a query writes,
    /// with `subjects`, `sessions`, `stacks`, `cohorts` and `axes`;
    /// otherwise one item a line, which is what a person writes and what
    /// `cut` produces.
    pub fn parse_all(text: &str) -> Result<Vec<Item>, String> {
        if text.trim_start().starts_with('{') {
            return parse_json(text);
        }
        let mut out = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if let Some(item) = Item::parse(line).map_err(|e| format!("line {}: {e}", n + 1))? {
                out.push(item);
            }
        }
        Ok(out)
    }

    pub fn describe(&self) -> String {
        match self {
            Item::Cohort(n) => format!("@{n}"),
            Item::Subject(s) => s.clone(),
            Item::Session(a, b) => format!("{a}:{b}"),
            Item::Stack(id) => id.to_string(),
            Item::Axis(a, v) => format!("{a}={v}"),
        }
    }
}

fn parse_json(text: &str) -> Result<Vec<Item>, String> {
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let strings = |key: &str| -> Vec<String> {
        doc[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    for s in strings("cohorts") {
        out.push(Item::Cohort(s));
    }
    for s in strings("subjects") {
        out.push(Item::Subject(s));
    }
    for value in doc["sessions"].as_array().unwrap_or(&Vec::new()) {
        match value {
            // `["abc", "M06"]` or `"abc:M06"`, because both are natural to
            // write and neither is ambiguous.
            serde_json::Value::Array(pair) if pair.len() == 2 => out.push(Item::Session(
                pair[0].as_str().unwrap_or_default().to_string(),
                pair[1].as_str().unwrap_or_default().to_string(),
            )),
            serde_json::Value::String(s) => match Item::parse(s)? {
                Some(item @ Item::Session(..)) => out.push(item),
                _ => return Err(format!("{s} is not a session")),
            },
            _ => {}
        }
    }
    for value in doc["stacks"].as_array().unwrap_or(&Vec::new()) {
        if let Some(n) = value.as_i64() {
            out.push(Item::Stack(n));
        }
    }
    // `{"contrast": "T1w"}`, `{"contrast": ["T1w", "T2w"]}` or `["contrast=T1w"]`.
    match &doc["axes"] {
        serde_json::Value::Object(map) => {
            for (axis, value) in map {
                match value {
                    serde_json::Value::Array(values) => {
                        for v in values {
                            if let Some(v) = v.as_str() {
                                out.push(Item::Axis(axis.clone(), v.to_string()));
                            }
                        }
                    }
                    serde_json::Value::String(v) => out.push(Item::Axis(axis.clone(), v.clone())),
                    _ => {}
                }
            }
        }
        serde_json::Value::Array(list) => {
            for v in list {
                if let Some(s) = v.as_str() {
                    match Item::parse(s)? {
                        Some(item @ Item::Axis(..)) => out.push(item),
                        _ => return Err(format!("{s} is not an axis; those are <axis>=<value>")),
                    }
                }
            }
        }
        _ => {}
    }
    Ok(out)
}

/// How one item resolved, for the person reading `nils select` and for the
/// release's record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// A cohort, with how many current members it has.
    Cohort { members: usize },
    /// A subject code the registry holds.
    Code,
    /// An identifier of the named type, resolved through the linkage store
    /// to the subject with this code.
    Identifier { id_type: String, code: String },
    /// A session, whose subject resolved this way; the label is matched
    /// once the sessions are derived.
    Session(Box<How>),
    /// A stack id; whether the registry holds it is the preview's count.
    Stack,
    /// An axis value, with how many stacks hold it.
    Axis { stacks: i64 },
}

/// The resolution of a whole selection.
#[derive(Debug, Default)]
pub struct Resolved {
    /// Each item with how it resolved, in the order given.
    pub items: Vec<(Item, How)>,
    /// The release's enumeration, in codes and ids.
    pub selection: Selection,
    /// What could not be resolved, and why. A release with any refuses.
    pub unresolved: Vec<(Item, String)>,
}

impl Resolved {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "items": self.items.iter().map(|(item, how)| serde_json::json!({
                "item": item.describe(),
                "how": how_json(how),
            })).collect::<Vec<_>>(),
            "selection": self.selection.as_json(),
            "unresolved": self.unresolved.iter().map(|(item, why)| serde_json::json!({
                "item": item.describe(), "why": why,
            })).collect::<Vec<_>>(),
        })
    }
}

fn how_json(how: &How) -> serde_json::Value {
    match how {
        How::Cohort { members } => serde_json::json!({"kind": "cohort", "members": members}),
        How::Code => serde_json::json!({"kind": "code"}),
        How::Identifier { id_type, code } => {
            serde_json::json!({"kind": "identifier", "id_type": id_type, "code": code})
        }
        How::Session(inner) => serde_json::json!({"kind": "session", "subject": how_json(inner)}),
        How::Stack => serde_json::json!({"kind": "stack"}),
        How::Axis { stacks } => serde_json::json!({"kind": "axis", "stacks": stacks}),
    }
}

impl How {
    pub fn describe(&self) -> String {
        match self {
            How::Cohort { members } => format!("cohort, {members} current member(s)"),
            How::Code => "subject, by code".to_string(),
            How::Identifier { id_type, code } => {
                format!("subject {code}, by its {id_type} identifier")
            }
            How::Session(inner) => format!("session of a {}", inner.describe()),
            How::Stack => "stack, by id".to_string(),
            How::Axis { stacks } => format!("axis value, held by {stacks} stack(s)"),
        }
    }
}

/// Resolve the items against the registry (Wave 4a §8). Cohorts become
/// their current members' codes; a subject that is not a code is tried as
/// an identifier of every type the linkage store knows, which is the one
/// thing of v0's manifest resolver worth carrying; an axis value is kept as
/// a predicate and counted. A pack, when given, refuses an axis it does not
/// decide, because a name that matches no row selects nothing silently.
pub fn resolve(
    registry: &mut Registry,
    items: &[Item],
    pack: Option<&nils_pack::pack::Pack>,
) -> Result<Resolved, Error> {
    let mut out = Resolved::default();
    // Every subject name at once, so the linkage store is read once.
    let names: Vec<String> = items
        .iter()
        .filter_map(|i| match i {
            Item::Subject(s) | Item::Session(s, _) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let subjects = resolve_subjects(registry, &names)?;
    for item in items {
        match item {
            Item::Cohort(name) => match cohort_members(registry, name)? {
                Some(codes) => {
                    out.items.push((
                        item.clone(),
                        How::Cohort {
                            members: codes.len(),
                        },
                    ));
                    out.selection.cohorts.push(name.clone());
                    for code in codes {
                        if !out.selection.subjects.contains(&code) {
                            out.selection.subjects.push(code);
                        }
                    }
                }
                None => out.unresolved.push((
                    item.clone(),
                    format!("no cohort named {name}; `nils clinical cohort list` names them"),
                )),
            },
            Item::Subject(name) => match subjects.get(name) {
                Some(Ok((code, how))) => {
                    out.items.push((item.clone(), how.clone()));
                    if !out.selection.subjects.contains(code) {
                        out.selection.subjects.push(code.clone());
                    }
                }
                Some(Err(why)) => out.unresolved.push((item.clone(), why.clone())),
                None => unreachable!("every subject name was resolved"),
            },
            Item::Session(name, label) => match subjects.get(name) {
                Some(Ok((code, how))) => {
                    out.items
                        .push((item.clone(), How::Session(Box::new(how.clone()))));
                    out.selection.sessions.push((code.clone(), label.clone()));
                }
                Some(Err(why)) => out.unresolved.push((item.clone(), why.clone())),
                None => unreachable!("every subject name was resolved"),
            },
            Item::Stack(id) => {
                out.items.push((item.clone(), How::Stack));
                if !out.selection.stacks.contains(id) {
                    out.selection.stacks.push(*id);
                }
            }
            Item::Axis(axis, value) => {
                if let Some(pack) = pack
                    && !pack.axes.iter().any(|a| a.name == *axis)
                {
                    out.unresolved.push((
                        item.clone(),
                        format!(
                            "the {} pack decides no axis named {axis}; its axes are {}",
                            pack.name,
                            pack.axes
                                .iter()
                                .map(|a| a.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                    continue;
                }
                let stacks = axis_count(registry, axis, value)?;
                out.items.push((item.clone(), How::Axis { stacks }));
                out.selection.axes.push((axis.clone(), value.clone()));
            }
        }
    }
    Ok(out)
}

/// Each name as it resolved: the code and how, or why not.
type Names = HashMap<String, Result<(String, How), String>>;

/// Each name as a code the registry holds, else as an identifier of any
/// type the linkage store knows, else why not.
fn resolve_subjects(registry: &mut Registry, names: &[String]) -> Result<Names, Error> {
    let mut out: Names = HashMap::new();
    if names.is_empty() {
        return Ok(out);
    }
    let mut wanted: Vec<String> = names.to_vec();
    wanted.sort();
    wanted.dedup();
    for s in linkage::subjects_by_code(registry.store(), &wanted)? {
        out.insert(s.code.clone(), Ok((s.code, How::Code)));
    }
    let rest: Vec<String> = wanted
        .iter()
        .filter(|n| !out.contains_key(*n))
        .cloned()
        .collect();
    if rest.is_empty() {
        return Ok(out);
    }
    // Not codes: identifiers, of any type. The lookup is keyed by type, so
    // each name is tried under every type the store knows.
    let key = registry
        .pseudonym_key()
        .map_err(|e| Error::Refused(format!("the linkage store: {e}")))?;
    let keys = Subkeys::derive(&key);
    let mut linkage_store = registry
        .open_linkage()
        .map_err(|e| Error::Refused(format!("the linkage store: {e}")))?;
    let types = linkage::id_types(&mut linkage_store)?;
    let mut lookups: Vec<Vec<u8>> = Vec::new();
    let mut of: Vec<(String, String, Vec<u8>)> = Vec::new();
    for name in &rest {
        for t in &types {
            let l = keys.lookup(&t.name, name);
            lookups.push(l.clone());
            of.push((name.clone(), t.name.clone(), l));
        }
    }
    let by_lookup: HashMap<(Vec<u8>, i64), i64> =
        linkage::identities_by_lookup(&mut linkage_store, &lookups)?
            .into_iter()
            .map(|i| ((i.lookup, i.id_type_id), i.subject_id))
            .collect();
    let type_ids: HashMap<&str, i64> = types.iter().map(|t| (t.name.as_str(), t.id)).collect();
    let mut found: BTreeMap<String, Vec<(String, i64)>> = BTreeMap::new();
    for (name, id_type, lookup) in &of {
        if let Some(subject) = by_lookup.get(&(lookup.clone(), type_ids[id_type.as_str()])) {
            found
                .entry(name.clone())
                .or_default()
                .push((id_type.clone(), *subject));
        }
    }
    let ids: Vec<i64> = found
        .values()
        .flat_map(|v| v.iter().map(|(_, id)| *id))
        .collect();
    let codes: HashMap<i64, String> = linkage::subjects_by_id(registry.store(), &ids)?
        .into_iter()
        .map(|s| (s.id, s.code))
        .collect();
    for name in &rest {
        match found.get(name) {
            None => {
                out.insert(
                    name.clone(),
                    Err(format!(
                        "{name} is neither a subject code nor an identifier of any type the linkage store knows"
                    )),
                );
            }
            Some(hits) => {
                let mut distinct: Vec<i64> = hits.iter().map(|(_, id)| *id).collect();
                distinct.sort();
                distinct.dedup();
                if distinct.len() > 1 {
                    out.insert(
                        name.clone(),
                        Err(format!(
                            "{name} is an identifier of {} subjects, under types {}; name the subject by code",
                            distinct.len(),
                            hits.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(", ")
                        )),
                    );
                    continue;
                }
                let (id_type, id) = &hits[0];
                match codes.get(id) {
                    Some(code) => {
                        out.insert(
                            name.clone(),
                            Ok((
                                code.clone(),
                                How::Identifier {
                                    id_type: id_type.clone(),
                                    code: code.clone(),
                                },
                            )),
                        );
                    }
                    None => {
                        out.insert(
                            name.clone(),
                            Err(format!(
                                "{name} resolves to a subject the registry no longer holds"
                            )),
                        );
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The current members of a cohort, by code, or nothing if there is no
/// cohort of that name (folded on case).
fn cohort_members(registry: &mut Registry, name: &str) -> Result<Option<Vec<String>>, Error> {
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE LOWER(name) = LOWER({})",
        store.qualified("cohort"),
        d.param(1, Type::Text)
    );
    let Some(row) = store.query_opt(&sql, &[Param::from(name)])? else {
        return Ok(None);
    };
    let cohort = row.int(0)?;
    let sql = format!(
        "SELECT su.code FROM {} m JOIN {} su ON su.id = m.subject_id \
         WHERE m.cohort_id = {} AND m.left_at IS NULL ORDER BY su.code",
        store.qualified("cohort_member"),
        store.qualified("subject"),
        d.param(1, Type::Int)
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[Param::Int(cohort)])? {
        out.push(r.text(0)?.to_string());
    }
    Ok(Some(out))
}

/// How many stacks hold `value` on `axis`.
fn axis_count(registry: &mut Registry, axis: &str, value: &str) -> Result<i64, Error> {
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT COUNT(DISTINCT stack_id) FROM {} WHERE axis = {} AND value = {}",
        store.qualified("classification_axis"),
        d.param(1, Type::Text),
        d.param(2, Type::Text)
    );
    Ok(store
        .query_opt(&sql, &[Param::from(axis), Param::from(value)])?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_one_of_five_things() {
        assert_eq!(Item::parse("42").unwrap(), Some(Item::Stack(42)));
        assert_eq!(
            Item::parse("@ms-2026").unwrap(),
            Some(Item::Cohort("ms-2026".into()))
        );
        assert_eq!(
            Item::parse("contrast=T1w").unwrap(),
            Some(Item::Axis("contrast".into(), "T1w".into()))
        );
        assert_eq!(
            Item::parse("abc:M06").unwrap(),
            Some(Item::Session("abc".into(), "M06".into()))
        );
        assert_eq!(
            Item::parse("19800101-1234").unwrap(),
            Some(Item::Subject("19800101-1234".into()))
        );
        assert_eq!(Item::parse("# a remark").unwrap(), None);
        assert_eq!(Item::parse("   ").unwrap(), None);
        assert!(Item::parse("abc:").is_err());
        assert!(Item::parse("=T1w").is_err());
        assert!(Item::parse("@").is_err());
    }

    #[test]
    fn a_json_selection_carries_the_five_grains() {
        let items = Item::parse_all(
            r#"{"cohorts": ["ms"], "subjects": ["a"], "sessions": ["a:M06", ["b", "M12"]],
                "stacks": [1, 2], "axes": {"contrast": ["T1w", "T2w"], "plane": "axial"}}"#,
        )
        .unwrap();
        assert_eq!(items.len(), 9);
        assert!(items.contains(&Item::Axis("plane".into(), "axial".into())));
        assert!(items.contains(&Item::Session("b".into(), "M12".into())));
        let listed = Item::parse_all(r#"{"axes": ["contrast=T1w"]}"#).unwrap();
        assert_eq!(listed, vec![Item::Axis("contrast".into(), "T1w".into())]);
        assert!(Item::parse_all(r#"{"axes": ["contrast"]}"#).is_err());
    }
}
