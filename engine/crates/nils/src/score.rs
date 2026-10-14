// SPDX-License-Identifier: AGPL-3.0-only

//! Scoring at arrival (record 55 E5, spec section 10.4): the last step of a
//! dataset's bring-in, `score @dataset --name <batch>`. For each operation
//! of its own (body part, then post-contrast) whose run here takes the
//! chosen model and whose pipeline declares `x-nils.on-arrival`, it freezes
//! the stacks the batch brought (those whose first batch is the chain's)
//! into a handle and queues the step's run over them, as the step's door
//! makes it, under the principal, grants and detail the chain recorded. The
//! run goes to the pipeline lane, so a long score never holds up a digest,
//! and the dataset's step shows it as it shows a run its door queued.
//!
//! Post-contrast reads the body part's answers (it answers only on brain),
//! so where both are queued the post-contrast step is queued after the
//! body part's run ends done: `score ... --step post_contrast` is that
//! run's `then`.
//!
//! A step it cannot run is skipped and the result says why (`skipped`,
//! each with the step, a reason a client tells apart and the words): its
//! pipeline does not score at arrival, nothing here runs it (no pipeline,
//! no model, no runtime, no working place), the machine lacks what it
//! needs, or the chain was queued by a caller who may not run a pipeline.
//! The bring-in never fails because of it.

use nils_registry::Registry;
use nils_registry::home::Home;
use nils_registry::job;
use nils_registry::store::{Cell, Row, Store};
use serde_json::{Value, json};

use crate::grants::{Access, Detail};
use crate::operations::{self, OPERATIONS};
use crate::{Exit, fail};

/// The arguments of `nils score`.
#[derive(Debug, clap::Args)]
pub(crate) struct ScoreArgs {
    /// The dataset, as @name
    dataset: String,
    /// The batch whose new stacks are scored: the name its digest was given
    #[arg(long)]
    name: String,
    /// Only this step (body_part, post_contrast or system1); every step by default
    #[arg(long = "step", value_name = "STEP")]
    steps: Vec<String>,
    /// Where the packs are, for the runs the step queues
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<std::path::PathBuf>,
}

/// The stacks a batch brought to a dataset: those whose first batch is one
/// of the batches of that name on the dataset's sources, each with its
/// subject, in stack order.
pub(crate) fn new_stacks(
    store: &mut Store,
    sources: &[i64],
    batch: &str,
) -> Result<Vec<(i64, Option<i64>)>, String> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let ids = sources
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let d = store.dialect();
    let sql = format!(
        "SELECT x.id, se.subject_id FROM {} x JOIN {} se ON se.id = x.series_id \
         WHERE x.first_batch_id IN (SELECT b.id FROM {} b WHERE b.name = {} AND b.source_id IN ({ids})) \
         ORDER BY x.id",
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("ingest_batch"),
        d.param(1, nils_registry::schema::Type::Text),
    );
    store
        .query(&sql, &[nils_registry::store::Param::from(batch)])
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| Ok((r.int(0)?, r.opt_int(1)?)))
        .collect::<Result<_, nils_registry::store::Error>>()
        .map_err(|e| e.to_string())
}

/// One step's arrival run as planned: the step, the run's command line and
/// the parameters its descriptor gives a run at arrival.
struct Planned {
    step: &'static str,
    command: Vec<String>,
}

/// The steps scored at arrival: the operations of their own, then System 1
/// (record 55 D3, spec §11.1: it runs over every stack at arrival), each
/// where its chosen model's pipeline declares `x-nils.on-arrival`.
fn arrival_steps() -> Vec<&'static str> {
    OPERATIONS
        .iter()
        .map(|(n, _)| *n)
        .chain(std::iter::once(crate::system1::STEP))
        .collect()
}

/// A step skipped, as the result says it.
fn skipped(step: &str, reason: &str, why: impl Into<String>) -> Value {
    json!({"step": step, "reason": reason, "why": why.into()})
}

/// What a step's arrival run would be, or why it is skipped: its plan as
/// the step's door makes it, the chosen model's pipeline declaring
/// `x-nils.on-arrival`, the run's command line with that declaration's
/// parameters, and what the run needs of this machine.
fn plan_step(
    home: &Home,
    registry: &mut Registry,
    step: &'static str,
    pack_dir: Option<&std::path::Path>,
) -> Result<Result<Planned, Value>, String> {
    let plan = match operations::plan(registry, step).map_err(|e| e.to_string())? {
        Ok(plan) => plan,
        Err(refused) => return Ok(Err(skipped(step, refused.reason, refused.error))),
    };
    let Some(p) = nils_registry::pipeline::resolve(registry.store(), &plan.pipeline)
        .map_err(|e| e.to_string())?
    else {
        return Ok(Err(skipped(
            step,
            "no_pipeline",
            format!("{} is not in the catalog", plan.pipeline),
        )));
    };
    let d = nils_pipeline::descriptor::from_value(p.descriptor.clone())
        .map_err(|e| format!("{}: {e}", p.label()))?;
    let Some(params) = d.on_arrival else {
        return Ok(Err(skipped(
            step,
            "not_on_arrival",
            format!(
                "{} does not score at arrival: its descriptor declares no x-nils.on-arrival",
                p.label()
            ),
        )));
    };
    let mut command = vec!["run".to_string(), plan.pipeline.clone()];
    for m in &plan.models {
        command.extend(["--model".to_string(), m.to_string()]);
    }
    for (id, value) in params {
        command.extend(["--param".to_string(), format!("{id}={value}")]);
    }
    let command = match crate::pipelines::located(pack_dir, command) {
        Ok(c) => c,
        Err(r) => return Ok(Err(skipped(step, "refused", reply_words(&r)))),
    };
    if let Err(r) = crate::requirements::at_the_door(home, pack_dir, registry, &command, false) {
        return Ok(Err(skipped(step, "requirements", reply_words(&r))));
    }
    Ok(Ok(Planned { step, command }))
}

fn reply_words(r: &crate::serve::Reply) -> String {
    r.body["error"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| r.body.to_string())
}

/// `nils score @dataset --name <batch> [--step S]`: queue each step's run
/// over the stacks the batch brought, as the module says. A job of kind
/// `score`, whose result names what it queued and what it skipped and why.
pub(crate) fn command(home: &Home, args: ScoreArgs) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    let place = crate::dataset_named(&mut registry, &args.dataset)?;
    let wanted: Vec<&'static str> = if args.steps.is_empty() {
        arrival_steps()
    } else {
        let mut out = Vec::new();
        for s in &args.steps {
            let Some(n) = arrival_steps().into_iter().find(|n| n == s) else {
                return Err(crate::usage(format!(
                    "--step is body_part, post_contrast or system1, not {s}"
                )));
            };
            out.push(n);
        }
        out
    };
    let job_id = job::claim(
        registry.store(),
        &job::Claim {
            kind: "score",
            name: &args.name,
            args: json!({"dataset": place.name, "batch": args.name, "steps": wanted}),
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    let result = score(
        home,
        &mut registry,
        &place,
        &args.name,
        &wanted,
        args.pack_dir.as_deref(),
        job_id,
    );
    let store = registry.store();
    match result {
        Ok(doc) => {
            job::set_result(store, job_id, &doc).map_err(|e| fail(e.to_string()))?;
            job::finish(store, job_id, job::State::Done, None).map_err(|e| fail(e.to_string()))?;
            println!("{doc}");
            Ok(())
        }
        Err(e) => {
            let _ = job::finish(store, job_id, job::State::Failed, Some(e.as_str()));
            Err(fail(e))
        }
    }
}

/// The score itself, under the job `job_id` has claimed.
fn score(
    home: &Home,
    registry: &mut Registry,
    place: &nils_registry::place::Place,
    batch: &str,
    wanted: &[&'static str],
    pack_dir: Option<&std::path::Path>,
    job_id: i64,
) -> Result<Value, String> {
    let me = job::show(registry.store(), job_id)
        .map_err(|e| e.to_string())?
        .ok_or("the score's own job is gone")?;
    // a chain's step runs under what its first job recorded; a score at the
    // keyboard with the keyboard's reach
    let adopted = std::env::var(job::ADOPT_VAR).is_ok();
    let access = if adopted {
        crate::chain::recorded_access(&me)
    } else {
        Access::everything()
    };
    let sources = crate::sources::source_ids(registry.store(), place).map_err(|e| e.to_string())?;
    let stacks = new_stacks(registry.store(), &sources, batch)?;
    let mut doc = json!({
        "dataset": place.name, "batch": batch, "new_stacks": stacks.len(),
        "queued": [], "skipped": [],
    });
    if stacks.is_empty() {
        doc["why"] = json!(format!(
            "the batch {batch} brought no new stack to {}, so there is nothing to score",
            place.name
        ));
        return Ok(doc);
    }
    // a run reads pixels (record 43 S2): pipelines:work at quasi detail
    let (grant, detail) =
        crate::serve::verb_needs(&["run".to_string()]).unwrap_or(("pipelines:work", Detail::Quasi));
    let cannot = if !access.holds(grant) {
        Some(format!(
            "the {grant} grant, which the caller who queued the bring-in does not hold"
        ))
    } else if access.detail < detail {
        Some(format!(
            "detail {}, and the bring-in was queued at {}",
            detail.name(),
            access.detail.name()
        ))
    } else {
        None
    };
    if let Some(why) = cannot {
        doc["skipped"] = json!(
            wanted
                .iter()
                .map(|s| skipped(s, "no_grant", format!("a run needs {why}")))
                .collect::<Vec<_>>()
        );
        doc["score_skipped"] = doc["skipped"].clone();
        return Ok(doc);
    }
    let mut planned: Vec<Planned> = Vec::new();
    let mut skips: Vec<Value> = Vec::new();
    for step in wanted {
        match plan_step(home, registry, step, pack_dir)? {
            Ok(p) => planned.push(p),
            Err(s) => skips.push(s),
        }
    }
    // post-contrast reads the body part's answers: after its run, where
    // both are queued
    let after_body_part = planned.iter().any(|p| p.step == "body_part")
        && planned.iter().any(|p| p.step == "post_contrast");
    let principal = me
        .principal()
        .map(str::to_string)
        .unwrap_or_else(crate::actor);
    let epoch = registry.meta().epoch;
    let target = format!("dataset:{}", place.name);
    let mut queued: Vec<Value> = Vec::new();
    for p in planned {
        if after_body_part && p.step == "post_contrast" {
            queued.push(json!({"step": p.step, "after": "body_part"}));
            continue;
        }
        let store = registry.store();
        let rows: Vec<Row> = stacks
            .iter()
            .map(|(s, subject)| Row(vec![Cell::Int(*s), subject.map_or(Cell::Null, Cell::Int)]))
            .collect();
        let (handle, frozen) = operations::save_stacks(
            store,
            rows,
            &principal,
            epoch,
            &operations::Pinned::default(),
        )?;
        // the handle goes after the pipeline, as the step's door writes it
        let mut command = p.command[..2].to_vec();
        command.extend(["--handle".to_string(), handle.to_string()]);
        command.extend_from_slice(&p.command[2..]);
        let mut extra = json!({
            "detail": me.args["detail"].clone(),
            "grants": me.args["grants"].clone(),
            "actor": me.args["actor"].clone(),
            "step": p.step,
            "for": target,
            "arrival": {"batch": batch, "score_job": job_id},
        });
        if !adopted {
            extra["detail"] = json!(Detail::Sensitive.name());
            extra["grants"] = json!(crate::grants::GRANTS.to_vec());
        }
        if after_body_part && p.step == "body_part" {
            let mut then = vec![
                "score".to_string(),
                format!("@{}", place.name),
                "--name".to_string(),
                batch.to_string(),
                "--step".to_string(),
                "post_contrast".to_string(),
            ];
            if let Some(d) = pack_dir {
                then.extend(["--pack-dir".to_string(), d.display().to_string()]);
            }
            extra["then"] = json!([then]);
        }
        let id = job::enqueue_with(
            store,
            &command,
            Some(target.as_str()),
            Some(principal.as_str()),
            extra,
        )
        .map_err(|e| e.to_string())?;
        queued.push(json!({
            "step": p.step, "job": id, "handle": handle, "stacks": frozen, "command": command,
        }));
    }
    doc["queued"] = json!(queued);
    if !skips.is_empty() {
        doc["score_skipped"] = json!(skips.clone());
    }
    doc["skipped"] = json!(skips);
    Ok(doc)
}
