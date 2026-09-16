// SPDX-License-Identifier: AGPL-3.0-only

//! The sources door, for the desk's Data page: every place with the source
//! role, the dataset it is (record 26: what arrives, its two trees with the
//! counts the last probe kept, its identity rule, what an unmapped
//! identifier does, the cohort it feeds, its tag lists, what becomes of the
//! originals, and what the pseudonymiser holds), what its digests added
//! over time, and what they still need. A source's rows are the `source`
//! roots under the place's path. Its subjects, studies, sessions and stacks
//! are the ones its digests created first, so something seen from two
//! sources counts under the first. Every number here is a count.

use std::path::Path;

use nils_registry::Registry;
use nils_registry::place::{self, Place, Role};
use nils_registry::session::Scheme;
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

fn count(store: &mut Store, sql: &str) -> Result<i64, StoreError> {
    store.query(sql, &[])?[0].int(0)
}

/// `(key, count)` rows as pairs.
fn pairs(store: &mut Store, sql: &str) -> Result<Vec<(i64, i64)>, StoreError> {
    store
        .query(sql, &[])?
        .iter()
        .map(|r| Ok((r.int(0)?, r.int(1)?)))
        .collect()
}

/// A comma list of ids read from the store, safe to set in SQL as it is.
fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the pseudonymiser holds of a dataset for want of a map: the files,
/// and the identifiers they carry, counted by their keyed lookup.
fn held(store: &mut Store, place_id: i64) -> Result<Value, StoreError> {
    let sql = format!(
        "SELECT COUNT(*), COUNT(DISTINCT lookup) FROM {} WHERE place_id = {place_id} AND state = 'held'",
        store.qualified("pseudonym_file")
    );
    let row = &store.query(&sql, &[])?[0];
    Ok(json!({"files": row.int(0)?, "identifiers": row.int(1)?}))
}

/// The sources document: each active source place with its digests, the
/// newest `recent` of them in full, and its totals.
pub fn document(registry: &mut Registry, recent: usize) -> Result<Value, StoreError> {
    let window = Scheme::default().window_days;
    let store = registry.store();
    let places: Vec<Place> = place::list(store)?
        .into_iter()
        .filter(|p| p.role == Role::Source && p.retired_at.is_none())
        .collect();
    let roots: Vec<(i64, String)> = store
        .query(
            &format!(
                "SELECT id, root_canonical FROM {}",
                store.qualified("source")
            ),
            &[],
        )?
        .iter()
        .map(|r| Ok((r.int(0)?, r.text(1)?.to_string())))
        .collect::<Result<_, StoreError>>()?;
    let mut sources = Vec::with_capacity(places.len());
    for p in &places {
        let ids: Vec<i64> = roots
            .iter()
            .filter(|(_, root)| p.holds_path(Path::new(root)))
            .map(|(id, _)| *id)
            .collect();
        sources.push(source(store, p, &ids, window, recent)?);
    }
    // record 26 §14: what this machine does per second, from the last run
    // of each step that ended here
    let rates = json!({
        "pseudonymize": crate::batches::rate_of(store, "pseudonymize")?,
        "digest": crate::batches::rate_of(store, "digest")?,
    });
    Ok(json!({"count": sources.len(), "window_days": window, "sources": sources, "rates": rates}))
}

fn source(
    store: &mut Store,
    p: &Place,
    ids: &[i64],
    window: i64,
    recent: usize,
) -> Result<Value, StoreError> {
    let mut doc = p.as_json();
    // the dataset's fields at the top, as the page reads them
    for key in [
        "arrives",
        "trees",
        "identity",
        "unmapped",
        "cohort",
        "tags",
        "originals_kept",
    ] {
        doc[key] = doc["dataset"][key].clone();
    }
    doc["held"] = held(store, p.id)?;
    doc["roots"] = json!(ids.len());
    if ids.is_empty() {
        doc["digests"] = json!({"count": 0, "first": null, "last": null, "recent": []});
        doc["totals"] = json!({"subjects": 0, "studies": 0, "sessions": 0, "stacks": 0, "refused_files": 0, "to_sort": 0});
        return Ok(doc);
    }
    let sources = list(ids);
    let q = |t: &str| store.qualified(t);
    let (batch, stack, study, subject, file, cache, member, item, class) = (
        q("ingest_batch"),
        q("stack"),
        q("study"),
        q("subject"),
        q("source_file"),
        q("session_cache_study"),
        q("review_member"),
        q("review_item"),
        q("classification"),
    );
    let of_source =
        format!("JOIN {batch} b ON b.id = x.first_batch_id WHERE b.source_id IN ({sources})");
    let open = "ri.status IN ('open', 'staged')";

    // the digests: a pseudonymise step is not one, and shows on the
    // digest it shares a name with (record 26 §14)
    let rows = store.query(
        &format!(
            "SELECT id, name, state, {}, {}, job_id, {} FROM {batch} WHERE source_id IN ({sources}) \
             AND (kind IS NULL OR kind = 'digest') ORDER BY id DESC",
            crate::text_of(store, "ingest_batch", "started_at"),
            crate::text_of(store, "ingest_batch", "finished_at"),
            crate::text_of(store, "ingest_batch", "counts"),
        ),
        &[],
    )?;
    let digests = rows.len();
    let mut shown = Vec::new();
    for r in rows.iter().take(recent.max(1)) {
        let counts = r
            .opt_text(6)?
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or(Value::Null);
        let n = |path: &[&str]| {
            path.iter()
                .fold(&counts, |v, k| &v[*k])
                .as_u64()
                .unwrap_or(0)
        };
        shown.push((
            r.int(0)?,
            json!({
                "id": r.int(0)?,
                "name": r.text(1)?,
                "state": r.text(2)?,
                "started_at": r.opt_text(3)?,
                "finished_at": r.opt_text(4)?,
                "job_id": r.opt_int(5)?,
                "files": {
                    "seen": n(&["seen"]),
                    "new": n(&["written", "ingested"]),
                    "changed": n(&["written", "changed"]),
                    "unchanged": n(&["unchanged"]),
                    "refused": n(&["quarantined"]),
                },
                "subjects_added": n(&["written", "subjects_created"]),
                "stacks_added": n(&["written", "stacks_created"]),
                // the pseudonymise step before the digest, once there is one
                "pseudonymised": null,
            }),
        ));
    }
    for (id, doc) in shown.iter_mut() {
        if let Some(b) = crate::batches::row(store, *id)?
            && let Some(step) = crate::batches::other_side(store, &b)?
        {
            let step = crate::batches::pseudonymised(&step);
            doc["pseudonymised"] = json!({
                "files": step["files"], "changed": step["changed"], "held": step["held"],
                "job": step["job"], "batch": step["batch"],
            });
        }
    }
    let first_last = |r: &nils_registry::store::Row| -> Result<Value, StoreError> {
        Ok(
            json!({"id": r.int(0)?, "name": r.text(1)?, "state": r.text(2)?, "started_at": r.opt_text(3)?, "finished_at": r.opt_text(4)?}),
        )
    };
    let last = rows.first().map(first_last).transpose()?;
    let first = rows.last().map(first_last).transpose()?;

    // what each of the recent digests has had judged, and what still waits for a person
    let shown_ids: Vec<i64> = shown.iter().map(|(id, _)| *id).collect();
    if !shown_ids.is_empty() {
        let batches = list(&shown_ids);
        let classified = pairs(
            store,
            &format!(
                "SELECT st.first_batch_id, COUNT(*) FROM {stack} st JOIN {class} cl ON cl.stack_id = st.id \
                 WHERE st.first_batch_id IN ({batches}) GROUP BY st.first_batch_id"
            ),
        )?;
        let unsure = pairs(
            store,
            &format!(
                "SELECT st.first_batch_id, COUNT(DISTINCT st.id) FROM {stack} st \
                 JOIN {member} rm ON rm.stack_id = st.id JOIN {item} ri ON ri.id = rm.item_id \
                 WHERE {open} AND st.first_batch_id IN ({batches}) GROUP BY st.first_batch_id"
            ),
        )?;
        for (id, doc) in shown.iter_mut() {
            let of = |v: &[(i64, i64)]| {
                v.iter()
                    .find(|(b, _)| b == id)
                    .map(|(_, n)| *n)
                    .unwrap_or(0)
            };
            doc["classified"] = json!(of(&classified));
            doc["to_sort"] = json!(of(&unsure));
        }
    }

    let subjects = count(
        store,
        &format!("SELECT COUNT(*) FROM {subject} x {of_source} AND x.merged_into IS NULL"),
    )?;
    let studies = count(
        store,
        &format!("SELECT COUNT(*) FROM {study} x {of_source}"),
    )?;
    let stacks = count(
        store,
        &format!("SELECT COUNT(*) FROM {stack} x {of_source}"),
    )?;
    let sessions = count(
        store,
        &format!(
            "SELECT COUNT(DISTINCT scs.session_id) FROM {cache} scs JOIN {study} x ON x.id = scs.study_id \
             {of_source} AND scs.window_days = {window}"
        ),
    )?;
    let refused = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined'"
        ),
    )?;
    let to_sort = count(
        store,
        &format!(
            "SELECT COUNT(DISTINCT rm.stack_id) FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
             JOIN {stack} x ON x.id = rm.stack_id {of_source} AND {open}"
        ),
    )?;
    doc["digests"] = json!({
        "count": digests,
        "first": first,
        "last": last,
        "recent": shown.into_iter().map(|(_, d)| d).collect::<Vec<_>>(),
    });
    doc["totals"] = json!({
        "subjects": subjects,
        "studies": studies,
        "sessions": sessions,
        "stacks": stacks,
        "refused_files": refused,
        "to_sort": to_sort,
    });
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::list;

    #[test]
    fn an_id_list_is_the_ids_joined() {
        assert_eq!(list(&[3, 14, 15]), "3, 14, 15");
        assert_eq!(list(&[]), "");
    }
}
