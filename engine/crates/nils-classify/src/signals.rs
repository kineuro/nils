// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4c §6.6: the classifier's own signals over a scope. Per axis: how
//! many stacks resolved at each tier, the confidence spread, the open review
//! items by kind, the shadowed keywords, the unused overlay terms, and the
//! terms that carried the most decisions that disagreed with the rules. And
//! beside the axes, the raw fingerprint fields of the stacks whose rule a
//! person overrode, because the signals that decided a real pick were the
//! image type string, the echo time, the slice count and the reconstruction
//! variant, not the six axes.
//!
//! Record 26, decision 12: the same signals by value, so that a person
//! tuning one value's words sees what reaches it. Per axis and value: how
//! many stacks were decided to it and by which kind of clause (a flag, a
//! word, a window), how many a person is still asked about, the words that
//! were shadowed on the way to it, and the words that carried decisions a
//! person overrode. Beside them the origins of the scope (manufacturers,
//! models, stations, with counts), which is what an overlay is keyed on.
//!
//! Everything here is an aggregate over rows the caller may read as a
//! reviewer; no row leaves, and the fingerprint fields shown are acquisition
//! parameters, never identifiers.

use std::collections::BTreeMap;

use nils_registry::store::{Error, Param, Store};
use serde_json::{Value, json};

use crate::scope::Scope;

/// The fingerprint fields summarised for the overridden stacks.
pub const FIELDS: &[&str] = &["image_type", "echo_time", "n_instances", "sequence_variant"];

/// The most terms or values listed per axis or field.
const TOP: usize = 12;

/// The most values reported per axis: a pack's vocabulary is far smaller,
/// and a bound is a bound.
const VALUES_MAX: usize = 200;

/// The origin kinds, as the fingerprint stores them and an overlay keys on
/// them.
const ORIGINS: &[(&str, &str)] = &[
    ("manufacturer", "manufacturer"),
    ("manufacturer_model_name", "model"),
    ("station_name", "station"),
];

fn count_rows(
    store: &mut Store,
    sql: &str,
    params: &[Param],
) -> Result<Vec<(String, String, i64)>, Error> {
    store
        .query(sql, params)?
        .iter()
        .map(|r| {
            Ok((
                r.opt_text(0)?.unwrap_or("").to_string(),
                r.opt_text(1)?.unwrap_or("").to_string(),
                r.int(2)?,
            ))
        })
        .collect()
}

pub fn signals(store: &mut Store, scope: &Scope) -> Result<Value, Error> {
    let (stacks, scope_params) = scope.stacks_sql(store, 1);
    let mut axes: BTreeMap<String, Value> = BTreeMap::new();
    let axis = |axes: &mut BTreeMap<String, Value>, name: &str| -> Value {
        axes.entry(name.to_string())
            .or_insert_with(|| {
                json!({"tiers": {}, "confidence": {}, "open_review": {}, "disagreeing_terms": []})
            })
            .clone()
    };

    // How many stacks resolved at each tier.
    let sql = format!(
        "SELECT axis, tier, COUNT(*) FROM {} WHERE stack_id IN {stacks} GROUP BY axis, tier",
        store.qualified("classification_axis")
    );
    for (a, tier, n) in count_rows(store, &sql, &scope_params)? {
        let mut doc = axis(&mut axes, &a);
        doc["tiers"][tier] = json!(n);
        axes.insert(a, doc);
    }

    // The confidence spread: the bounds, the mean and the counts under
    // three thresholds, which both backends compute the same way.
    let sql = format!(
        "SELECT axis, COUNT(*), MIN(confidence), AVG(confidence), MAX(confidence), SUM(CASE WHEN confidence < 0.5 THEN 1 ELSE 0 END), SUM(CASE WHEN confidence < 0.7 THEN 1 ELSE 0 END), SUM(CASE WHEN confidence < 0.9 THEN 1 ELSE 0 END) FROM {} WHERE stack_id IN {stacks} GROUP BY axis",
        store.qualified("classification_axis")
    );
    for r in store.query(&sql, &scope_params)? {
        let a = r.text(0)?.to_string();
        let num = |i: usize| -> f64 {
            match r.get(i) {
                nils_registry::store::Cell::Double(d) => *d,
                nils_registry::store::Cell::Int(n) => *n as f64,
                nils_registry::store::Cell::Text(t) => t.parse().unwrap_or(0.0),
                _ => 0.0,
            }
        };
        let mut doc = axis(&mut axes, &a);
        doc["confidence"] = json!({
            "n": r.int(1)?,
            "min": num(2), "mean": num(3), "max": num(4),
            "below_0_5": num(5) as i64, "below_0_7": num(6) as i64, "below_0_9": num(7) as i64,
        });
        axes.insert(a, doc);
    }

    // Open review items by kind, through the grouped items' members: the
    // kind names the axis before the colon.
    let sql = format!(
        "SELECT i.kind, '', COUNT(DISTINCT i.id) FROM {} AS i JOIN {} AS m ON m.item_id = i.id WHERE i.status = 'open' AND m.stack_id IN {stacks} GROUP BY i.kind",
        store.qualified("review_item"),
        store.qualified("review_member")
    );
    let mut open_by_kind: BTreeMap<String, i64> = BTreeMap::new();
    for (kind, _, n) in count_rows(store, &sql, &scope_params)? {
        if let Some((a, _)) = kind.split_once(':') {
            let mut doc = axis(&mut axes, a);
            doc["open_review"][&kind] = json!(n);
            axes.insert(a.to_string(), doc);
        }
        open_by_kind.insert(kind, n);
    }

    // The terms that carried the most decisions that disagreed with the
    // rules: a person's stack decision beside the rule's citation.
    let sql = format!(
        "SELECT e.axis, e.matched, COUNT(*) FROM {} AS d JOIN {} AS e ON CAST(e.stack_id AS TEXT) = d.ref AND e.axis = d.axis WHERE d.scope = 'stack' AND d.withdrawn_at IS NULL AND e.source = 'text' AND (d.value IS NULL OR d.value <> e.value) AND e.stack_id IN {stacks} GROUP BY e.axis, e.matched ORDER BY COUNT(*) DESC, e.axis, e.matched LIMIT 200",
        store.qualified("decision"),
        store.qualified("classification_evidence")
    );
    for (a, term, n) in count_rows(store, &sql, &scope_params)? {
        let mut doc = axis(&mut axes, &a);
        if doc["disagreeing_terms"]
            .as_array()
            .is_some_and(|v| v.len() < TOP)
        {
            doc["disagreeing_terms"]
                .as_array_mut()
                .expect("an array")
                .push(json!({"term": term, "decisions": n}));
        }
        axes.insert(a, doc);
    }

    // The raw fields of the overridden stacks, as values: acquisition
    // parameters, which is what a rule is written from.
    let overridden = format!(
        "(SELECT CAST(e.stack_id AS TEXT) FROM {} AS d JOIN {} AS e ON CAST(e.stack_id AS TEXT) = d.ref AND e.axis = d.axis WHERE d.scope = 'stack' AND d.withdrawn_at IS NULL AND (d.value IS NULL OR d.value <> e.value))",
        store.qualified("decision"),
        store.qualified("classification_evidence")
    );
    let mut fields: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for field in FIELDS {
        let sql = format!(
            "SELECT CAST(f.{field} AS TEXT), '', COUNT(*) FROM {} AS f WHERE CAST(f.stack_id AS TEXT) IN {overridden} AND f.stack_id IN {stacks} GROUP BY f.{field} ORDER BY COUNT(*) DESC, 1 LIMIT {TOP}",
            store.qualified("stack_fingerprint")
        );
        let rows = count_rows(store, &sql, &scope_params)?;
        fields.insert(
            field.to_string(),
            rows.into_iter()
                .map(|(v, _, n)| json!({"value": v, "stacks": n}))
                .collect(),
        );
    }

    // The batch diagnostics of Wave 2 §10, over the batches in scope.
    let (batches, batch_params) = scope.batches_sql(store, 1);
    let d = store.dialect();
    let t = nils_registry::schema::table("diagnostic");
    let sql = format!(
        "SELECT kind, {}, count FROM {} WHERE batch_id IN {batches} AND kind IN ('axis_conflict', 'axis_unresolved', 'keyword_shadowed', 'overlay_unused') ORDER BY batch_id, kind",
        d.text_of(t.column("sample").expect("sample column")),
        store.qualified("diagnostic")
    );
    let mut diagnostics: BTreeMap<String, i64> = BTreeMap::new();
    let mut shadowed: Vec<String> = Vec::new();
    let mut unused: Vec<String> = Vec::new();
    let mut samples: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (kind, sample, n) in count_rows(store, &sql, &batch_params)? {
        *diagnostics.entry(kind.clone()).or_insert(0) += n;
        let list: Vec<String> = serde_json::from_str(&sample).unwrap_or_default();
        match kind.as_str() {
            "keyword_shadowed" => shadowed.extend(list),
            "overlay_unused" => unused.extend(list),
            _ => samples.entry(kind).or_default().extend(list),
        }
    }
    shadowed.sort();
    shadowed.dedup();
    unused.sort();
    unused.dedup();
    for v in samples.values_mut() {
        v.sort();
        v.dedup();
        v.truncate(TOP);
    }
    // A shadowed keyword's sample names its axis first.
    for line in &shadowed {
        if let Some((a, _)) = line.split_once(": ") {
            let mut doc = axis(&mut axes, a);
            doc["shadowed_keywords"]
                .as_array_mut()
                .map(|v| v.push(json!(line)))
                .unwrap_or_else(|| doc["shadowed_keywords"] = json!([line]));
            axes.insert(a.to_string(), doc);
        }
    }

    let by_value = by_value(store, &stacks, &scope_params, &shadowed)?;
    let origins = origins(store, &stacks, &scope_params)?;

    Ok(json!({
        "scope": scope.text(),
        "axes": axes,
        "by_value": by_value,
        "origins": origins,
        "open_review": open_by_kind,
        "diagnostics": diagnostics,
        "diagnostic_samples": samples,
        "shadowed_keywords": shadowed,
        "unused_overlay_terms": unused,
        "fields": fields,
    }))
}

/// One axis value's signals.
#[derive(Default)]
struct ValueSignals {
    decided: i64,
    by_flag: i64,
    by_word: i64,
    by_physics: i64,
    unsure: i64,
    shadowed: Vec<String>,
    overrode_with: Vec<(String, i64)>,
}

/// The signals by axis and value (record 26, decision 12), over the rows in
/// scope. `decided` counts the rows that store the value, whoever decided
/// it; `by_flag`, `by_word` and `by_physics` the rows the rules decided by
/// that kind of clause; `unsure` the stacks a person is still asked about
/// on that axis; `shadowed` the words that matched on the way to it and
/// were never cited, from the batch samples; `overrode_with` the words the
/// rule cited on stacks a person then decided otherwise.
fn by_value(
    store: &mut Store,
    stacks: &str,
    params: &[Param],
    shadowed: &[String],
) -> Result<BTreeMap<String, BTreeMap<String, Value>>, Error> {
    let mut out: BTreeMap<String, BTreeMap<String, ValueSignals>> = BTreeMap::new();
    // A slot for a value, bounded per axis; None when the axis is full.
    fn slot<'a>(
        out: &'a mut BTreeMap<String, BTreeMap<String, ValueSignals>>,
        axis: &str,
        value: &str,
    ) -> Option<&'a mut ValueSignals> {
        if value.is_empty() {
            return None;
        }
        let values = out.entry(axis.to_string()).or_default();
        if !values.contains_key(value) && values.len() >= VALUES_MAX {
            return None;
        }
        Some(values.entry(value.to_string()).or_default())
    }

    // How many rows store each value, and by which kind of clause.
    let sql = format!(
        "SELECT axis, value, tier, COUNT(*) FROM {} WHERE stack_id IN {stacks} AND value IS NOT NULL GROUP BY axis, value, tier ORDER BY axis, value, tier",
        store.qualified("classification_axis")
    );
    for r in store.query(&sql, params)? {
        let axis = r.text(0)?.to_string();
        let value = r.opt_text(1)?.unwrap_or("").to_string();
        let tier = r.text(2)?.to_string();
        let n = r.int(3)?;
        let Some(v) = slot(&mut out, &axis, &value) else {
            continue;
        };
        v.decided += n;
        match tier.as_str() {
            "exclusive" | "alternative" | "combination" => v.by_flag += n,
            "keywords" => v.by_word += n,
            "physics" => v.by_physics += n,
            _ => {}
        }
    }

    // The stacks a person is still asked about, on that axis, by the value
    // they were decided to: the open review items whose kind names the
    // axis before the colon.
    let sql = format!(
        "SELECT ca.axis, ca.value, COUNT(DISTINCT ca.stack_id) FROM {} AS ca JOIN {} AS m ON m.stack_id = ca.stack_id JOIN {} AS i ON i.id = m.item_id WHERE i.status = 'open' AND SUBSTR(i.kind, 1, LENGTH(ca.axis) + 1) = ca.axis || ':' AND ca.value IS NOT NULL AND ca.stack_id IN {stacks} GROUP BY ca.axis, ca.value",
        store.qualified("classification_axis"),
        store.qualified("review_member"),
        store.qualified("review_item")
    );
    for (axis, value, n) in count_rows(store, &sql, params)? {
        if let Some(v) = slot(&mut out, &axis, &value) {
            v.unsure = n;
        }
    }

    // The words the rule cited on stacks a person then decided otherwise,
    // by the value the rule had decided.
    let sql = format!(
        "SELECT e.axis, e.value, e.matched, COUNT(*) FROM {} AS d JOIN {} AS e ON CAST(e.stack_id AS TEXT) = d.ref AND e.axis = d.axis WHERE d.scope = 'stack' AND d.withdrawn_at IS NULL AND e.source = 'text' AND (d.value IS NULL OR d.value <> e.value) AND e.stack_id IN {stacks} GROUP BY e.axis, e.value, e.matched ORDER BY COUNT(*) DESC, e.axis, e.value, e.matched LIMIT 400",
        store.qualified("decision"),
        store.qualified("classification_evidence")
    );
    for r in store.query(&sql, params)? {
        let axis = r.text(0)?.to_string();
        let value = r.text(1)?.to_string();
        let word = r.opt_text(2)?.unwrap_or("").to_string();
        let n = r.int(3)?;
        if let Some(v) = slot(&mut out, &axis, &value)
            && v.overrode_with.len() < TOP
        {
            v.overrode_with.push((word, n));
        }
    }

    // A shadowed word belongs to the value its rule sets. An axis's own
    // rule is named after the value; a longhand rule is mapped through the
    // evidence rows in scope that cite it, which is what the batch has.
    let sql = format!(
        "SELECT rule_set, rule, axis, value FROM {} WHERE stack_id IN {stacks} AND source = 'text' GROUP BY rule_set, rule, axis, value LIMIT 2000",
        store.qualified("classification_evidence")
    );
    let mut rule_values: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    for r in store.query(&sql, params)? {
        rule_values
            .entry((r.text(0)?.to_string(), r.text(1)?.to_string()))
            .or_default()
            .push((r.text(2)?.to_string(), r.text(3)?.to_string()));
    }
    for line in shadowed {
        // "{axis}: {word} in {set}/{rule} behind {by_rule} [{by_matched}] ({n})"
        let Some((left, _)) = line.rsplit_once(" behind ") else {
            continue;
        };
        let Some((named, at)) = left.rsplit_once(" in ") else {
            continue;
        };
        let Some((axis, word)) = named.split_once(": ") else {
            continue;
        };
        let Some((set, rule)) = at.split_once('/') else {
            continue;
        };
        let targets: Vec<(String, String)> = if set == axis {
            vec![(axis.to_string(), rule.to_string())]
        } else {
            rule_values
                .get(&(set.to_string(), rule.to_string()))
                .cloned()
                .unwrap_or_default()
        };
        for (a, value) in targets {
            if let Some(v) = slot(&mut out, &a, &value)
                && v.shadowed.len() < TOP
                && !v.shadowed.iter().any(|w| w == word)
            {
                v.shadowed.push(word.to_string());
            }
        }
    }

    Ok(out
        .into_iter()
        .map(|(axis, values)| {
            let values = values
                .into_iter()
                .map(|(value, v)| {
                    let overrode: Vec<Value> = v
                        .overrode_with
                        .iter()
                        .map(|(w, n)| json!({"word": w, "count": n}))
                        .collect();
                    (
                        value,
                        json!({
                            "decided": v.decided,
                            "by_flag": v.by_flag,
                            "by_word": v.by_word,
                            "by_physics": v.by_physics,
                            "unsure": v.unsure,
                            "shadowed": v.shadowed,
                            "overrode_with": overrode,
                        }),
                    )
                })
                .collect();
            (axis, values)
        })
        .collect())
}

/// The origins of the stacks in scope, for the scope chips: each
/// manufacturer, model and station with how many stacks carry it, the
/// most common first and at most `TOP` of each kind.
fn origins(store: &mut Store, stacks: &str, params: &[Param]) -> Result<Vec<Value>, Error> {
    let mut out = Vec::new();
    for (column, kind) in ORIGINS {
        let sql = format!(
            "SELECT {column}, '', COUNT(*) FROM {} WHERE stack_id IN {stacks} AND {column} IS NOT NULL AND {column} <> '' GROUP BY {column} ORDER BY COUNT(*) DESC, 1 LIMIT {TOP}",
            store.qualified("stack_fingerprint")
        );
        for (name, _, n) in count_rows(store, &sql, params)? {
            out.push(json!({"name": name, "kind": kind, "stacks": n}));
        }
    }
    Ok(out)
}
