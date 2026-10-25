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

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use nils_pipeline::workflow as doc;
use nils_registry::home::Home;
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
    let job_id = nils_registry::job::adopted_job();
    nils_registry::job::disown_adoption();
    let id = start(home, &mut registry, &w, &scope, &person, job_id, &plan)?;
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
    if r.status == "done" {
        Ok(())
    } else {
        Err(fail(format!(
            "workflow run {id} ended {}: {}",
            r.status,
            r.error.unwrap_or_default()
        )))
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
    plan: &Value,
) -> Result<i64, Exit> {
    start_with(
        home,
        registry,
        w,
        scope,
        person,
        job_id,
        plan,
        crate::pipelines::run_command,
    )
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
    /// Of those, the steps whose files it reads as a typed input: waited
    /// for whole.
    whole: Vec<usize>,
    /// Every step before it: the order of the document is the order of
    /// the run.
    before: Vec<usize>,
    handle: i64,
    pack: String,
    pack_dir: Option<PathBuf>,
}

/// [`start`] with the runner named: every step's parameters are checked
/// before anything is made, every step thread records how it ended, and
/// the run is finished whatever the threads did.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_with(
    home: &Home,
    registry: &mut Registry,
    w: &rows::Workflow,
    scope: &Scope,
    person: &Person,
    job_id: Option<i64>,
    plan: &Value,
    runner: Runner,
) -> Result<i64, Exit> {
    let d = doc::from_value(w.document.clone()).map_err(|e| fail(format!("{}: {e}", w.label())))?;
    let resolved = crate::workflows::resolve_steps(registry.store(), &d);
    // every step's parameters first: a knob a person may not set is
    // refused before a row is made or a thread started
    let params: Vec<Vec<(String, String)>> = d
        .steps
        .iter()
        .map(|s| crate::workflows::step_params(&d, s, person))
        .collect::<Result<_, _>>()
        .map_err(usage)?;
    // the links that feed a typed input: a step so fed waits for the whole
    // step before, since a run binds its derivative inputs at its start
    let fed_whole: Vec<Vec<String>> = d
        .steps
        .iter()
        .zip(&resolved)
        .map(|(s, r)| {
            let Some(desc) = r.descriptor.as_ref() else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for l in d.links.iter().filter(|l| l.to_step == s.id) {
                if desc.inputs.iter().any(|t| t.id == l.input) && !out.contains(&l.from_step) {
                    out.push(l.from_step.clone());
                }
            }
            out
        })
        .collect();
    let steps: Vec<Value> = d
        .steps
        .iter()
        .zip(&resolved)
        .enumerate()
        .map(|(i, (s, r))| {
            json!({
                "step": i, "id": s.id, "of": s.of, "name": s.name, "reference": s.reference(),
                "pipeline": r.pipeline.as_ref().map(|p| p.label()),
                "pipeline_id": r.pipeline.as_ref().map(|p| p.id),
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
            person: &person.doc(),
            steps: &json!(steps),
            principal: &who,
            actor: Some(&actor),
            started_at: &now,
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    let _ = plan;

    // one thread a step
    let position = |id: &str| d.steps.iter().position(|q| q.id == id);
    let mut handles = Vec::new();
    for (i, s) in d.steps.iter().enumerate() {
        let run = StepRun {
            home: home.clone(),
            workflow_run: id,
            step: i,
            s: s.clone(),
            label: resolved[i].pipeline.as_ref().map(|p| p.label()),
            params: params[i].clone(),
            goal: s.goal.clone().or_else(|| person.goal.clone()),
            waits_for: d.fed_by(&s.id).iter().filter_map(|q| position(q)).collect(),
            whole: fed_whole[i].iter().filter_map(|q| position(q)).collect(),
            before: (0..i).collect(),
            handle: scope.handle,
            pack: scope.pack.clone(),
            pack_dir: scope.pack_dir.clone(),
        };
        handles.push(std::thread::spawn(move || run_step(&run, runner)));
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
    finish(registry.store(), id, &errors).map_err(fail)?;
    Ok(id)
}

/// What the steps before say to a step about to start.
#[derive(Debug, PartialEq)]
enum Wait {
    /// Every step before is far enough: the runs this step chains to per
    /// unit.
    Ready(Vec<i64>),
    /// A step before has not yet made its run, or one read whole still
    /// runs.
    NotYet,
    /// A step before ended without a run this step could go on from: the
    /// words this step is skipped with (section 8.4's rule).
    Skipped(String),
}

/// Decide, from the steps before as recorded, whether a step may start:
/// `pipeline_status` answers a run's status (none where the run is gone).
fn wait_of(
    steps: &[Value],
    before: &[usize],
    waits_for: &[usize],
    whole: &[usize],
    pipeline_status: &mut dyn FnMut(i64) -> Result<Option<String>, String>,
) -> Result<Wait, String> {
    let mut after: Vec<i64> = Vec::new();
    let mut ready = true;
    for &k in before {
        let st = steps.get(k).cloned().unwrap_or(Value::Null);
        let status = st["status"].as_str().unwrap_or("queued");
        let name = format!("step {} ({})", k + 1, st["id"].as_str().unwrap_or_default());
        match st["run"].as_i64() {
            Some(run) => {
                if waits_for.contains(&k) {
                    after.push(run);
                }
                if whole.contains(&k) {
                    let pr = pipeline_status(run)?;
                    if pr.is_some_and(|s| matches!(s.as_str(), "running" | "interrupted")) {
                        ready = false;
                    }
                }
            }
            None if !TERMINAL.contains(&status) => ready = false,
            // the step before ended with no run: nothing of it ran
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
        Ok(Wait::Ready(after))
    } else {
        Ok(Wait::NotYet)
    }
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
    let after = loop {
        let r = rows::run(registry.store(), p.workflow_run)
            .map_err(err)?
            .ok_or_else(|| format!("no workflow run {}", p.workflow_run))?;
        let steps = r.steps.as_array().cloned().unwrap_or_default();
        let store = registry.store();
        let mut status_of = |run: i64| -> Result<Option<String>, String> {
            Ok(prows::run(store, run).map_err(err)?.map(|pr| pr.status))
        };
        match wait_of(&steps, &p.before, &p.waits_for, &p.whole, &mut status_of)? {
            Wait::Ready(after) => break after,
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
    argv.push("--json".into());
    let mut args = StepCli::try_parse_from(&argv)
        .map_err(|e| format!("the step's run arguments: {e}"))?
        .run;
    args.chain = Some(crate::pipelines::Chain {
        workflow_run: p.workflow_run,
        step: p.step,
        after,
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

/// Close the workflow run: its status from its steps', and a summary that
/// sums each step's units.
fn finish(store: &mut Store, id: i64, errors: &[String]) -> Result<(), String> {
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
    let status = if !errors.is_empty() {
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
            "--handle" | "--select" | "--goal" | "--knob" | "--pack" => {
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

    /// A registry with the stand-in pipeline and the two-step workflow.
    fn lab(name: &str) -> (nils_dicom::synth::TempDir, Home, rows::Workflow) {
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
        let d = nils_pipeline::descriptor::parse(DESCRIPTOR).unwrap();
        let digest = d.digest();
        prows::add(
            registry.store(),
            &prows::New {
                name: "stand-in",
                tool_version: "1",
                descriptor: &d.document,
                descriptor_digest: &digest,
                image: "example.org/stand-in",
                image_digest: "sha256:aaaa",
                layout: "stacks",
                level: "stack",
                added_by: "test",
                added_at: "2026-10-25T00:00:00Z",
                pack: None,
                pack_digest: None,
                version_digest: &digest,
            },
        )
        .unwrap();
        let w = doc::parse(WORKFLOW).unwrap();
        let (w, _) = rows::add(
            registry.store(),
            "two",
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
                &json!({}),
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
            wait_of(&s, &[0], &[0], &[], &mut over).unwrap(),
            Wait::Skipped(
                "skipped: step 1 (a) ended done without a run, and this step reads its results"
                    .into()
            )
        );
        // done, no run, only ordered after: nothing to chain to
        assert_eq!(
            wait_of(&s, &[0], &[], &[], &mut over).unwrap(),
            Wait::Ready(vec![])
        );
        // failed with no run: skipped with its words, linked or not
        let s =
            steps(json!({"step": 0, "id": "a", "status": "failed", "run": null, "error": "boom"}));
        assert_eq!(
            wait_of(&s, &[0], &[], &[], &mut over).unwrap(),
            Wait::Skipped("skipped: step 1 (a) failed: boom".into())
        );
        // not yet started, or running without a run yet: waited for
        for status in ["queued", "running"] {
            let s = steps(json!({"step": 0, "id": "a", "status": status, "run": null}));
            assert_eq!(
                wait_of(&s, &[0], &[0], &[], &mut over).unwrap(),
                Wait::NotYet
            );
        }
        // a run claimed: chained to it per unit; read whole, waited for
        let s = steps(json!({"step": 0, "id": "a", "status": "running", "run": 7}));
        assert_eq!(
            wait_of(&s, &[0], &[0], &[], &mut running).unwrap(),
            Wait::Ready(vec![7])
        );
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &mut running).unwrap(),
            Wait::NotYet
        );
        assert_eq!(
            wait_of(&s, &[0], &[0], &[0], &mut over).unwrap(),
            Wait::Ready(vec![7])
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
        finish(registry.store(), id, &[]).unwrap();
        let r = rows::run(registry.store(), id).unwrap().unwrap();
        assert_eq!(r.status, "done");
        assert_eq!(r.summary["units"]["total"], 3);
    }
}
