// SPDX-License-Identifier: AGPL-3.0-only

//! A workflow run (Wave 7b A7, section 8.4): the steps run as pipeline runs
//! over one frozen scope, each step's units going on as soon as the same
//! unit of the step before is over, without waiting for the whole
//! selection; a unit skipped or failed at one step is skipped downstream
//! with the same reason; a step fed by another's files (a typed derivative
//! input) starts when that step is done, since a run binds its derivative
//! inputs when it starts; the run's line sums each step's units.

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

fn err(e: nils_registry::Error) -> String {
    e.to_string()
}

/// A step of a workflow run learned its pipeline run: written as soon as
/// the run is claimed, so the step after it can wait on it per unit.
pub(crate) fn record_step_run(
    store: &mut Store,
    workflow_run: i64,
    step: usize,
    run_id: i64,
) -> Result<(), String> {
    let r = rows::run(store, workflow_run)
        .map_err(err)?
        .ok_or_else(|| format!("no workflow run {workflow_run}"))?;
    let mut steps = r.steps;
    if let Some(s) = steps.get_mut(step) {
        s["run"] = json!(run_id);
        s["status"] = json!("running");
    }
    rows::set_steps(store, workflow_run, &steps).map_err(err)
}

fn set_step(
    store: &mut Store,
    workflow_run: i64,
    step: usize,
    fields: Value,
) -> Result<(), String> {
    let r = rows::run(store, workflow_run)
        .map_err(err)?
        .ok_or_else(|| format!("no workflow run {workflow_run}"))?;
    let mut steps = r.steps;
    if let Some(s) = steps.get_mut(step)
        && let (Some(o), Some(f)) = (s.as_object_mut(), fields.as_object())
    {
        for (k, v) in f {
            o.insert(k.clone(), v.clone());
        }
    }
    rows::set_steps(store, workflow_run, &steps).map_err(err)
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
    // the worker's job this command runs as, where it does; the steps claim
    // jobs of their own, so the variable is taken off before they start
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
    let d = doc::from_value(w.document.clone()).map_err(|e| fail(format!("{}: {e}", w.label())))?;
    let resolved = crate::workflows::resolve_steps(registry.store(), &d);
    // the links that feed a typed input: a step so fed waits for the whole
    // step before, since a run binds its derivative inputs at its start
    let fed_whole: Vec<Vec<String>> = d
        .steps
        .iter()
        .map(|s| {
            let Some(desc) = resolved[d.steps.iter().position(|q| q.id == s.id).unwrap()]
                .descriptor
                .as_ref()
            else {
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
                "run": Value::Null, "status": "queued",
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
    let mut handles = Vec::new();
    for (i, s) in d.steps.iter().enumerate() {
        let home = home.clone();
        let step = s.clone();
        let label = resolved[i].pipeline.as_ref().map(|p| p.label());
        let params = crate::workflows::step_params(&d, s, person).map_err(usage)?;
        let goal = s.goal.clone().or_else(|| person.goal.clone());
        let waits_for: Vec<usize> = d
            .fed_by(&s.id)
            .iter()
            .filter_map(|id| d.steps.iter().position(|q| &q.id == id))
            .collect();
        let whole: Vec<usize> = fed_whole[i]
            .iter()
            .filter_map(|id| d.steps.iter().position(|q| &q.id == id))
            .collect();
        // a step waits for every step before it that it does not wait for
        // by a link too: the order of the document is the order of the run
        let before: Vec<usize> = (0..i).collect();
        let handle = scope.handle;
        let pack = scope.pack.clone();
        let pack_dir = scope.pack_dir.clone();
        handles.push(std::thread::spawn(move || -> Result<(), String> {
            run_step(
                &home,
                id,
                i,
                &step,
                label.as_deref(),
                &params,
                goal.as_deref(),
                &waits_for,
                &whole,
                &before,
                handle,
                &pack,
                pack_dir.as_deref(),
            )
        }));
    }
    let mut errors: Vec<String> = Vec::new();
    for h in handles {
        match h.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(e),
            Err(_) => errors.push("a step's thread ended in a panic".into()),
        }
    }
    finish(registry.store(), id, &errors).map_err(fail)?;
    Ok(id)
}

/// One step: wait for the runs it is chained to, run its pipeline over the
/// handle with the chain set, and record how it ended.
#[allow(clippy::too_many_arguments)]
fn run_step(
    home: &Home,
    workflow_run: i64,
    step: usize,
    s: &doc::Step,
    label: Option<&str>,
    params: &[(String, String)],
    goal: Option<&str>,
    waits_for: &[usize],
    whole: &[usize],
    before: &[usize],
    handle: i64,
    pack: &str,
    pack_dir: Option<&std::path::Path>,
) -> Result<(), String> {
    let mut registry = crate::open(home).map_err(|e| e.message)?;
    let Some(label) = label else {
        set_step(
            registry.store(),
            workflow_run,
            step,
            json!({"status": "refused", "error": format!("{} {} is not in the catalog", s.of, s.name)}),
        )?;
        return Ok(());
    };
    // the runs of the steps before: chained per unit to those a link feeds
    // from, and started after those whose files it reads are done
    let mut after: Vec<i64> = Vec::new();
    loop {
        let r = rows::run(registry.store(), workflow_run)
            .map_err(err)?
            .ok_or_else(|| format!("no workflow run {workflow_run}"))?;
        let mut ready = true;
        after.clear();
        for &k in before {
            let st = &r.steps[k];
            let status = st["status"].as_str().unwrap_or("queued");
            match st["run"].as_i64() {
                Some(run) => {
                    if waits_for.contains(&k) {
                        after.push(run);
                    }
                    if whole.contains(&k) {
                        let pr = prows::run(registry.store(), run).map_err(err)?;
                        if pr.is_some_and(|pr| {
                            matches!(pr.status.as_str(), "running" | "interrupted")
                        }) {
                            ready = false;
                        }
                    }
                }
                None if matches!(status, "refused" | "failed" | "skipped") => {
                    // nothing of the step before ran: this step is skipped
                    // with the same reason
                    set_step(
                        registry.store(),
                        workflow_run,
                        step,
                        json!({
                            "status": "skipped",
                            "error": format!(
                                "skipped: step {} ({}) {status}: {}",
                                k + 1,
                                r.steps[k]["id"].as_str().unwrap_or_default(),
                                r.steps[k]["error"].as_str().unwrap_or("no reason recorded")
                            ),
                        }),
                    )?;
                    return Ok(());
                }
                None => ready = false,
            }
        }
        if ready {
            break;
        }
        std::thread::sleep(POLL);
    }
    drop(registry);
    // the step's run, as `nils run` would take it
    let mut argv: Vec<String> = vec![
        "run".into(),
        label.to_string(),
        "--handle".into(),
        handle.to_string(),
        "--pack".into(),
        pack.to_string(),
    ];
    if let Some(d) = pack_dir {
        argv.extend(["--pack-dir".to_string(), d.display().to_string()]);
    }
    if let Some(g) = goal {
        argv.extend(["--goal".to_string(), g.to_string()]);
    }
    for (k, v) in &s.switches {
        let v = match v {
            Value::String(t) => t.clone(),
            other => other.to_string(),
        };
        argv.extend(["--switch".to_string(), format!("{k}={v}")]);
    }
    for (k, v) in params {
        argv.extend(["--param".to_string(), format!("{k}={v}")]);
    }
    argv.push("--json".into());
    let mut args = StepCli::try_parse_from(&argv)
        .map_err(|e| format!("the step's run arguments: {e}"))?
        .run;
    args.chain = Some(crate::pipelines::Chain {
        workflow_run,
        step,
        after,
    });
    let outcome = crate::pipelines::run_command(home, args);
    let mut registry = crate::open(home).map_err(|e| e.message)?;
    let r = rows::run(registry.store(), workflow_run)
        .map_err(err)?
        .ok_or_else(|| format!("no workflow run {workflow_run}"))?;
    let run_id = r.steps[step]["run"].as_i64();
    let pr = match run_id {
        Some(id) => prows::run(registry.store(), id).map_err(err)?,
        None => None,
    };
    let fields = match (outcome, pr) {
        (_, Some(pr)) => json!({
            "status": pr.status, "error": pr.error,
            "units": pr.summary["units"],
            "finished_at": pr.finished_at,
        }),
        (Ok(()), None) => json!({"status": "done"}),
        (Err(e), None) => {
            json!({"status": if e.code == crate::USAGE { "refused" } else { "failed" }, "error": e.message})
        }
    };
    set_step(registry.store(), workflow_run, step, fields)?;
    Ok(())
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
