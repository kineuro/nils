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

/// Every `source` row with its canonical root.
pub(crate) fn roots(store: &mut Store) -> Result<Vec<(i64, String)>, StoreError> {
    store
        .query(
            &format!(
                "SELECT id, root_canonical FROM {}",
                store.qualified("source")
            ),
            &[],
        )?
        .iter()
        .map(|r| Ok((r.int(0)?, r.text(1)?.to_string())))
        .collect()
}

pub(crate) fn ids_under(p: &Place, roots: &[(i64, String)]) -> Vec<i64> {
    roots
        .iter()
        .filter(|(_, root)| p.holds_path(Path::new(root)))
        .map(|(id, _)| *id)
        .collect()
}

/// A place's `source` rows: the roots under its path, as the sources door
/// counts its totals by (Wave 7a: the scans door lists the same stacks).
pub(crate) fn source_ids(store: &mut Store, p: &Place) -> Result<Vec<i64>, StoreError> {
    Ok(ids_under(p, &roots(store)?))
}

/// The sources document: each active source place with its digests, the
/// newest `recent` of them in full, and its totals.
/// The sources door's answer kept (the Data page's speed, 2026-10-15): the
/// document is a reading of the registry and the places, and the page asks
/// it on every visit, where computing it reads every dataset's counts
/// (some 400 to 600 ms on a registry of a few hundred scans on slow
/// storage). It is kept under what it was read from, the registry's epoch,
/// the places' rows, the digests' and the pseudonymised files' counts, and
/// answered again while those are the same, for a minute, or a few seconds
/// while a digest runs, whose counts move without the epoch; any write at
/// a door forgets it, and the engine reads it once as it starts.
struct KeptSources {
    key: String,
    at: std::time::Instant,
    ttl: std::time::Duration,
    doc: Value,
}

static KEPT: std::sync::Mutex<Option<KeptSources>> = std::sync::Mutex::new(None);

/// How many times the document was read from the registry rather than
/// answered from the kept one, for the tests.
static READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// What the kept document was read under, and whether a digest runs now.
fn stamp(store: &mut Store, recent: usize) -> Result<(String, bool), StoreError> {
    let meta = store.qualified("registry_meta");
    let epoch = store
        .query(
            &format!("SELECT value FROM {meta} WHERE key = 'epoch'"),
            &[],
        )?
        .first()
        .map(|r| r.opt_text(0).map(|t| t.unwrap_or_default().to_string()))
        .transpose()?
        .unwrap_or_default();
    let places: Vec<Value> = place::list(store)?.iter().map(Place::as_json).collect();
    let batch = store.qualified("ingest_batch");
    let b = &store.query(
        &format!(
            "SELECT COUNT(*), COALESCE(MAX(id), 0), \
             COALESCE(SUM(CASE WHEN finished_at IS NULL THEN 1 ELSE 0 END), 0) FROM {batch}"
        ),
        &[],
    )?[0];
    let (batches, last, running) = (b.int(0)?, b.int(1)?, b.int(2)?);
    let files = store.qualified("pseudonym_file");
    let f = &store.query(
        &format!(
            "SELECT COUNT(*), COALESCE(MAX(id), 0), \
             COALESCE(SUM(CASE WHEN state = 'written' THEN 1 ELSE 0 END), 0) FROM {files}"
        ),
        &[],
    )?[0];
    let key = format!(
        "{epoch}|{recent}|{batches}|{last}|{running}|{}|{}|{}|{}",
        f.int(0)?,
        f.int(1)?,
        f.int(2)?,
        Value::Array(places)
    );
    Ok((key, running > 0))
}

/// The sources door's document, from the kept one where it holds.
pub fn document_kept(registry: &mut Registry, recent: usize) -> Result<Value, StoreError> {
    let (key, running) = stamp(registry.store(), recent)?;
    {
        let kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = kept.as_ref()
            && k.key == key
            && k.at.elapsed() < k.ttl
        {
            return Ok(k.doc.clone());
        }
    }
    READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let doc = document(registry, recent)?;
    *KEPT.lock().unwrap_or_else(|e| e.into_inner()) = Some(KeptSources {
        key,
        at: std::time::Instant::now(),
        ttl: std::time::Duration::from_secs(if running { 3 } else { 60 }),
        doc: doc.clone(),
    });
    Ok(doc)
}

/// Forget the kept document: a write at a door may have changed it.
pub fn forget() {
    *KEPT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// How many times the document was read from the registry.
#[cfg(test)]
pub fn reads() -> usize {
    READS.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn document(registry: &mut Registry, recent: usize) -> Result<Value, StoreError> {
    let store = registry.store();
    // record 26: the sessions counted are the ones the cache holds, under
    // the window it was built with; the scheme's own where it is empty
    let window = nils_registry::cohort::built_window(store)?
        .unwrap_or_else(|| Scheme::default().window_days);
    let places: Vec<Place> = place::list(store)?
        .into_iter()
        .filter(|p| p.role == Role::Source && p.retired_at.is_none())
        .collect();
    let roots = roots(store)?;
    // what makes a scan need a look, read once for every dataset
    let asks = crate::certainty::Asks::sort(store)?;
    let mut sources = Vec::with_capacity(places.len());
    for p in &places {
        let ids = ids_under(p, &roots);
        sources.push(source(store, &asks, p, &ids, window, recent)?);
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
    asks: &crate::certainty::Asks,
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
        "patient_id",
        "subjects",
        "copy_folder",
        "cohort",
        "tags",
        "originals_kept",
        "originals_vault",
        "picks",
    ] {
        doc[key] = doc["dataset"][key].clone();
    }
    doc["held"] = held(store, p.id)?;
    doc["roots"] = json!(ids.len());
    if ids.is_empty() {
        doc["digests"] = json!({"count": 0, "first": null, "last": null, "recent": []});
        doc["totals"] = json!({
            "subjects": 0, "studies": 0, "sessions": 0, "stacks": 0, "refused_files": 0,
            "same_instance_files": 0,
            "to_sort": 0, "sure": 0, "unsorted": 0, "need_a_look": {}, "noted": {},
        });
        return Ok(doc);
    }
    let sources = list(ids);
    let q = |t: &str| store.qualified(t);
    let (batch, stack, study, file, cache, class) = (
        q("ingest_batch"),
        q("stack"),
        q("study"),
        q("source_file"),
        q("session_cache_study"),
        q("classification"),
    );
    // record 55 (2026-10-10): what the dataset holds is every scan its tree
    // has a file of, whoever read it first
    let in_tree = crate::operations::held_by(store, "x", &sources);
    let studies_in_tree = crate::operations::studies_held_by(store, "x", &sources);

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
                    // record 55 (2026-10-10): copies of an instance the
                    // registry held, another dataset's or this one's, and
                    // files held because it holds their instance UID under
                    // another subject, study or series
                    "known": n(&["written", "known"]),
                    "twice": n(&["written", "twice"]),
                    "same_instance": n(&["written", "same_instance"]),
                    "changed": n(&["written", "changed"]),
                    "unchanged": n(&["unchanged"]),
                    "refused": n(&["quarantined"]),
                },
                "subjects_added": n(&["written", "subjects_created"]),
                "stacks_added": n(&["written", "stacks_created"]),
                // the pseudonymise step before the digest, once there is one
                "pseudonymised": null,
                // record 26 §14: the jobs of the thread by stage, so that a
                // page can walk from a digest to the run that wrote the
                // files and to the runs that sorted them
                "chain": {"pseudonymize": null, "digest": r.opt_int(5)?, "classify": []},
            }),
        ));
    }
    for (id, doc) in shown.iter_mut() {
        if let Some(b) = crate::batches::row(store, *id)?
            && let Some(step) = crate::batches::other_side(store, &b)?
        {
            let step = crate::batches::pseudonymised(&step);
            doc["chain"]["pseudonymize"] = step["job"].clone();
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

    // what each of the recent digests has had judged, and how many of its
    // stacks need a look, as the card counts them
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
        let look = asks.by_first_batch(store, &shown_ids)?;
        // the classify runs over each batch's stacks, the last stage of the
        // thread the sources door can name
        let classifiers = pairs(
            store,
            &format!(
                "SELECT DISTINCT st.first_batch_id, cl.job_id FROM {stack} st \
                 JOIN {class} cl ON cl.stack_id = st.id \
                 WHERE st.first_batch_id IN ({batches}) AND cl.job_id IS NOT NULL \
                 ORDER BY st.first_batch_id, cl.job_id"
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
            doc["to_sort"] = json!(look.get(id).map_or(0, |a| a.scans));
            doc["chain"]["classify"] = json!(
                classifiers
                    .iter()
                    .filter(|(b, _)| b == id)
                    .map(|(_, job)| *job)
                    .collect::<Vec<_>>()
            );
        }
    }

    // record 26 §14: the subjects whose files this dataset's tree holds,
    // whoever made them. A map makes the subjects of a dataset and its
    // digests meet them, so counting what a digest created answers nothing
    // for exactly the dataset that has a map. Record 55 (2026-10-10): and
    // so are its studies, scans and visits, whoever read them first. Nima,
    // 2026-10-12: a dataset holds stacks, never subjects; the subjects
    // counted in it are those with a stack its tree holds a file of, as
    // every reader counts them (`nils_registry::dataset_subjects`)
    let subjects = nils_registry::dataset_subjects::count(store, ids)?;
    let studies = count(
        store,
        &format!("SELECT COUNT(*) FROM {study} x WHERE {studies_in_tree}"),
    )?;
    let stacks = count(
        store,
        &format!("SELECT COUNT(*) FROM {stack} x WHERE {in_tree}"),
    )?;
    let sessions = count(
        store,
        &format!(
            "SELECT COUNT(DISTINCT scs.session_id) FROM {cache} scs JOIN {study} x ON x.id = scs.study_id \
             WHERE {studies_in_tree} AND scs.window_days = {window}"
        ),
    )?;
    // a file held because the registry holds its instance under another
    // subject, study or series waits for a person, and is no refusal
    let same = nils_registry::review::SAME_INSTANCE_KIND;
    let refused = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
             AND (reason IS NULL OR reason <> '{same}')"
        ),
    )?;
    let same_instance = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
             AND reason = '{same}'"
        ),
    )?;
    // record 55 H2 (round 4): how sure the sort is, "N scans · N sure ·
    // N need a look", with what the questions are; a look as everywhere
    // (record 56: the sort's own questions alone)
    let certainty = crate::certainty::of_sources(store, asks, &sources, stacks)?;
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
        "same_instance_files": same_instance,
        "to_sort": certainty.to_sort,
        "sure": certainty.sure,
        "unsorted": certainty.unsorted,
        "need_a_look": certainty.need_a_look,
        "noted": certainty.noted,
    });
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::list;

    /// The sources door's document is read once and kept while the
    /// registry's epoch and the places are the same; a write (the epoch
    /// moves) or a forget reads it again.
    #[test]
    fn the_sources_document_is_kept_until_the_registry_changes() {
        use nils_registry::home::{Home, InitOptions};
        let dir = nils_dicom::synth::TempDir::new("sources-kept");
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        let mut registry = home
            .init(&InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::pseudonym::Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
                session_scheme: None,
            })
            .unwrap();
        let read = |registry: &mut nils_registry::Registry| {
            let before = super::reads();
            let doc = super::document_kept(registry, 12).unwrap();
            (doc, super::reads() - before)
        };
        let (first, n) = read(&mut registry);
        assert_eq!(n, 1);
        assert_eq!(first["count"], 0);
        let (again, n) = read(&mut registry);
        assert_eq!((n, &again), (0, &first), "kept");
        registry.next_epoch().unwrap();
        assert_eq!(read(&mut registry).1, 1, "the epoch moved");
        assert_eq!(read(&mut registry).1, 0);
        super::forget();
        assert_eq!(read(&mut registry).1, 1, "forgotten");
    }

    #[test]
    fn an_id_list_is_the_ids_joined() {
        assert_eq!(list(&[3, 14, 15]), "3, 14, 15");
        assert_eq!(list(&[]), "");
    }
}
