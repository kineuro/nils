// SPDX-License-Identifier: AGPL-3.0-only

//! Record 56, section 2 (2026-10-09): body part and post-contrast are their
//! own operations, each a step of a dataset and of a cohort with its own
//! review of what it is unsure of, and sorting asks nothing about either.
//! What the Data page reads of them:
//!
//! - **each operation's step** over a dataset's scans or a cohort's:
//!   whether it is served (a pipeline in the catalog proposes its axes and
//!   an admitted or promoted model answers them), the run over those scans
//!   that runs now or ran last, how many of them its model answered at or
//!   above its threshold, and how many wait on a person;
//! - **what the sort asks**, which is all a card's certainty line counts,
//!   never one about an axis an operation owns: the one definition of a
//!   look, and who asks every other question, is `certainty`'s.
//!
//! A model runs over a frozen selection or a handle and names its models
//! (`nils run <pipeline> --select selection:<name>@<v> --model ...`), so no
//! door starts one over a dataset or a cohort: a step says where it is and
//! nothing here starts it. Post-contrast's own label (record 55 F2 and F5,
//! the time rule as a session pass) is not in the engine yet; until a model
//! for its axis is served, its step is not available.

use std::collections::BTreeSet;

use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

/// The operations that are steps of their own, by the step's name, with
/// the axes their models answer: the body part's fine and coarse modes
/// (record 50), and post-contrast.
pub(crate) const OPERATIONS: [(&str, &[&str]); 2] = [
    ("body_part", &["body_part", "body_region"]),
    ("post_contrast", &["post_contrast"]),
];

/// How many of an operation's newest runs over the scans are looked
/// through for the one that runs and the jobs of its log.
const RUNS: usize = 50;

/// The operation of its own an axis belongs to, by its step's name; none
/// for an axis the sort answers.
pub(crate) fn owner(axis: &str) -> Option<&'static str> {
    OPERATIONS
        .iter()
        .find(|(_, axes)| axes.contains(&axis))
        .map(|(name, _)| *name)
}

/// The scans a step counts.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Scope<'a> {
    /// A dataset's: the stacks its sources (a comma list of `source` ids)
    /// created first; none where the list is empty.
    Sources(&'a str),
    /// A cohort's: the stacks of its open members.
    Cohort(i64),
}

impl Scope<'_> {
    /// The condition a stack aliased `x` meets to be in scope.
    pub(crate) fn holds(&self, store: &Store) -> String {
        match self {
            Scope::Sources(ids) if ids.trim().is_empty() => "1 = 0".to_string(),
            Scope::Sources(ids) => format!(
                "x.first_batch_id IN (SELECT b.id FROM {} b WHERE b.source_id IN ({ids}))",
                store.qualified("ingest_batch")
            ),
            Scope::Cohort(id) => format!(
                "x.series_id IN (SELECT se.id FROM {} se JOIN {} m ON m.subject_id = se.subject_id \
                 WHERE m.cohort_id = {id} AND m.left_at IS NULL)",
                store.qualified("series"),
                store.qualified("cohort_member"),
            ),
        }
    }
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn quoted(words: &[String]) -> String {
    words
        .iter()
        .map(|w| format!("'{}'", w.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn count(store: &mut Store, sql: &str) -> Result<i64, StoreError> {
    store.query(sql, &[])?[0].int(0)
}

/// One run of an operation over the scans, as its step reads it.
struct Ran {
    id: i64,
    job: Option<i64>,
    status: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

/// The pipelines in the catalog whose proposals answer one of the axes, of
/// any state, and whether one of them is active.
fn proposing(store: &mut Store, axes: &[&str]) -> Result<(Vec<i64>, bool), StoreError> {
    let all =
        nils_registry::pipeline::list(store).map_err(|e| StoreError::Message(e.to_string()))?;
    let mut ids = Vec::new();
    let mut active = false;
    for p in all {
        let proposes = p.descriptor["x-nils"]["proposals"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|x| x["axis"].as_str().is_some_and(|a| axes.contains(&a)));
        if proposes {
            active |= p.state == "active";
            ids.push(p.id);
        }
    }
    Ok((ids, active))
}

/// The newest runs of the pipelines over a stack in scope, newest first.
fn runs_over(
    store: &mut Store,
    pipelines: &[i64],
    scope: Scope<'_>,
) -> Result<Vec<Ran>, StoreError> {
    if pipelines.is_empty() {
        return Ok(Vec::new());
    }
    let holds = scope.holds(store);
    let sql = format!(
        "SELECT r.id, r.job_id, r.status, {}, {} FROM {} r \
         WHERE r.pipeline_id IN ({}) AND r.handle_id IS NOT NULL \
         AND EXISTS (SELECT 1 FROM {} hm JOIN {} x ON x.id = hm.key \
         WHERE hm.handle_id = r.handle_id AND {holds}) ORDER BY r.id DESC LIMIT {RUNS}",
        crate::text_of(store, "pipeline_run", "started_at"),
        crate::text_of(store, "pipeline_run", "finished_at"),
        store.qualified("pipeline_run"),
        list(pipelines),
        store.qualified("handle_member"),
        store.qualified("stack"),
    );
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| {
            Ok(Ran {
                id: r.int(0)?,
                job: r.opt_int(1)?,
                status: r.text(2)?.to_string(),
                started_at: r.opt_text(3)?.map(str::to_string),
                finished_at: r.opt_text(4)?.map(str::to_string),
            })
        })
        .collect()
}

/// The jobs of the operations' runs over a dataset's scans, for its log.
pub(crate) fn run_jobs(store: &mut Store, scope: Scope<'_>) -> Result<BTreeSet<i64>, StoreError> {
    let mut out = BTreeSet::new();
    for (_, axes) in OPERATIONS {
        let (pipelines, _) = proposing(store, axes)?;
        for r in runs_over(store, &pipelines, scope)? {
            out.extend(r.job);
        }
    }
    Ok(out)
}

/// Each operation's step over the scans in scope, `scans` of them: its
/// state (running or queued while a run over them goes on, done once one
/// finished or its model answered one of them, waiting while it is served
/// and has not run, off where nothing serves it), the run with its job and
/// times, the units over while it runs, how many scans its model answered
/// at or above its threshold, how many wait on a person, and the jobs of
/// its runs over them, newest first.
pub(crate) fn steps(
    store: &mut Store,
    scope: Scope<'_>,
    scans: i64,
) -> Result<Vec<Value>, StoreError> {
    let holds = scope.holds(store);
    let (member, item, stack, model, unit) = (
        store.qualified("review_member"),
        store.qualified("review_item"),
        store.qualified("stack"),
        store.qualified("model"),
        store.qualified("pipeline_unit"),
    );
    let mut out = Vec::new();
    for (name, axes) in OPERATIONS {
        let (pipelines, active) = proposing(store, axes)?;
        let tasks: Vec<String> = axes.iter().map(|a| format!("axis:{a}")).collect();
        let models = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {model} WHERE task IN ({}) AND state IN ('admitted', 'promoted')",
                quoted(&tasks)
            ),
        )?;
        let served = active && models > 0;
        let groups: Vec<String> = axes.iter().map(|a| format!("{a}:model")).collect();
        // the scans its model answered at or above its threshold: staged,
        // or put in force since
        let answered = count(
            store,
            &format!(
                "SELECT COUNT(DISTINCT rm.stack_id) FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
                 JOIN {stack} x ON x.id = rm.stack_id WHERE ri.kind IN ({}) \
                 AND ri.status IN ('staged', 'accepted') AND {holds}",
                quoted(&groups)
            ),
        )?;
        // the scans a question about its axes waits on a person for: what
        // its model was unsure of, a pass's about them, and what a sort
        // asked before record 56, by who asks it (`certainty`)
        let look = crate::certainty::Asks::of(store, "'open'", &|a| {
            a == crate::certainty::Asker::Operation(name)
        })?
        .count(store, &holds)?
        .scans;
        let runs = runs_over(store, &pipelines, scope)?;
        let going = runs
            .iter()
            .find(|r| matches!(r.status.as_str(), "running" | "interrupted"));
        let last = going.or(runs.first());
        let finished = runs
            .iter()
            .any(|r| matches!(r.status.as_str(), "done" | "partial"));
        let state = match going.map(|r| r.status.as_str()) {
            Some("running") => "running",
            // the engine that ran it went away, and the lane takes it up
            Some(_) => "queued",
            None if finished || answered > 0 => "done",
            None if served => "waiting",
            None => "off",
        };
        let progress = match going {
            Some(r) => {
                let rows = store.query(
                    &format!(
                        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN state = 'over' THEN 1 ELSE 0 END), 0) \
                         FROM {unit} WHERE run_id = {}",
                        r.id
                    ),
                    &[],
                )?;
                let (total, over) = (rows[0].int(0)?, rows[0].int(1)?);
                (total > 0).then(|| json!({"done": over, "total": total}))
            }
            None => None,
        };
        out.push(json!({
            "step": name,
            "state": state,
            "job": last.and_then(|r| r.job),
            "run": last.map(|r| r.id),
            "started_at": last.and_then(|r| r.started_at.clone()),
            "finished_at": last.and_then(|r| r.finished_at.clone()),
            "progress": progress,
            "served": served,
            "answered": answered,
            "look": look,
            "of": scans,
            "jobs": runs.iter().filter_map(|r| r.job).collect::<Vec<_>>(),
        }));
    }
    Ok(out)
}

/// A cohort's steps, as its document carries them: its members' scans
/// sorted, with what the sort asks of them, then each operation's step
/// over them. The main scans of its members are the picks summary's.
pub(crate) fn of_cohort(store: &mut Store, cohort: i64) -> Result<Vec<Value>, StoreError> {
    let scope = Scope::Cohort(cohort);
    let holds = scope.holds(store);
    let (stack, class) = (store.qualified("stack"), store.qualified("classification"));
    let scans = count(
        store,
        &format!("SELECT COUNT(*) FROM {stack} x WHERE {holds}"),
    )?;
    let sorted = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {stack} x WHERE {holds} \
             AND EXISTS (SELECT 1 FROM {class} cl WHERE cl.stack_id = x.id)"
        ),
    )?;
    // a look as the card counts one, and a pass's questions beside it
    let look = crate::certainty::Asks::sort(store)?
        .count(store, &holds)?
        .scans;
    let passes = crate::certainty::passes(store, &holds)?;
    let mut out = vec![json!({
        "step": "sorted",
        "state": if scans > 0 && sorted >= scans { "done" } else { "waiting" },
        "job": null,
        "started_at": null,
        "finished_at": null,
        "progress": null,
        "scans": sorted,
        "of": scans,
        "look": look,
        "passes": passes,
        "unsorted": (scans - sorted).max(0),
    })];
    out.extend(steps(store, scope, scans)?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::owner;

    #[test]
    fn an_operation_owns_its_axes() {
        assert_eq!(owner("body_part"), Some("body_part"));
        assert_eq!(owner("body_region"), Some("body_part"));
        assert_eq!(owner("post_contrast"), Some("post_contrast"));
        assert_eq!(owner("technique"), None);
    }
}
