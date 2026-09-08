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

    Ok(json!({
        "scope": scope.text(),
        "axes": axes,
        "open_review": open_by_kind,
        "diagnostics": diagnostics,
        "diagnostic_samples": samples,
        "shadowed_keywords": shadowed,
        "unused_overlay_terms": unused,
        "fields": fields,
    }))
}
