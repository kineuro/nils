// SPDX-License-Identifier: AGPL-3.0-only

//! What holds a group of stacks the digest would fold into one, and what a
//! fold clears (wave 7a). The digest joins the stacks of a series that only
//! the echo number told apart (`nils_digest::echo`): the stacks that go give
//! their instances to the one that stays.
//!
//! The rows the machine made do not hold a group, and go with it: a question
//! no person answered (open, superseded, or answered by a model or an agent
//! and never put in force by a person), a decision of a model's or an
//! agent's no person put in force, and what a model made of a stack (a
//! derivative that names a model, an embedding, or what a run of a model
//! wrote: a run that names models, or of a pipeline that proposes values
//! for an axis, which is how an operation's model runs, record 56; and the
//! measures read from them). The stack that stays loses the same rows, and
//! what the sort said of it, so the next sort judges it anew and asks its
//! questions again, and an operation whose model answered it reads as not
//! run for it.
//!
//! What a person or a release did holds the group as it is: on a stack
//! that would go, a person's decision, a question a person answered, a
//! pick, a seal, a campaign, a release, a pipeline's own derivative or
//! measure; on the one that would stay, what fixed or was made of what it
//! holds: a seal (a sealed sample is not changed by a fold), a campaign, a
//! release, a pipeline's own derivative or measure. The empty-stack sweep
//! keeps its own rules ([`crate::empty`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::empty::{Row, STACK_TABLES, delete_where, list, named_in};
use crate::schema::table;
use crate::store::{Error, Store};

/// The tables whose rows hold a group whichever of its stacks they name,
/// because they fixed what the stack holds.
const FIXED: [&str; 3] = ["sealed_stack", "campaign_", "release_"];

/// Why each stack of a group holds it, the first reason found: `going` are
/// the stacks that would go, `staying` the ones that would stay. A stack the
/// map does not name holds nothing.
pub fn holds(
    store: &mut Store,
    going: &[i64],
    staying: &[i64],
) -> Result<BTreeMap<i64, &'static str>, Error> {
    let mut out: BTreeMap<i64, &'static str> = BTreeMap::new();
    let all: Vec<i64> = going.iter().chain(staying).copied().collect();
    if all.is_empty() {
        return Ok(out);
    }
    let made = Made::of(store, &all)?;
    for id in made.kept_derivatives.iter().chain(&made.kept_measures) {
        out.entry(*id)
            .or_insert("a pipeline's own derivative or measure was made of it");
    }
    for (t, row) in STACK_TABLES {
        let Row::Keeps(why) = row else { continue };
        if matches!(*t, "review_member" | "derivative" | "measure") {
            continue;
        }
        let ids = match FIXED.iter().any(|f| t.starts_with(f)) {
            true => &all,
            false => going,
        };
        for id in named_in(store, t, "stack_id", ids)? {
            out.entry(id).or_insert(why);
        }
    }
    if !going.is_empty() {
        for id in people_decided(store, going)? {
            out.entry(id).or_insert("a person's decision names it");
        }
        for (stack, _) in Questions::of(store, going)?.people {
            out.entry(stack)
                .or_insert("a person answered a question about it");
        }
    }
    Ok(out)
}

/// Clear what the machine made of `stacks`, the stacks of groups a fold
/// joins, those that go and those that stay: their questions no person
/// answered, the answers no person put in force, what a model made of them,
/// and what the sort said of them. A grouped question keeps its other
/// members, and goes when none is left. Runs inside the caller's
/// transaction.
pub fn clear(store: &mut Store, stacks: &[i64]) -> Result<(), Error> {
    if stacks.is_empty() {
        return Ok(());
    }
    let questions = Questions::of(store, stacks)?;
    // the stacks' places in grouped questions, then the grouped questions
    // left with no member, which go, and the others' counts
    let members: Vec<i64> = questions.grouped.iter().filter_map(|q| q.member).collect();
    delete_where(store, "review_member", "id", &members)?;
    let touched: Vec<i64> = questions
        .grouped
        .iter()
        .map(|q| q.item)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut emptied: BTreeSet<i64> = BTreeSet::new();
    for chunk in touched.chunks(500) {
        let sql = format!(
            "SELECT i.id FROM {} i WHERE i.id IN ({}) AND NOT EXISTS \
             (SELECT 1 FROM {} m WHERE m.item_id = i.id)",
            store.qualified("review_item"),
            list(chunk),
            store.qualified("review_member"),
        );
        for r in store.query(&sql, &[])? {
            emptied.insert(r.int(0)?);
        }
    }
    let left: Vec<i64> = touched
        .iter()
        .copied()
        .filter(|id| !emptied.contains(id))
        .collect();
    for chunk in left.chunks(500) {
        let sql = format!(
            "UPDATE {i} SET members = (SELECT COUNT(*) FROM {m} WHERE {m}.item_id = {i}.id) \
             WHERE id IN ({})",
            list(chunk),
            i = store.qualified("review_item"),
            m = store.qualified("review_member"),
        );
        store.execute(&sql, &[])?;
    }
    // the questions that go, the stacks' own and the emptied grouped ones,
    // with their answers and the stacks' own decisions where no person
    // wrote them or put them in force
    let going: Vec<&Question> = questions
        .single
        .iter()
        .chain(
            questions
                .grouped
                .iter()
                .filter(|q| emptied.contains(&q.item)),
        )
        .collect();
    let items: Vec<i64> = going
        .iter()
        .map(|q| q.item)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    delete_where(store, "review_item", "id", &items)?;
    let mut decisions: Vec<i64> = going.iter().filter_map(|q| q.decision).collect();
    decisions.extend(decided_on(store, stacks)?.into_iter().map(|(id, _)| id));
    decisions.sort_unstable();
    decisions.dedup();
    let by = authors(store, &decisions)?;
    let machine: Vec<i64> = decisions
        .into_iter()
        .filter(|id| by.get(id).is_some_and(|person| !person))
        .collect();
    delete_where(store, "decision", "id", &machine)?;
    // what a model made of them, the measures first
    let made = Made::of(store, stacks)?;
    delete_where(store, "measure", "id", &made.model_measures)?;
    delete_where(store, "derivative", "id", &made.model_derivatives)?;
    // and what the sort said of them
    for (t, row) in STACK_TABLES {
        if *row == Row::Goes {
            delete_where(store, t, "stack_id", stacks)?;
        }
    }
    Ok(())
}

/// A question about a stack no person answered, as a fold clears it.
struct Question {
    item: i64,
    /// The stack's place in it, for a grouped question.
    member: Option<i64>,
    /// The decision that answered it, if any: a model's or an agent's.
    decision: Option<i64>,
}

/// The questions about some stacks.
struct Questions {
    /// Each stack's own, which no person answered.
    single: Vec<Question>,
    /// The stacks' places in grouped ones no person answered.
    grouped: Vec<Question>,
    /// Those a person answered, as `(stack, item)`.
    people: Vec<(i64, i64)>,
}

impl Questions {
    fn of(store: &mut Store, stacks: &[i64]) -> Result<Questions, Error> {
        let wanted: BTreeSet<i64> = stacks.iter().copied().collect();
        // (item, member, stack, status, decision)
        type Found = (i64, Option<i64>, i64, String, Option<i64>);
        let mut found: Vec<Found> = Vec::new();
        let t = table("review_item");
        let sql = format!(
            "SELECT id, {}, status, decision_id FROM {} WHERE scope = 'stack'",
            store.dialect().text_of(t.column("ref").expect("ref")),
            store.qualified("review_item"),
        );
        for r in store.query(&sql, &[])? {
            let Some(stack) = r
                .opt_text(1)?
                .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                .and_then(|v| v["stack_id"].as_i64())
            else {
                continue;
            };
            if wanted.contains(&stack) {
                found.push((
                    r.int(0)?,
                    None,
                    stack,
                    r.text(2)?.to_string(),
                    r.opt_int(3)?,
                ));
            }
        }
        for chunk in stacks.chunks(500) {
            let sql = format!(
                "SELECT i.id, m.id, m.stack_id, i.status, i.decision_id FROM {} m \
                 JOIN {} i ON i.id = m.item_id WHERE m.stack_id IN ({})",
                store.qualified("review_member"),
                store.qualified("review_item"),
                list(chunk),
            );
            for r in store.query(&sql, &[])? {
                found.push((
                    r.int(0)?,
                    Some(r.int(1)?),
                    r.int(2)?,
                    r.text(3)?.to_string(),
                    r.opt_int(4)?,
                ));
            }
        }
        let decisions: Vec<i64> = found.iter().filter_map(|f| f.4).collect();
        let by = authors(store, &decisions)?;
        let mut out = Questions {
            single: Vec::new(),
            grouped: Vec::new(),
            people: Vec::new(),
        };
        for (item, member, stack, status, decision) in found {
            let unanswered = match status.as_str() {
                "open" | "superseded" => true,
                // an answer stands for a person where a person wrote it or
                // put it in force; an acknowledgement names no decision
                "staged" | "accepted" => {
                    decision.is_some_and(|d| by.get(&d).is_some_and(|person| !person))
                }
                _ => false,
            };
            if !unanswered {
                out.people.push((stack, item));
                continue;
            }
            let q = Question {
                item,
                member,
                decision,
            };
            match member {
                Some(_) => out.grouped.push(q),
                None => out.single.push(q),
            }
        }
        Ok(out)
    }
}

/// Whether each decision is a person's: a person wrote it, or put it in
/// force (record 42 R6: a model's or an agent's answer is in force only
/// once a person commits it).
fn authors(store: &mut Store, ids: &[i64]) -> Result<HashMap<i64, bool>, Error> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT id, author_kind, committed_by FROM {} WHERE id IN ({})",
            store.qualified("decision"),
            list(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(
                r.int(0)?,
                r.text(1)? == "person" || r.opt_text(2)?.is_some(),
            );
        }
    }
    Ok(out)
}

/// The stacks among `stacks` a person's decision names.
fn people_decided(store: &mut Store, stacks: &[i64]) -> Result<BTreeSet<i64>, Error> {
    let found = decided_on(store, stacks)?;
    let ids: Vec<i64> = found.iter().map(|(id, _)| *id).collect();
    let by = authors(store, &ids)?;
    Ok(found
        .into_iter()
        .filter(|(id, _)| by.get(id).copied().unwrap_or(true))
        .map(|(_, stack)| stack)
        .collect())
}

/// The decisions scoped to one of `stacks`, of any author, as
/// `(decision, stack)`.
fn decided_on(store: &mut Store, stacks: &[i64]) -> Result<Vec<(i64, i64)>, Error> {
    let mut out = Vec::new();
    for chunk in stacks.chunks(500) {
        let refs = chunk
            .iter()
            .map(|i| format!("'{i}'"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, ref FROM {} WHERE scope = 'stack' AND ref IN ({refs})",
            store.qualified("decision")
        );
        for r in store.query(&sql, &[])? {
            if let Ok(stack) = r.text(1)?.parse::<i64>() {
                out.push((r.int(0)?, stack));
            }
        }
    }
    Ok(out)
}

/// What runs and models made of some stacks: the derivatives and measures a
/// model made, and the stacks a pipeline's own derivative or measure names.
struct Made {
    model_derivatives: Vec<i64>,
    model_measures: Vec<i64>,
    kept_derivatives: BTreeSet<i64>,
    kept_measures: BTreeSet<i64>,
}

impl Made {
    fn of(store: &mut Store, stacks: &[i64]) -> Result<Made, Error> {
        let models = ModelRuns::load(store)?;
        let mut out = Made {
            model_derivatives: Vec::new(),
            model_measures: Vec::new(),
            kept_derivatives: BTreeSet::new(),
            kept_measures: BTreeSet::new(),
        };
        for chunk in stacks.chunks(500) {
            let sql = format!(
                "SELECT id, stack_id, kind, model_id, run_id FROM {} WHERE stack_id IN ({})",
                store.qualified("derivative"),
                list(chunk)
            );
            for r in store.query(&sql, &[])? {
                let by_model = r.opt_int(3)?.is_some()
                    || r.text(2)? == "embedding"
                    || r.opt_int(4)?.is_some_and(|run| models.runs.contains(&run));
                match by_model {
                    true => out.model_derivatives.push(r.int(0)?),
                    false => {
                        out.kept_derivatives.insert(r.int(1)?);
                    }
                }
            }
        }
        for chunk in stacks.chunks(500) {
            let sql = format!(
                "SELECT id, stack_id, run_id, pipeline_id, derivative_id FROM {} \
                 WHERE stack_id IN ({})",
                store.qualified("measure"),
                list(chunk)
            );
            for r in store.query(&sql, &[])? {
                let by_model = models.runs.contains(&r.int(2)?)
                    || models.pipelines.contains(&r.int(3)?)
                    || r.opt_int(4)?
                        .is_some_and(|d| out.model_derivatives.contains(&d));
                match by_model {
                    true => out.model_measures.push(r.int(0)?),
                    false => {
                        out.kept_measures.insert(r.int(1)?);
                    }
                }
            }
        }
        Ok(out)
    }
}

/// The pipelines that propose values for an axis, which is how an
/// operation's model runs (record 56), and the runs of a model: a run of
/// such a pipeline, or one that names the models it ran.
struct ModelRuns {
    pipelines: BTreeSet<i64>,
    runs: BTreeSet<i64>,
}

impl ModelRuns {
    fn load(store: &mut Store) -> Result<ModelRuns, Error> {
        let mut pipelines = BTreeSet::new();
        let p = table("pipeline");
        let sql = format!(
            "SELECT id, {} FROM {}",
            store
                .dialect()
                .text_of(p.column("descriptor").expect("descriptor")),
            store.qualified("pipeline")
        );
        for r in store.query(&sql, &[])? {
            let descriptor: serde_json::Value = r
                .opt_text(1)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or_default();
            let proposes = descriptor["x-nils"]["proposals"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x["axis"].is_string()));
            if proposes {
                pipelines.insert(r.int(0)?);
            }
        }
        let mut runs = BTreeSet::new();
        let run = table("pipeline_run");
        let sql = format!(
            "SELECT id, pipeline_id, {} FROM {}",
            store
                .dialect()
                .text_of(run.column("model_ids").expect("model_ids")),
            store.qualified("pipeline_run")
        );
        for r in store.query(&sql, &[])? {
            let named = r
                .opt_text(2)?
                .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                .and_then(|v| v.as_array().map(|a| !a.is_empty()))
                .unwrap_or(false);
            if named || pipelines.contains(&r.int(1)?) {
                runs.insert(r.int(0)?);
            }
        }
        Ok(ModelRuns { pipelines, runs })
    }
}
