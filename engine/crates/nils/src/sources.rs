// SPDX-License-Identifier: AGPL-3.0-only

//! The sources door, for the desk's Data page: every place with the source
//! role, the dataset it is (record 26: what arrives, its two trees with the
//! counts the last probe kept, its identity rule, what an unmapped
//! identifier does, the cohort it feeds, its tag lists, what becomes of the
//! originals, and what the pseudonymiser holds), what its digests added
//! over time, and what they still need. A source's rows are the `source`
//! roots under the place's path. Its subjects, studies, sessions and stacks
//! are the ones its digests created first, so something seen from two
//! sources counts under the first. Every number here is a count. Where a
//! run counted a tree whole, the tree is counted as the dataset's summary
//! counts it rather than as the probe did (2026-10-17).
//!
//! The door answers at once (2026-11-12, [`answer`]). The places' rows,
//! the datasets' fields, the roots and the rates are read on every request.
//! What counts a dataset is kept for each dataset ([`crate::kept`], as the
//! dataset's summary keeps its counts), answered whatever its age and
//! counted again apart, one count at a time for a dataset, and kept in
//! `registry_meta` (`source_counts:<place>:<recent>`) for the next start
//! of the engine.
//!
//! What moves a dataset's stamp, so starts a count apart at the next
//! request: a batch begun or ended (a read, a pseudonymise run), the
//! newest finished job that is no worker (a sort, a pick, a model run, a
//! purge of the originals run as a job), a question raised or closed, a
//! judgement written, a merge of subjects, a label, the session cache
//! rebuilt, a pseudonymised file recorded for the first time, the
//! dataset's roots, any field of its place (what a probe keeps among
//! them), and the number of the counting ([`COUNTING`]), never the engine's
//! version. A write at a place's or a dataset's door marks every kept
//! count stale besides ([`forget`]). Whatever else changes a count (a
//! write straight to the registry by a command that runs no job) shows
//! within the keep's ten minutes, the bound after which the next request
//! starts a count apart; while a job or a batch runs the bound is a few
//! seconds, and at least ten times what the last count took. The
//! registry's epoch is not in the stamp: a person's every decision moves
//! it, and what a decision changes here moves the questions and the
//! judgements.
//!
//! The document is the same for every caller (nothing in it depends on
//! who reads a sealed sample), so one keep answers all.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use nils_registry::Registry;
use nils_registry::place::{self, Place, Role};
use nils_registry::session::Scheme;
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

use crate::kept::{Ask, Keep, Restored};

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
    let held = p.holds_paths(roots.iter().map(|(_, root)| Path::new(root)));
    roots
        .iter()
        .zip(held)
        .filter(|(_, held)| *held)
        .map(|((id, _), _)| *id)
        .collect()
}

/// A place's `source` rows: the roots under its path, as the sources door
/// counts its totals by (Wave 7a: the scans door lists the same stacks).
pub(crate) fn source_ids(store: &mut Store, p: &Place) -> Result<Vec<i64>, StoreError> {
    Ok(ids_under(p, &roots(store)?))
}

/// The number of the counting: what a dataset's kept counts are and how
/// they are counted ([`counts`]). Bump it by hand in the change that makes
/// them count something else or count it differently, so that the counts
/// an engine before kept are read again once, apart, while they are still
/// answered. It is not the engine's version: an update that changes
/// nothing here counts nothing again.
const COUNTING: u32 = 1;

/// How long a dataset's counts are kept while their stamp is the same:
/// the bound on what moves a count without moving the stamp.
const KEEP: Duration = Duration::from_secs(600);

/// And while a job or a batch runs, whose counts move as it goes: a few
/// seconds, and never less than ten times what the count took, so that a
/// dataset whose count takes seconds is not counted without a pause.
const KEEP_BUSY: Duration = Duration::from_secs(3);

/// The counts kept, by dataset and by how many digests are shown in full
/// ([`crate::kept`]).
static KEPT: LazyLock<Keep<Value>> = LazyLock::new(Default::default);

/// A dataset's slot in the keep.
fn slot_of(p: &Place, recent: usize) -> String {
    format!("{}:{recent}:{}", p.id, p.path)
}

/// The `registry_meta` key a dataset's counts are kept under between
/// starts of the engine.
fn stored_key(p: &Place, recent: usize) -> String {
    format!("source_counts:{}:{recent}", p.id)
}

/// What every dataset's counts are read under, read on every request from
/// indexes and small tables, and whether a job or a batch runs now: the
/// counting, what moves a dataset's summary ([`crate::dataset_summary::moved`]:
/// the batches, the newest finished job that is no worker, the questions,
/// the judgements, the merges, the labels and the session cache), the
/// newest pseudonymised file, and how the batches ended: a tree's count is
/// whole or partial by how its runs ended, and a run stopped after it
/// ended moves neither the batches' number nor the newest of them.
fn moved(store: &mut Store) -> Result<(String, bool), StoreError> {
    let (moved, busy) = crate::dataset_summary::moved(store)?;
    let batch = store.qualified("ingest_batch");
    let mut ended = Vec::new();
    for r in store.query(
        &format!("SELECT state, COUNT(*), MAX(id) FROM {batch} GROUP BY state ORDER BY state"),
        &[],
    )? {
        ended.push(format!(
            "{}:{}:{}",
            r.text(0)?,
            r.int(1)?,
            r.opt_int(2)?.unwrap_or(0)
        ));
    }
    Ok((
        format!(
            "counting {COUNTING}|{moved}|{}|{}",
            copies(store)?,
            ended.join(",")
        ),
        busy,
    ))
}

/// What one dataset's counts are read under: what moves them all, how many
/// digests are shown in full, its sources and its place's own fields (the
/// probe's counts among them).
fn stamp(moved: &str, p: &Place, ids: &[i64], recent: usize) -> String {
    format!("{moved}|{recent}|{ids:?}|{}", p.as_json())
}

/// What the pseudonymised files say, read from an index in a millisecond
/// however many there are (2026-10-17: a count of every row of
/// `pseudonym_file` cost 2.5 seconds, and up to 9 under load, on a registry
/// of 34 million, on every request to the sources door; 2026-11-01: the
/// counts of the held, refused and unchanged files grouped by place still
/// cost 1.7 seconds): only the newest row, which moves with every file a
/// writer records for the first time, in a batch or outside one. What a run
/// does to files it already holds moves with its batch (the batches' count,
/// newest id and running state are in the stamp), and a place removed is
/// no longer listed.
fn copies(store: &mut Store) -> Result<String, StoreError> {
    let files = store.qualified("pseudonym_file");
    let newest = store.query(&format!("SELECT MAX(id) FROM {files}"), &[])?[0]
        .opt_int(0)?
        .unwrap_or(0);
    Ok(newest.to_string())
}

/// One reading of a dataset's counts.
struct Count {
    place: Place,
    ids: Vec<i64>,
    recent: usize,
}

impl crate::kept::Reading<Value> for Count {
    type Error = StoreError;

    fn read(&mut self, registry: &mut Registry) -> Result<Value, StoreError> {
        let store = registry.store();
        let window = window(store)?;
        let asks = crate::certainty::Asks::sort(store)?;
        counts(store, &asks, &self.place, &self.ids, window, self.recent)
    }

    fn stored(&self, stamp: &str, as_of: &str, value: &Value) -> Option<String> {
        Some(
            json!({"path": self.place.path, "stamp": stamp, "as_of": as_of, "counts": value})
                .to_string(),
        )
    }
}

/// The counts the registry keeps of a dataset, read once after a start of
/// the engine; none where it keeps another folder's.
fn stored_read(text: &str, p: &Place) -> Option<Restored<Value>> {
    let stored: Value = serde_json::from_str(text).ok()?;
    if stored["path"].as_str() != Some(p.path.as_str()) || !stored["counts"].is_object() {
        return None;
    }
    Some(Restored {
        stamp: stored["stamp"].as_str()?.to_string(),
        as_of: stored["as_of"].as_str()?.to_string(),
        value: stored["counts"].clone(),
    })
}

/// The active source places.
fn listed(store: &mut Store) -> Result<Vec<Place>, StoreError> {
    Ok(place::list(store)?
        .into_iter()
        .filter(|p| p.role == Role::Source && p.retired_at.is_none())
        .collect())
}

/// Record 26: the sessions counted are the ones the cache holds, under
/// the window it was built with; the scheme's own where it is empty.
fn window(store: &mut Store) -> Result<i64, StoreError> {
    Ok(
        nils_registry::cohort::built_window(store)?
            .unwrap_or_else(|| Scheme::default().window_days),
    )
}

/// Record 26 section 14: what this machine does per second, from the last
/// run of each step that ended here.
fn rates(store: &mut Store) -> Result<Value, StoreError> {
    Ok(json!({
        "pseudonymize": crate::batches::rate_of(store, "pseudonymize")?,
        "digest": crate::batches::rate_of(store, "digest")?,
    }))
}

/// The sources door's document, answered at once (2026-11-12). What is
/// cheap is read on every request, so a write at a door shows in the very
/// next answer: the places' rows, each dataset's fields, its roots and the
/// rates. What counts a dataset (what the pseudonymiser holds, its trees
/// as its runs counted them, its digests and its totals) is kept for each
/// dataset under a stamp of what it reads ([`stamp`]) and answered whatever
/// its age, while one count of that dataset runs apart where the stamp
/// moved, a door wrote or the keep ran out ([`crate::kept`]); each source
/// says so (`counting`, with `counts_as_of`), and the document does where
/// any of them does. The counts are kept in the registry too, so a start
/// of the engine answers the last ones.
///
/// Only a dataset never counted makes a request wait for its first count;
/// with `at_once` it does not either: the source is answered with
/// `counted` false and without its `held`, `digests` and `totals`, as the
/// summary's door answers a first count, and the count runs apart. `fresh`
/// counts every dataset now, for a caller who asked for a probe and reads
/// what it found.
///
/// Until 2026-11-12 (from 2026-10-15) the whole document was kept for a
/// minute and read again in front of the request that found it run out: on
/// a registry of 34 million pseudonymised files one request in five waited
/// 7 to 17 seconds for every dataset's counts.
pub fn answer(
    registry: &mut Registry,
    recent: usize,
    at_once: bool,
    fresh: bool,
) -> Result<Value, StoreError> {
    let store = registry.store();
    let places = listed(store)?;
    let roots = roots(store)?;
    let rates = rates(store)?;
    let (moved, busy) = moved(store)?;
    let mut sources = Vec::with_capacity(places.len());
    let (mut counting, mut oldest, mut window) = (false, None::<String>, None::<i64>);
    for p in &places {
        let ids = ids_under(p, &roots);
        let answer = KEPT.answer(
            registry,
            &Ask {
                slot: &slot_of(p, recent),
                stored: &stored_key(p, recent),
                stamp: &stamp(&moved, p, &ids, recent),
                ttl: if busy { KEEP_BUSY } else { KEEP },
                paced: busy,
                wait: !at_once,
                fresh,
            },
            |text| stored_read(text, p),
            || Count {
                place: p.clone(),
                ids: ids.clone(),
                recent,
            },
        )?;
        let mut doc = laid(p, ids.len(), answer.value.as_deref());
        doc["counted"] = json!(answer.value.is_some());
        doc["counting"] = json!(answer.counting);
        doc["counts_as_of"] = match &answer.value {
            Some(_) => json!(answer.as_of),
            None => Value::Null,
        };
        counting |= answer.counting;
        if let Some(c) = &answer.value {
            window = window.or(c["window_days"].as_i64());
            if oldest.as_ref().is_none_or(|o| answer.as_of < *o) {
                oldest = Some(answer.as_of.clone());
            }
        }
        sources.push(doc);
    }
    // the window the kept sessions were counted under; the cache's own
    // where no dataset is counted yet
    let window = match window {
        Some(w) => w,
        None => self::window(registry.store())?,
    };
    Ok(json!({
        "count": sources.len(),
        "window_days": window,
        "sources": sources,
        "rates": rates,
        "counting": counting,
        "counts_as_of": oldest,
    }))
}

/// The sources door's document for a caller who waits for a dataset's
/// first count: the engine as it starts, and a desk from before the door
/// answered a first count as running.
pub fn document_kept(registry: &mut Registry, recent: usize) -> Result<Value, StoreError> {
    answer(registry, recent, false, false)
}

/// Mark every dataset's kept counts stale: a write at a door may have
/// changed them, so they are counted again at the next request, which they
/// still answer (2026-11-12; forgotten outright until then, so the request
/// after a write waited for every count).
pub fn forget() {
    KEPT.forget();
}

/// The sources document read whole now, nothing kept: each active source
/// place with its digests, the newest `recent` of them in full, and its
/// totals. What the tests compare the kept answer with.
#[cfg(test)]
pub fn document(registry: &mut Registry, recent: usize) -> Result<Value, StoreError> {
    let store = registry.store();
    let window = window(store)?;
    let places = listed(store)?;
    let roots = roots(store)?;
    // what makes a scan need a look, read once for every dataset
    let asks = crate::certainty::Asks::sort(store)?;
    let mut sources = Vec::with_capacity(places.len());
    for p in &places {
        let ids = ids_under(p, &roots);
        let counts = counts(store, &asks, p, &ids, window, recent)?;
        sources.push(laid(p, ids.len(), Some(&counts)));
    }
    let rates = rates(store)?;
    Ok(json!({"count": sources.len(), "window_days": window, "sources": sources, "rates": rates}))
}

/// One source as the door answers it: its place's row and its dataset's
/// fields as they are now, and its counts as they were counted, none while
/// its first count runs.
fn laid(p: &Place, roots: usize, counts: Option<&Value>) -> Value {
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
    doc["roots"] = json!(roots);
    let Some(counts) = counts else {
        return doc;
    };
    doc["held"] = counts["held"].clone();
    // the trees as the dataset's runs counted them, laid over the probe's
    for at in ["trees", "dataset"] {
        let trees = if at == "trees" {
            &mut doc["trees"]
        } else {
            &mut doc["dataset"]["trees"]
        };
        for tree in ["originals", "anon"] {
            let Some(counted) = counts["trees"][tree].as_object() else {
                continue;
            };
            if trees[tree].is_object() {
                for (key, value) in counted {
                    trees[tree][key] = value.clone();
                }
            }
        }
    }
    doc["digests"] = counts["digests"].clone();
    doc["totals"] = counts["totals"].clone();
    doc
}

/// What counts a dataset, the part of a source that is kept: what the
/// pseudonymiser holds, what its runs counted of its trees, its digests
/// and its totals, with the window its sessions are counted under.
fn counts(
    store: &mut Store,
    asks: &crate::certainty::Asks,
    p: &Place,
    ids: &[i64],
    window: i64,
    recent: usize,
) -> Result<Value, StoreError> {
    let mut doc = json!({
        "window_days": window,
        "held": held(store, p.id)?,
        "trees": {"originals": {}, "anon": {}},
    });
    if ids.is_empty() {
        doc["digests"] = json!({"count": 0, "first": null, "last": null, "recent": []});
        doc["totals"] = json!({
            "subjects": 0, "studies": 0, "sessions": 0, "stacks": 0, "refused_files": 0,
            "same_instance_files": 0,
            "to_sort": 0, "sure": 0, "unsorted": 0, "outside": 0, "outside_by": {}, "need_a_look": {}, "noted": {},
        });
        return Ok(doc);
    }
    // 2026-10-17: the trees counted as the dataset's summary counts them,
    // from its runs once they walked a tree, since the probe is bounded and
    // stops short of a large one: for a dataset of 30 million files whose
    // run had written all but the refused ones, the Originals box said what
    // the probe reached in two seconds and the copy none. The place's own
    // `probed` stays the probe's.
    // The overlay sets a tree's counts where the tree is one, so what it
    // would set is kept apart from the place's row and laid over it as the
    // row is at each request.
    let counted = crate::dataset_summary::counted(store, p, ids)?;
    counted.overlay(&mut doc["trees"]);
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
        "outside": certainty.outside,
        "outside_by": certainty.outside_by,
        "need_a_look": certainty.need_a_look,
        "noted": certainty.noted,
    });
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::list;
    use nils_registry::home::{Home, InitOptions};
    use nils_registry::place::{self, Place};
    use nils_registry::{Registry, Store};
    use serde_json::{Value, json};

    /// The tests of the keep run one at a time: each marks every kept
    /// count stale as a door does.
    static KEPT_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct MadeUp {
        _dir: nils_dicom::synth::TempDir,
        home: Home,
        registry: Registry,
        ward: i64,
        source: i64,
    }

    /// A registry with one dataset, `ward`, that has one root and nothing
    /// read yet.
    fn made_up(name: &str) -> MadeUp {
        let dir = nils_dicom::synth::TempDir::new(name);
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
        let folder = dir.path().join("ward");
        std::fs::create_dir_all(&folder).unwrap();
        let ward = place::add(
            registry.store(),
            &place::New {
                name: "ward",
                role: place::Role::Source,
                path: &folder.display().to_string(),
                guarantees: json!({}),
                probed: json!({}),
                handling: Value::Null,
                dataset: json!({
                    "kind": "legacy",
                    "arrives": "deidentified",
                    "state": "anonymised",
                    "trees": {"originals": null, "anon": "."},
                    "patient_id": "id-type:patient-id",
                    "subjects": "map",
                }),
            },
        )
        .unwrap();
        let root = std::fs::canonicalize(&folder).unwrap();
        let store = registry.store();
        let source = store.qualified("source");
        store
            .execute(
                &format!(
                    "INSERT INTO {source} (root, root_canonical, first_seen_at) \
                     VALUES ('{0}', '{0}', '2026-10-17T00:00:00Z')",
                    root.display()
                ),
                &[],
            )
            .unwrap();
        let source = store
            .query(&format!("SELECT MAX(id) FROM {source}"), &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        MadeUp {
            _dir: dir,
            home,
            registry,
            ward,
            source,
        }
    }

    impl MadeUp {
        fn place(&mut self) -> Place {
            place::list(self.registry.store())
                .unwrap()
                .into_iter()
                .find(|p| p.id == self.ward)
                .unwrap()
        }

        fn slot(&mut self) -> String {
            super::slot_of(&self.place(), 12)
        }

        /// How many times the dataset was counted.
        fn reads(&mut self) -> usize {
            let slot = self.slot();
            super::KEPT.reads_of(&slot)
        }

        fn settled(&mut self) {
            let slot = self.slot();
            super::KEPT.settled(&slot);
        }

        /// A digest of the dataset, begun.
        fn batch(&mut self, state: &str) {
            let store = self.registry.store();
            let batch = store.qualified("ingest_batch");
            store
                .execute(
                    &format!(
                        "INSERT INTO {batch} (source_id, name, config, started_at, state, kind) \
                         VALUES ({}, 'b', '{{}}', '2026-10-17T00:00:00Z', '{state}', 'digest')",
                        self.source
                    ),
                    &[],
                )
                .unwrap();
        }
    }

    /// What of a source is counted, without when and whether it is counted
    /// again.
    fn counted_of(doc: &Value) -> Value {
        let s = &doc["sources"][0];
        json!({"held": s["held"], "digests": s["digests"], "totals": s["totals"], "trees": s["trees"]})
    }

    /// 2026-11-12: a dataset's counts are counted once and answered from
    /// then on whatever moves: at once with the kept counts, said to be
    /// counted again, while one count runs apart, and with the new counts
    /// once it is over. A write at a door shows in the very next answer.
    #[test]
    fn a_source_answers_its_kept_counts_at_once_and_counts_again_apart() {
        let _one_at_a_time = KEPT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = made_up("sources-kept");
        let first = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(m.reads(), 1);
        assert_eq!(first["count"], 1);
        assert_eq!(first["counting"], false, "{first}");
        assert!(first["counts_as_of"].as_str().unwrap().starts_with("20"));
        let source = &first["sources"][0];
        assert_eq!(source["counted"], true, "{source}");
        assert_eq!(source["counting"], false, "{source}");
        assert_eq!(source["counts_as_of"], first["counts_as_of"]);
        assert_eq!(source["digests"]["count"], 0, "{source}");
        assert_eq!(source["totals"]["stacks"], 0, "{source}");
        assert_eq!(source["held"], json!({"files": 0, "identifiers": 0}));
        // what is counted is what a reading of the whole document says
        let whole = super::document(&mut m.registry, 12).unwrap();
        assert_eq!(counted_of(&first), counted_of(&whole));
        // kept: a second request counts nothing
        let again = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(again, first);
        assert_eq!(m.reads(), 1, "kept");
        // the registry's epoch alone, which a person's decision moves, and
        // a worker's row count nothing again
        m.registry.next_epoch().unwrap();
        m.registry
            .store()
            .execute(
                "INSERT INTO job (kind, name, state, started_at) \
                 VALUES ('worker', 'queue', 'running', '2026-10-17T00:00:00Z')",
                &[],
            )
            .unwrap();
        assert_eq!(super::document_kept(&mut m.registry, 12).unwrap(), first);
        assert_eq!(m.reads(), 1, "the epoch and a worker move nothing");

        // what is counted moved (a digest ended): the kept counts at once,
        // said to be counted again, and one count apart
        m.batch("done");
        let stale = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(stale["counting"], true, "{stale}");
        assert_eq!(stale["sources"][0]["counting"], true);
        assert_eq!(stale["sources"][0]["counts_as_of"], first["counts_as_of"]);
        assert_eq!(counted_of(&stale), counted_of(&first));
        assert_eq!(m.reads(), 2, "counted again apart");
        m.settled();
        // and the new counts once it is over
        let new = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(new["counting"], false, "{new}");
        assert_eq!(new["sources"][0]["digests"]["count"], 1, "{new}");
        assert_eq!(m.reads(), 2, "the count apart was the one");

        // a write at a door (the place renamed, a field of its dataset
        // set): the very next answer says it, with the kept counts
        let dataset = json!({
            "kind": "legacy",
            "arrives": "deidentified",
            "state": "anonymised",
            "trees": {"originals": null, "anon": "."},
            "patient_id": "id-type:patient-id",
            "subjects": "map",
            "cohort": "made-up-cohort",
        });
        m.registry
            .store()
            .execute(
                &format!(
                    "UPDATE place SET name = 'ward-two', dataset = '{}' WHERE id = {}",
                    dataset, m.ward
                ),
                &[],
            )
            .unwrap();
        super::forget();
        let written = super::document_kept(&mut m.registry, 12).unwrap();
        let source = &written["sources"][0];
        assert_eq!(source["name"], "ward-two", "{source}");
        assert_eq!(source["cohort"], "made-up-cohort", "{source}");
        assert_eq!(source["dataset"]["cohort"], "made-up-cohort", "{source}");
        assert_eq!(source["counting"], true, "{source}");
        assert_eq!(source["digests"], new["sources"][0]["digests"]);
        assert_eq!(m.reads(), 3);
        m.settled();
        let after = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(after["counting"], false, "{after}");
        assert_eq!(m.reads(), 3);

        // the keep run out: answered still, counted again apart
        let slot = m.slot();
        super::KEPT.aged(&slot);
        let old = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(old["counting"], true, "{old}");
        assert_eq!(counted_of(&old), counted_of(&after));
        assert_eq!(m.reads(), 4);
        m.settled();

        // six requests at once after a door wrote: each answered at once
        // with the kept counts, and one count apart
        super::forget();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(6));
        let docs: Vec<Value> = (0..6)
            .map(|_| {
                let (gate, home) = (gate.clone(), m.home.clone());
                std::thread::spawn(move || {
                    let mut mine = home.open().unwrap();
                    gate.wait();
                    super::document_kept(&mut mine, 12).unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        for doc in &docs {
            assert_eq!(counted_of(doc), counted_of(&after));
        }
        m.settled();
        assert_eq!(m.reads(), 5, "one count for six requests");
    }

    /// 2026-11-12: the counts outlive a start of the engine, kept in the
    /// registry under their stamp, which names the counting and not the
    /// engine's version; a dataset never counted waits for its first count
    /// or, asked at once, is answered as being counted; and a probe asked
    /// for is answered with a count made after it.
    #[test]
    fn a_start_answers_the_registry_s_counts_and_a_first_count_can_run_apart() {
        let _one_at_a_time = KEPT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = made_up("sources-start");
        // never counted and asked at once: no counts yet, one count apart
        let pending = super::answer(&mut m.registry, 12, true, false).unwrap();
        let source = &pending["sources"][0];
        assert_eq!(pending["counting"], true, "{pending}");
        assert_eq!(pending["counts_as_of"], Value::Null);
        assert_eq!(source["counted"], false, "{source}");
        assert_eq!(source["counting"], true, "{source}");
        assert_eq!(source["counts_as_of"], Value::Null);
        assert_eq!(source["name"], "ward");
        assert_eq!(source["roots"], 1);
        assert!(source.get("totals").is_none(), "{source}");
        assert_eq!(m.reads(), 1);
        m.settled();
        let first = super::answer(&mut m.registry, 12, true, false).unwrap();
        assert_eq!(first["sources"][0]["counted"], true, "{first}");
        assert_eq!(first["counting"], false, "{first}");
        assert_eq!(m.reads(), 1);

        // a start of the engine: its memory is empty and the registry's
        // counts are answered, with nothing counted
        let slot = m.slot();
        super::KEPT.dropped(&slot);
        let started = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(started, first);
        assert_eq!(m.reads(), 1, "a start counts nothing");

        // the stamp names the counting, never the engine's version, so an
        // update alone counts nothing again
        let place = m.place();
        let (moved, _) = super::moved(m.registry.store()).unwrap();
        let stamp = super::stamp(&moved, &place, &[m.source], 12);
        assert!(stamp.starts_with("counting 1|"), "{stamp}");
        assert!(!stamp.contains(env!("CARGO_PKG_VERSION")), "{stamp}");
        let key = super::stored_key(&place, 12);
        let text = nils_registry::home::get_meta(m.registry.store(), &key)
            .unwrap()
            .unwrap();
        let mut stored: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(stored["stamp"], stamp);
        // counts kept by another counting: answered at once, counted again
        // apart
        stored["stamp"] = json!(stamp.replacen("counting 1|", "counting 0|", 1));
        nils_registry::home::set_meta(m.registry.store(), &key, &stored.to_string()).unwrap();
        super::KEPT.dropped(&slot);
        let other = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(other["counting"], true, "{other}");
        assert_eq!(counted_of(&other), counted_of(&first));
        assert_eq!(m.reads(), 2);
        m.settled();

        // a probe asked for: counted now, in front, after the probe
        m.batch("done");
        let probed = super::answer(&mut m.registry, 12, false, true).unwrap();
        assert_eq!(probed["counting"], false, "{probed}");
        assert_eq!(probed["sources"][0]["digests"]["count"], 1, "{probed}");
        assert_eq!(m.reads(), 3);

        // with neither the engine's memory nor the registry's copy, a
        // caller who waits is answered with the first count
        super::KEPT.dropped(&slot);
        m.registry
            .store()
            .execute(
                &format!("DELETE FROM registry_meta WHERE key = '{key}'"),
                &[],
            )
            .unwrap();
        let waited = super::document_kept(&mut m.registry, 12).unwrap();
        assert_eq!(waited["counting"], false, "{waited}");
        assert_eq!(counted_of(&waited), counted_of(&probed));
        assert_eq!(m.reads(), 4);
    }

    /// 2026-10-17: the stamp reads an index and small tables, and still
    /// moves wherever a count of every state of every pseudonymised file
    /// would (the stamp before this one): when a run records a file it
    /// wrote, held or refused for the first time, and when it writes, holds,
    /// refuses or finds unchanged one it recorded before in another state.
    /// A run that only touches a file it recorded, as it does an unchanged
    /// copy, moves neither. 2026-11-12: nor does the registry's epoch,
    /// which a person's every decision moves; the place's own fields do.
    #[test]
    fn the_stamp_moves_on_a_batch_a_new_file_and_the_place_s_fields_not_on_a_worker() {
        let mut m = made_up("sources-stamp");
        let (ward, source) = (m.ward, m.source);
        let files = m.registry.store().qualified("pseudonym_file");
        let stamp = |m: &mut MadeUp| {
            let place = m.place();
            let (moved, _) = super::moved(m.registry.store()).unwrap();
            super::stamp(&moved, &place, &[source], 12)
        };
        // a file as a run's flush records it: upserted on its place and path
        let record = |store: &mut Store, path: &str, state: &str| {
            store
                .execute(
                    &format!(
                        "INSERT INTO {files} (place_id, path, size, mtime, state, batch_id, first_seen, code_anyway) \
                         VALUES ({ward}, '{path}', 1, 1, '{state}', 1, '2026-10-17T00:00:00Z', 0) \
                         ON CONFLICT (place_id, path) DO UPDATE SET state = excluded.state, batch_id = excluded.batch_id"
                    ),
                    &[],
                )
                .unwrap();
        };
        let at = "2026-10-17T00:00:00Z";
        let was = stamp(&mut m);
        // a worker's row, its beat and its end move nothing
        let store = m.registry.store();
        store
            .execute(
                &format!("INSERT INTO job (kind, name, state, started_at) VALUES ('worker', 'queue', 'running', '{at}')"),
                &[],
            )
            .unwrap();
        store
            .execute(
                &format!("UPDATE job SET heartbeat_at = '{at}' WHERE kind = 'worker'"),
                &[],
            )
            .unwrap();
        assert_eq!(stamp(&mut m), was, "a worker's row");
        // a file recorded for the first time, outside any batch, moves it
        record(m.registry.store(), "a.dcm", "written");
        let after_write = stamp(&mut m);
        assert_ne!(after_write, was, "a file written outside a batch");
        // a file a run already holds, held, refused or found unchanged, moves
        // nothing by itself: the run's batch carries it
        for state in ["held", "refused", "unchanged", "written"] {
            record(m.registry.store(), "a.dcm", state);
            assert_eq!(stamp(&mut m), after_write, "a file {state} again");
        }
        // a batch begun and ended moves it
        m.batch("running");
        let begun = stamp(&mut m);
        assert_ne!(begun, after_write, "a batch began");
        m.registry
            .store()
            .execute(
                &format!("UPDATE ingest_batch SET finished_at = '{at}', state = 'done'"),
                &[],
            )
            .unwrap();
        let ended = stamp(&mut m);
        assert_ne!(ended, begun, "a batch ended");
        // the epoch alone moves nothing
        m.registry.next_epoch().unwrap();
        assert_eq!(stamp(&mut m), ended, "the epoch moved");
        // the place's own fields, what a probe keeps among them, move it
        m.registry
            .store()
            .execute(
                &format!("UPDATE place SET name = 'ward-two' WHERE id = {ward}"),
                &[],
            )
            .unwrap();
        assert_ne!(stamp(&mut m), ended, "the place renamed");
    }

    #[test]
    fn an_id_list_is_the_ids_joined() {
        assert_eq!(list(&[3, 14, 15]), "3, 14, 15");
        assert_eq!(list(&[]), "");
    }
}
