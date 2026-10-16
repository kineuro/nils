// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55, ruling 35 (d): a campaign's results as its maker and an
//! administrator read them in the desk, at `GET /api/campaigns/{id}/results`.
//! Per item the answers standing now by rater, the adjudications, the
//! settled value and what the close made of it; the agreement, the unsure
//! count and the counts by value over the whole campaign.
//!
//! Blind as the answers are: while a campaign is open its results are read
//! by those who read every answer (an adjudicator of it and a holder of
//! review:work, [`crate::campaigns::sees_all`]) and by its maker, unless the
//! maker rates in it; anyone else is refused. The raters are named to the
//! maker and to a holder of review:work alone; anyone else reads them as
//! "rater 1", "adjudicator 1" and so on, and their own answers by their
//! own name.

use std::collections::{BTreeMap, HashMap};

use nils_registry::campaign;
use serde_json::{Value, json};

use crate::campaigns::{axes_value, campaign_err, oversees, plain, plain_answer, plain_item};
use crate::serve::{Caller, Reply};

/// Whether the caller is the campaign's maker and does not rate in it: named
/// as a rater, or holding a rater's assignment of it.
fn maker_apart(
    store: &mut nils_registry::Store,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<bool, Reply> {
    let p = caller.principal.as_str();
    if c.owner != p || c.raters().iter().any(|r| r == p) {
        return Ok(false);
    }
    Ok(!campaign::assignments(store, c.id)
        .map_err(campaign_err)?
        .iter()
        .any(|a| a.role == "rater" && a.principal.as_deref() == Some(p)))
}

/// `GET /api/campaigns/{id}/results`.
pub(crate) fn door(
    registry: &mut nils_registry::Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<Reply, Reply> {
    let principal = caller.principal.as_str();
    let store = registry.store();
    let maker = maker_apart(store, caller, c)?;
    if !(crate::campaigns::sees_all(store, caller, principal, c)? || maker) {
        return Err(Reply::error(
            403,
            format!(
                "rating in {} is blind until it closes: its results are read by its maker, an adjudicator or a holder of review:work",
                c.name
            ),
        ));
    }
    // the raters by name to the maker and to who oversees every campaign
    let named = c.owner == principal || oversees(caller);
    let free = c.question["kind"] == "free";
    let axes = c.question["kind"] == "axes";
    let at_plain = plain(caller);
    let answers = campaign::current_answers(store, c.id).map_err(campaign_err)?;
    let items = campaign::items(store, c.id).map_err(campaign_err)?;
    let agreement = campaign::agreement(store, c.id).map_err(campaign_err)?;

    // a stand-in name per person, in the order they first answered
    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut counted: HashMap<&'static str, usize> = HashMap::new();
    let mut alias = |p: &str, role: &str| -> String {
        if named || p == principal {
            return p.to_string();
        }
        if let Some(a) = aliases.get(p) {
            return a.clone();
        }
        let word: &'static str = if role == "adjudicator" {
            "adjudicator"
        } else {
            "rater"
        };
        let n = counted.entry(word).or_insert(0);
        *n += 1;
        let a = format!("{word} {n}");
        aliases.insert(p.to_string(), a.clone());
        a
    };

    // per rater: answers, adjudications and those marked unsure
    let mut raters: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut by_item: BTreeMap<i64, Vec<Value>> = BTreeMap::new();
    let mut adjudicated: BTreeMap<i64, bool> = BTreeMap::new();
    // counts by value: (axis, value) -> (answers, settled)
    let mut values: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
    for a in &answers {
        let who = alias(&a.principal, &a.role);
        let row = raters.entry(who.clone()).or_default();
        if a.role == "adjudicator" {
            row.1 += 1;
            adjudicated.insert(a.item_id, true);
        } else {
            row.0 += 1;
            if a.unsure {
                row.2 += 1;
            }
            if !free {
                for key in value_keys(a.value.as_deref(), axes) {
                    values.entry(key).or_default().0 += 1;
                }
            }
        }
        let mut v = json!({
            "id": a.id, "rater": who, "role": a.role, "round": a.round,
            "author_kind": a.author_kind, "value": a.value, "form": a.form,
            "derivative_id": a.derivative_id, "why": a.why, "unsure": a.unsure,
            "answered_at": a.answered_at, "derived": a.derived,
        });
        axes_value(&c.question, &mut v["value"]);
        if at_plain {
            plain_answer(&mut v, free);
        }
        by_item.entry(a.item_id).or_default().push(v);
    }

    let mut rows = Vec::with_capacity(items.len());
    let mut settled = 0usize;
    let mut decisions = std::collections::BTreeSet::new();
    let mut picks = 0usize;
    for it in &items {
        let mut j = it.as_json();
        // an item settles once its raters agreed or an adjudicator decided;
        // a close then marks it resolved
        if it.outcome.is_object()
            && matches!(it.state.as_str(), "agreed" | "adjudicated" | "resolved")
        {
            settled += 1;
            if !free {
                for key in value_keys(it.outcome["value"].as_str(), axes) {
                    values.entry(key).or_default().1 += 1;
                }
            }
        }
        // what the close made of it: one decision, one per axis, or a pick
        decisions.extend(it.decision_id);
        if let Some(m) = it.outcome["decisions"].as_object() {
            decisions.extend(m.values().filter_map(Value::as_i64));
        }
        picks += usize::from(it.pick_id.is_some());
        // an item with no outcome keeps it null
        if it.outcome.is_object() {
            axes_value(&c.question, &mut j["outcome"]["value"]);
        }
        if at_plain {
            plain_item(&mut j, &c.grain, free);
        }
        j["answers"] = json!(by_item.remove(&it.id).unwrap_or_default());
        j["adjudicated"] = json!(adjudicated.get(&it.id).copied().unwrap_or(false));
        rows.push(j);
    }

    let mut counts: Vec<Value> = values
        .into_iter()
        .map(|((axis, value), (answers, settled))| {
            json!({"axis": (!axis.is_empty()).then_some(axis), "value": value,
                   "answers": answers, "settled": settled})
        })
        .collect();
    counts.sort_by(|a, b| {
        a["axis"]
            .as_str()
            .cmp(&b["axis"].as_str())
            .then(b["settled"].as_u64().cmp(&a["settled"].as_u64()))
            .then(b["answers"].as_u64().cmp(&a["answers"].as_u64()))
            .then(a["value"].as_str().cmp(&b["value"].as_str()))
    });
    let unsure = agreement["unsure"].as_u64().unwrap_or(0);
    Ok(Reply::ok(json!({
        "campaign": c.id, "name": c.name, "status": c.status, "owner": c.owner,
        "closes_into": c.closes_into, "closed_at": c.closed_at, "named": named,
        "agreement": agreement, "unsure": unsure,
        "settled": settled, "adjudicated": adjudicated.len(),
        "decisions": decisions.len(), "picks": picks,
        "raters": raters.into_iter().map(|(rater, (answers, adjudications, unsure))| json!({
            "rater": rater, "answers": answers, "adjudications": adjudications, "unsure": unsure,
        })).collect::<Vec<_>>(),
        "values": if free { Value::Null } else { json!(counts) },
        "count": rows.len(), "items": rows,
    })))
}

/// The (axis, value) pairs an answer's or an outcome's value counts under:
/// one per axis of an axes value, the value itself under no axis otherwise.
fn value_keys(value: Option<&str>, axes: bool) -> Vec<(String, String)> {
    let Some(text) = value else {
        return Vec::new();
    };
    if axes && let Ok(Value::Object(m)) = serde_json::from_str::<Value>(text) {
        return m
            .into_iter()
            .map(|(axis, v)| {
                let v = match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                (axis, v)
            })
            .collect();
    }
    vec![(String::new(), text.to_string())]
}

#[cfg(test)]
mod tests {
    use super::value_keys;

    #[test]
    fn an_axes_value_counts_per_axis_and_any_other_whole() {
        assert_eq!(
            value_keys(Some(r#"{"base":"T1w","technique":"MPRAGE"}"#), true),
            vec![
                ("base".to_string(), "T1w".to_string()),
                ("technique".to_string(), "MPRAGE".to_string())
            ]
        );
        assert_eq!(
            value_keys(Some("brain"), false),
            vec![(String::new(), "brain".to_string())]
        );
        assert!(value_keys(None, true).is_empty());
    }
}
