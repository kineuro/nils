// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 4c §6.4: one diff of the canonical form, as a door, so the desk,
//! the command line and the assistant cannot write three diffs of one
//! form. Two documents differ set by set and part by part; two handles
//! differ by content hash, and a capped handle has none.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ast::{Ask, canonical_json};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Change {
    /// The set the change is in; absent for the document's own parts
    /// (`params`, `scheme`, `keep`, `out`, `name`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    pub part: String,
    /// `added`, `removed` or `changed`.
    pub kind: String,
    pub before: Value,
    pub after: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentDiff {
    pub same: bool,
    pub canonical_a: String,
    pub canonical_b: String,
    pub changes: Vec<Change>,
}

/// The structural diff of two documents over their canonical form.
pub fn documents(a: &Ask, b: &Ask) -> DocumentDiff {
    let va = serde_json::to_value(a).unwrap_or(Value::Null);
    let vb = serde_json::to_value(b).unwrap_or(Value::Null);
    let mut changes = Vec::new();
    let empty = serde_json::Map::new();
    let oa = va.as_object().unwrap_or(&empty);
    let ob = vb.as_object().unwrap_or(&empty);
    // the document's own parts
    let mut keys: Vec<&String> = oa
        .keys()
        .chain(ob.keys())
        .filter(|k| *k != "sets")
        .collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        push_change(&mut changes, None, k, oa.get(k), ob.get(k));
    }
    // the sets, then the parts of a set both hold
    let sa = oa
        .get("sets")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let sb = ob
        .get("sets")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut names: Vec<&String> = sa.keys().chain(sb.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        match (sa.get(name), sb.get(name)) {
            (Some(x), None) => changes.push(Change {
                set: Some(name.clone()),
                part: "set".into(),
                kind: "removed".into(),
                before: x.clone(),
                after: Value::Null,
            }),
            (None, Some(y)) => changes.push(Change {
                set: Some(name.clone()),
                part: "set".into(),
                kind: "added".into(),
                before: Value::Null,
                after: y.clone(),
            }),
            (Some(x), Some(y)) => {
                let px = x.as_object().cloned().unwrap_or_default();
                let py = y.as_object().cloned().unwrap_or_default();
                let mut parts: Vec<&String> = px.keys().chain(py.keys()).collect();
                parts.sort();
                parts.dedup();
                for part in parts {
                    push_change(&mut changes, Some(name), part, px.get(part), py.get(part));
                }
            }
            (None, None) => {}
        }
    }
    DocumentDiff {
        same: changes.is_empty(),
        canonical_a: canonical_json(&va),
        canonical_b: canonical_json(&vb),
        changes,
    }
}

fn push_change(
    changes: &mut Vec<Change>,
    set: Option<&String>,
    part: &str,
    x: Option<&Value>,
    y: Option<&Value>,
) {
    let kind = match (x, y) {
        (Some(a), Some(b)) if canonical_json(a) == canonical_json(b) => return,
        (Some(_), Some(_)) => "changed",
        (Some(_), None) => "removed",
        (None, Some(_)) => "added",
        (None, None) => return,
    };
    changes.push(Change {
        set: set.cloned(),
        part: part.to_string(),
        kind: kind.into(),
        before: x.cloned().unwrap_or(Value::Null),
        after: y.cloned().unwrap_or(Value::Null),
    });
}

/// Two handles: the same answer, or not, by content hash; a capped one
/// has no hash and the comparison is refused by name.
pub fn handles(a: &crate::handle::Handle, b: &crate::handle::Handle) -> Value {
    if a.truncated || b.truncated {
        let which: Vec<i64> = [a, b]
            .iter()
            .filter(|h| h.truncated)
            .map(|h| h.id)
            .collect();
        return json!({
            "same": Value::Null,
            "refused": format!("handle {} is truncated: a capped result has no hash and cannot be compared; run it as a job", which.iter().map(i64::to_string).collect::<Vec<_>>().join(" and ")),
            "a": {"handle": a.id, "row_count": a.row_count, "truncated": a.truncated},
            "b": {"handle": b.id, "row_count": b.row_count, "truncated": b.truncated},
        });
    }
    json!({
        "same": a.content_hash.is_some() && a.content_hash == b.content_hash,
        "a": {"handle": a.id, "hash": a.content_hash, "row_count": a.row_count, "grain": a.grain, "ask_hash": a.ask_hash()},
        "b": {"handle": b.id, "hash": b.content_hash, "row_count": b.row_count, "grain": b.grain, "ask_hash": b.ask_hash()},
    })
}
