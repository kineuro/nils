// SPDX-License-Identifier: AGPL-3.0-only

//! A workflow run (Wave 7b A7, section 8.4): the steps run as pipeline runs
//! over one frozen scope, each step's units going on as soon as the same
//! unit of the step before is over, without waiting for the whole
//! selection; a unit skipped or failed at one step is skipped downstream
//! with the same reason; a step fed by another's files (a typed derivative
//! input) starts when that step is done, since a run binds its derivative
//! inputs when it starts; the run's line sums each step's units.
//!
//! After review (2026-10-25): each step is a row of its own
//! (`workflow_step`), written by (run, step), so the step threads never
//! write over each other; every exit of a step thread records a terminal
//! status, and a step downstream of one that ended without a run ends too,
//! skipped with the reason, so a workflow run always ends.
//!
//! After the second review (2026-10-25): the worker's `workflow` job is
//! beaten while the steps run, with the steps done of total as its
//! progress, so a cancel is seen: the step runs in flight are cancelled
//! through the pipeline run's own cancel path, no step after them starts,
//! and the workflow run and the job end `cancelled`. Each link of the
//! document is carried into the step's run, bound to the run of the step
//! it comes from, so a typed input takes that run's files and a fed role
//! that run's image, never another run's kept result.
//!
//! After the third review (2026-10-25): a step fed through a role waits for
//! the whole step before, as one fed through a typed input does (a run
//! binds both as it starts); the per-unit chaining is for order-only steps.
//! A step thread that ends with its pipeline run still `running` concludes
//! that run (failed, in its words), and a run before that is `interrupted`,
//! or whose step is over, is over to the steps after it. A link into an
//! `optional` typed input lets the step run where the step before ended
//! without a run, recording what fed nothing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use nils_pipeline::workflow as doc;
use nils_registry::home::Home;
use nils_registry::job;
use nils_registry::pipeline as prows;
use nils_registry::workflow as rows;
use nils_registry::{Registry, Store};
use serde_json::{Value, json};

use crate::serve::{Caller, Reply};
use crate::workflows::{Person, PlanArgs, Scope};
use crate::{Exit, fail, usage};

/// How often a step looks for the run of the step before it.
const POLL: Duration = Duration::from_millis(300);

/// A step's statuses that are over: nothing more will be recorded of it.
const TERMINAL: [&str; 8] = [
    "done",
    "partial",
    "failed",
    "cancelled",
    "interrupted",
    "refused",
    "skipped",
    "gone",
];

fn err(e: nils_registry::Error) -> String {
    e.to_string()
}

/// Whether a step's status is terminal: the chain's gate reads a run whose
/// step is over as over, whatever the run row says (after the third
/// review, finding 2).
pub(crate) fn step_is_over(status: &str) -> bool {
    TERMINAL.contains(&status)
}

/// What runs a step's pipeline run: `nils run`'s own command, or a stand-in
/// in a test of the chain.
pub(crate) type Runner = fn(&Home, crate::pipelines::RunArgs) -> Result<(), Exit>;

/// A step of a workflow run learned its pipeline run: written as soon as
/// the run is claimed, so the step after it can wait on it per unit.
pub(crate) fn record_step_run(
    store: &mut Store,
    workflow_run: i64,
    step: usize,
    run_id: i64,
) -> Result<(), String> {
    rows::update_step(
        store,
        workflow_run,
        step,
        &rows::StepUpdate {
            run_id: Some(run_id),
            status: Some("running"),
            ..Default::default()
        },
    )
    .map_err(err)
}

/// A step ended: its terminal status, its error where it has one, its
/// units summed where it had a run, and when.
fn record_end(
    store: &mut Store,
    workflow_run: i64,
    step: usize,
    status: &str,
    error: Option<&str>,
    units: Option<&Value>,
    finished_at: Option<&str>,
) -> Result<(), String> {
    let now = nils_registry::time::now_iso();
    rows::update_step(
        store,
        workflow_run,
        step,
        &rows::StepUpdate {
            run_id: None,
            status: Some(status),
            error,
            units,
            finished_at: Some(finished_at.unwrap_or(&now)),
        },
    )
    .map_err(err)
}

/// `nils workflow run`: plan first, refuse what the plan refuses, then run
/// the chain here, step by step per unit, and print the run.
pub(crate) fn command(home: &Home, mut registry: Registry, args: PlanArgs) -> Result<(), Exit> {
    let (w, scope) = crate::workflows::scope_of(home, &mut registry, &args)?;
    let person = crate::workflows::Person::of_args(&args)?;
    let plan = crate::workflows::plan(home, &mut registry, &w, &scope, &person).map_err(usage)?;
    if plan["runnable"] != true {
        return Err(usage(
            plan["words"]
                .as_str()
                .unwrap_or("the plan refused the workflow"),
        ));
    }
    // the worker's job this command runs as, where it does (a `workflow`
    // job, never a pipeline one); the steps claim pipeline jobs of their
    // own, so the variable is taken off before they start
    let job_id = job::adopted_job();
    job::disown_adoption();
    let id = start(home, &mut registry, &w, &scope, &person, job_id)?;
    let r = rows::run(registry.store(), id)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| fail(format!("no workflow run {id}")))?;
    let v = run_doc(registry.store(), &r);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&v).map_err(|e| fail(e.to_string()))?
        );
    } else {
        println!("{}", run_text(&v));
    }
    match r.status.as_str() {
        "done" => Ok(()),
        // asked to stop: the job ends cancelled, as a pipeline run's does
        "cancelled" => {
            if let Some(j) = job_id {
                let _ = job::finish(
                    registry.store(),
                    j,
                    job::State::Cancelled,
                    Some(&format!("workflow run {id} was cancelled")),
                );
            }
            Err(Exit {
                code: crate::STOPPED,
                message: format!(
                    "workflow run {id} was cancelled; the units its steps finished are kept"
                ),
            })
        }
        other => Err(fail(format!(
            "workflow run {id} ended {other}: {}",
            r.error.unwrap_or_default()
        ))),
    }
}

/// Start a workflow run over a frozen scope and run its steps to the end:
/// one thread a step, each a `nils run` of its pipeline over the scope's
/// handle, chained to the runs before it. Answers the workflow run's id
/// once every step is over.
pub(crate) fn start(
    home: &Home,
    registry: &mut Registry,
    w: &rows::Workflow,
    scope: &Scope,
    person: &Person,
    job_id: Option<i64>,
) -> Result<i64, Exit> {
    start_with(
        home,
        registry,
        w,
        scope,
        person,
        job_id,
        crate::pipelines::run_command,
    )
}

/// One link of the document into a step, as the step's thread carries it
/// into its run once the step before has a run.
struct StepLink {
    from_step: usize,
    from_id: String,
    output: String,
    input: String,
    /// Whether the input is a pick role of the step's descriptor (else a
    /// typed derivative input).
    role: bool,
}

/// What one step's thread is given.
struct StepRun {
    home: Home,
    workflow_run: i64,
    step: usize,
    s: doc::Step,
    /// The pipeline's label where the catalog holds it.
    label: Option<String>,
    params: Vec<(String, String)>,
    goal: Option<String>,
    /// The steps whose results this one reads per unit (a link).
    waits_for: Vec<usize>,
    /// Of those, the steps whose files it reads as a typed input or whose
    /// image it takes for a role: waited for whole (after the third review,
    /// finding 1: a run binds both as it starts).
    whole: Vec<usize>,
    /// Of those, the steps whose every link into this one feeds an
    /// `optional` typed input: this step goes on without them where they
    /// ended without a run (after the third review, finding 9).
    optional: Vec<usize>,
    /// Every step before it: the order of the document is the order of
    /// the run.
    before: Vec<usize>,
    /// The document's links into this step.
    links: Vec<StepLink>,
    /// Set once the workflow run was asked to stop: a step not yet started
    /// ends cancelled instead.
    cancel: Arc<AtomicBool>,
    handle: i64,
    pack: String,
    pack_dir: Option<PathBuf>,
    /// Record 57 P9: the proposal the person accepted, which every step's
    /// run records.
    proposed_by: Option<crate::proposed_by::ProposedBy>,
}

/// [`start`] with the runner named: every step's parameters are checked
/// before anything is made, every step thread records how it ended, and
/// the run is finished whatever the threads did.
pub(crate) fn start_with(
    home: &Home,
    registry: &mut Registry,
    w: &rows::Workflow,
    scope: &Scope,
    person: &Person,
    job_id: Option<i64>,
    runner: Runner,
) -> Result<i64, Exit> {
    let d = doc::from_value(w.document.clone()).map_err(|e| fail(format!("{}: {e}", w.label())))?;
    let resolved = crate::workflows::resolve_steps(registry.store(), &d);
    // every step's parameters first: a knob a person may not set, or a knob
    // naming a step the document lacks, is refused before a row is made or
    // a thread started
    crate::workflows::knob_steps(&d, person).map_err(usage)?;
    let params: Vec<Vec<(String, String)>> = d
        .steps
        .iter()
        .map(|s| crate::workflows::step_params(&d, s, person))
        .collect::<Result<_, _>>()
        .map_err(usage)?;
    // the links that feed a typed input or a role: a step so fed waits for
    // the whole step before, since a run binds both at its start (after the
    // third review, finding 1: a role too); the steps whose links all feed
    // an optional input are not waited for where they ended without a run
    let fed_whole = crate::workflows::waits_whole(&d, &resolved);
    let fed_optional = crate::workflows::optional_from(&d, &resolved);
    let steps: Vec<Value> = d
        .steps
        .iter()
        .zip(&resolved)
        .enumerate()
        .map(|(i, (s, r))| {
            json!({
                "step": i, "id": s.id, "of": s.of, "name": s.name, "reference": s.reference(),
                "pipeline": r.run_label(),
                "pipeline_id": r.pipeline.as_ref().map(|p| p.id),
                "family": r.family.as_ref().map(|(f, _)| f.name.clone()),
                "waits_for": d.fed_by(&s.id), "waits_whole": fed_whole[i],
                "goal": s.goal.clone().or_else(|| person.goal.clone()), "switches": s.switches,
            })
        })
        .collect();
    let who = crate::actor();
    let actor = nils_registry::actor::current();
    let now = nils_registry::time::now_iso();
    let id = rows::start(
        registry.store(),
        &rows::NewRun {
            workflow_id: w.id,
            job_id,
            handle_id: Some(scope.handle),
            selection: scope.selection.as_deref(),
            person: &person.record(&who),
            steps: &json!(steps),
            principal: &who,
            actor: Some(&actor),
            started_at: &now,
        },
    )
    .map_err(|e| fail(e.to_string()))?;

    // one thread a step
    let position = |id: &str| d.steps.iter().position(|q| q.id == id);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();
    for (i, s) in d.steps.iter().enumerate() {
        let links: Vec<StepLink> = d
            .links
            .iter()
            .filter(|l| l.to_step == s.id)
            .filter_map(|l| {
                let descs = resolved[i].descriptors();
                if descs.is_empty() {
                    return None;
                }
                let role = descs.iter().any(|desc| desc.roles.contains(&l.input));
                let typed = descs
                    .iter()
                    .any(|desc| desc.inputs.iter().any(|t| t.id == l.input));
                let from_step = position(&l.from_step)?;
                (role || typed).then(|| StepLink {
                    from_step,
                    from_id: l.from_step.clone(),
                    output: l.output.clone(),
                    input: l.input.clone(),
                    role,
                })
            })
            .collect();
        let run = StepRun {
            home: home.clone(),
            workflow_run: id,
            step: i,
            s: s.clone(),
            label: resolved[i].run_label(),
            params: params[i].clone(),
            goal: s.goal.clone().or_else(|| person.goal.clone()),
            waits_for: d.fed_by(&s.id).iter().filter_map(|q| position(q)).collect(),
            whole: fed_whole[i].iter().filter_map(|q| position(q)).collect(),
            optional: fed_optional[i].iter().filter_map(|q| position(q)).collect(),
            before: (0..i).collect(),
            links,
            cancel: Arc::clone(&cancel),
            handle: scope.handle,
            pack: scope.pack.clone(),
            pack_dir: scope.pack_dir.clone(),
            proposed_by: person.proposed_by.clone(),
        };
        handles.push(std::thread::spawn(move || run_step(&run, runner)));
    }
    // the heart while the steps run: the worker's job is beaten once a
    // second with the steps done of total, and a cancel asked of it stops
    // the step runs in flight and starts no step after them (after review,
    // finding 2 of the second review)
    let mut cancelled = false;
    let mut beaten: Option<Instant> = None;
    while !handles.iter().all(|h| h.is_finished()) {
        if let Some(j) = job_id
            && beaten.is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
        {
            beaten = Some(Instant::now());
            let progress = progress_of(registry.store(), id).map_err(fail)?;
            if matches!(
                job::beat(registry.store(), j, Some(&progress)),
                Ok(job::Asked::Cancel)
            ) {
                cancelled = true;
                cancel.store(true, Ordering::SeqCst);
            }
            if cancelled {
                // asked again each second: a step that claimed its run
                // between two beats is cancelled too
                cancel_steps(registry.store(), id).map_err(fail)?;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut errors: Vec<String> = Vec::new();
    for h in handles {
        match h.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(e),
            Err(_) => errors.push("a step's thread ended in a panic".into()),
        }
    }
    // a step whose thread could not record its end is failed here, so the
    // run's steps are all over when it is finished
    let r = rows::run(registry.store(), id)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| fail(format!("no workflow run {id}")))?;
    for (i, st) in r.steps.as_array().into_iter().flatten().enumerate() {
        let status = st["status"].as_str().unwrap_or("queued");
        if !TERMINAL.contains(&status) {
            record_end(
                registry.store(),
                id,
                i,
                "failed",
                Some("the step's thread ended without recording how"),
                None,
                None,
            )
            .map_err(fail)?;
        }
    }
    finish(registry.store(), id, &errors, cancelled).map_err(fail)?;
    if let Some(j) = job_id {
        let progress = progress_of(registry.store(), id).map_err(fail)?;
        let _ = job::beat(registry.store(), j, Some(&progress));
    }
    Ok(id)
}

/// The job's progress: the steps done of total, and each step's status
/// and run.
fn progress_of(store: &mut Store, id: i64) -> Result<Value, String> {
    let r = rows::run(store, id)
        .map_err(err)?
        .ok_or_else(|| format!("no workflow run {id}"))?;
    let steps = r.steps.as_array().cloned().unwrap_or_default();
    Ok(json!({
        "workflow_run": id,
        "steps_total": steps.len(),
        "steps_done": steps.iter().filter(|s| s["status"] == "done").count(),
        "steps_over": steps.iter().filter(|s| TERMINAL.contains(&s["status"].as_str().unwrap_or("queued"))).count(),
        "steps": steps.iter().map(|s| json!({"step": s["step"], "id": s["id"], "status": s["status"], "run": s["run"]})).collect::<Vec<_>>(),
    }))
}

/// Ask every step run of the workflow run that is not over to stop, through
/// its own job: the pipeline run sees the cancel at its next heartbeat, as
/// `nils jobs cancel` on it would have it.
fn cancel_steps(store: &mut Store, id: i64) -> Result<(), String> {
    let r = rows::run(store, id)
        .map_err(err)?
        .ok_or_else(|| format!("no workflow run {id}"))?;
    for st in r.steps.as_array().into_iter().flatten() {
        let status = st["status"].as_str().unwrap_or("queued");
        let Some(run) = st["run"].as_i64() else {
            continue;
        };
        if TERMINAL.contains(&status) {
            continue;
        }
        if let Some(pr) = prows::run(store, run).map_err(err)?
            && let Some(j) = pr.job_id
        {
            job::request_cancel(store, j).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// What the steps before say to a step about to start.
#[derive(Debug, PartialEq)]
enum Wait {
    /// Every step before is far enough: the runs this step chains to per
    /// unit, each with the step it is, and the steps before that fed
    /// nothing (an optional input's step that ended without a run), each
    /// with the words recorded on the units (after the third review,
    /// finding 9).
    Ready {
        after: Vec<(usize, i64)>,
        unfed: Vec<(usize, String)>,
    },
    /// A step before has not yet made its run, or one read whole still
    /// runs.
    NotYet,
    /// A step before ended without a run this step could go on from: the
    /// words this step is skipped with (section 8.4's rule).
    Skipped(String),
}

/// Decide, from the steps before as recorded, whether a step may start:
/// `pipeline_status` answers a run's status (none where the run is gone).
/// A run before whose step is over, or that is `interrupted`, is over
/// (after the third review, finding 2): only a step still running holds a
/// step read whole.
fn wait_of(
    steps: &[Value],
    before: &[usize],
    waits_for: &[usize],
    whole: &[usize],
    optional: &[usize],
    pipeline_status: &mut dyn FnMut(i64) -> Result<Option<String>, String>,
) -> Result<Wait, String> {
    let mut after: Vec<(usize, i64)> = Vec::new();
    let mut unfed: Vec<(usize, String)> = Vec::new();
    let mut ready = true;
    for &k in before {
        let st = steps.get(k).cloned().unwrap_or(Value::Null);
        let status = st["status"].as_str().unwrap_or("queued");
        let name = format!("step {} ({})", k + 1, st["id"].as_str().unwrap_or_default());
        match st["run"].as_i64() {
            Some(run) => {
                if waits_for.contains(&k) {
                    after.push((k, run));
                }
                if whole.contains(&k) && !TERMINAL.contains(&status) {
                    let pr = pipeline_status(run)?;
                    if pr.as_deref() == Some("running") {
                        ready = false;
                    }
                }
            }
            None if !TERMINAL.contains(&status) => ready = false,
            // the step before ended with no run: nothing of it ran; a step
            // whose links all feed an optional input goes on without it
            None if optional.contains(&k) => {
                unfed.push((
                    k,
                    format!(
                        "fed by no step for this unit: {name} ended {status} without a run{}",
                        st["error"]
                            .as_str()
                            .map_or(String::new(), |e| format!(": {e}"))
                    ),
                ));
            }
            None if matches!(status, "done" | "partial") => {
                if waits_for.contains(&k) || whole.contains(&k) {
                    return Ok(Wait::Skipped(format!(
                        "skipped: {name} ended {status} without a run, and this step reads its results"
                    )));
                }
            }
            None => {
                return Ok(Wait::Skipped(format!(
                    "skipped: {name} {status}: {}",
                    st["error"].as_str().unwrap_or("no reason recorded")
                )));
            }
        }
    }
    if ready {
        Ok(Wait::Ready { after, unfed })
    } else {
        Ok(Wait::NotYet)
    }
}

/// After the third review (finding 2): a step thread that ended in an
/// error or a panic may have left its pipeline run `running` (the run was
/// claimed, then the thread unwound before the run concluded): conclude it
/// here, failed in the thread's words, and its job with it, so no run row
/// says running for a run nothing runs, and the steps after it see it over.
fn conclude_left_running(
    store: &mut Store,
    workflow_run: i64,
    step: usize,
    words: &str,
) -> Result<Option<i64>, String> {
    let Some(st) = rows::step(store, workflow_run, step).map_err(err)? else {
        return Ok(None);
    };
    let Some(run) = st["run"].as_i64() else {
        return Ok(None);
    };
    let Some(pr) = prows::run(store, run).map_err(err)? else {
        return Ok(None);
    };
    if pr.status != "running" {
        return Ok(None);
    }
    let now = nils_registry::time::now_iso();
    prows::finish(
        store,
        run,
        &prows::Finish {
            status: "failed",
            finished_at: &now,
            exit_code: None,
            results_digest: None,
            summary: &json!({}),
            error: Some(words),
            failure: None,
        },
    )
    .map_err(err)?;
    if let Some(j) = pr.job_id {
        let _ = job::finish(store, j, job::State::Failed, Some(words));
    }
    Ok(Some(run))
}

/// One step: wait for the runs it is chained to, run its pipeline over the
/// handle with the chain set, and record how it ended. Whatever way the
/// body ends, in an error or a panic, the step's row says so: a step
/// downstream never waits for one that is over.
fn run_step(p: &StepRun, runner: Runner) -> Result<(), String> {
    let result =
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| step_body(p, runner))) {
            Ok(r) => r,
            Err(payload) => {
                let words = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "no words".to_string());
                Err(format!("the step's thread panicked: {words}"))
            }
        };
    if let Err(e) = &result
        && let Ok(mut registry) = crate::open(&p.home)
    {
        // the step's run, where the thread left it running (after the third
        // review, finding 2): concluded before the step is
        let _ = conclude_left_running(registry.store(), p.workflow_run, p.step, e);
        let _ = record_end(
            registry.store(),
            p.workflow_run,
            p.step,
            "failed",
            Some(e),
            None,
            None,
        );
    }
    result
}

fn step_body(p: &StepRun, runner: Runner) -> Result<(), String> {
    let mut registry = crate::open(&p.home).map_err(|e| e.message)?;
    let Some(label) = p.label.as_deref() else {
        return record_end(
            registry.store(),
            p.workflow_run,
            p.step,
            "refused",
            Some(&format!("{} {} is not in the catalog", p.s.of, p.s.name)),
            None,
            None,
        );
    };
    // the runs of the steps before: chained per unit to those a link feeds
    // from, and started after those whose files it reads are done
    let (after, unfed) = loop {
        if p.cancel.load(Ordering::SeqCst) {
            return record_end(
                registry.store(),
                p.workflow_run,
                p.step,
                "cancelled",
                Some("cancelled: the workflow run was cancelled before this step started"),
                None,
                None,
            );
        }
        let r = rows::run(registry.store(), p.workflow_run)
            .map_err(err)?
            .ok_or_else(|| format!("no workflow run {}", p.workflow_run))?;
        let steps = r.steps.as_array().cloned().unwrap_or_default();
        let store = registry.store();
        let mut status_of = |run: i64| -> Result<Option<String>, String> {
            Ok(prows::run(store, run).map_err(err)?.map(|pr| pr.status))
        };
        match wait_of(
            &steps,
            &p.before,
            &p.waits_for,
            &p.whole,
            &p.optional,
            &mut status_of,
        )? {
            Wait::Ready { after, unfed } => break (after, unfed),
            Wait::NotYet => std::thread::sleep(POLL),
            Wait::Skipped(words) => {
                return record_end(
                    registry.store(),
                    p.workflow_run,
                    p.step,
                    "skipped",
                    Some(&words),
                    None,
                    None,
                );
            }
        }
    };
    drop(registry);
    // the step's run, as `nils run` would take it
    let mut argv: Vec<String> = vec![
        "run".into(),
        label.to_string(),
        "--handle".into(),
        p.handle.to_string(),
        "--pack".into(),
        p.pack.clone(),
    ];
    if let Some(d) = &p.pack_dir {
        argv.extend(["--pack-dir".to_string(), d.display().to_string()]);
    }
    if let Some(g) = &p.goal {
        argv.extend(["--goal".to_string(), g.to_string()]);
    }
    for (k, v) in &p.s.switches {
        let v = match v {
            Value::String(t) => t.clone(),
            other => other.to_string(),
        };
        argv.extend(["--switch".to_string(), format!("{k}={v}")]);
    }
    for (k, v) in &p.params {
        argv.extend(["--param".to_string(), format!("{k}={v}")]);
    }
    if let Some(pb) = &p.proposed_by {
        argv.extend(pb.args());
    }
    argv.push("--json".into());
    let mut args = StepCli::try_parse_from(&argv)
        .map_err(|e| format!("the step's run arguments: {e}"))?
        .run;
    // each link bound to the run of the step it comes from (after review,
    // finding 3 of the second review); a link from a step whose run is not
    // among them is one the wait skipped this step for already
    let links: Vec<crate::pipelines::ChainLink> = p
        .links
        .iter()
        .filter_map(|l| {
            let (_, run) = after.iter().find(|(k, _)| *k == l.from_step)?;
            Some(crate::pipelines::ChainLink {
                from_step: l.from_step,
                from_id: l.from_id.clone(),
                run: *run,
                output: l.output.clone(),
                input: l.input.clone(),
                role: l.role,
            })
        })
        .collect();
    // a link from a step that fed nothing (an optional input's step that
    // ended without a run): the input takes nothing, and the units record
    // why (after the third review, finding 9)
    let unfed: Vec<crate::pipelines::Unfed> = p
        .links
        .iter()
        .filter_map(|l| {
            let (_, words) = unfed.iter().find(|(k, _)| *k == l.from_step)?;
            Some(crate::pipelines::Unfed {
                input: l.input.clone(),
                words: words.clone(),
            })
        })
        .collect();
    args.chain = Some(crate::pipelines::Chain {
        workflow_run: p.workflow_run,
        step: p.step,
        after: after.iter().map(|(_, run)| *run).collect(),
        links,
        unfed,
    });
    let outcome = runner(&p.home, args);
    let mut registry = crate::open(&p.home).map_err(|e| e.message)?;
    let st = rows::step(registry.store(), p.workflow_run, p.step)
        .map_err(err)?
        .ok_or_else(|| format!("workflow run {} has no step {}", p.workflow_run, p.step))?;
    let pr = match st["run"].as_i64() {
        Some(id) => prows::run(registry.store(), id).map_err(err)?,
        None => None,
    };
    let (status, error, units, finished_at) = match (outcome, pr) {
        (_, Some(pr)) => (
            pr.status,
            pr.error,
            Some(pr.summary["units"].clone()),
            pr.finished_at,
        ),
        (Ok(()), None) => ("done".to_string(), None, None, None),
        (Err(e), None) => (
            if e.code == crate::USAGE {
                "refused".to_string()
            } else {
                "failed".to_string()
            },
            Some(e.message),
            None,
            None,
        ),
    };
    record_end(
        registry.store(),
        p.workflow_run,
        p.step,
        &status,
        error.as_deref(),
        units.as_ref(),
        finished_at.as_deref(),
    )
}

/// `nils run`'s arguments, parsed for a step.
#[derive(clap::Parser)]
struct StepCli {
    #[command(flatten)]
    run: crate::pipelines::RunArgs,
}

/// Close the workflow run: its status from its steps' (`cancelled` where it
/// was asked to stop), and a summary that sums each step's units.
fn finish(store: &mut Store, id: i64, errors: &[String], cancelled: bool) -> Result<(), String> {
    let r = rows::run(store, id)
        .map_err(err)?
        .ok_or_else(|| format!("no workflow run {id}"))?;
    let steps = r.steps.as_array().cloned().unwrap_or_default();
    let mut totals: std::collections::BTreeMap<&str, i64> = Default::default();
    for s in &steps {
        for key in [
            "total",
            "succeeded",
            "failed",
            "skipped",
            "reused",
            "unreported",
        ] {
            *totals.entry(key).or_default() += s["units"][key].as_i64().unwrap_or(0);
        }
    }
    let statuses: Vec<&str> = steps
        .iter()
        .map(|s| s["status"].as_str().unwrap_or("queued"))
        .collect();
    let status = if cancelled {
        "cancelled"
    } else if !errors.is_empty() {
        "failed"
    } else if statuses.iter().all(|s| *s == "done") {
        "done"
    } else if statuses.iter().any(|s| matches!(*s, "done" | "partial")) {
        "partial"
    } else if statuses.contains(&"cancelled") {
        "cancelled"
    } else {
        "failed"
    };
    let step_errors: Vec<String> = steps
        .iter()
        .filter_map(|s| {
            s["error"].as_str().map(|e| {
                format!(
                    "step {} ({}): {e}",
                    s["step"].as_i64().unwrap_or(0) + 1,
                    s["id"].as_str().unwrap_or_default()
                )
            })
        })
        .chain(errors.iter().cloned())
        .collect();
    let summary = json!({
        "steps": steps.iter().map(|s| json!({
            "step": s["step"], "id": s["id"], "pipeline": s["pipeline"], "run": s["run"], "status": s["status"], "units": s["units"],
        })).collect::<Vec<_>>(),
        "units": totals,
        "steps_done": statuses.iter().filter(|s| **s == "done").count(),
        "steps_total": steps.len(),
    });
    rows::finish(
        store,
        id,
        status,
        &summary,
        (!step_errors.is_empty())
            .then(|| step_errors.join("; "))
            .as_deref(),
        &nils_registry::time::now_iso(),
    )
    .map_err(err)
}

/// A workflow run as the doors answer it.
pub(crate) fn run_doc(store: &mut Store, r: &rows::Run) -> Value {
    let w = rows::get(store, r.workflow_id).ok().flatten();
    let steps: Vec<Value> = r
        .steps
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut s| {
            // a running step's units as they stand
            if let Some(run) = s["run"].as_i64()
                && s["units"].is_null()
                && let Ok(units) = prows::units(store, run)
            {
                let count = |st: &str| units.iter().filter(|u| u.outcome["status"] == st).count();
                s["units"] = json!({
                    "total": units.len(), "succeeded": count("succeeded"), "failed": count("failed"),
                    "skipped": count("skipped"), "reused": count("reused"),
                    "over": units.iter().filter(|u| u.state == "over").count(),
                });
            }
            s
        })
        .collect();
    json!({
        "id": r.id, "workflow": w.as_ref().map(|w| w.label()), "workflow_id": r.workflow_id,
        "job": r.job_id, "handle": r.handle_id, "selection": r.selection,
        "person": r.person, "status": r.status, "started_at": r.started_at, "finished_at": r.finished_at,
        "steps": steps, "summary": r.summary, "principal": r.principal, "error": r.error,
        // record 57 P9: the assistant's proposal the person accepted, null
        // where the person chose without one
        "proposed_by": r.person["proposed_by"],
    })
}

pub(crate) fn run_line(v: &Value) -> String {
    let units = &v["summary"]["units"];
    format!(
        "workflow run {} {} {}: {} of {} steps done; units {} succeeded, {} reused, {} skipped, {} failed",
        v["id"],
        v["workflow"].as_str().unwrap_or_default(),
        v["status"].as_str().unwrap_or_default(),
        v["summary"]["steps_done"],
        v["summary"]["steps_total"],
        units["succeeded"],
        units["reused"],
        units["skipped"],
        units["failed"]
    )
}

pub(crate) fn run_text(v: &Value) -> String {
    let mut out = vec![run_line(v)];
    // record 57 P9: the plan the person accepted from the assistant
    if let Some(l) = crate::proposed_by::line(&v["proposed_by"]) {
        out.push(format!("  plan: {l}"));
    }
    for s in v["steps"].as_array().into_iter().flatten() {
        let u = &s["units"];
        out.push(format!(
            "  step {} {} ({}): {}{}{}",
            s["step"].as_i64().unwrap_or(0) + 1,
            s["id"].as_str().unwrap_or_default(),
            s["pipeline"].as_str().unwrap_or("not in the catalog"),
            s["status"].as_str().unwrap_or_default(),
            s["run"]
                .as_i64()
                .map_or(String::new(), |r| format!(", run {r}")),
            if u.is_object() {
                format!(
                    ": {} units, {} succeeded, {} reused, {} skipped, {} failed",
                    u["total"], u["succeeded"], u["reused"], u["skipped"], u["failed"]
                )
            } else {
                s["error"]
                    .as_str()
                    .map_or(String::new(), |e| format!(": {e}"))
            }
        ));
    }
    if let Some(e) = v["error"].as_str() {
        out.push(format!("  {e}"));
    }
    out.join("\n")
}

/// `POST /api/workflows/{id}/run`: plan, refuse what the plan refuses, then
/// queue `nils workflow run` on the pipelines lane; 202 with the job.
pub(crate) fn run_door(
    home: &Home,
    pack_dir: Option<&std::path::Path>,
    registry: &mut Registry,
    caller: &Caller,
    id: &str,
    body: &str,
) -> Result<Reply, Reply> {
    let w = rows::resolve(registry.store(), id)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .ok_or_else(|| Reply::error(404, format!("no workflow {id}")))?;
    let doc_ = crate::serve::json_body(body)?;
    let scope = crate::workflows::scope_of_body(home, registry, &w, &doc_, pack_dir)?;
    let person = Person::of_body(&doc_).map_err(|e| Reply::error(400, e))?;
    let plan = crate::workflows::plan(home, registry, &w, &scope, &person)
        .map_err(|e| Reply::error(400, e))?;
    if plan["runnable"] != true {
        return Err(Reply::error(
            409,
            plan["words"]
                .as_str()
                .unwrap_or("the plan refused the workflow"),
        ));
    }
    let mut command: Vec<String> = vec![
        "workflow".into(),
        "run".into(),
        w.label(),
        "--handle".into(),
        scope.handle.to_string(),
        "--pack".into(),
        scope.pack.clone(),
    ];
    if let Some(g) = &person.goal {
        command.extend(["--goal".to_string(), g.clone()]);
    }
    for (step, knobs) in &person.knobs {
        for (k, v) in knobs {
            command.extend(["--knob".to_string(), format!("{step}.{k}={v}")]);
        }
    }
    // record 57 P9: the proposal the person accepted
    if let Some(pb) = &person.proposed_by {
        command.extend(pb.args());
    }
    let job = nils_registry::job::enqueue_with(
        registry.store(),
        &command,
        Some(&format!("workflow {}", w.label())),
        Some(&caller.principal),
        crate::serve::queued_by(caller),
    )
    .map_err(|e| Reply::error(500, e.to_string()))?;
    Ok(Reply::accepted(json!({
        "job": job, "state": "queued", "workflow": w.label(), "workflow_id": w.id,
        "handle": scope.handle, "plan": plan["verdicts"],
        "message": format!("workflow {} is queued as job {job} over handle {}; GET /api/workflow-runs lists its run once it starts", w.label(), scope.handle),
    })))
}

/// The jobs door: `workflow run <workflow> --handle <id>` and the choices,
/// never a path a caller composes.
pub(crate) fn located(command: Vec<String>) -> Result<Vec<String>, Reply> {
    let mut out = vec!["workflow".to_string(), "run".to_string()];
    let mut it = command.into_iter().skip(2);
    let mut named = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--handle" | "--select" | "--goal" | "--knob" | "--pack" | "--proposed-by"
            | "--proposal" => {
                let value = it.next().ok_or_else(|| {
                    Reply::error(400, format!("workflow run {arg} takes a value"))
                })?;
                out.push(arg);
                out.push(value);
            }
            "--json" => out.push(arg),
            w if w.starts_with('-') => {
                return Err(Reply::error(
                    400,
                    format!("workflow run does not take {w} at the door"),
                ));
            }
            _ if !named => {
                named = true;
                out.push(arg);
            }
            w => {
                return Err(Reply::error(
                    400,
                    format!("workflow run names one workflow, not {w}"),
                ));
            }
        }
    }
    if !named {
        return Err(Reply::error(400, "workflow run names the workflow"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTOR: &str = r#"name: stand-in
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/stand-in@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
command-line: "run [Manifest] [OutputLocation]"
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  units: apart
  outputs:
    - {id: out, kind: image, path-template: "stack-{stack}/out.txt", media-type: text/plain}
"#;

    const WORKFLOW: &str = r#"nils_workflow: 1
name: two
title: Two steps
what: "a into b"
steps:
  - {id: a, pipeline: stand-in}
  - {id: b, pipeline: stand-in}
links:
  - {from: a.out, to: b.prev}
"#;

    /// A bids stand-in with two pick roles: a step fed through its `flair`
    /// role waits for the whole step before (after the third review,
    /// finding 1).
    const BIDS_DESCRIPTOR: &str = r#"name: stand-in-bids
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/stand-in-bids@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
command-line: "run [InputDataset] [OutputLocation]"
x-nils:
  analysis-level: session
  input: {layout: bids, roles: [t1w, flair]}
  outputs:
    - {id: seen, kind: output, path-template: "sub-{subject}/ses-{session}/anat/*_desc-seen_T2w.nii.gz"}
"#;

    const ROLE_WORKFLOW: &str = r#"nils_workflow: 1
name: into-a-role
title: An image into a role
what: "a's image as b's flair"
steps:
  - {id: a, pipeline: stand-in}
  - {id: b, pipeline: stand-in-bids}
links:
  - {from: a.out, to: b.flair}
"#;

    /// A registry with the stand-in pipeline and the two-step workflow.
    fn lab(name: &str) -> (nils_dicom::synth::TempDir, Home, rows::Workflow) {
        lab_of(name, &[DESCRIPTOR], WORKFLOW)
    }

    /// A registry with these descriptors in the catalog and this workflow.
    fn lab_of(
        name: &str,
        descriptors: &[&str],
        workflow: &str,
    ) -> (nils_dicom::synth::TempDir, Home, rows::Workflow) {
        let dir = nils_dicom::synth::TempDir::new(name);
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        let mut registry = home
            .init(&nils_registry::InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::pseudonym::Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
                session_scheme: None,
            })
            .unwrap();
        for text in descriptors {
            let d = nils_pipeline::descriptor::parse(text).unwrap();
            let digest = d.digest();
            prows::add(
                registry.store(),
                &prows::New {
                    name: &d.name,
                    tool_version: "1",
                    descriptor: &d.document,
                    descriptor_digest: &digest,
                    image: &format!("example.org/{}", d.name),
                    image_digest: "sha256:aaaa",
                    layout: d.layout.name(),
                    level: d.level.name(),
                    added_by: "test",
                    added_at: "2026-10-25T00:00:00Z",
                    pack: None,
                    pack_digest: None,
                    version_digest: &digest,
                },
            )
            .unwrap();
        }
        let w = doc::parse(workflow).unwrap();
        let (w, _) = rows::add(
            registry.store(),
            &w.name,
            &w.document,
            &w.digest(),
            "test",
            "2026-10-25T00:00:00Z",
            None,
        )
        .unwrap();
        (dir, home, w)
    }

    /// Run the workflow with a stand-in runner, in a thread, and fail
    /// unless it ends within the deadline: a chain that never ends is the
    /// failure these tests show.
    fn run_bounded(home: &Home, w: &rows::Workflow, runner: Runner) -> rows::Run {
        let (tx, rx) = std::sync::mpsc::channel();
        let home2 = home.clone();
        let w2 = w.clone();
        std::thread::spawn(move || {
            let mut registry = home2.open().unwrap();
            let scope = Scope {
                handle: 1,
                selection: None,
                main: None,
                pack: "mri".into(),
                pack_dir: None,
            };
            let r = start_with(
                &home2,
                &mut registry,
                &w2,
                &scope,
                &Person::default(),
                None,
                runner,
            );
            let _ = tx.send(r);
        });
        let id = rx
            .recv_timeout(Duration::from_secs(60))
            .expect("the workflow run did not end within 60 s")
            .unwrap_or_else(|e| panic!("the workflow run: {}", e.message));
        let mut registry = home.open().unwrap();
        rows::run(registry.store(), id).unwrap().unwrap()
    }

    fn done_without_a_run(_: &Home, _: crate::pipelines::RunArgs) -> Result<(), Exit> {
        Ok(())
    }

    fn blows_up(_: &Home, _: crate::pipelines::RunArgs) -> Result<(), Exit> {
        panic!("the stand-in run blew up")
    }

    fn no_runtime(_: &Home, _: crate::pipelines::RunArgs) -> Result<(), Exit> {
        Err(fail("no container runtime here"))
    }

    /// A runner that claims its pipeline run, as `nils run` does before
    /// its containers, and then unwinds: the run row is left `running`.
    fn claims_then_blows_up(home: &Home, args: crate::pipelines::RunArgs) -> Result<(), Exit> {
        let c = args.chain.as_ref().expect("the step's chain");
        let mut registry = home.open().unwrap();
        let p = prows::resolve(registry.store(), "stand-in")
            .unwrap()
            .unwrap();
        let run = prows::start(
            registry.store(),
            &prows::NewRun {
                pipeline_id: p.id,
                job_id: None,
                handle_id: Some(1),
                selection: None,
                params: &json!({}),
                runtime: "stand-in",
                runtime_version: "1",
                host: "lab",
                device: "cpu",
                model_ids: &[],
                label_set_id: None,
                place_id: None,
                scratch_place_id: None,
                principal: "test",
                actor: None,
                started_at: "2026-10-25T00:00:00Z",
            },
        )
        .unwrap();
        record_step_run(registry.store(), c.workflow_run, c.step, run).unwrap();
        drop(registry);
        panic!("the stand-in run blew up after claiming run {run}")
    }

    /// After the third review (finding 1): a step fed through a pick role
    /// waits for the whole step before, as one fed through a typed input
    /// does, since a run binds both as it starts; the run's steps say so
    /// (`waits_whole`).
    #[test]
    fn a_role_fed_step_waits_for_the_whole_step_before() {
        let (_dir, home, w) = lab_of(
            "wf-role-whole",
            &[DESCRIPTOR, BIDS_DESCRIPTOR],
            ROLE_WORKFLOW,
        );
        let r = run_bounded(&home, &w, done_without_a_run);
        let steps = r.steps.as_array().unwrap();
        assert_eq!(steps[1]["waits_for"], json!(["a"]), "{steps:?}");
        assert_eq!(steps[1]["waits_whole"], json!(["a"]), "{steps:?}");
        // and the wait itself: a run before still running holds it
        let s = vec![
            json!({"step": 0, "id": "a", "status": "running", "run": 7}),
            json!({"step": 1, "id": "b", "status": "queued"}),
        ];
        let mut running = |_: i64| Ok(Some("running".to_string()));
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &[], &mut running).unwrap(),
            Wait::NotYet
        );
    }

    /// After the third review (finding 2): a step thread that unwinds after
    /// claiming its run leaves no run row `running`: the run is concluded
    /// failed in the thread's words, the step is failed, and the workflow
    /// run ends.
    #[test]
    fn a_step_whose_thread_dies_leaves_no_running_run() {
        let (_dir, home, w) = lab("wf-run-left-running");
        let r = run_bounded(&home, &w, claims_then_blows_up);
        let steps = r.steps.as_array().unwrap();
        // step b chains per unit to a's run, so it starts once a's run is
        // claimed and dies the same way: both runs are concluded
        let mut registry = home.open().unwrap();
        for s in steps {
            assert_eq!(s["status"], "failed", "{steps:?}");
            let run = s["run"].as_i64().expect("the claimed run");
            let pr = prows::run(registry.store(), run).unwrap().unwrap();
            assert_eq!(pr.status, "failed", "{pr:?}");
            assert!(
                pr.error
                    .as_deref()
                    .unwrap()
                    .contains("the stand-in run blew up after claiming"),
                "{pr:?}"
            );
            assert!(pr.finished_at.is_some(), "{pr:?}");
        }
        assert!(
            prows::runs_in(registry.store(), "running")
                .unwrap()
                .is_empty(),
            "no run row is left running"
        );
        assert_eq!(r.status, "failed", "{r:?}");
    }

    /// After the third review (finding 2): a run before that is
    /// `interrupted`, or whose step is over while its row still says
    /// running, is over to a step read whole; and (finding 9) a step whose
    /// links all feed an optional input goes on where that step ended
    /// without a run, recording what fed nothing.
    #[test]
    fn an_interrupted_or_concluded_step_before_is_over_and_an_optional_one_may_feed_nothing() {
        let b = json!({"step": 1, "id": "b", "status": "queued"});
        let mut interrupted = |_: i64| Ok(Some("interrupted".to_string()));
        let mut running = |_: i64| Ok(Some("running".to_string()));
        let s = vec![
            json!({"step": 0, "id": "a", "status": "running", "run": 7}),
            b.clone(),
        ];
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &[], &mut interrupted).unwrap(),
            Wait::Ready {
                after: vec![(0, 7)],
                unfed: vec![]
            }
        );
        // the step is over though its run row still says running
        let s = vec![
            json!({"step": 0, "id": "a", "status": "failed", "run": 7, "error": "boom"}),
            b.clone(),
        ];
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &[], &mut running).unwrap(),
            Wait::Ready {
                after: vec![(0, 7)],
                unfed: vec![]
            }
        );
        // an optional input's step ended without a run: not a skip
        let s = vec![
            json!({"step": 0, "id": "a", "status": "failed", "run": null, "error": "boom"}),
            b.clone(),
        ];
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &[0], &mut running).unwrap(),
            Wait::Ready {
                after: vec![],
                unfed: vec![(
                    0,
                    "fed by no step for this unit: step 1 (a) ended failed without a run: boom"
                        .into()
                )]
            }
        );
        let s = vec![
            json!({"step": 0, "id": "a", "status": "done", "run": null}),
            b,
        ];
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &[0], &mut running).unwrap(),
            Wait::Ready {
                after: vec![],
                unfed: vec![(
                    0,
                    "fed by no step for this unit: step 1 (a) ended done without a run".into()
                )]
            }
        );
        // a required one still skips
        assert!(matches!(
            wait_of(&s, &[0], &[0], &[0], &[], &mut running).unwrap(),
            Wait::Skipped(_)
        ));
    }

    /// After review (finding 4): a step before that ended without a run is
    /// terminal to the step after it: one linked to it is skipped with the
    /// reason, one only ordered after it goes on; a step still to run, or a
    /// step read whole whose run still runs, is waited for.
    #[test]
    fn a_step_before_that_ended_without_a_run_is_terminal() {
        let steps = |a: Value| vec![a, json!({"step": 1, "id": "b", "status": "queued"})];
        let mut running = |_: i64| Ok(Some("running".to_string()));
        let mut over = |_: i64| Ok(Some("done".to_string()));
        // done, no run, linked: skipped and said why
        let s = steps(json!({"step": 0, "id": "a", "status": "done", "run": null}));
        assert_eq!(
            wait_of(&s, &[0], &[0], &[], &[], &mut over).unwrap(),
            Wait::Skipped(
                "skipped: step 1 (a) ended done without a run, and this step reads its results"
                    .into()
            )
        );
        // done, no run, only ordered after: nothing to chain to
        assert_eq!(
            wait_of(&s, &[0], &[], &[], &[], &mut over).unwrap(),
            Wait::Ready {
                after: vec![],
                unfed: vec![]
            }
        );
        // failed with no run: skipped with its words, linked or not
        let s =
            steps(json!({"step": 0, "id": "a", "status": "failed", "run": null, "error": "boom"}));
        assert_eq!(
            wait_of(&s, &[0], &[], &[], &[], &mut over).unwrap(),
            Wait::Skipped("skipped: step 1 (a) failed: boom".into())
        );
        // not yet started, or running without a run yet: waited for
        for status in ["queued", "running"] {
            let s = steps(json!({"step": 0, "id": "a", "status": status, "run": null}));
            assert_eq!(
                wait_of(&s, &[0], &[0], &[], &[], &mut over).unwrap(),
                Wait::NotYet
            );
        }
        // a run claimed: chained to it per unit; read whole, waited for
        let s = steps(json!({"step": 0, "id": "a", "status": "running", "run": 7}));
        assert_eq!(
            wait_of(&s, &[0], &[0], &[], &[], &mut running).unwrap(),
            Wait::Ready {
                after: vec![(0, 7)],
                unfed: vec![]
            }
        );
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &[], &mut running).unwrap(),
            Wait::NotYet
        );
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &[], &mut over).unwrap(),
            Wait::Ready {
                after: vec![(0, 7)],
                unfed: vec![]
            }
        );
    }

    /// After review (finding 4): a step whose run ends without a run
    /// recorded is done, and the step linked to it ends skipped, so the
    /// workflow run ends (it polled forever before).
    #[test]
    fn a_step_done_without_a_run_lets_the_chain_end() {
        let (_dir, home, w) = lab("wf-done-no-run");
        let r = run_bounded(&home, &w, done_without_a_run);
        let steps = r.steps.as_array().unwrap();
        assert_eq!(steps[0]["status"], "done", "{steps:?}");
        assert!(steps[0]["run"].is_null(), "{steps:?}");
        assert_eq!(steps[1]["status"], "skipped", "{steps:?}");
        assert!(
            steps[1]["error"]
                .as_str()
                .unwrap()
                .contains("ended done without a run"),
            "{steps:?}"
        );
        assert_eq!(r.status, "partial", "{r:?}");
        assert!(r.finished_at.is_some());
    }

    /// After review (finding 4): a step thread that ends in an error or a
    /// panic records the step as failed, in the error's words, and the
    /// steps after it are skipped with the reason; the run ends failed.
    #[test]
    fn a_step_that_errs_or_panics_is_recorded_failed_and_the_run_ends() {
        let (_dir, home, w) = lab("wf-step-panics");
        let r = run_bounded(&home, &w, blows_up);
        let steps = r.steps.as_array().unwrap();
        assert_eq!(steps[0]["status"], "failed", "{steps:?}");
        assert!(
            steps[0]["error"]
                .as_str()
                .unwrap()
                .contains("the stand-in run blew up"),
            "{steps:?}"
        );
        assert_eq!(steps[1]["status"], "skipped", "{steps:?}");
        assert!(
            steps[1]["error"]
                .as_str()
                .unwrap()
                .starts_with("skipped: step 1 (a) failed: the step's thread panicked"),
            "{steps:?}"
        );
        assert_eq!(r.status, "failed", "{r:?}");
        assert!(r.error.as_deref().unwrap().contains("blew up"), "{r:?}");

        let (_dir, home, w) = lab("wf-step-errs");
        let r = run_bounded(&home, &w, no_runtime);
        let steps = r.steps.as_array().unwrap();
        assert_eq!(steps[0]["status"], "failed", "{steps:?}");
        assert_eq!(steps[0]["error"], "no container runtime here", "{steps:?}");
        assert_eq!(steps[1]["status"], "skipped", "{steps:?}");
        assert_eq!(r.status, "failed", "{r:?}");
    }

    /// After review (finding 3): each step is its own row, written by
    /// (run, step): two steps recording at once never lose each other's
    /// words, and a run's steps read back in order.
    #[test]
    fn steps_are_rows_written_by_run_and_step() {
        let (_dir, home, w) = lab("wf-step-rows");
        let mut registry = home.open().unwrap();
        let id = rows::start(
            registry.store(),
            &rows::NewRun {
                workflow_id: w.id,
                job_id: None,
                handle_id: Some(1),
                selection: None,
                person: &json!({}),
                steps: &json!([{"id": "a", "pipeline": "stand-in@1"}, {"id": "b", "pipeline": "stand-in@1"}]),
                principal: "test",
                actor: None,
                started_at: "2026-10-25T00:00:00Z",
            },
        )
        .unwrap();
        drop(registry);
        // every thread writes its own step many times, with a store of
        // its own; nothing is read, changed and written back
        let threads: Vec<_> = (0..2usize)
            .map(|step| {
                let home = home.clone();
                std::thread::spawn(move || {
                    let mut registry = home.open().unwrap();
                    for n in 0..20i64 {
                        rows::update_step(
                            registry.store(),
                            id,
                            step,
                            &rows::StepUpdate {
                                run_id: Some(100 * step as i64 + n),
                                status: Some("running"),
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    }
                    record_end(
                        registry.store(),
                        id,
                        step,
                        "done",
                        None,
                        Some(&json!({"total": step + 1})),
                        None,
                    )
                    .unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let mut registry = home.open().unwrap();
        let r = rows::run(registry.store(), id).unwrap().unwrap();
        let steps = r.steps.as_array().unwrap();
        assert_eq!(steps.len(), 2);
        for (i, s) in steps.iter().enumerate() {
            assert_eq!(s["step"], i, "{s}");
            assert_eq!(s["status"], "done", "{s}");
            assert_eq!(s["run"], 100 * i as i64 + 19, "{s}");
            assert_eq!(s["units"]["total"], i + 1, "{s}");
            assert_eq!(s["pipeline"], "stand-in@1", "{s}");
            assert!(s["finished_at"].is_string(), "{s}");
        }
        assert_eq!(steps[0]["id"], "a");
        assert_eq!(steps[1]["id"], "b");
        assert!(rows::step(registry.store(), id, 2).unwrap().is_none());
        finish(registry.store(), id, &[], false).unwrap();
        let r = rows::run(registry.store(), id).unwrap().unwrap();
        assert_eq!(r.status, "done");
        assert_eq!(r.summary["units"]["total"], 3);
    }

    /// After review (finding 2 of the second review): a workflow run asked
    /// to stop ends cancelled whatever its steps say, and the steps not yet
    /// started end cancelled with the reason once the flag is set.
    #[test]
    fn a_cancel_ends_the_run_cancelled() {
        let (_dir, home, w) = lab("wf-cancel-status");
        let mut registry = home.open().unwrap();
        let id = rows::start(
            registry.store(),
            &rows::NewRun {
                workflow_id: w.id,
                job_id: None,
                handle_id: Some(1),
                selection: None,
                person: &json!({}),
                steps: &json!([{"id": "a", "pipeline": "stand-in@1"}, {"id": "b", "pipeline": "stand-in@1"}]),
                principal: "test",
                actor: None,
                started_at: "2026-10-25T00:00:00Z",
            },
        )
        .unwrap();
        record_end(
            registry.store(),
            id,
            0,
            "done",
            None,
            Some(&json!({"total": 4, "succeeded": 4})),
            None,
        )
        .unwrap();
        record_end(
            registry.store(),
            id,
            1,
            "cancelled",
            Some("cancelled: the workflow run was cancelled before this step started"),
            None,
            None,
        )
        .unwrap();
        finish(registry.store(), id, &[], true).unwrap();
        let r = rows::run(registry.store(), id).unwrap().unwrap();
        assert_eq!(r.status, "cancelled", "{r:?}");
        assert_eq!(r.summary["steps_done"], 1, "{r:?}");
        let progress = progress_of(registry.store(), id).unwrap();
        assert_eq!(progress["steps_total"], 2, "{progress}");
        assert_eq!(progress["steps_done"], 1, "{progress}");
        assert_eq!(progress["steps_over"], 2, "{progress}");
        // without the cancel, the same steps read partial
        let id2 = rows::start(
            registry.store(),
            &rows::NewRun {
                workflow_id: w.id,
                job_id: None,
                handle_id: Some(1),
                selection: None,
                person: &json!({}),
                steps: &json!([{"id": "a", "pipeline": "stand-in@1"}, {"id": "b", "pipeline": "stand-in@1"}]),
                principal: "test",
                actor: None,
                started_at: "2026-10-25T00:00:00Z",
            },
        )
        .unwrap();
        record_end(registry.store(), id2, 0, "done", None, None, None).unwrap();
        record_end(registry.store(), id2, 1, "cancelled", None, None, None).unwrap();
        finish(registry.store(), id2, &[], false).unwrap();
        assert_eq!(
            rows::run(registry.store(), id2).unwrap().unwrap().status,
            "partial"
        );
    }
}
