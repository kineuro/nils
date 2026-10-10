// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 7a, the Data page (2026-10-09): one dataset whole, for the detail
//! under its card and the six steps on the card itself.
//! `GET /api/datasets/{name}/summary` answers what the dataset holds (its
//! subjects, visits and scans, how sure the sort is, the kinds of scan and
//! the body regions the sort found, its files and the refused ones) and
//! where it is: found, pseudonymised where it has originals, read, sorted,
//! body part, post-contrast, main scans, pictures and 3D views, each with
//! its counts, its state and when it last ran. Body part and post-contrast
//! are operations of their own (record 56, section 2, in `operations`), and
//! how sure the sort is counts only the questions the sort asks, by the one
//! definition of a look the card, the sources door, the scans doors and the
//! Grid count by (`certainty`). Every number is a count and nothing
//! quasi-identifying is answered, so the door is Data reading at plain, as
//! the sources door is. A stack of a sample sealed now is never counted by
//! kind or region for a caller who does not read sealed stacks (record 48),
//! as the scans door never lists it.
//!
//! The jobs of a dataset are the ones whose command line names it (`@name`
//! or a tree under it, `--dataset name`, `dataset:name`, `place originals
//! name`) or that carry its name and a date as the desk and a bring-in name
//! them, the ones its digests ran under, the sorts that judged its stacks,
//! the runs of the body-part and post-contrast models over its stacks and
//! the jobs a door queued for those steps (`for` dataset:name), and every
//! job queued after any of those: the next step of a chain, the pictures
//! and the pick run after a sort. `GET /api/jobs?dataset=name` lists the
//! same jobs, newest first.

use std::collections::BTreeSet;
use std::path::Path;

use nils_registry::Registry;
use nils_registry::place::{self, Place};
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

use crate::grants::Access;
use crate::serve::Reply;

/// The kinds of job a dataset's steps are made of: what is looked through
/// for its jobs.
const KINDS: &[&str] = &[
    "pseudonymize",
    "digest",
    "ingest",
    "fingerprint",
    "classify",
    "pick",
    "pyramid",
    "preview",
    "originals",
    "pipeline",
];

/// How many of the newest jobs of those kinds are looked through.
pub(crate) const WINDOW: usize = 2000;

/// How long after a sort's row says done its pictures may still be being
/// made in the same run, in seconds: past it, a run that never wrote them
/// is not said to be making them.
const MAKING: u64 = 6 * 3600;

/// The axis the kinds of scan are read from, and the modifier that makes a
/// scan of any base a FLAIR, as a person names it (the MRI pack's axes).
const KIND_AXIS: &str = "base";
const FLAIR: (&str, &str) = ("modifier", "FLAIR");
/// The axis the body regions are read from (record 50: the body part's
/// coarse mode).
const REGION_AXIS: &str = "body_region";

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One job as the relation and the steps read it: no result, which may be
/// large and is not needed here.
#[derive(Debug, Clone)]
pub(crate) struct Light {
    pub(crate) id: i64,
    pub(crate) kind: String,
    pub(crate) name: Option<String>,
    pub(crate) state: String,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) args: Value,
    pub(crate) progress: Option<Value>,
}

impl Light {
    fn open(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "running" | "cancelling")
    }
}

/// The words of a job's command line, as a card reads them: what a door or
/// the keyboard queued, else the line it ran with the binary and the
/// registry taken off.
fn words(args: &Value) -> Vec<String> {
    let strings = |v: &Value| {
        v.as_array().map(|a| {
            a.iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
    };
    strings(&args["queued"])
        .or_else(|| strings(&args["argv"]).map(|a| nils_registry::job::words_of(&a)))
        .unwrap_or_default()
}

/// Whether a command line names the dataset: by name, or by a path in its
/// folder, as the jobs door locates `@name` before it queues a digest.
pub(crate) fn names(words: &[String], dataset: &str, folders: &[String]) -> bool {
    let at = format!("@{dataset}");
    let under = format!("{at}/");
    let flag = format!("--dataset={dataset}");
    let label = format!("dataset:{dataset}");
    let inside = |w: &str| {
        folders.iter().any(|f| {
            w.strip_prefix(f.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
    };
    let named = words.iter().enumerate().any(|(i, w)| {
        *w == at
            || w.starts_with(&under)
            || *w == flag
            || *w == label
            || (w == "--dataset" && words.get(i + 1).is_some_and(|n| n == dataset))
            || (w.starts_with('/') && inside(w))
    });
    let originals = words.first().is_some_and(|w| w == "place")
        && words.get(1).is_some_and(|w| w == "originals")
        && words.get(2).is_some_and(|w| w == dataset);
    named || originals
}

/// Whether a job's name is the dataset's with a day, as the desk and a
/// bring-in name a dataset's steps (`study-2026-10-09`), or the dataset's
/// own name. The day is read whole, so `study-2026-10-09` names `study`
/// and never `study-2026`.
pub(crate) fn named_for(name: Option<&str>, dataset: &str) -> bool {
    let Some(name) = name else {
        return false;
    };
    if name == dataset {
        return true;
    }
    let Some(day) = name
        .strip_prefix(dataset)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    let b = day.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// The value after a flag on a command line, as a job id.
fn flag_id(words: &[String], flag: &str) -> Option<i64> {
    words
        .iter()
        .position(|w| w == flag)
        .and_then(|i| words.get(i + 1))
        .and_then(|v| v.parse::<i64>().ok())
}

/// The newest jobs of the kinds a dataset's steps are made of.
fn newest(store: &mut Store, window: usize) -> Result<Vec<Light>, StoreError> {
    let kinds = KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, kind, name, state, {}, {}, {}, {} FROM {} WHERE kind IN ({kinds}) \
         ORDER BY id DESC LIMIT {window}",
        crate::text_of(store, "job", "started_at"),
        crate::text_of(store, "job", "finished_at"),
        crate::text_of(store, "job", "args"),
        crate::text_of(store, "job", "progress"),
        store.qualified("job"),
    );
    let json = |t: Option<&str>| t.and_then(|t| serde_json::from_str::<Value>(t).ok());
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| {
            Ok(Light {
                id: r.int(0)?,
                kind: r.text(1)?.to_string(),
                name: r.opt_text(2)?.map(str::to_string),
                state: r.text(3)?.to_string(),
                started_at: r.opt_text(4)?.map(str::to_string),
                finished_at: r.opt_text(5)?.map(str::to_string),
                args: json(r.opt_text(6)?).unwrap_or(Value::Null),
                progress: json(r.opt_text(7)?),
            })
        })
        .collect()
}

/// The jobs a dataset's digests ran under, the sorts that judged its
/// stacks, and the runs of the operations' models over them.
fn seeds(store: &mut Store, sources: &[i64]) -> Result<BTreeSet<i64>, StoreError> {
    let mut out = BTreeSet::new();
    if sources.is_empty() {
        return Ok(out);
    }
    let ids = list(sources);
    out.extend(crate::operations::run_jobs(
        store,
        crate::operations::Scope::Sources(&ids),
    )?);
    for r in store.query(
        &format!(
            "SELECT DISTINCT job_id FROM {} WHERE source_id IN ({ids}) AND job_id IS NOT NULL",
            store.qualified("ingest_batch")
        ),
        &[],
    )? {
        out.insert(r.int(0)?);
    }
    for r in store.query(
        &format!(
            "SELECT DISTINCT cl.job_id FROM {} cl JOIN {} x ON x.id = cl.stack_id WHERE {}",
            store.qualified("classification"),
            store.qualified("stack"),
            crate::operations::held_by(store, "x", &ids),
        ),
        &[],
    )? {
        out.insert(r.int(0)?);
    }
    Ok(out)
}

/// A dataset's jobs among the newest `window`, newest first: the ones that
/// name it, its digests' and its sorts', and every one queued after any of
/// those (a chain's next step, the pictures and the pick run after a
/// sort). One pass from the oldest finds every link, since a job is only
/// ever queued after one that came before it.
pub(crate) fn jobs_of(
    store: &mut Store,
    dataset: &Place,
    window: usize,
) -> Result<Vec<Light>, StoreError> {
    let sources = crate::sources::source_ids(store, dataset)?;
    let mut related = seeds(store, &sources)?;
    // the folder as declared and as the disk resolves it, which a linked
    // tree makes two
    let mut folders = vec![dataset.path.trim_end_matches('/').to_string()];
    if let Ok(real) = std::fs::canonicalize(&dataset.path) {
        let real = real.display().to_string();
        if !folders.contains(&real) {
            folders.push(real);
        }
    }
    // a step's run a door queued for it, before its run names its stacks
    let queued_for = format!("dataset:{}", dataset.name);
    // a folder added again after its dataset was removed (2026-10-10): the
    // removed dataset's runs are its own, never the new one's, so where the
    // folder held a dataset before, only the runs since this one was added
    // are this one's
    let added_again = was_a_dataset_before(store, dataset, &folders)?;
    let mut jobs = newest(store, window)?;
    jobs.reverse();
    let mut out = Vec::new();
    for j in jobs {
        // a run made for another dataset is that one's, whatever folder it read
        if j.args["place_id"]
            .as_i64()
            .is_some_and(|id| id != dataset.id)
        {
            continue;
        }
        if added_again
            && j.started_at
                .as_deref()
                .is_some_and(|t| t < dataset.created_at.as_str())
        {
            continue;
        }
        let w = words(&j.args);
        let linked = |id: Option<i64>| id.is_some_and(|id| related.contains(&id));
        let mine = related.contains(&j.id)
            || names(&w, &dataset.name, &folders)
            || named_for(j.name.as_deref(), &dataset.name)
            || j.args["for"].as_str() == Some(queued_for.as_str())
            || linked(j.args["chain_before"].as_i64())
            || linked(j.args["after"].as_i64())
            || linked(flag_id(&w, "--classified"))
            || linked(flag_id(&w, "--after-sort"));
        if mine {
            related.insert(j.id);
            out.push(j);
        }
    }
    out.reverse();
    Ok(out)
}

/// Whether the dataset's folder or its name was another dataset's, removed
/// before this one was added: a source place retired on the same folder, as
/// declared or as the disk resolves it, or one that gave its name up to
/// this one (`place::retired_name`), whose runs name it still.
fn was_a_dataset_before(
    store: &mut Store,
    dataset: &Place,
    folders: &[String],
) -> Result<bool, StoreError> {
    let same = |path: &str| {
        let declared = path.trim_end_matches('/').to_string();
        let real = std::fs::canonicalize(path)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| declared.clone());
        folders.contains(&declared) || folders.contains(&real)
    };
    Ok(place::list(store)?.into_iter().any(|p| {
        p.id != dataset.id
            && p.role == place::Role::Source
            && p.retired_at.is_some()
            && (same(&p.path) || p.name == place::retired_name(&dataset.name, p.id))
    }))
}

/// The ids of a dataset's jobs, newest first, for the jobs door.
pub(crate) fn job_ids(store: &mut Store, dataset: &Place) -> Result<Vec<i64>, StoreError> {
    Ok(jobs_of(store, dataset, WINDOW)?
        .into_iter()
        .map(|j| j.id)
        .collect())
}

fn count(store: &mut Store, sql: &str) -> Result<i64, StoreError> {
    store.query(sql, &[])?[0].int(0)
}

/// `(label, scans)` by value of an axis over the dataset's stacks, most
/// first; for the kind axis a FLAIR is named as one whatever its base.
fn by_axis(
    store: &mut Store,
    sources: &str,
    axis: &str,
    sealed: Option<&str>,
) -> Result<Vec<(String, i64)>, StoreError> {
    let (class_axis, stack) = (
        store.qualified("classification_axis"),
        store.qualified("stack"),
    );
    let held = crate::operations::held_by(store, "x", sources);
    let label = if axis == KIND_AXIS {
        format!(
            "CASE WHEN EXISTS (SELECT 1 FROM {class_axis} m WHERE m.stack_id = a.stack_id \
             AND m.axis = '{}' AND m.value = '{}') THEN '{}' ELSE a.value END",
            FLAIR.0, FLAIR.1, FLAIR.1
        )
    } else {
        "a.value".to_string()
    };
    let hidden = sealed
        .map(|s| {
            format!(
                " AND NOT EXISTS (SELECT 1 FROM {s} sst WHERE sst.stack_id = a.stack_id AND sst.unsealed_at IS NULL)"
            )
        })
        .unwrap_or_default();
    let sql = format!(
        "SELECT k.label, COUNT(*) FROM (SELECT {label} AS label FROM {class_axis} a \
         JOIN {stack} x ON x.id = a.stack_id \
         WHERE a.axis = '{axis}' AND a.value IS NOT NULL AND a.value <> '' \
         AND {held}{hidden}) k GROUP BY k.label"
    );
    let mut out: Vec<(String, i64)> = store
        .query(&sql, &[])?
        .iter()
        .map(|r| Ok((r.text(0)?.to_string(), r.int(1)?)))
        .collect::<Result<_, StoreError>>()?;
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

/// How many of the stacks have a preview and how many a 3D view built
/// under the working place: a file each, looked at on a few threads, since
/// a working place may be a share.
fn made(working: &Path, stacks: &[i64]) -> (i64, i64) {
    if stacks.is_empty() {
        return (0, 0);
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8);
    let chunk = stacks.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = stacks
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    let mut pictures = 0;
                    let mut views = 0;
                    for stack in part {
                        if crate::preview::path(working, *stack, false).is_file() {
                            pictures += 1;
                        }
                        if crate::pyramid::dir(working, *stack)
                            .join("manifest.json")
                            .is_file()
                        {
                            views += 1;
                        }
                    }
                    (pictures, views)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or((0, 0)))
            .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    })
}

/// A step's state: the open job's, else done or waiting.
fn state_of(open: Option<&Light>, done: bool) -> &'static str {
    match open.map(|j| j.state.as_str()) {
        Some("running" | "cancelling") => "running",
        Some(_) => "queued",
        None if done => "done",
        None => "waiting",
    }
}

/// A step of the dataset's chain (pseudonymise, read, sort, main scans):
/// the open job's state, else `failed` where the step's newest job failed,
/// so a run that stopped says so (2026-10-10: a pseudonymise run that failed
/// at once left the page as if nothing had happened), else done or waiting.
/// The job's own words are the jobs door's, never the summary's.
fn chain_state(open: Option<&Light>, newest: Option<&Light>, done: bool) -> &'static str {
    if open.is_none() && newest.is_some_and(|j| j.state == "failed") {
        return "failed";
    }
    state_of(open, done)
}

/// One step: its name, state and counts, and the job that ran it last (or
/// runs it now) with its times and its progress while it runs.
fn step(name: &str, state: &str, job: Option<&Light>, counts: Value) -> Value {
    let mut doc = json!({
        "step": name,
        "state": state,
        "job": job.map(|j| j.id),
        "started_at": job.and_then(|j| j.started_at.clone()),
        "finished_at": job.and_then(|j| j.finished_at.clone()),
        "progress": job.filter(|j| j.open()).and_then(|j| j.progress.clone()),
    });
    if let (Some(doc), Some(counts)) = (doc.as_object_mut(), counts.as_object()) {
        for (k, v) in counts {
            doc.insert(k.clone(), v.clone());
        }
    }
    doc
}

/// The summary of one dataset, as `GET /api/datasets/{name}/summary`
/// answers it.
pub(crate) fn document(
    registry: &mut Registry,
    access: &Access,
    dataset: &Place,
) -> Result<Value, Reply> {
    // what a run of body part or post-contrast would be here, or why a door
    // would refuse it
    let plans = crate::operations::plans(registry).map_err(failed)?;
    let store = registry.store();
    let ids = crate::sources::source_ids(store, dataset).map_err(failed)?;
    let state = dataset.dataset["state"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let doc = &dataset.dataset_doc();
    let tree = |t: &str, k: &str| doc["trees"][t][k].as_i64();
    let has_originals = !doc["trees"]["originals"].is_null();
    let jobs = jobs_of(store, dataset, WINDOW).map_err(failed)?;
    let newest_of = |kinds: &[&str]| jobs.iter().find(|j| kinds.contains(&j.kind.as_str()));
    let open_of = |kinds: &[&str]| {
        jobs.iter()
            .find(|j| j.open() && kinds.contains(&j.kind.as_str()))
    };
    let held = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {} WHERE place_id = {} AND state = 'held'",
            store.qualified("pseudonym_file"),
            dataset.id
        ),
    )
    .map_err(failed)?;

    // what the registry holds of it: nothing before its first read
    let window = nils_registry::cohort::built_window(store).map_err(failed)?;
    let sources = list(&ids);
    // the visits are none where nobody has built the session cache, and
    // a count of it where somebody has
    let (mut subjects, mut studies, mut stacks, mut sessions) = (0, 0, 0, window.map(|_| 0));
    let (mut read, mut refused, mut reads, mut classified) = (0, 0, 0, 0);
    // record 55 (2026-10-10): the files read as their instance's own, the
    // copies of an instance another dataset read (`known`) or this one holds
    // a file of already (`twice`), and the files held because the registry
    // holds their instance UID under another subject, study or series
    let (mut new, mut known, mut twice, mut same_instance) = (0, 0, 0, 0);
    // and the files a person let go out of the read, the files gone from the
    // tree since a read, and the open questions about the dataset's held
    // files (the duplicate policy's defaults, 2026-10-10)
    let (mut left_out, mut gone, mut identity_questions) = (0, 0, 0);
    let mut refused_batch: Option<i64> = None;
    let mut certainty = crate::certainty::Certainty::default();
    let mut passes = 0;
    let (mut kinds, mut regions) = (Vec::new(), Vec::new());
    let mut last_digest: Option<(String, Option<String>)> = None;
    if !ids.is_empty() {
        let q = |t: &str| store.qualified(t);
        let (batch, stack, study, subject, file, cache, instance, series, class) = (
            q("ingest_batch"),
            q("stack"),
            q("study"),
            q("subject"),
            q("source_file"),
            q("session_cache_study"),
            q("instance"),
            q("series"),
            q("classification"),
        );
        // record 55 (2026-10-10): every scan its tree has a file of, whoever
        // read it first
        let in_tree = crate::operations::held_by(store, "x", &sources);
        let studies_in_tree = crate::operations::studies_held_by(store, "x", &sources);
        // as the sources door counts them, so the card and its detail agree
        subjects = count(
            store,
            &format!(
                "SELECT COUNT(DISTINCT se.subject_id) FROM {stack} x \
                 JOIN {series} se ON se.id = x.series_id JOIN {subject} su ON su.id = se.subject_id \
                 WHERE {in_tree} AND su.merged_into IS NULL"
            ),
        )
        .map_err(failed)?;
        studies = count(
            store,
            &format!("SELECT COUNT(*) FROM {study} x WHERE {studies_in_tree}"),
        )
        .map_err(failed)?;
        stacks = count(
            store,
            &format!("SELECT COUNT(*) FROM {stack} x WHERE {in_tree}"),
        )
        .map_err(failed)?;
        if let Some(window) = window {
            sessions = Some(
                count(
                    store,
                    &format!(
                        "SELECT COUNT(DISTINCT scs.session_id) FROM {cache} scs \
                         JOIN {study} x ON x.id = scs.study_id WHERE {studies_in_tree} \
                         AND scs.window_days = {window}"
                    ),
                )
                .map_err(failed)?,
            );
        }
        read = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) \
                 AND status IN ('ingested', 'duplicate')"
            ),
        )
        .map_err(failed)?;
        let same = nils_registry::review::SAME_INSTANCE_KIND;
        refused = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
                 AND (reason IS NULL OR reason NOT LIKE '{same}%')"
            ),
        )
        .map_err(failed)?;
        let drop = nils_registry::review::SAME_INSTANCE_DROP;
        left_out = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
                 AND reason = '{drop}'"
            ),
        )
        .map_err(failed)?;
        gone = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'gone'"
            ),
        )
        .map_err(failed)?;
        let item = store.qualified("review_item");
        let keys: Vec<String> = ids
            .iter()
            .map(|id| format!("ri.group_key LIKE 'source:{id}|%'"))
            .collect();
        identity_questions = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {item} ri WHERE ri.kind = '{same}' AND ri.status = 'open' AND ({})",
                keys.join(" OR ")
            ),
        )
        .map_err(failed)?;
        same_instance = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
                 AND reason = '{same}'"
            ),
        )
        .map_err(failed)?;
        new = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'ingested'"
            ),
        )
        .map_err(failed)?;
        let copies = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'duplicate'"
            ),
        )
        .map_err(failed)?;
        // one copy of each instance whose own file is no live file of this
        // dataset is known; every other copy is this dataset's second file
        known = count(
            store,
            &format!(
                "SELECT COUNT(DISTINCT f.instance_id) FROM {file} f \
                 JOIN {instance} i ON i.id = f.instance_id LEFT JOIN {file} o ON o.id = i.source_file_id \
                 WHERE f.source_id IN ({sources}) AND f.status = 'duplicate' \
                 AND (o.id IS NULL OR o.source_id NOT IN ({sources}) OR o.status NOT IN ('ingested', 'duplicate'))"
            ),
        )
        .map_err(failed)?;
        twice = (copies - known).max(0);
        refused_batch = store
            .query(
                &format!(
                    "SELECT MAX(batch_id) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined'"
                ),
                &[],
            )
            .map_err(failed)?
            .first()
            .map(|r| r.opt_int(0))
            .transpose()
            .map_err(failed)?
            .flatten();
        reads = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {batch} WHERE source_id IN ({sources}) \
                 AND (kind IS NULL OR kind = 'digest')"
            ),
        )
        .map_err(failed)?;
        last_digest = store
            .query(
                &format!(
                    "SELECT {}, {} FROM {batch} WHERE source_id IN ({sources}) \
                     AND (kind IS NULL OR kind = 'digest') ORDER BY id DESC LIMIT 1",
                    crate::text_of(store, "ingest_batch", "started_at"),
                    crate::text_of(store, "ingest_batch", "finished_at"),
                ),
                &[],
            )
            .map_err(failed)?
            .first()
            .map(|r| -> Result<_, StoreError> {
                Ok((
                    r.opt_text(0)?.unwrap_or_default().to_string(),
                    r.opt_text(1)?.map(str::to_string),
                ))
            })
            .transpose()
            .map_err(failed)?;
        classified = count(
            store,
            &format!("SELECT COUNT(*) FROM {stack} x JOIN {class} cl ON cl.stack_id = x.id WHERE {in_tree}"),
        )
        .map_err(failed)?;
        // record 56: how sure the sort is counts what the sort asks, never
        // a question about body part or post-contrast, which are steps of
        // their own; counted as the card and the sources door count it, so
        // the two agree, and a pass's questions beside it
        let asks = crate::certainty::Asks::sort(store).map_err(failed)?;
        certainty = crate::certainty::of_sources(store, &asks, &sources, stacks).map_err(failed)?;
        let holds = crate::operations::Scope::Sources(&sources).holds(store);
        passes = crate::certainty::passes(store, &holds).map_err(failed)?;
        let sealed = (!crate::sealed::reads(access)).then(|| store.qualified("sealed_stack"));
        kinds = by_axis(store, &sources, KIND_AXIS, sealed.as_deref()).map_err(failed)?;
        regions = by_axis(store, &sources, REGION_AXIS, sealed.as_deref()).map_err(failed)?;
    }

    // the main scans of its subjects, as the picks summary door says them
    let picks = crate::pick_after::summary(store, dataset, None).map_err(failed)?;
    let (picked, borders) = picks["roles"]
        .as_object()
        .map(|roles| {
            roles.values().fold((0, 0), |a, r| {
                (
                    a.0 + r["picked"].as_i64().unwrap_or(0),
                    a.1 + r["review_items"].as_i64().unwrap_or(0),
                )
            })
        })
        .unwrap_or((0, 0));

    // the pictures and the 3D views, under the working place where one is bound
    let working = crate::pyramid::working_place(store, None).ok();
    let (pictures, views) = match &working {
        Some(w) if stacks > 0 => {
            let all = crate::preview::dataset_stacks(store, dataset).map_err(failed)?;
            made(Path::new(&w.path), &all)
        }
        _ => (0, 0),
    };

    let mut steps = Vec::new();
    // found: the tree NILS reads, or the originals of an identified dataset
    let found_tree = if has_originals { "originals" } else { "anon" };
    let found_files = tree(found_tree, "files");
    let found_state = if state == "unknown" {
        "waiting"
    } else if found_files.unwrap_or(0) > 0 || reads > 0 {
        "done"
    } else {
        "waiting"
    };
    let mut found = json!({
        "step": "found",
        "state": found_state,
        "job": null,
        "started_at": null,
        "finished_at": dataset.probed_at.clone().unwrap_or_else(|| dataset.created_at.clone()),
        "progress": null,
        "files": found_files,
        "bytes": tree(found_tree, "bytes"),
    });
    found["tree"] = json!(found_tree);
    steps.push(found);
    if has_originals {
        let originals = tree("originals", "files").unwrap_or(0);
        let copied = tree("anon", "files").unwrap_or(0);
        let waiting = (originals - copied).max(0);
        let open = open_of(&["pseudonymize"]);
        steps.push(step(
            "pseudonymised",
            chain_state(
                open,
                newest_of(&["pseudonymize"]),
                copied > 0 && waiting == 0,
            ),
            open.or_else(|| newest_of(&["pseudonymize"])),
            json!({"files": copied, "waiting": waiting, "held": held}),
        ));
    }
    let open = open_of(&["digest", "ingest"]);
    let mut read_step = step(
        "read",
        chain_state(open, newest_of(&["digest", "ingest"]), reads > 0),
        open.or_else(|| newest_of(&["digest", "ingest"])),
        json!({
            "files": read, "new": new, "known": known, "twice": twice,
            "same_instance": same_instance, "left_out": left_out, "gone": gone,
            "refused": refused, "reads": reads,
        }),
    );
    if read_step["job"].is_null()
        && let Some((started, finished)) = &last_digest
    {
        read_step["started_at"] = json!(started);
        read_step["finished_at"] = json!(finished);
    }
    steps.push(read_step);
    let unsorted = certainty.unsorted;
    let open = open_of(&["fingerprint", "classify"]);
    let sort_failed = newest_of(&["fingerprint", "classify"]).filter(|j| j.state == "failed");
    steps.push(step(
        "sorted",
        chain_state(
            open,
            newest_of(&["fingerprint", "classify"]),
            stacks > 0 && unsorted == 0,
        ),
        open.or(sort_failed).or_else(|| newest_of(&["classify"])),
        json!({
            "scans": classified, "of": stacks, "look": certainty.to_sort,
            "passes": passes, "unsorted": unsorted,
        }),
    ));
    // body part and post-contrast, the operations of their own, over its
    // stacks
    steps.extend(
        crate::operations::steps(
            store,
            crate::operations::Scope::Sources(&sources),
            &format!("dataset:{}", dataset.name),
            stacks,
            &plans,
            // counted as the card counts them (record 48: never by kind); a
            // cohort's steps leave a sealed sample out
            false,
        )
        .map_err(failed)?,
    );
    let open = open_of(&["pick"]);
    let picks_off = !place::picks_after_sort(&dataset.dataset);
    let mut main = step(
        "main_scans",
        if picks_off && open.is_none() {
            "off"
        } else {
            chain_state(
                open,
                newest_of(&["pick"]),
                picked > 0 || !picks["last_run"].is_null(),
            )
        },
        open.or_else(|| newest_of(&["pick"])),
        json!({"picked": picked, "borders": borders}),
    );
    if main["job"].is_null() && !picks["last_run"].is_null() {
        main["job"] = picks["last_run"]["job"].clone();
        main["finished_at"] = picks["last_run"]["finished_at"].clone();
    }
    steps.push(main);
    // a sort makes the pictures of what it judged in the same run
    let open = open_of(&["classify", "preview"]);
    let sort = newest_of(&["classify", "preview"]);
    // the sort's row says done once it has judged, and its run goes on to
    // make the pictures, writing them into its result at the end: until
    // the result names them they are still being made, for a while
    let making = open.is_none()
        && working.is_some()
        && pictures < stacks
        && sort.is_some_and(|j| {
            j.kind == "classify"
                && j.state == "done"
                && j.finished_at
                    .as_deref()
                    .and_then(nils_registry::time::secs_of)
                    .is_some_and(|at| nils_registry::time::now_secs().saturating_sub(at) < MAKING)
                && nils_registry::job::show(store, j.id)
                    .ok()
                    .flatten()
                    .is_some_and(|row| {
                        row.result
                            .as_ref()
                            .is_none_or(|r| r.get("previews").is_none())
                    })
        });
    let mut picture_step = step(
        "pictures",
        if working.is_none() {
            "off"
        } else if making {
            "running"
        } else {
            state_of(open, pictures > 0)
        },
        open.or(sort),
        json!({"made": pictures, "of": stacks}),
    );
    if making {
        picture_step["progress"] = json!({"done": pictures, "total": stacks});
    }
    picture_step["in_sort"] = json!(open.or(sort).is_some_and(|j| j.kind == "classify"));
    steps.push(picture_step);
    let open = open_of(&["pyramid"]);
    steps.push(step(
        "views",
        if working.is_none() {
            "off"
        } else {
            state_of(open, views > 0)
        },
        open.or_else(|| newest_of(&["pyramid"])),
        json!({"made": views, "of": stacks}),
    ));

    let pairs = |v: Vec<(String, i64)>, key: &str| -> Vec<Value> {
        v.into_iter()
            .map(|(label, n)| {
                let mut m = serde_json::Map::new();
                m.insert(key.to_string(), json!(label));
                m.insert("scans".to_string(), json!(n));
                Value::Object(m)
            })
            .collect()
    };
    Ok(json!({
        "dataset": dataset.name,
        "dataset_id": dataset.id,
        "detail": access.detail.name(),
        "state": state,
        "added_at": dataset.created_at,
        "subjects": subjects,
        "sessions": sessions,
        "studies": studies,
        "scans": stacks,
        "sure": certainty.sure,
        "need_a_look": certainty.to_sort,
        "unsorted": unsorted,
        "look_kinds": certainty.need_a_look,
        "identity_questions": identity_questions,
        "kinds": pairs(kinds, "kind"),
        "body_regions": pairs(regions, "region"),
        "files": {
            "found": found_files,
            "bytes": tree(found_tree, "bytes"),
            "read": read,
            "new": new,
            "known": known,
            "twice": twice,
            "same_instance": same_instance,
            "left_out": left_out,
            "gone": gone,
            "refused": refused,
            "refused_batch": refused_batch,
            "held": held,
        },
        "pictures_place": working.map(|w| w.name),
        "steps": steps,
    }))
}

#[cfg(test)]
mod tests {
    use super::{Light, chain_state, flag_id, named_for, names};

    fn w(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_command_line_names_a_dataset_by_its_tree_its_flag_its_label_or_its_folder() {
        let folders = ["/srv/in/study".to_string()];
        let names = |line: &str, dataset: &str| names(&w(line), dataset, &folders);
        assert!(names("digest @study --name study-2026-10-09", "study"));
        assert!(names("pseudonymize @study/originals", "study"));
        assert!(names("pick run --dataset study", "study"));
        assert!(names("pick run --dataset=study", "study"));
        assert!(names("preview dataset:study", "study"));
        assert!(names("place originals study --purge --why x", "study"));
        // the jobs door locates @study before it queues a digest
        assert!(names(
            "digest /srv/in/study/derivatives/dcm-anon --name x",
            "study"
        ));
        assert!(names("digest /srv/in/study", "study"));
        assert!(!names(
            "digest /srv/in/study-big/derivatives/dcm-anon",
            "study"
        ));
        assert!(!names("digest @study-big", "study"));
        assert!(!names("pick run --dataset study-big", "study"));
        assert!(!names("classify --pack mri", "study"));
    }

    #[test]
    fn a_name_with_a_whole_day_is_the_dataset_s() {
        assert!(named_for(Some("study-2026-10-09"), "study"));
        assert!(named_for(Some("study"), "study"));
        assert!(!named_for(Some("study-2026-10-09"), "study-2026"));
        assert!(!named_for(Some("study-big-2026-10-09"), "study"));
        assert!(!named_for(Some("pictures after job 4"), "study"));
        assert!(!named_for(None, "study"));
    }

    #[test]
    fn a_flag_s_value_is_read_as_an_id() {
        assert_eq!(
            flag_id(
                &w("pyramid build --classified 42 --place w"),
                "--classified"
            ),
            Some(42)
        );
        assert_eq!(flag_id(&w("pick run --after-sort x"), "--after-sort"), None);
        assert_eq!(flag_id(&w("pick run"), "--after-sort"), None);
    }

    /// A job of a step as the summary reads it, in one state.
    fn light(id: i64, state: &str) -> Light {
        Light {
            id,
            kind: "pseudonymize".into(),
            name: None,
            state: state.into(),
            started_at: None,
            finished_at: None,
            args: serde_json::json!({}),
            progress: None,
        }
    }

    /// 2026-10-10: a pseudonymise run that failed at once left the step
    /// waiting, as if nothing had happened; the chain's step says failed.
    #[test]
    fn a_chain_step_whose_newest_job_failed_says_so() {
        let failed = light(154, "failed");
        let done = light(150, "done");
        let running = light(155, "running");
        assert_eq!(chain_state(None, Some(&failed), false), "failed");
        // a failure is said even where an earlier run had done the step
        assert_eq!(chain_state(None, Some(&failed), true), "failed");
        // a run of it going again is what the step is
        assert_eq!(
            chain_state(Some(&running), Some(&running), false),
            "running"
        );
        // the newest that ended well, or none, is done or waiting as before
        assert_eq!(chain_state(None, Some(&done), true), "done");
        assert_eq!(chain_state(None, Some(&done), false), "waiting");
        assert_eq!(chain_state(None, None, false), "waiting");
    }
}
