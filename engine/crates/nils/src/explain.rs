// SPDX-License-Identifier: AGPL-3.0-only

//! Why one stack was judged so (Wave 2 §12, record 26 §11): one reading for
//! `nils explain` and `GET /api/explain/{stack}`, so the command line and
//! the door cannot disagree. Each axis row of the classification with its
//! value, the pack's label for it, its confidence and tier, the evidence
//! rows that carried it (the rule set, the rule, the source it read, what
//! matched) and, where a person, an agent or a model decided it, who and
//! why. A value somebody decided says so in the same place a rule's answer
//! says which rule, because a model's answer must never read like a rule's.

use std::collections::BTreeMap;
use std::path::Path;

use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

/// The explanation, or none when the stack has not been classified.
pub(crate) fn document(
    store: &mut Store,
    stack: i64,
    pack_dir: Option<&Path>,
) -> Result<Option<Value>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT pack, pack_version, contract, overlay, review_items FROM {} WHERE stack_id = {}",
        store.qualified("classification"),
        d.param(1, Type::Int)
    );
    let Some(m) = store.query_opt(&sql, &[Param::Int(stack)])? else {
        return Ok(None);
    };
    let pack = m.text(0)?.to_string();
    let version = m.text(1)?.to_string();
    let contract = m.opt_int(2)?;
    let overlay = m.opt_text(3)?.map(str::to_string);
    let review_items = m.opt_int(4)?.unwrap_or(0);
    let labels = labels_of(pack_dir, &pack);

    let sql = format!(
        "SELECT axis, value, confidence, tier FROM {} WHERE stack_id = {} ORDER BY axis, value",
        store.qualified("classification_axis"),
        d.param(1, Type::Int)
    );
    let axes: Vec<(String, Option<String>, f64, String)> = store
        .query(&sql, &[Param::Int(stack)])?
        .iter()
        .map(|r| {
            Ok((
                r.text(0)?.to_string(),
                r.opt_text(1)?.map(str::to_string),
                r.double(2)?,
                r.text(3)?.to_string(),
            ))
        })
        .collect::<Result<_, StoreError>>()?;
    let sql = format!(
        "SELECT axis, value, tier, confidence, rule_set, rule, source, matched, \
                pass, reference, author, author_kind FROM {} \
         WHERE stack_id = {} ORDER BY axis, id",
        store.qualified("classification_evidence"),
        d.param(1, Type::Int)
    );
    let evidence: Vec<Value> = store
        .query(&sql, &[Param::Int(stack)])?
        .iter()
        .map(|r| {
            Ok(json!({
                "axis": r.text(0)?,
                "value": r.text(1)?,
                "tier": r.text(2)?,
                "confidence": r.double(3)?,
                "rule_set": r.text(4)?,
                "rule": r.text(5)?,
                "source": r.text(6)?,
                "matched": r.opt_text(7)?,
                "pass": r.opt_text(8)?,
                "reference": r.opt_text(9)?,
                "author": r.opt_text(10)?,
                "author_kind": r.opt_text(11)?,
            }))
        })
        .collect::<Result<_, StoreError>>()?;
    let decisions = decisions_of(store, stack)?;

    let mut per_axis: BTreeMap<&str, usize> = BTreeMap::new();
    for (axis, ..) in &axes {
        *per_axis.entry(axis.as_str()).or_insert(0) += 1;
    }
    let axes_doc: Vec<Value> = axes
        .iter()
        .map(|(axis, value, confidence, tier)| {
            // the rows that carried this value; every row of the axis when
            // the axis has one value, so a rule that voted otherwise shows
            let mine: Vec<Value> = evidence
                .iter()
                .filter(|e| e["axis"].as_str() == Some(axis.as_str()))
                .filter(|e| {
                    per_axis.get(axis.as_str()).copied().unwrap_or(1) == 1
                        || value
                            .as_deref()
                            .is_none_or(|v| e["value"].as_str() == Some(v))
                })
                .map(|e| {
                    json!({
                        "rule_set": e["rule_set"],
                        "rule": e["rule"],
                        "source": e["source"],
                        "matched": e["matched"],
                        "value": e["value"],
                        "tier": e["tier"],
                        "confidence": e["confidence"],
                        "pass": e["pass"],
                        "reference": e["reference"],
                    })
                })
                .collect();
            // Wave 4a §10.1: a value somebody decided says who, with what
            // standing, and why
            let decided = evidence.iter().find(|e| {
                e["axis"].as_str() == Some(axis.as_str())
                    && e["author_kind"].is_string()
                    && value
                        .as_deref()
                        .is_none_or(|v| e["value"].as_str() == Some(v))
            });
            let decision = decided.map(|e| {
                let why = decisions
                    .iter()
                    .find(|(a, v, _)| {
                        a == axis && (v.is_none() || v.as_deref() == value.as_deref())
                    })
                    .or_else(|| decisions.iter().find(|(a, _, _)| a == axis))
                    .and_then(|(_, _, why)| why.clone());
                json!({
                    "kind": e["author_kind"],
                    "actor": e["author"],
                    "why": why,
                    "version": e["matched"],
                })
            });
            let label = value.as_deref().and_then(|v| {
                labels
                    .get(&(axis.clone(), v.to_string()))
                    .cloned()
                    .or_else(|| {
                        labels
                            .contains_key(&(axis.clone(), v.to_string()))
                            .then(|| v.to_string())
                    })
            });
            json!({
                "axis": axis,
                "value": value,
                "label": label,
                "confidence": confidence,
                "tier": tier,
                "evidence": mine,
                "decision": decision,
            })
        })
        .collect();
    Ok(Some(json!({
        "stack": stack,
        "pack": pack,
        "version": version,
        "contract": contract,
        "overlay": overlay,
        "review_items": review_items,
        "axes": axes_doc,
    })))
}

/// One decision in force: the axis, the value and the why.
type Decided = (String, Option<String>, Option<String>);

/// The decisions in force on the stack, its series, its subject or its
/// origin, newest first.
fn decisions_of(store: &mut Store, stack: i64) -> Result<Vec<Decided>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT k.series_id, r.subject_id, \
                (SELECT manufacturer FROM {} f WHERE f.stack_id = k.id) \
         FROM {} k JOIN {} r ON r.id = k.series_id WHERE k.id = {}",
        store.qualified("stack_fingerprint"),
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(stack)])? else {
        return Ok(Vec::new());
    };
    let series = row.int(0)?.to_string();
    let subject = row.int(1)?.to_string();
    let origin = row
        .opt_text(2)?
        .filter(|m| !m.is_empty())
        .map(|m| format!("manufacturer={}", m.to_lowercase()))
        .unwrap_or_default();
    let sql = format!(
        "SELECT axis, value, why FROM {} WHERE withdrawn_at IS NULL \
         AND (staged_at IS NULL OR committed_at IS NOT NULL) \
         AND ((scope = 'stack' AND ref = {}) OR (scope = 'series' AND ref = {}) \
              OR (scope = 'subject' AND ref = {}) OR (scope = 'origin' AND ref = {})) \
         ORDER BY id DESC",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
        d.param(4, Type::Text),
    );
    store
        .query(
            &sql,
            &[
                Param::from(stack.to_string()),
                Param::from(series),
                Param::from(subject),
                Param::from(origin),
            ],
        )?
        .iter()
        .map(|r| {
            Ok((
                r.text(0)?.to_string(),
                r.opt_text(1)?.map(str::to_string),
                r.opt_text(2)?.map(str::to_string),
            ))
        })
        .collect()
}

/// The pack's label for each `(axis, value)`, keyed by the value's id and
/// by its label, when the pack can be found; empty otherwise, and the
/// document says null where it cannot say.
fn labels_of(pack_dir: Option<&Path>, pack: &str) -> BTreeMap<(String, String), String> {
    let mut out = BTreeMap::new();
    let Some(dir) = pack_dir else {
        return out;
    };
    let Ok(loaded) = nils_pack::load(&dir.join(pack), None) else {
        return out;
    };
    for axis in &loaded.axes {
        for v in &axis.values {
            out.insert((axis.name.clone(), v.id.clone()), v.label.clone());
            out.insert((axis.name.clone(), v.label.clone()), v.label.clone());
        }
    }
    out
}

/// The text `nils explain` prints.
pub(crate) fn text(doc: &Value) -> String {
    let mut out = String::new();
    let stack = doc["stack"].as_i64().unwrap_or(0);
    out.push_str(&format!(
        "stack {stack}, judged by {}@{}\n",
        doc["pack"].as_str().unwrap_or_default(),
        doc["version"].as_str().unwrap_or_default()
    ));
    if let Some(o) = doc["overlay"].as_str() {
        out.push_str(&format!("  under overlay {o}\n"));
    }
    for a in doc["axes"].as_array().into_iter().flatten() {
        let value = a["value"].as_str().unwrap_or("");
        out.push_str(&format!(
            "  {:16} {:20} {:.2}  {}\n",
            a["axis"].as_str().unwrap_or_default(),
            if value.is_empty() { "(nothing)" } else { value },
            a["confidence"].as_f64().unwrap_or(0.0),
            a["tier"].as_str().unwrap_or_default()
        ));
        // §10.1. A value somebody decided says who, and with what standing,
        // in the same place a rule's answer says which rule.
        if let Some(d) = a["decision"].as_object() {
            out.push_str(&format!(
                "      a {}, {}, decided {} for the {}{}\n",
                d["kind"].as_str().unwrap_or("person"),
                d["actor"].as_str().unwrap_or("unnamed"),
                value,
                a["axis"].as_str().unwrap_or_default(),
                match d["why"].as_str() {
                    Some(why) => format!(": {why}"),
                    None => String::new(),
                }
            ));
        }
        for e in a["evidence"].as_array().into_iter().flatten() {
            let line = format!(
                "      {} said {} by {}, from {} {}",
                e["rule_set"].as_str().unwrap_or_default(),
                e["value"].as_str().unwrap_or_default(),
                e["tier"].as_str().unwrap_or_default(),
                e["source"].as_str().unwrap_or_default(),
                e["matched"].as_str().unwrap_or_default()
            );
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    if let Some(n) = doc["review_items"].as_i64()
        && n > 0
    {
        out.push_str(&format!(
            "  {n} review item(s) were raised for this stack\n"
        ));
    }
    out
}
