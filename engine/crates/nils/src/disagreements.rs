// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 H3 (Nima's ruling of 2026-10-09): the pack decides, and where
//! nothing in the pack ranks two answers on one axis, that is a defect of
//! the pack, reported to whoever tunes it and never a review item.
//!
//! The pack's ranking is its order: rule sets run in the pack's `order` and
//! an axis a set decides is closed to every set after it; inside a set that
//! decides, the first rule that fires decides; inside an exclusion group the
//! lower priority wins. Two answers stand at equal rank only where that order
//! says nothing: one rule whose values' conditions both hold on an axis that
//! holds one value, a set that collects writing two values on such an axis,
//! or two values of one exclusion group at the same priority. The sort keeps
//! each case on the stack's classification (`notes.equal_rank`) and counts
//! them in `classification.disagreements`; this door gathers them by rule
//! pair, with how many stacks and some of them, for the pack the door names.

use std::collections::{BTreeMap, BTreeSet};

use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

/// The stacks named per pair, at most.
pub(crate) const EXAMPLES: usize = 10;

/// One rule pair's disagreements.
#[derive(Default)]
struct Pair {
    axis: String,
    why: BTreeSet<String>,
    sides: Vec<Value>,
    stacks: BTreeSet<i64>,
    versions: BTreeSet<String>,
}

/// `GET /api/packs/{name}/disagreements`: the pack's equal-rank
/// disagreements by rule pair, most stacks first. A stack of a sample sealed
/// now is left out unless the caller reads sealed stacks (record 48).
pub(crate) fn document(store: &mut Store, pack: &str, sealed: bool) -> Result<Value, StoreError> {
    let d = store.dialect();
    let notes = {
        let t = nils_registry::schema::table("classification");
        d.text_of(t.column("notes").expect("notes"))
    };
    let sql = format!(
        "SELECT stack_id, pack_version, {notes} FROM {} \
         WHERE pack = {} AND disagreements IS NOT NULL AND disagreements > 0 ORDER BY stack_id",
        store.qualified("classification"),
        d.param(1, Type::Text)
    );
    let rows: Vec<(i64, String, Value)> = store
        .query(&sql, &[Param::from(pack)])?
        .iter()
        .map(|r| {
            Ok((
                r.int(0)?,
                r.text(1)?.to_string(),
                r.opt_text(2)?
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or(Value::Null),
            ))
        })
        .collect::<Result<_, StoreError>>()?;
    let ids: Vec<i64> = rows.iter().map(|(s, ..)| *s).collect();
    let (held, _) = if sealed {
        (BTreeSet::new(), BTreeSet::new())
    } else {
        nils_registry::labels::sealed_now(store, &ids, &[])
            .map_err(|e| StoreError::Message(e.to_string()))?
    };
    let mut pairs: BTreeMap<String, Pair> = BTreeMap::new();
    let mut stacks: BTreeSet<i64> = BTreeSet::new();
    for (stack, version, notes) in rows {
        if held.contains(&stack) {
            continue;
        }
        let Ok(found) = serde_json::from_value::<Vec<Found>>(notes["equal_rank"].clone()) else {
            continue;
        };
        for e in found {
            let sides: Vec<&FoundSide> = e.sides.iter().collect();
            for (i, a) in sides.iter().enumerate() {
                for b in &sides[i + 1..] {
                    if a.value == b.value {
                        continue;
                    }
                    let one = format!("{}/{}={}", a.rule_set, a.rule, a.value);
                    let two = format!("{}/{}={}", b.rule_set, b.rule, b.value);
                    let (x, y, first, second) = if one <= two {
                        (one, two, a, b)
                    } else {
                        (two, one, b, a)
                    };
                    let key = format!("{}: {x} | {y}", e.axis);
                    let p = pairs.entry(key).or_default();
                    if p.sides.is_empty() {
                        p.axis = e.axis.clone();
                        p.sides = [first, second]
                            .iter()
                            .map(|s| {
                                json!({
                                    "rule_set": s.rule_set, "rule": s.rule, "value": s.value,
                                    "tier": s.tier, "confidence": s.confidence,
                                })
                            })
                            .collect();
                    }
                    p.why.insert(e.why.clone());
                    p.stacks.insert(stack);
                    p.versions.insert(version.clone());
                    stacks.insert(stack);
                }
            }
        }
    }
    let mut list: Vec<(String, Pair)> = pairs.into_iter().collect();
    list.sort_by(|(ka, a), (kb, b)| b.stacks.len().cmp(&a.stacks.len()).then(ka.cmp(kb)));
    Ok(json!({
        "pack": pack,
        "stacks": stacks.len(),
        "pairs": list
            .into_iter()
            .map(|(key, p)| json!({
                "pair": key,
                "axis": p.axis,
                "why": p.why.into_iter().collect::<Vec<_>>(),
                "sides": p.sides,
                "stacks": p.stacks.len(),
                "examples": p.stacks.iter().take(EXAMPLES).collect::<Vec<_>>(),
                "versions": p.versions.into_iter().collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    }))
}

/// An equal-rank disagreement as the classification keeps it.
#[derive(serde::Deserialize)]
struct Found {
    axis: String,
    why: String,
    sides: Vec<FoundSide>,
}

#[derive(serde::Deserialize)]
struct FoundSide {
    value: String,
    rule_set: String,
    rule: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    confidence: f64,
}
