// SPDX-License-Identifier: AGPL-3.0-only

//! The re-sort after a commit as a queued job (UX round 5, 2026-11-08).
//!
//! A committed decision is in force, and every page reads the axis the sort
//! writes, so the scans a commit reaches are sorted again
//! (`review_questions::resort_committed`). Until this round the commit doors
//! ran that sort inside the request and answered at its end: three minutes
//! for 50,006 scans, eight for 450,866, with the desk's confirm open and its
//! button live the whole time. Now the doors and `nils review commit` write
//! the decisions, queue the re-sort as one job named as before (`re-sort
//! after <who> committed N decision(s)`) and answer at once with
//! `resort: {state: "queued", job, stacks}`; the job's progress (`done` of
//! `of` scans) is read through the jobs doors as every job's is, and the
//! command line follows it unless `--no-wait`.
//!
//! The job is `nils review resort --decisions IDS`, of kind `resort`, run
//! by the main lane's worker. It adopts its queued row, then disowns the
//! adoption so that each sort it runs claims a `classify` row of its own
//! and the rule that one sort runs at a time stays as it is; it sorts the
//! stacks in chunks, beats its progress after each, stops at a cancel asked
//! through the jobs door, and waits up to [`JOB_WAIT`] for a sort that
//! holds the registry (the inline re-sort waited three seconds). A
//! campaign's close, which runs as a job already, keeps the inline re-sort.

use std::time::Duration;

use nils_registry::Registry;
use nils_registry::job::{self, Asked, Claim, State};
use serde_json::{Value, json};

use crate::review_questions::{resort_stacks_with, stacks_reached};
use crate::{Exit, fail};

/// How many stacks go in one sort of the job: its progress moves at least
/// this often.
const CHUNK: usize = 1_000;

/// How long the job waits for a sort that holds the registry.
const JOB_WAIT: Duration = Duration::from_secs(600);

/// How often the command line reads the job it follows.
const FOLLOW_EVERY: Duration = Duration::from_millis(500);

/// After this long still queued, the command line says a worker runs it.
const SAY_WORKER_AFTER: Duration = Duration::from_secs(10);

/// The job's name, as the inline re-sort named its sort.
pub(crate) fn name_of(who: &str, decisions: usize) -> String {
    format!("re-sort after {who} committed {decisions} decision(s)")
}

/// Queue the re-sort of the scans these committed decisions reach, under
/// `who`, and say so at once: `{state: "queued", job, stacks, unreached,
/// words}`; `{state: "done", job: null, stacks: 0, words}` where nothing
/// is to be sorted, and `{state: "failed", ...}` where the queue refused.
pub(crate) fn queue_resort_committed(
    registry: &mut Registry,
    pack_dir: Option<&std::path::Path>,
    who: &str,
    decisions: &[i64],
) -> Value {
    if decisions.is_empty() {
        return json!({"state": "done", "job": null, "stacks": 0, "sorted": 0, "jobs": [], "words": "nothing committed"});
    }
    let (stacks, unreached) = match stacks_reached(registry.store(), decisions) {
        Ok(found) => found,
        Err(words) => {
            return json!({"state": "failed", "job": null, "stacks": 0, "sorted": 0, "jobs": [], "words": words});
        }
    };
    if stacks.is_empty() {
        return json!({
            "state": "done", "job": null, "stacks": 0, "sorted": 0, "jobs": [], "unreached": unreached,
            "words": "no scan to re-sort",
        });
    }
    let mut argv = vec![
        "review".to_string(),
        "resort".to_string(),
        "--decisions".to_string(),
        decisions
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(","),
        "--who".to_string(),
        who.to_string(),
    ];
    if let Some(dir) = pack_dir {
        argv.push("--pack-dir".to_string());
        argv.push(dir.display().to_string());
    }
    let name = name_of(who, decisions.len());
    let queued = job::enqueue(registry.store(), &argv, Some(&name), Some(who));
    if queued.is_ok() {
        crate::worker::wake();
    }
    match queued {
        Ok(id) => json!({
            "state": "queued", "job": id, "stacks": stacks.len(), "sorted": 0, "jobs": [],
            "unreached": unreached,
            "words": format!("{} scan(s) to sort again, queued as job {id}", stacks.len()),
        }),
        Err(e) => json!({
            "state": "failed", "job": null, "stacks": stacks.len(), "sorted": 0, "jobs": [],
            "unreached": unreached,
            "words": format!("the re-sort could not be queued: {e}; the decisions stand, and the next sort re-sorts them"),
        }),
    }
}

/// `nils review resort`: the job's work. Claims the row the worker handed
/// it (or makes one at the keyboard), sorts the stacks the decisions reach
/// in chunks with its progress on the row, and ends the row as the sort
/// went: done, cancelled, or failed with why.
pub(crate) fn run_verb(
    registry: &mut Registry,
    pack_dir: Option<&std::path::Path>,
    who: &str,
    decisions: &[i64],
) -> Result<(), Exit> {
    let (stacks, unreached) = stacks_reached(registry.store(), decisions).map_err(fail)?;
    let name = name_of(who, decisions.len());
    let job_id = job::claim(
        registry.store(),
        &Claim {
            kind: job::RESORT,
            name: &name,
            args: json!({"decisions": decisions.len(), "stacks": stacks.len(), "who": who}),
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    // the sorts below claim rows of their own, never this one
    job::disown_adoption();
    let of = stacks.len();
    let done = if stacks.is_empty() {
        json!({
            "state": "done", "stacks": 0, "sorted": 0, "jobs": [], "unreached": unreached,
            "words": "no scan to re-sort",
        })
    } else {
        let _ = job::beat(
            registry.store(),
            job_id,
            Some(&json!({"done": 0, "of": of, "what": "scans"})),
        );
        // the heartbeat with how far, and whether a cancel was asked
        let mut progress = |reg: &mut Registry, sorted: usize, of: usize| -> bool {
            let beat = job::beat(
                reg.store(),
                job_id,
                Some(&json!({"done": sorted, "of": of, "what": "scans"})),
            );
            !matches!(beat, Ok(Asked::Cancel))
        };
        let mut resort = resort_stacks_with(
            registry,
            pack_dir,
            &name,
            "committed decisions",
            &stacks,
            JOB_WAIT,
            CHUNK,
            &mut progress,
        );
        resort["unreached"] = json!(unreached);
        resort
    };
    let state = done["state"].as_str().unwrap_or("failed");
    let words = done["words"].as_str().unwrap_or("").to_string();
    let _ = job::set_result(registry.store(), job_id, &done);
    let _ = job::beat(
        registry.store(),
        job_id,
        Some(&json!({
            "done": done["sorted"].as_u64().unwrap_or(0), "of": of, "what": "scans",
        })),
    );
    match state {
        "done" => {
            job::finish(registry.store(), job_id, State::Done, None)
                .map_err(|e| fail(e.to_string()))?;
            println!("re-sort: {words}");
            Ok(())
        }
        "cancelled" => {
            job::finish(registry.store(), job_id, State::Cancelled, None)
                .map_err(|e| fail(e.to_string()))?;
            println!("re-sort stopped: {words}");
            Ok(())
        }
        _ => {
            job::finish(registry.store(), job_id, State::Failed, Some(&words))
                .map_err(|e| fail(e.to_string()))?;
            Err(fail(format!("re-sort: {words}")))
        }
    }
}

/// Whether a worker of the main lane (or of every lane) runs for this
/// registry now, to take the queued re-sort.
fn main_worker_live(registry: &mut Registry) -> bool {
    job::list(registry.store(), false, 1000)
        .ok()
        .is_some_and(|jobs| {
            jobs.iter().any(|w| {
                w.kind == "worker"
                    && (w.args["lane"] == "main"
                        || w.args["lane"] == "all"
                        || w.args["lane"].is_null())
                    && job::is_live(w)
            })
        })
}

/// Follow a queued re-sort from the keyboard until it is over: a line of
/// progress as it moves, and its words at the end; a failed one fails the
/// command. Where no worker runs for the registry the job stays queued for
/// the next one, and the command says so and returns rather than wait for
/// nobody.
pub(crate) fn follow(registry: &mut Registry, job_id: i64) -> Result<(), Exit> {
    let started = std::time::Instant::now();
    let mut said_worker = false;
    let mut last = String::new();
    let waits_for_nobody = |registry: &mut Registry| -> Result<(), Exit> {
        println!(
            "re-sort job {job_id} waits for a worker (nils jobs work); it runs when one does, and the decisions stand meanwhile"
        );
        let _ = registry;
        Ok(())
    };
    if !main_worker_live(registry) {
        return waits_for_nobody(registry);
    }
    loop {
        let Some(j) = job::show(registry.store(), job_id).map_err(|e| fail(e.to_string()))? else {
            return Err(fail(format!("job {job_id} is gone")));
        };
        if let Some(p) = &j.progress
            && let (Some(done), Some(of)) = (p["done"].as_u64(), p["of"].as_u64())
        {
            let line = format!("  {done} of {of} sorted again");
            if line != last {
                eprintln!("{line}");
                last = line;
            }
        }
        if j.state.is_over() {
            let words = j
                .result
                .as_ref()
                .and_then(|r| r["words"].as_str().map(str::to_string));
            return match j.state {
                State::Done => {
                    println!("re-sort: {}", words.unwrap_or_else(|| "done".into()));
                    Ok(())
                }
                State::Cancelled => {
                    println!(
                        "re-sort stopped: {}",
                        words.unwrap_or_else(|| "cancelled".into())
                    );
                    Ok(())
                }
                _ => Err(fail(format!(
                    "re-sort job {job_id} failed: {}",
                    j.error.clone().or(words).unwrap_or_default()
                ))),
            };
        }
        if j.state == State::Queued && !said_worker && started.elapsed() > SAY_WORKER_AFTER {
            if !main_worker_live(registry) {
                return waits_for_nobody(registry);
            }
            eprintln!(
                "re-sort job {job_id} is queued behind the worker's other jobs; --no-wait leaves it"
            );
            said_worker = true;
        }
        std::thread::sleep(FOLLOW_EVERY);
    }
}
