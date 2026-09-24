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
                pass, reference, author, author_kind, model_id FROM {} \
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
                "model_id": r.opt_int(12)?,
            }))
        })
        .collect::<Result<_, StoreError>>()?;
    let decisions = decisions_of(store, stack)?;
    let mut campaigns: BTreeMap<i64, Value> = BTreeMap::new();
    for (.., id, campaign) in &decisions {
        if let Some(c) = campaign {
            campaigns.insert(*id, campaign_answers(store, *id, *c)?);
        }
    }
    // Record 42 S2: a value a model decided names the model by its name,
    // its version and its digest, never a free-text version.
    let mut models: BTreeMap<i64, Value> = BTreeMap::new();
    for id in evidence.iter().filter_map(|e| e["model_id"].as_i64()) {
        if let std::collections::btree_map::Entry::Vacant(slot) = models.entry(id)
            && let Some(m) = nils_registry::model::get(store, id)?
        {
            slot.insert(m.named());
        }
    }
    let review = review_of(store, stack)?;

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
                let held = decisions
                    .iter()
                    .find(|(a, v, ..)| {
                        a == axis && (v.is_none() || v.as_deref() == value.as_deref())
                    })
                    .or_else(|| decisions.iter().find(|(a, ..)| a == axis));
                let why = held.and_then(|(_, _, why, ..)| why.clone());
                let committed_by = held.and_then(|(_, _, _, by, ..)| by.clone());
                let campaign = held.and_then(|(.., id, _)| campaigns.get(id).cloned());
                json!({
                    "kind": e["author_kind"],
                    "actor": e["author"],
                    "why": why,
                    "version": e["matched"],
                    "model": e["model_id"]
                        .as_i64()
                        .and_then(|id| models.get(&id).cloned()),
                    "committed_by": committed_by,
                    "campaign": campaign,
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
        "review": review,
        "axes": axes_doc,
    })))
}

/// Record 38 S4: the review items that hold the stack, each named. A count
/// of questions tells a reader there is something to look at and not what:
/// a stack carrying two conflicts said `2 review item(s)` and neither the
/// axis nor the losing value. Each item here is its id and kind, its status,
/// and what it asks about this stack, read from the stack's own evidence
/// (a group item's member row, or a stack item's own). Superseded items
/// are an earlier run's questions and are left out.
fn review_of(store: &mut Store, stack: i64) -> Result<Vec<Value>, StoreError> {
    let d = store.dialect();
    let item = nils_registry::schema::table("review_item");
    let member = nils_registry::schema::table("review_member");
    let column = |t: &nils_registry::schema::Table, alias: &str, c: &str| {
        d.text_of_qualified(Some(alias), t.column(c).expect("a review column"))
    };
    let mut out: Vec<Value> = Vec::new();
    // grouped questions, through their membership
    let sql = format!(
        "SELECT i.id, i.kind, i.scope, i.status, i.members, {}, {} FROM {} i \
         JOIN {} m ON m.item_id = i.id \
         WHERE m.stack_id = {} AND i.status <> 'superseded' ORDER BY i.id",
        column(item, "i", "evidence"),
        column(member, "m", "evidence"),
        store.qualified("review_item"),
        store.qualified("review_member"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(stack)])? {
        let group: Value = r
            .opt_text(5)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        let own: Value = r
            .opt_text(6)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        let evidence = if own.is_object() { own } else { group };
        let kind = r.text(1)?.to_string();
        out.push(json!({
            "id": r.int(0)?,
            "kind": kind,
            "scope": r.text(2)?,
            "status": r.text(3)?,
            "members": r.opt_int(4)?,
            "about": about(&kind, &evidence),
            "evidence": evidence,
        }));
    }
    // questions asked of this stack alone; the ref is matched exactly once
    // read, since its text is spelled apart on the two backends
    let reference = column(item, "i", "ref");
    let sql = format!(
        "SELECT i.id, i.kind, i.scope, i.status, i.members, {}, {reference} FROM {} i \
         WHERE i.scope = 'stack' AND i.status <> 'superseded' AND {reference} LIKE {} \
         ORDER BY i.id",
        column(item, "i", "evidence"),
        store.qualified("review_item"),
        d.param(1, Type::Text)
    );
    for r in store.query(&sql, &[Param::from(format!("%\"stack_id\"%{stack}%"))])? {
        let reference: Value = r
            .opt_text(6)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        if reference["stack_id"].as_i64() != Some(stack) {
            continue;
        }
        let evidence: Value = r
            .opt_text(5)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        let kind = r.text(1)?.to_string();
        out.push(json!({
            "id": r.int(0)?,
            "kind": kind,
            "scope": r.text(2)?,
            "status": r.text(3)?,
            "members": r.opt_int(4)?,
            "about": about(&kind, &evidence),
            "evidence": evidence,
        }));
    }
    out.sort_by_key(|v| v["id"].as_i64().unwrap_or(0));
    Ok(out)
}

/// What one of the classifier's questions asks, in a line: the axis, the
/// value and what it is weighed against.
fn about(kind: &str, e: &Value) -> String {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let rule = |v: &Value| {
        let set = text(&v["rule_set"]);
        let rule = text(&v["rule"]);
        match (set.is_empty(), rule.is_empty()) {
            (true, true) => "a rule".to_string(),
            (false, true) => set,
            (true, false) => rule,
            (false, false) => format!("{set}/{rule}"),
        }
    };
    let axis = text(&e["axis"]);
    let value = |v: &Value| match v.as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "(nothing)".to_string(),
    };
    match kind.split_once(':').map(|(_, k)| k) {
        Some("conflict") => format!(
            "{axis} {} over {}: {} decided it, {} would have said otherwise",
            value(&e["value"]),
            value(&e["other"]),
            rule(&e["decided_by"]),
            rule(&e["over"])
        ),
        Some("low_confidence") => format!(
            "{axis} {} at {:.2}, below {:.2} ({})",
            value(&e["value"]),
            e["confidence"].as_f64().unwrap_or(0.0),
            e["below"].as_f64().unwrap_or(0.0),
            text(&e["tier"])
        ),
        Some("missing") => format!("{axis} has no value"),
        Some("decision") => format!(
            "{axis}: the rule says {}, a decision says {}",
            value(&e["rule"]),
            value(&e["decision"])
        ),
        Some("one_image_per_stack") => format!(
            "split on {}: {} stacks of {} image(s) in the series",
            value(&e["value"]),
            e["stacks_in_series"],
            e["n_instances"]
        ),
        _ if !axis.is_empty() => format!("{axis} {}", value(&e["value"])),
        _ => e.to_string(),
    }
}

/// One decision in force: the axis, the value and the why.
/// A decision in force: its axis, value, why and who committed it.
/// A decision in force: its axis, value, why, who committed it, its id and
/// the campaign that closed into it.
type Decided = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
);

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
        "SELECT axis, value, why, committed_by, id, campaign_id FROM {} WHERE withdrawn_at IS NULL \
         AND (staged_at IS NULL OR committed_at IS NOT NULL) \
         AND ((scope = 'stack' AND ref = {}) OR (scope = 'series' AND ref = {}) \
              OR (scope = 'subject' AND ref = {}) OR (scope = 'origin' AND ref = {}) \
              OR (scope = 'group' AND ref IN \
                  (SELECT CAST(item_id AS TEXT) FROM {} WHERE stack_id = {}))) \
         ORDER BY id DESC",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
        d.param(4, Type::Text),
        store.qualified("review_member"),
        d.param(5, Type::Int),
    );
    store
        .query(
            &sql,
            &[
                Param::from(stack.to_string()),
                Param::from(series),
                Param::from(subject),
                Param::from(origin),
                Param::Int(stack),
            ],
        )?
        .iter()
        .map(|r| {
            Ok((
                r.text(0)?.to_string(),
                r.opt_text(1)?.map(str::to_string),
                r.opt_text(2)?.map(str::to_string),
                r.opt_text(3)?.map(str::to_string),
                r.int(4)?,
                r.opt_int(5)?,
            ))
        })
        .collect()
}

/// The answers a campaign's item came to before it closed into a decision:
/// who answered, as what kind of author, in which role and with what
/// value. A decision a campaign closed is authored by its adjudicator, its
/// one model or the principal who closed it (record 42 R6), so the raters
/// behind it, an agent among them, show here and nowhere else (wave 43's
/// proof).
fn campaign_answers(store: &mut Store, decision: i64, campaign: i64) -> Result<Value, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT a.principal, a.author_kind, a.role, a.value, a.model_id \
         FROM {} a JOIN {} i ON i.id = a.item_id \
         WHERE i.decision_id = {} AND i.campaign_id = {} ORDER BY a.id",
        store.qualified("campaign_answer"),
        store.qualified("campaign_item"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
    );
    let answers: Vec<Value> = store
        .query(&sql, &[Param::Int(decision), Param::Int(campaign)])?
        .iter()
        .map(|r| {
            Ok(json!({
                "principal": r.text(0)?,
                "author_kind": r.text(1)?,
                "role": r.text(2)?,
                "value": r.opt_text(3)?,
                "model_id": r.opt_int(4)?,
            }))
        })
        .collect::<Result<_, StoreError>>()?;
    let name = store
        .query_opt(
            &format!(
                "SELECT name FROM {} WHERE id = {}",
                store.qualified("campaign"),
                d.param(1, Type::Int)
            ),
            &[Param::Int(campaign)],
        )?
        .map(|r| r.text(0).map(str::to_string))
        .transpose()?;
    Ok(json!({"id": campaign, "name": name, "answers": answers}))
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
            // Record 42 S2: which model, by name, version and digest, and
            // the person who let its answer in.
            if let Some(m) = d.get("model").and_then(Value::as_object) {
                let digest = m["digest"].as_str().unwrap_or_default();
                out.push_str(&format!(
                    "      the model {}@{} ({}), committed by {}\n",
                    m["name"].as_str().unwrap_or_default(),
                    m["version"].as_str().unwrap_or_default(),
                    digest,
                    d["committed_by"].as_str().unwrap_or("nobody recorded")
                ));
            }
            // the answers a campaign closed into this decision, by whom
            if let Some(c) = d.get("campaign").and_then(Value::as_object) {
                let by: Vec<String> = c["answers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| {
                        format!(
                            "{} {} ({}{})",
                            with_article(a["author_kind"].as_str().unwrap_or("person")),
                            a["principal"].as_str().unwrap_or_default(),
                            a["role"].as_str().unwrap_or("rater"),
                            a["value"]
                                .as_str()
                                .map(|v| format!(", {v}"))
                                .unwrap_or_default()
                        )
                    })
                    .collect();
                out.push_str(&format!(
                    "      from campaign {}, answered by {}{}\n",
                    c["name"].as_str().unwrap_or_default(),
                    if by.is_empty() {
                        "nobody recorded".to_string()
                    } else {
                        by.join("; ")
                    },
                    d["committed_by"]
                        .as_str()
                        .map(|w| format!(", committed by {w}"))
                        .unwrap_or_default()
                ));
            }
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
    let items = doc["review"].as_array().cloned().unwrap_or_default();
    if let Some(n) = doc["review_items"].as_i64()
        && n > 0
    {
        out.push_str(&format!(
            "  {n} review item(s) were raised for this stack\n"
        ));
    }
    // record 38 S4: each item by its id and kind, with what it asks
    for i in &items {
        let members = match i["members"].as_i64() {
            Some(m) if i["scope"] == "group" => format!(", one of {m} stack(s)"),
            _ => String::new(),
        };
        out.push_str(&format!(
            "  review item {} {} ({}{members}): {}\n",
            i["id"].as_i64().unwrap_or(0),
            i["kind"].as_str().unwrap_or_default(),
            i["status"].as_str().unwrap_or_default(),
            i["about"].as_str().unwrap_or_default()
        ));
    }
    out
}

/// An author kind as a sentence names it.
fn with_article(kind: &str) -> &'static str {
    match kind {
        "agent" => "an agent",
        "model" => "a model",
        _ => "a person",
    }
}
