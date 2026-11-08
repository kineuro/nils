// SPDX-License-Identifier: AGPL-3.0-only

//! UX round 3 (2026-11-08): a job goes again through the queue, never in
//! the caller's foreground.
//!
//! `nils jobs resume <id>` and `POST /api/jobs/{id}/resume` put a job that
//! is over back on the queue, from the command line it recorded, and the
//! worker in the engine's service takes it in its turn, as `nils jobs
//! enqueue` does: a pipeline run that can go on becomes `run --resume
//! <run>`, so the units it finished are kept. A pipeline run started at
//! the keyboard beside an engine whose worker runs pipelines is refused
//! before anything starts, with the `nils jobs enqueue` line for the same
//! words: only the engine's own worker has the session (the user's
//! session bus, a readable working folder) its containers need.

use nils_registry::job::{self, Job, State};
use nils_registry::store::Store;
use serde_json::{Value, json};

/// Why a job cannot go again.
#[derive(Debug, PartialEq)]
pub(crate) enum Refused {
    NoJob(i64),
    NotOver(i64, State),
    NoCommand(i64),
    /// Another job, queued or running, already runs the same words.
    Taken {
        job: i64,
        by: i64,
        state: State,
    },
    Registry(String),
}

impl Refused {
    /// The words a person reads.
    pub(crate) fn words(&self) -> String {
        match self {
            Refused::NoJob(id) => format!("no job {id}"),
            Refused::NotOver(id, state) => format!(
                "job {id} is {}; a job goes again once it is over (nils jobs cancel {id} stops it)",
                state.name()
            ),
            Refused::NoCommand(id) => format!(
                "job {id} recorded no command line; it was made before this binary or by a door, \
                 so queue its verb with nils jobs enqueue"
            ),
            Refused::Taken { job, by, state } => format!(
                "job {job} is already going again as job {by} ({}); nils jobs show {by} follows it",
                state.name()
            ),
            Refused::Registry(e) => e.clone(),
        }
    }

    /// The status a door answers with.
    pub(crate) fn status(&self) -> u16 {
        match self {
            Refused::NoJob(_) => 404,
            Refused::NotOver(..) | Refused::Taken { .. } | Refused::NoCommand(_) => 409,
            Refused::Registry(_) => 500,
        }
    }
}

/// The words a job goes again with: the command line it recorded, after
/// `nils` and its registry; a pipeline run that did not finish goes on
/// where it stopped (record 49 A1), with the units it finished kept,
/// rather than starting a run of its own.
pub(crate) fn words(store: &mut Store, j: &Job) -> Result<Vec<String>, Refused> {
    let mut words = j
        .queued()
        .filter(|w| !w.is_empty())
        .ok_or(Refused::NoCommand(j.id))?;
    if j.kind == "pipeline"
        && let Some(run) = j.args["run"].as_i64()
        && nils_registry::pipeline::run(store, run)
            .map_err(|e| Refused::Registry(e.to_string()))?
            .is_some_and(|r| {
                matches!(r.status.as_str(), "running" | "cancelled" | "interrupted")
                    || (r.status == "failed" && r.results_digest.is_none())
            })
        && let Some(at) = words.iter().position(|w| w == "run")
    {
        words.truncate(at + 1);
        words.extend(["--resume".to_string(), run.to_string()]);
    }
    Ok(words)
}

/// What the queue carries of the job that went before: who asked and the
/// reach they recorded (the principal, the detail or roles, the grants,
/// the actor, the projection flag), and nothing a verb wrote of itself.
pub(crate) fn carried(j: &Job) -> Value {
    let mut extra = json!({});
    for key in [
        "principal",
        "detail",
        "roles",
        "grants",
        "actor",
        "may_project_raw",
    ] {
        if let Some(v) = j.args.get(key).filter(|v| !v.is_null()) {
            extra[key] = v.clone();
        }
    }
    extra
}

/// What went on the queue.
#[derive(Debug)]
pub(crate) struct Requeued {
    pub(crate) job: i64,
    pub(crate) words: Vec<String>,
    /// The worker that will take it, where one runs pipelines for this
    /// registry now; none where the job waits for the engine.
    pub(crate) worker: Option<i64>,
}

/// Put job `id` back on the queue. `reach` is laid over what the job
/// recorded (a door's caller, or the keyboard's reach where the job
/// recorded none); `principal` is who is recorded as asking where the job
/// recorded nobody.
pub(crate) fn requeue(
    store: &mut Store,
    id: i64,
    reach: impl FnOnce(&Job, Value) -> Value,
    principal: &str,
) -> Result<Requeued, Refused> {
    let reg = |e: job::Error| Refused::Registry(e.to_string());
    let j = job::show(store, id)
        .map_err(reg)?
        .ok_or(Refused::NoJob(id))?;
    if !j.state.is_over() {
        return Err(Refused::NotOver(id, j.state));
    }
    let words = words(store, &j)?;
    // the same words queued or running already: one goes, not two
    for other in job::list(store, false, 1000).map_err(reg)? {
        if other.id != id
            && !job::is_worker(&other.kind)
            && other.queued().as_deref() == Some(words.as_slice())
        {
            return Err(Refused::Taken {
                job: id,
                by: other.id,
                state: other.state,
            });
        }
    }
    let mut extra = reach(&j, carried(&j));
    extra["again_of"] = json!(id);
    let who = extra["principal"]
        .as_str()
        .map(str::to_string)
        .or_else(|| j.principal().map(str::to_string))
        .unwrap_or_else(|| principal.to_string());
    let job =
        job::enqueue_with(store, &words, j.name.as_deref(), Some(&who), extra).map_err(reg)?;
    crate::worker::wake();
    Ok(Requeued {
        job,
        words,
        worker: pipeline_worker(store).map(|w| w.id),
    })
}

/// The worker that runs pipelines for this registry now, where one is
/// live: the pipeline lane's, or a worker of every lane.
pub(crate) fn pipeline_worker(store: &mut Store) -> Option<Job> {
    job::list(store, false, 1000).ok()?.into_iter().find(|w| {
        (w.kind == job::PIPELINE_WORKER || (w.kind == "worker" && w.args["lane"] == "all"))
            && job::is_live(w)
    })
}

/// UX round 3: whether this process is a job a worker runs (its own verb,
/// or a step a workflow run started in it), which is never refused.
pub(crate) fn run_by_a_worker() -> bool {
    std::env::var(job::ADOPT_VAR).is_ok_and(|v| !v.trim().is_empty())
}

/// The `nils jobs enqueue` line for a pipeline run typed at the keyboard:
/// the same words, `--here` left out, named for what it runs.
pub(crate) fn enqueue_line(own: &[String]) -> String {
    let words: Vec<String> = job::words_of(own)
        .into_iter()
        .filter(|w| w != "--here")
        .collect();
    // a run taken up again is named for its run, else for its pipeline,
    // which `nils run` takes as its first word
    let name = words
        .iter()
        .position(|w| w == "--resume")
        .and_then(|i| words.get(i + 1))
        .map(|r| format!("run-{r}"))
        .or_else(|| words.get(1).filter(|w| !w.starts_with('-')).cloned())
        .unwrap_or_else(|| "run".to_string());
    format!(
        "nils jobs enqueue --name {} -- {}",
        quoted(&name),
        words
            .iter()
            .map(|w| quoted(w))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

/// A word as a shell reads it back: quoted where it holds anything but
/// plain characters.
fn quoted(w: &str) -> String {
    if !w.is_empty()
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@=,+%".contains(c))
    {
        w.to_string()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
}

/// The refusal of a pipeline run typed at the keyboard while `worker` runs
/// pipelines for this registry.
pub(crate) fn foreground_refusal(worker: &Job, own: &[String]) -> String {
    format!(
        "the engine is running (its pipeline worker is job {}{}), and a run started here would run \
         outside its session, where apptainer may find no readable working folder and no user \
         session bus for the run's limits; queue it so the engine runs it: {} \
         (or add --here to run it in this shell all the same)",
        worker.id,
        worker
            .host
            .as_deref()
            .map(|h| format!(" on {h}"))
            .unwrap_or_default(),
        enqueue_line(own)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn the_enqueue_line_is_the_same_run_without_here() {
        let line = enqueue_line(&own(&[
            "/usr/local/bin/nils",
            "--registry",
            "/srv/reg",
            "run",
            "postcontrast-full@2",
            "--handle",
            "249",
            "--model",
            "8",
            "--here",
            "--param",
            "note=a b",
        ]));
        assert_eq!(
            line,
            "nils jobs enqueue --name postcontrast-full@2 -- run postcontrast-full@2 --handle 249 --model 8 --param 'note=a b'"
        );
        let line = enqueue_line(&own(&["nils", "run", "--resume", "191"]));
        assert_eq!(line, "nils jobs enqueue --name run-191 -- run --resume 191");
    }

    #[test]
    fn what_goes_again_is_who_asked_and_their_reach_alone() {
        let j = Job {
            id: 7,
            kind: "pipeline".into(),
            name: Some("n4".into()),
            state: State::Cancelled,
            pid: Some(1),
            host: None,
            started_at: "2026-11-08T00:00:00Z".into(),
            heartbeat_at: None,
            finished_at: None,
            progress: None,
            error: None,
            args: json!({
                "argv": ["nils", "run", "n4"], "queued": ["run", "n4"], "run": 3,
                "principal": "anna", "detail": "quasi", "grants": ["pipelines:work"],
                "then": [["digest"]], "chain_after": 9,
            }),
            result: None,
        };
        assert_eq!(
            carried(&j),
            json!({"principal": "anna", "detail": "quasi", "grants": ["pipelines:work"]})
        );
    }
}
