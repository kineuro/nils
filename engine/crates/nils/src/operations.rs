// SPDX-License-Identifier: AGPL-3.0-only

//! Record 56, section 2 (2026-10-09): body part and post-contrast are their
//! own operations, each a step of a dataset and of a cohort with its own
//! review of what it is unsure of, and sorting asks nothing about either.
//! What the Data page reads of them:
//!
//! - **each operation's step** over a dataset's scans or a cohort's:
//!   whether it is served (a pipeline in the catalog proposes its axes and
//!   an admitted or promoted model answers them), the run over those scans
//!   that is queued, runs now or ran last, how many of them its model
//!   answered at or above its threshold, how many wait on a person, and why
//!   a run of it would be refused now;
//! - **what the sort asks**, which is all a card's certainty line counts,
//!   never one about an axis an operation owns: the one definition of a
//!   look, and who asks every other question, is `certainty`'s.
//!
//! A model runs over a frozen selection or a handle and names its models
//! (`nils run <pipeline> --select selection:<name>@<v> --model ...`). Since
//! 2026-10-10 a door starts an operation's run over a dataset's or a
//! cohort's scans: `POST /api/datasets/{name}/steps/{step}/run` and its
//! cohort twin freeze the scans the step counts into a handle and queue
//! `run <pipeline> --handle <id> --model ...` for them, a job of kind
//! `pipeline` that names its step and what it is for (`step`, `for`), which
//! the pipeline lane runs, so a run of hours never holds up a digest. The
//! step follows the job and then its run: queued, running, then done or
//! failed. Body part is the certified model of record 50, the pipeline
//! `bodypart-infer-fusion` given its registered parts (the encoder the head
//! names, the head that answers `body_part`, and the coarse mode that
//! answers `body_region` from the same encoder where one is served).
//! Post-contrast's own label (record 55 F2 and F5, the time rule as a
//! session pass) is not in the engine yet, and no model answers it (E3):
//! its run is refused, and until a model for its axis is served its step
//! is not available.

use std::collections::BTreeSet;

use nils_registry::Registry;
use nils_registry::model::{self, Model};
use nils_registry::store::{Cell, Error as StoreError, Row, Store};
use serde_json::{Value, json};

use crate::serve::{Caller, Doors, Reply};

/// The operations that are steps of their own, by the step's name, with
/// the axes their models answer: the body part's fine and coarse modes
/// (record 50), and post-contrast.
pub(crate) const OPERATIONS: [(&str, &[&str]); 2] = [
    ("body_part", &["body_part", "body_region"]),
    ("post_contrast", &["post_contrast"]),
];

/// The pipeline body part's step runs: the certified model of record 50,
/// one frozen model with two modes, given its registered parts.
pub(crate) const BODY_PART_PIPELINE: &str = "bodypart-infer-fusion";

/// How many of an operation's newest runs over the scans are looked
/// through for the one that runs and the jobs of its log.
const RUNS: usize = 50;

/// How many of the newest pipeline jobs are looked through for the ones a
/// door queued for a step.
const ASKED: usize = 200;

/// The operation of its own an axis belongs to, by its step's name; none
/// for an axis the sort answers.
pub(crate) fn owner(axis: &str) -> Option<&'static str> {
    OPERATIONS
        .iter()
        .find(|(_, axes)| axes.contains(&axis))
        .map(|(name, _)| *name)
}

/// An operation's step as a person reads it, in a sentence.
fn title(step: &str) -> &str {
    match step {
        "body_part" => "body part",
        "post_contrast" => "post-contrast",
        other => other,
    }
}

/// The scans a step counts.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Scope<'a> {
    /// A dataset's: the stacks its sources (a comma list of `source` ids)
    /// hold a file of, whoever read them first (record 55, 2026-10-10);
    /// none where the list is empty.
    Sources(&'a str),
    /// A cohort's: the stacks of its open members.
    Cohort(i64),
}

/// Record 55 (Nima's duplicate policy, 2026-10-10): the condition a stack
/// aliased `alias` meets when one of the sources (a comma list of `source`
/// ids) holds a file of it, its instance's own or a copy, whoever read the
/// stack first. Every door that counts or lists a dataset's scans asks it,
/// so a dataset holds what its tree holds.
pub(crate) fn held_by(store: &Store, alias: &str, sources: &str) -> String {
    if sources.trim().is_empty() {
        return "1 = 0".to_string();
    }
    format!(
        "{alias}.id IN (SELECT ss.stack_id FROM {} ss WHERE ss.source_id IN ({sources}))",
        store.qualified("source_stack")
    )
}

/// The condition a study aliased `alias` meets when one of the sources
/// holds a file of a stack of it ([`held_by`]).
pub(crate) fn studies_held_by(store: &Store, alias: &str, sources: &str) -> String {
    if sources.trim().is_empty() {
        return "1 = 0".to_string();
    }
    format!(
        "{alias}.id IN (SELECT hse.study_id FROM {} hse JOIN {} hst ON hst.series_id = hse.id \
         JOIN {} hss ON hss.stack_id = hst.id WHERE hss.source_id IN ({sources}))",
        store.qualified("series"),
        store.qualified("stack"),
        store.qualified("source_stack")
    )
}

impl Scope<'_> {
    /// The condition a stack aliased `x` meets to be in scope.
    pub(crate) fn holds(&self, store: &Store) -> String {
        match self {
            Scope::Sources(ids) => held_by(store, "x", ids),
            Scope::Cohort(id) => format!(
                "x.series_id IN (SELECT se.id FROM {} se JOIN {} m ON m.subject_id = se.subject_id \
                 WHERE m.cohort_id = {id} AND m.left_at IS NULL)",
                store.qualified("series"),
                store.qualified("cohort_member"),
            ),
        }
    }
}

/// A scope's condition with a sample sealed now left out where the caller
/// does not read sealed stacks (record 48): a step counts neither those
/// scans nor its model's answers on them for such a caller (the review of
/// Wave 7a's merge, 2026-10-10). A freeze for a run keeps them, and never
/// asks this.
fn shown(store: &Store, holds: String, hide_sealed: bool) -> String {
    if hide_sealed {
        format!(
            "({holds}) AND NOT EXISTS (SELECT 1 FROM {} sst WHERE sst.stack_id = x.id AND sst.unsealed_at IS NULL)",
            store.qualified("sealed_stack")
        )
    } else {
        holds
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

/// Why a step's run is refused: the status a door answers it with, a short
/// reason a client tells refusals apart by, and the words a person reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Refused {
    pub(crate) status: u16,
    pub(crate) reason: &'static str,
    pub(crate) error: String,
}

impl Refused {
    fn new(status: u16, reason: &'static str, error: impl Into<String>) -> Refused {
        Refused {
            status,
            reason,
            error: error.into(),
        }
    }

    /// As a step of a summary carries it.
    fn doc(&self) -> Value {
        json!({"reason": self.reason, "error": self.error})
    }

    /// As the door answers it: the words, the reason and the step.
    fn reply(&self, step: &str) -> Reply {
        let mut r = Reply::error(self.status, self.error.clone());
        r.body["reason"] = json!(self.reason);
        r.body["step"] = json!(step);
        r
    }
}

/// What an operation's run is here: the pipeline by its label, and the
/// registered models it reads, by id, in the order its descriptor declares
/// its model inputs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    pub(crate) pipeline: String,
    pub(crate) models: Vec<i64>,
}

/// Each operation's run as it would be here, by its step's name.
pub(crate) type Plans = Vec<(&'static str, Result<Plan, Refused>)>;

/// The models served for a task, the one a run takes first: admitted or
/// promoted, the promoted ones first, then the newest; with `reads`, only
/// those that read that encoder.
fn served(store: &mut Store, task: &str, reads: Option<i64>) -> Result<Vec<Model>, StoreError> {
    let mut all: Vec<Model> = model::list(
        store,
        &model::Filter {
            task: Some(task),
            ..Default::default()
        },
    )?
    .into_iter()
    .rev()
    .filter(|m| m.answers())
    .filter(|m| reads.is_none_or(|e| m.encoder_model_ids.contains(&e)))
    .collect();
    // a stable sort keeps the newest first among each
    all.sort_by_key(|m| m.state != "promoted");
    Ok(all)
}

/// Body part's run: the newest active `bodypart-infer-fusion` and the
/// certified model's three parts, each the one served, in the order the
/// descriptor declares them: the encoder the head names, the head that
/// answers `body_part`, and the coarse mode that answers `body_region`
/// from the same encoder, its card naming the head where one does, which
/// the run goes without where none is served.
fn body_part(store: &mut Store) -> Result<Result<Plan, Refused>, StoreError> {
    let refused =
        |reason: &'static str, error: String| -> Result<Result<Plan, Refused>, StoreError> {
            Ok(Err(Refused::new(409, reason, error)))
        };
    let Some(p) = nils_registry::pipeline::resolve(store, BODY_PART_PIPELINE)? else {
        return refused("no_pipeline", "no body-part pipeline is installed".into());
    };
    let d = match nils_pipeline::descriptor::from_value(p.descriptor.clone()) {
        Ok(d) => d,
        Err(e) => {
            return refused(
                "no_pipeline",
                format!(
                    "{}: the catalog's descriptor no longer checks: {e}",
                    p.label()
                ),
            );
        }
    };
    let Some(head) = served(store, "axis:body_part", None)?.into_iter().next() else {
        return refused("no_model", "no body-part model is installed".into());
    };
    let encoder = match head.encoder_model_id {
        Some(id) => model::get(store, id)?.filter(|m| m.state != "retired"),
        None => None,
    };
    let Some(encoder) = encoder else {
        return refused(
            "no_model",
            format!("the encoder {} reads is not registered", head.label()),
        );
    };
    // the coarse mode names the head it sums: one whose card names this
    // head first, else one of the same encoder (the image refuses a mode
    // file of another head)
    let modes = served(store, "axis:body_region", Some(encoder.id))?;
    let coarse = modes
        .iter()
        .find(|m| m.card.to_string().contains(&head.digest))
        .or(modes.first());
    let mut models = Vec::new();
    let mut skipped: Option<&str> = None;
    for t in d.inputs.iter().filter(|t| t.ty == "model") {
        let given = match t.id.as_str() {
            "encoder" => Some(encoder.id),
            "head" => Some(head.id),
            "coarse" => coarse.map(|m| m.id),
            other => {
                return refused(
                    "no_pipeline",
                    format!(
                        "{} reads a model input {other} the body-part step does not fill",
                        p.label()
                    ),
                );
            }
        };
        // models are given in the order of the inputs, so one left out
        // can only be the last
        match (given, skipped) {
            (Some(_), Some(gone)) => {
                return refused(
                    "no_model",
                    format!(
                        "{} reads {} after {gone}, for which no model is served",
                        p.label(),
                        t.id
                    ),
                );
            }
            (Some(id), None) => models.push(id),
            (None, _) if t.optional => skipped = Some(t.id.as_str()),
            (None, _) => {
                return refused(
                    "no_model",
                    format!("no model is served for {}'s input {}", p.label(), t.id),
                );
            }
        }
    }
    Ok(Ok(Plan {
        pipeline: p.label(),
        models,
    }))
}

/// What an operation's run would be here, or why it is refused: a pipeline
/// in the catalog runs it, the models it reads are served, and pipelines
/// run here, with a container runtime and a working place.
pub(crate) fn plan(
    registry: &mut Registry,
    step: &str,
) -> Result<Result<Plan, Refused>, StoreError> {
    let planned = match step {
        "body_part" => body_part(registry.store())?,
        // record 55 F5 and E3: no model answers post-contrast yet
        "post_contrast" => Err(Refused::new(
            409,
            "no_model",
            "no post-contrast model is installed",
        )),
        other => Err(Refused::new(
            404,
            "no_step",
            format!("{other} is not a step a door runs; body_part and post_contrast are"),
        )),
    };
    let plan = match planned {
        Ok(plan) => plan,
        Err(refused) => return Ok(Err(refused)),
    };
    let found = crate::pipelines::detect_cached(registry);
    if found.runtime.is_none() {
        let why = found
            .reason
            .unwrap_or_else(|| "pipelines are off: no container runtime here".to_string());
        return Ok(Err(Refused::new(409, "no_runtime", why)));
    }
    if let Err(why) = crate::pipelines::run_places(registry.store()) {
        return Ok(Err(Refused::new(409, "no_place", why)));
    }
    Ok(Ok(plan))
}

/// Every operation's run as it would be here, by its step's name, for a
/// summary that says of each step why a run of it would be refused.
pub(crate) fn plans(registry: &mut Registry) -> Result<Plans, StoreError> {
    OPERATIONS
        .iter()
        .map(|(name, _)| Ok((*name, plan(registry, name)?)))
        .collect()
}

/// One run of an operation over the scans, as its step reads it.
struct Ran {
    id: i64,
    job: Option<i64>,
    status: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

/// One job a door queued for an operation's run over a dataset or a cohort.
struct Asked {
    id: i64,
    state: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    progress: Option<Value>,
}

impl Asked {
    fn open(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "running" | "cancelling")
    }
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

/// The jobs a door queued for a step's run over a dataset or a cohort
/// (`for`, `dataset:<name>` or `cohort:<name>`), newest first, among the
/// newest pipeline jobs. The args are read here rather than matched in SQL,
/// since the two backends render JSON differently.
fn asked(store: &mut Store, step: &str, target: &str) -> Result<Vec<Asked>, StoreError> {
    let sql = format!(
        "SELECT id, state, {}, {}, {}, {} FROM {} WHERE kind = 'pipeline' \
         ORDER BY id DESC LIMIT {ASKED}",
        crate::text_of(store, "job", "started_at"),
        crate::text_of(store, "job", "finished_at"),
        crate::text_of(store, "job", "progress"),
        crate::text_of(store, "job", "args"),
        store.qualified("job"),
    );
    let json = |t: Option<&str>| t.and_then(|t| serde_json::from_str::<Value>(t).ok());
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        let args = json(r.opt_text(5)?).unwrap_or(Value::Null);
        if args["step"].as_str() != Some(step) || args["for"].as_str() != Some(target) {
            continue;
        }
        out.push(Asked {
            id: r.int(0)?,
            state: r.text(1)?.to_string(),
            started_at: r.opt_text(2)?.map(str::to_string),
            finished_at: r.opt_text(3)?.map(str::to_string),
            progress: json(r.opt_text(4)?),
        });
    }
    Ok(out)
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

/// Where an operation's run over a scope's scans is: its runs over them
/// and the jobs a door queued for it, each newest first.
struct Attempts {
    runs: Vec<Ran>,
    asked: Vec<Asked>,
}

impl Attempts {
    /// A step's attempts over a scope's scans, by the pipelines that
    /// propose its axes.
    fn of(
        store: &mut Store,
        step: &str,
        pipelines: &[i64],
        scope: Scope<'_>,
        target: &str,
    ) -> Result<Attempts, StoreError> {
        Ok(Attempts {
            runs: runs_over(store, pipelines, scope)?,
            asked: asked(store, step, target)?,
        })
    }

    /// The run over the scans that runs now, or whose engine went away and
    /// which the lane takes up.
    fn going(&self) -> Option<&Ran> {
        self.runs
            .iter()
            .find(|r| matches!(r.status.as_str(), "running" | "interrupted"))
    }

    /// A job a door queued for the step that is not over.
    fn open(&self) -> Option<&Asked> {
        self.asked.iter().find(|j| j.open())
    }

    /// The job of a run queued or running now, none where nothing is.
    fn busy(&self) -> Option<Option<i64>> {
        self.going()
            .map(|r| r.job)
            .or_else(|| self.open().map(|j| Some(j.id)))
    }

    /// The newest job a door queued for the step that made no run, where
    /// it is newer than every run over the scans: one still waiting, or one
    /// that ended before its run began.
    fn unmade(&self) -> Option<&Asked> {
        let newest = self.runs.first().and_then(|r| r.job);
        self.asked
            .first()
            .filter(|j| newest.is_none_or(|run| j.id > run))
    }

    /// Whether the newest attempt failed: the door's job that made no run,
    /// or else the newest run. A cancelled one is no failure.
    fn failed(&self) -> bool {
        match self.unmade() {
            Some(j) => j.state == "failed",
            None => self.runs.first().is_some_and(|r| r.status == "failed"),
        }
    }

    /// Every job of the step over the scans, newest first.
    fn jobs(&self) -> Vec<i64> {
        let mut all: Vec<i64> = self
            .runs
            .iter()
            .filter_map(|r| r.job)
            .chain(self.asked.iter().map(|j| j.id))
            .collect();
        all.sort_unstable_by(|a, b| b.cmp(a));
        all.dedup();
        all
    }
}

/// Why a step's run over a scope's scans is refused now, or none: a run
/// of it over them is queued or runs, nothing here runs it, or there are
/// no scans to run it over.
fn refusal(
    step: &str,
    busy: Option<Option<i64>>,
    plan: &Result<Plan, Refused>,
    scans: i64,
) -> Option<Refused> {
    if let Some(job) = busy {
        let words = match job {
            Some(job) => format!(
                "{} runs already over these scans, as job {job}",
                title(step)
            ),
            None => format!("{} runs already over these scans", title(step)),
        };
        return Some(Refused::new(409, "running", words));
    }
    match plan {
        Err(refused) => Some(refused.clone()),
        Ok(_) if scans <= 0 => Some(Refused::new(
            409,
            "no_scans",
            "there are no scans to run it over yet",
        )),
        Ok(_) => None,
    }
}

/// Each operation's step over the scans in scope, `scans` of them, for a
/// dataset or a cohort (`target`, as a door's job names what it is for):
/// its state (queued while a door's job for it waits, or its run's engine
/// went away and the lane takes it up; running while that job or a run over
/// the scans goes on; failed when the newest of those failed; done once one
/// finished or its model answered one of them; waiting while it is served
/// and has not run; off where nothing serves it), the run or the job with
/// its times, the units over while it runs, how many scans its model
/// answered at or above its threshold, how many wait on a person, the jobs
/// of its runs and of the door over them, newest first, and why a run of
/// it would be refused now (`refusal`, none where the door would queue it).
pub(crate) fn steps(
    store: &mut Store,
    scope: Scope<'_>,
    target: &str,
    scans: i64,
    plans: &Plans,
    hide_sealed: bool,
) -> Result<Vec<Value>, StoreError> {
    let holds = shown(store, scope.holds(store), hide_sealed);
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
        let at = Attempts::of(store, name, &pipelines, scope, target)?;
        let going = at.going();
        let open = at.open();
        let finished = at
            .runs
            .iter()
            .any(|r| matches!(r.status.as_str(), "done" | "partial"));
        let state = match (going.map(|r| r.status.as_str()), open) {
            (Some("running"), _) => "running",
            // the engine that ran it went away, and the lane takes it up
            (Some(_), _) => "queued",
            (None, Some(j)) if j.state == "queued" => "queued",
            // its job has started, and its run's row comes next
            (None, Some(_)) => "running",
            (None, None) if at.failed() => "failed",
            (None, None) if finished || answered > 0 => "done",
            (None, None) if served => "waiting",
            (None, None) => "off",
        };
        // the attempt the step's times are of: the run going on, the door's
        // job not over, a door's job that failed before its run began, or
        // the newest run
        let job_only = open.or_else(|| at.unmade().filter(|j| j.state == "failed"));
        let (job, run, started_at, finished_at) = match (going, job_only) {
            (Some(r), _) => (
                r.job,
                Some(r.id),
                r.started_at.clone(),
                r.finished_at.clone(),
            ),
            (None, Some(j)) => (
                Some(j.id),
                None,
                j.started_at.clone(),
                j.finished_at.clone(),
            ),
            (None, None) => match at.runs.first() {
                Some(r) => (
                    r.job,
                    Some(r.id),
                    r.started_at.clone(),
                    r.finished_at.clone(),
                ),
                None => (None, None, None, None),
            },
        };
        let progress = match (going, open) {
            (Some(r), _) => {
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
            (None, Some(j)) => j.progress.clone(),
            (None, None) => None,
        };
        let planned = plans
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| {
                Err(Refused::new(
                    404,
                    "no_step",
                    format!("{name} is not a step a door runs"),
                ))
            });
        let refused = refusal(name, at.busy(), &planned, scans);
        out.push(json!({
            "step": name,
            "state": state,
            "job": job,
            "run": run,
            "started_at": started_at,
            "finished_at": finished_at,
            "progress": progress,
            "served": served,
            "answered": answered,
            "look": look,
            "of": scans,
            "jobs": at.jobs(),
            "refusal": refused.map(|r| r.doc()),
        }));
    }
    Ok(out)
}

/// A cohort's steps, as its document carries them: its members' scans
/// sorted, with what the sort asks of them, then each operation's step
/// over them. The main scans of its members are the picks summary's.
pub(crate) fn of_cohort(
    registry: &mut Registry,
    cohort: i64,
    name: &str,
    hide_sealed: bool,
) -> Result<Vec<Value>, StoreError> {
    let plans = plans(registry)?;
    let store = registry.store();
    let scope = Scope::Cohort(cohort);
    let holds = shown(store, scope.holds(store), hide_sealed);
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
    out.extend(steps(
        store,
        scope,
        &format!("cohort:{name}"),
        scans,
        &plans,
        hide_sealed,
    )?);
    Ok(out)
}

/// Freeze the scans in scope into a handle a run pins, each with its
/// subject, in stack order: every stack the step counts, a sample sealed
/// now among them, as a selection frozen for a run keeps them (record 48,
/// D1 of the move), so the handle opens only to a scope that reads sealed
/// stacks. Answers the handle and how many stacks it holds.
fn freeze(
    store: &mut Store,
    scope: Scope<'_>,
    principal: &str,
    epoch: i64,
) -> Result<(i64, usize), String> {
    let holds = scope.holds(store);
    let sql = format!(
        "SELECT x.id, se.subject_id FROM {} x JOIN {} se ON se.id = x.series_id \
         WHERE {holds} ORDER BY x.id",
        store.qualified("stack"),
        store.qualified("series"),
    );
    let rows: Vec<Row> = store
        .query(&sql, &[])
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| {
            Ok(Row(vec![
                Cell::Int(r.int(0)?),
                r.opt_int(1)?.map_or(Cell::Null, Cell::Int),
            ]))
        })
        .collect::<Result<_, StoreError>>()
        .map_err(|e| e.to_string())?;
    // the hash the ask gives an answer: each row's cells as text
    let hash = {
        use blake2::Digest as _;
        let mut h = blake2::Blake2b::<blake2::digest::consts::U32>::new();
        for row in &rows {
            let cells: Vec<String> = row.0.iter().map(nils_ask::exec::render).collect();
            h.update(cells.join("\t").as_bytes());
            h.update(b"\n");
        }
        hex::encode(h.finalize())
    };
    let answer = nils_ask::exec::Answer {
        columns: vec!["_key".to_string(), "_subject".to_string()],
        rows,
        truncated: false,
        content_hash: Some(hash),
        elapsed_ms: 0,
    };
    let node = nils_registry::job::hostname();
    let spec = nils_ask::handle::Spec {
        name: None,
        grain: nils_ask::Grain::Stack,
        ask: None,
        params: json!({}),
        selection_versions: json!([]),
        values_unresolved: json!({}),
        provenance: nils_ask::handle::Provenance {
            principal,
            node: &node,
            pack_version: None,
            epoch,
            scheme_digest: None,
            disclosure: "local",
            suppression: json!({"classes": ["technical"], "sealed": "read", "quasi": "shape"}),
        },
        page_rows: nils_catalog::Caps::default().page_rows as usize,
    };
    store.begin().map_err(|e| e.to_string())?;
    match nils_ask::handle::save(store, &spec, &answer) {
        Ok(h) => {
            store.commit().map_err(|e| e.to_string())?;
            Ok((h.id, answer.rows.len()))
        }
        Err(e) => {
            store.rollback().ok();
            Err(e.to_string())
        }
    }
}

/// `POST /api/datasets/{name}/steps/{step}/run` and `POST
/// /api/cohorts/{name}/steps/{step}/run`: the step's run over the scans it
/// counts, frozen now into a handle, queued for the dataset or the cohort
/// as a pipeline job under the caller's principal, grants and detail, and
/// answered 202 with the job. Refused, with its reason (`reason`), while a
/// run of it over those scans is queued or runs, where nothing here runs
/// it (no pipeline or no model for it, no container runtime, no working
/// place: post-contrast always, until a model for it is installed), and
/// where there are no scans to run it over.
pub(crate) fn run_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    kind: &str,
    name: &str,
    step: &str,
) -> Result<Reply, Reply> {
    let failed = |e: StoreError| Reply::error(500, e.to_string());
    let scope = match kind {
        "cohorts" => crate::viewer::Scope::cohort(registry, name)?,
        _ => crate::viewer::Scope::dataset(registry, name)?,
    };
    let Some((step, axes)) = OPERATIONS.iter().find(|(n, _)| *n == step).copied() else {
        return Err(Refused::new(
            404,
            "no_step",
            format!("{step} is not a step a door runs; body_part and post_contrast are"),
        )
        .reply(step));
    };
    let target = format!("{}:{}", scope.kind(), scope.name());
    let planned = plan(registry, step).map_err(failed)?;
    let epoch = registry.meta().epoch;
    let ids;
    let held = match &scope {
        crate::viewer::Scope::Dataset { sources, .. } => {
            ids = list(sources);
            Scope::Sources(&ids)
        }
        crate::viewer::Scope::Cohort(c) => Scope::Cohort(c.id),
    };
    let store = registry.store();
    let scans = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {} x WHERE {}",
            store.qualified("stack"),
            held.holds(store)
        ),
    )
    .map_err(failed)?;
    let (pipelines, _) = proposing(store, axes).map_err(failed)?;
    let at = Attempts::of(store, step, &pipelines, held, &target).map_err(failed)?;
    if let Some(refused) = refusal(step, at.busy(), &planned, scans) {
        return Err(refused.reply(step));
    }
    let Ok(plan) = planned else {
        return Err(Reply::error(500, "a refused run was planned"));
    };
    let (handle, frozen) =
        freeze(store, held, &caller.principal, epoch).map_err(|e| Reply::error(500, e))?;
    let mut command = vec![
        "run".to_string(),
        plan.pipeline,
        "--handle".to_string(),
        handle.to_string(),
    ];
    for m in &plan.models {
        command.extend(["--model".to_string(), m.to_string()]);
    }
    let command = crate::pipelines::located(doors.pack_dir.as_deref(), command)?;
    // the step and what it is for, which its state and the dataset's log
    // find it by, beside who asked and under what
    let mut extra = crate::serve::queued_by(caller);
    extra["step"] = json!(step);
    extra["for"] = json!(target);
    let id = nils_registry::job::enqueue_with(
        store,
        &command,
        Some(&target),
        Some(&caller.principal),
        extra,
    )
    .map_err(crate::serve::job_err)?;
    Ok(Reply::accepted(json!({
        "job": id, "state": "queued", "step": step, "for": target,
        "scans": frozen, "handle": handle, "command": command,
    })))
}

#[cfg(test)]
mod tests {
    use super::{Plan, Refused, owner, refusal};

    #[test]
    fn an_operation_owns_its_axes() {
        assert_eq!(owner("body_part"), Some("body_part"));
        assert_eq!(owner("body_region"), Some("body_part"));
        assert_eq!(owner("post_contrast"), Some("post_contrast"));
        assert_eq!(owner("technique"), None);
    }

    #[test]
    fn a_run_is_refused_while_one_goes_on_then_by_its_plan_then_for_no_scans() {
        let plan = Ok(Plan {
            pipeline: "bodypart-infer-fusion@1".to_string(),
            models: vec![1, 2],
        });
        let no_model = Err(Refused {
            status: 409,
            reason: "no_model",
            error: "no post-contrast model is installed".to_string(),
        });
        let running = refusal("body_part", Some(Some(7)), &plan, 3).unwrap();
        assert_eq!((running.status, running.reason), (409, "running"));
        assert!(running.error.contains("job 7"), "{}", running.error);
        assert!(
            running.error.starts_with("body part runs"),
            "{}",
            running.error
        );
        let refused = refusal("post_contrast", None, &no_model, 3).unwrap();
        assert_eq!(refused.reason, "no_model");
        assert_eq!(refused.error, "no post-contrast model is installed");
        assert_eq!(
            refusal("body_part", None, &plan, 0).map(|r| r.reason),
            Some("no_scans")
        );
        assert_eq!(refusal("body_part", None, &plan, 3), None);
    }
}
