// SPDX-License-Identifier: AGPL-3.0-only

//! System 1 as a pipeline (record 55 D3, spec §11.1; slice P2).
//!
//! System 1 runs as the pipeline [`PIPELINE`] whose model is a site model,
//! never part of the public engine: the model chosen for the task
//! `system1` (record 55 D4), which a site's admin chooses on the Models
//! page, or by default the one a recorded certificate names. A door runs it
//! over a dataset's or a cohort's scans, or over every scan of the registry
//! (the archive, on request), as a pipeline job; its results' `opinions`
//! are kept as System 1's answer, probability, reliability and the fields
//! that mattered on each stack and axis (`nils_registry::opinion`), never a
//! decision.
//!
//! The pipeline reads the registry's rows a header model needs
//! (`x-nils.input.rows`, [`header_rows`]): each stack's whole fingerprint
//! row, its classification votes as `nils classify votes` writes them, and
//! its series' private elements and frame of reference (as a digest), for
//! the run's stacks and the other MR stacks of their sessions, which are
//! read and never scored.

use std::collections::{BTreeMap, BTreeSet};

use nils_registry::schema::table;
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

/// The pipeline System 1 runs as.
pub(crate) const PIPELINE: &str = "system1-score";

/// The step's name at the steps door.
pub(crate) const STEP: &str = "system1";

/// The fingerprint columns a header model is never given: the row's own
/// keys and bookkeeping, and the acquisition's date and time, which no
/// header model needs.
const LEFT_OUT: [&str; 6] = [
    "id",
    "job_id",
    "epoch",
    "earliest_acquisition_date",
    "earliest_acquisition_time",
    "fingerprint_revision",
];

fn ids_in(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn sha256_hex(text: &str) -> String {
    crate::supervise::sha256_hex(text.as_bytes())
}

/// `stacks.json`'s `rows` for a run's stacks (`x-nils.input.rows`): by
/// stack, the whole fingerprint row (less [`LEFT_OUT`]), the classification
/// votes and whether the stack is the run's own (`scored`); by series, the
/// digest of its frame of reference and its private elements. The other MR
/// stacks of the run's sessions are listed too, unscored, so a model reads
/// a stack against its session; a stack of a sample sealed now is listed
/// only where it is the run's own (and the pipeline's answer about it is
/// dropped when it is taken in).
pub(crate) fn header_rows(store: &mut Store, run: &[i64]) -> Result<Value, String> {
    let err = |e: StoreError| e.to_string();
    let own: BTreeSet<i64> = run.iter().copied().collect();
    // the sessions the run's stacks are part of
    let mut studies: BTreeSet<i64> = BTreeSet::new();
    for chunk in run.chunks(500) {
        let sql = format!(
            "SELECT DISTINCT se.study_id FROM {} st JOIN {} se ON se.id = st.series_id WHERE st.id IN ({})",
            store.qualified("stack"),
            store.qualified("series"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            studies.insert(r.int(0).map_err(err)?);
        }
    }
    // their MR stacks, the run's own first
    let mut stacks: BTreeSet<i64> = own.clone();
    let studies: Vec<i64> = studies.into_iter().collect();
    for chunk in studies.chunks(500) {
        let sql = format!(
            "SELECT st.id FROM {} st JOIN {} se ON se.id = st.series_id LEFT JOIN {} fp ON fp.stack_id = st.id \
             WHERE se.study_id IN ({}) AND UPPER(COALESCE(NULLIF(st.modality, ''), fp.modality, '')) = 'MR'",
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("stack_fingerprint"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            stacks.insert(r.int(0).map_err(err)?);
        }
    }
    let others: Vec<i64> = stacks.difference(&own).copied().collect();
    let (sealed, _) =
        nils_registry::labels::sealed_now(store, &others, &[]).map_err(|e| e.to_string())?;
    let stacks: Vec<i64> = stacks
        .into_iter()
        .filter(|s| own.contains(s) || !sealed.contains(s))
        .collect();
    // the fingerprint rows, every column but the left out
    let t = table("stack_fingerprint");
    let d = store.dialect();
    let columns: Vec<&nils_registry::schema::Column> = t
        .columns
        .iter()
        .filter(|c| !LEFT_OUT.contains(&c.name))
        .collect();
    let select = columns
        .iter()
        .map(|c| d.text_of(c))
        .collect::<Vec<_>>()
        .join(", ");
    let mut by_stack: BTreeMap<i64, serde_json::Map<String, Value>> = BTreeMap::new();
    let mut series_of: BTreeMap<i64, i64> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT stack_id, {select} FROM {} WHERE stack_id IN ({})",
            store.qualified("stack_fingerprint"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let stack = r.int(0).map_err(err)?;
            let mut row = serde_json::Map::new();
            for (i, c) in columns.iter().enumerate() {
                use nils_registry::schema::Type;
                let at = i + 1;
                let v = match c.ty {
                    Type::Int | Type::Id | Type::Bool => json!(r.opt_int(at).map_err(err)?),
                    Type::Double => json!(r.opt_double(at).map_err(err)?),
                    _ => json!(r.opt_text(at).map_err(err)?),
                };
                row.insert(c.name.to_string(), v);
            }
            if let Some(s) = row.get("series_id").and_then(Value::as_i64) {
                series_of.insert(stack, s);
            }
            by_stack.insert(stack, row);
        }
    }
    // the votes, as `nils classify votes` writes them, the run's own only
    let mut voters: BTreeMap<i64, Value> = BTreeMap::new();
    let sql = format!(
        "SELECT id, axis, rule_set, rule, clause, tier, restates FROM {}",
        store.qualified("classification_voter")
    );
    for r in store.query(&sql, &[]).map_err(err)? {
        voters.insert(
            r.int(0).map_err(err)?,
            json!([
                r.text(1).map_err(err)?,
                r.text(2).map_err(err)?,
                r.text(3).map_err(err)?,
                r.int(4).map_err(err)?,
                Value::Null,
                r.text(5).map_err(err)?,
                r.int(6).map_err(err)?,
            ]),
        );
    }
    let mut votes: BTreeMap<i64, Vec<Value>> = BTreeMap::new();
    let mine: Vec<i64> = own.iter().copied().collect();
    for chunk in mine.chunks(500) {
        let sql = format!(
            "SELECT stack_id, votes FROM {} WHERE stack_id IN ({}) ORDER BY stack_id, phase",
            store.qualified("classification_vote"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let stack = r.int(0).map_err(err)?;
            let pairs: Vec<(i64, String)> =
                serde_json::from_str(r.text(1).map_err(err)?).unwrap_or_default();
            let list = votes.entry(stack).or_default();
            for (voter, value) in pairs {
                if let Some(v) = voters.get(&voter) {
                    // [axis, rule_set, rule, clause, value, tier, restates]
                    let mut row = v.clone();
                    row[4] = json!(value);
                    list.push(row);
                }
            }
        }
    }
    // each series's frame of reference, as a digest, and private elements
    let series: Vec<i64> = series_of
        .values()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut series_doc = serde_json::Map::new();
    let pt = table("series_private");
    for chunk in series.chunks(500) {
        let sql = format!(
            "SELECT id, frame_of_reference_uid FROM {} WHERE id IN ({})",
            store.qualified("series"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let frame = r
                .opt_text(1)
                .map_err(err)?
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(sha256_hex);
            series_doc.insert(
                r.int(0).map_err(err)?.to_string(),
                json!({"frame_of_reference": frame, "private": Value::Null}),
            );
        }
        let sql = format!(
            "SELECT series_id, {} FROM {} WHERE series_id IN ({})",
            d.text_of(pt.column("elements").expect("elements")),
            store.qualified("series_private"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let elements: Value = r
                .opt_text(1)
                .map_err(err)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            if let Some(entry) = series_doc.get_mut(&r.int(0).map_err(err)?.to_string()) {
                entry["private"] = elements;
            }
        }
    }
    let mut stacks_doc = serde_json::Map::new();
    for (stack, row) in by_stack {
        stacks_doc.insert(
            stack.to_string(),
            json!({
                "scored": own.contains(&stack),
                "fingerprint": Value::Object(row),
                "votes": votes.remove(&stack).unwrap_or_default(),
            }),
        );
    }
    Ok(json!({
        "votes": ["axis", "rule_set", "rule", "clause", "value", "tier", "restates"],
        "stacks": Value::Object(stacks_doc),
        "series": Value::Object(series_doc),
    }))
}
