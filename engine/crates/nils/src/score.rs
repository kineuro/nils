// SPDX-License-Identifier: AGPL-3.0-only

//! Scoring at arrival (record 55 E5, spec section 10.4; ruling 35 (b) of
//! 2026-10-16): the last step of a dataset's bring-in, `score @dataset
//! --name <batch>`. System 1 is part of NILS always and scores every
//! dataset at arrival: where its chosen model's pipeline declares
//! `x-nils.on-arrival`, the score freezes the stacks the batch brought
//! (those whose first batch is the chain's) into a handle and queues its
//! run over them, as its door makes it, under the principal, grants and
//! detail the chain recorded. The run goes to the pipeline lane, so a long
//! score never holds up a digest, and the dataset's step shows it as it
//! shows a run its door queued. A batch of more new stacks than one
//! container of the pipeline holds is run in parts (`--chunk`,
//! `crate::pipelines::part_stacks`), each a run of its own, in one job.
//!
//! Body part and post-contrast are optional models that add value where the
//! data need them: a person runs them from the dataset's step (its door,
//! `POST /api/datasets/{name}/steps/{step}/run`), and the dataset's summary
//! says on each step how many scans its model would add an answer to
//! (`hint`). They are never run at arrival; the score's result names them
//! under `optional`. `score ... --step body_part` (or `post_contrast`)
//! still queues one over a batch's new stacks, with the parameters its
//! pipeline declares for a run at arrival (body part's: the slice cache).
//!
//! Post-contrast reads the body part's answers (it answers only on brain),
//! so where both are asked for the post-contrast step is queued after the
//! body part's run ends done: `score ... --step post_contrast` is that
//! run's `then`. The cascade answers only T1-type brain, so the stacks sent
//! to it are those that can be, by their stored axes (`can_be_t1_brain`);
//! the others are not sent, and the result counts them in `left_out`.
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
    /// Only this step (body_part, post_contrast or system1); System 1 alone
    /// by default, as at arrival
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

/// Why a stack is not sent to post-contrast's cascade.
pub(crate) const NOT_T1_BRAIN: &str = "not T1-type brain by the stored axes";

/// Whether a stack can be T1-type brain, read from its stored axes with the
/// test the post-contrast cascade applies before it scores (its `family`
/// and `brain` in `nils_postcontrast`): the base, taken as the greatest
/// value as the cascade takes it, is T1w and the technique is not TOF or PC;
/// and the body part says brain, or the body region says head, or the body
/// part is not answered yet (the body-part step may still say brain). A
/// stack this leaves out is never answered by that: it is not sent.
pub(crate) fn can_be_t1_brain(
    base: &[String],
    technique: &[String],
    body_part: &[String],
    body_region: &[String],
) -> bool {
    let greatest = |v: &[String]| v.iter().max().cloned().unwrap_or_default();
    let t1 = greatest(base) == "T1w" && !matches!(greatest(technique).as_str(), "TOF" | "PC");
    // kept where neither body axis is answered yet, or where one says the head
    let brain = (body_part.is_empty() && body_region.is_empty())
        || greatest(body_part).contains("brain")
        || body_region.iter().any(|r| r == "head");
    t1 && brain
}

/// The stacks of `stacks` that can be T1-type brain by their stored axes
/// (`can_be_t1_brain`), in their order.
pub(crate) fn t1_brain_of(
    store: &mut Store,
    stacks: &[(i64, Option<i64>)],
) -> Result<Vec<(i64, Option<i64>)>, String> {
    use std::collections::HashMap;
    let mut axes: HashMap<i64, [Vec<String>; 4]> = HashMap::new();
    for chunk in stacks.chunks(500) {
        let ids = chunk
            .iter()
            .map(|(s, _)| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT stack_id, axis, value FROM {} WHERE axis IN \
             ('base', 'technique', 'body_part', 'body_region') \
             AND value IS NOT NULL AND value <> '' AND stack_id IN ({ids})",
            store.qualified("classification_axis"),
        );
        for r in store.query(&sql, &[]).map_err(|e| e.to_string())? {
            let slot = match r.text(1).map_err(|e| e.to_string())? {
                "base" => 0,
                "technique" => 1,
                "body_part" => 2,
                _ => 3,
            };
            axes.entry(r.int(0).map_err(|e| e.to_string())?)
                .or_default()[slot]
                .push(r.text(2).map_err(|e| e.to_string())?.to_string());
        }
    }
    Ok(stacks
        .iter()
        .filter(|(s, _)| {
            let [b, t, p, g] = axes.get(s).cloned().unwrap_or_default();
            can_be_t1_brain(&b, &t, &p, &g)
        })
        .copied()
        .collect())
}

/// One step's arrival run as planned: the step, the run's command line and
/// the parameters its descriptor gives a run at arrival.
struct Planned {
    step: &'static str,
    command: Vec<String>,
}

/// The steps scored at arrival (ruling 35 (b)): System 1 alone (record 55
/// D3, spec §11.1: it runs over every stack at arrival), where its chosen
/// model's pipeline declares `x-nils.on-arrival`.
const AT_ARRIVAL: [&str; 1] = [crate::system1::STEP];

/// The steps a score may be asked for by name: the operations of their own,
/// which a person runs when the data need them, then System 1.
fn named_steps() -> Vec<&'static str> {
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
        AT_ARRIVAL.to_vec()
    } else {
        let mut out = Vec::new();
        for s in &args.steps {
            let Some(n) = named_steps().into_iter().find(|n| n == s) else {
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
    let keyboard = Access::keyboard(crate::sealed::keyboard_reads());
    let access = if adopted {
        crate::chain::recorded_access(&me)
    } else {
        keyboard.clone()
    };
    let sources = crate::sources::source_ids(registry.store(), place).map_err(|e| e.to_string())?;
    let stacks = new_stacks(registry.store(), &sources, batch)?;
    let mut doc = json!({
        "dataset": place.name, "batch": batch, "new_stacks": stacks.len(),
        "queued": [], "skipped": [],
    });
    // ruling 35 (b): the optional models a person runs from the dataset's
    // step, never at arrival
    let optional: Vec<&str> = OPERATIONS
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !wanted.contains(n))
        .collect();
    doc["optional"] = json!(optional);
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
    let mut left_out: Vec<Value> = Vec::new();
    for p in planned {
        if after_body_part && p.step == "post_contrast" {
            queued.push(json!({"step": p.step, "after": "body_part"}));
            continue;
        }
        // post-contrast's cascade answers only T1-type brain: the others
        // are not sent (nothing is answered for them)
        let sent = if p.step == "post_contrast" {
            t1_brain_of(registry.store(), &stacks)?
        } else {
            stacks.clone()
        };
        if sent.len() < stacks.len() {
            left_out.push(json!({
                "step": p.step, "stacks": stacks.len() - sent.len(), "why": NOT_T1_BRAIN,
            }));
        }
        if sent.is_empty() {
            skips.push(skipped(
                p.step,
                "not_t1_brain",
                format!(
                    "none of the {} new stacks is T1-type brain by the stored axes, so nothing is queued",
                    stacks.len()
                ),
            ));
            continue;
        }
        let store = registry.store();
        let rows: Vec<Row> = sent
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
        // 2026-10-17: more new stacks than one container of the pipeline
        // holds are run in parts, each a run of its own
        let command = crate::pipelines::in_parts(store, command, frozen);
        let mut extra = json!({
            "detail": me.args["detail"].clone(),
            "grants": me.args["grants"].clone(),
            "actor": me.args["actor"].clone(),
            "step": p.step,
            "for": target,
            "arrival": {"batch": batch, "score_job": job_id},
        });
        if !adopted {
            extra["detail"] = json!(keyboard.detail.name());
            extra["grants"] = json!(keyboard.list());
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
    if !left_out.is_empty() {
        doc["left_out"] = json!(left_out);
    }
    if !skips.is_empty() {
        doc["score_skipped"] = json!(skips.clone());
    }
    doc["skipped"] = json!(skips);
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn put(store: &mut Store, stack: i64, axis: &str, value: &str) {
        let sql = format!(
            "INSERT INTO {} (stack_id, axis, value, confidence, tier) VALUES ({stack}, '{axis}', '{value}', 1.0, 'rule')",
            store.qualified("classification_axis")
        );
        store.execute(&sql, &[]).unwrap();
    }

    /// 2026-10-16: a batch of T1 brain, T2 brain and T1 spine sends only
    /// the T1 brain stack; a stack whose body part is not answered yet is
    /// kept; TOF is left out; a head region keeps a stack whose part is
    /// another.
    fn scope_case(mut store: Store) {
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        // 1 T1 brain, 2 T2 brain, 3 T1 spine, 4 T1 no body part yet,
        // 5 T1 TOF brain, 6 T1 head region with another part, 7 no base
        for (s, base, part, region, tech) in [
            (1, "T1w", Some("brain"), Some("head"), "MPRAGE"),
            (2, "T2w", Some("brain"), Some("head"), "TSE"),
            (3, "T1w", Some("spine-cervical"), Some("spine"), "TSE"),
            (4, "T1w", None, None, "TSE"),
            (5, "T1w", Some("brain"), Some("head"), "TOF"),
            (6, "T1w", Some("orbit"), Some("head"), "TSE"),
        ] {
            put(&mut store, s, "base", base);
            put(&mut store, s, "technique", tech);
            if let Some(p) = part {
                put(&mut store, s, "body_part", p);
            }
            if let Some(r) = region {
                put(&mut store, s, "body_region", r);
            }
        }
        let stacks: Vec<(i64, Option<i64>)> = (1..=7).map(|s| (s, Some(s * 10))).collect();
        let sent: Vec<i64> = t1_brain_of(&mut store, &stacks)
            .unwrap()
            .iter()
            .map(|(s, _)| *s)
            .collect();
        assert_eq!(sent, vec![1, 4, 6]);
        // the batch of three: only the T1 brain stack
        let three: Vec<(i64, Option<i64>)> = stacks[..3].to_vec();
        let sent = t1_brain_of(&mut store, &three).unwrap();
        assert_eq!(sent, vec![(1, Some(10))]);
    }

    #[test]
    fn only_stacks_that_can_be_t1_brain_are_sent_to_post_contrast() {
        scope_case(Store::sqlite_in_memory().unwrap());
        let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
            .ok()
            .filter(|d| !d.is_empty())
        else {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            return;
        };
        let schema = "nils_pc_scope_test";
        let mut admin = Store::connect_postgres(&dsn, "public").unwrap();
        admin
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
            ))
            .unwrap();
        drop(admin);
        scope_case(Store::connect_postgres(&dsn, schema).unwrap());
        let mut admin = Store::connect_postgres(&dsn, "public").unwrap();
        admin
            .batch(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .unwrap();
    }

    #[test]
    fn the_cascades_own_test_is_applied_to_the_axes() {
        let ok = |b: &[&str], t: &[&str], p: &[&str], g: &[&str]| {
            can_be_t1_brain(&w(b), &w(t), &w(p), &w(g))
        };
        assert!(ok(&["T1w"], &["MPRAGE"], &["brain"], &["head"]));
        assert!(ok(&["T1w"], &[], &[], &[]));
        assert!(!ok(&[], &[], &["brain"], &[]));
        assert!(!ok(&["T1w"], &["PC"], &["brain"], &[]));
        assert!(!ok(&["T2w"], &[], &[], &[]));
        assert!(!ok(&["T1w"], &[], &["spine"], &["spine"]));
    }
}
