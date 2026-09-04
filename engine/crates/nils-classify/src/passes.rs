// SPDX-License-Identifier: AGPL-3.0-only

//! Running a pack's passes (`docs/specs/wave2-fingerprint-and-classify.md`,
//! §7).
//!
//! The per-stack rules have run and every stack has a verdict. A pass is the
//! part that reads more than one stack: it builds the **reference** the pack
//! declared, asks its kind for an answer, and writes the answer down with the
//! evidence that made it. The algorithm and what it reads are `nils_pack`'s,
//! so the checker that runs a pack over a CSV runs the same code; what is
//! here is the registry: which columns to read, and what to write back.
//!
//! v0 does this against the live table, in whatever order the database
//! returned it, and keeps nothing about the result. So the answer a stack got
//! depended on what had been sorted before it, sorting one cohort could change
//! another, and nothing about a stack explained its own result. Here the
//! reference is named, the vote is written down, and a tie says so.

use std::collections::{HashMap, HashSet};

use nils_pack::Pack;
use nils_pack::pass::{Corpus, Pass, Phase, Vote, run_vote};
use nils_registry::schema::table;
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::now_iso;

use crate::classify::FIELDS;
use crate::job::Error;

/// What one pass did, for the report and for the job record.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Ran {
    pub pass: String,
    pub kind: String,
    pub reference: String,
    /// How many stacks the reference holds, which is the pass's own scale.
    pub pool: usize,
    pub targets: i64,
    pub decided: i64,
    pub review_items: i64,
    /// How each answer was reached, and how the silent ones stayed silent.
    pub by_method: std::collections::BTreeMap<String, i64>,
}

/// The corpus, read once: only the fields the passes name, and the axes the
/// rules decided. A pass that reads five columns does not pay for
/// forty-three, which is the difference between tens of megabytes and
/// gigabytes on half a million stacks.
fn read_corpus(store: &mut Store, pack: &Pack, modality: Option<&str>) -> Result<Corpus, Error> {
    let mut corpus = Corpus::new(pack);
    let needed = corpus.needed();
    if needed.is_empty() {
        return Ok(corpus);
    }
    let t = table("stack_fingerprint");
    let dialect = store.dialect();
    let columns: Vec<String> = std::iter::once("stack_id".to_string())
        .chain(needed.iter().map(|f| {
            let column = t
                .column(FIELDS[*f].1)
                .unwrap_or_else(|| panic!("stack_fingerprint.{} is not a column", FIELDS[*f].1));
            dialect.text_of_qualified(None, column)
        }))
        .collect();
    let filter = match modality {
        Some(m) => format!(" WHERE modality = '{}'", m.replace('\'', "''")),
        None => String::new(),
    };
    let sql = format!(
        "SELECT {} FROM {}{filter} ORDER BY stack_id",
        columns.join(", "),
        store.qualified("stack_fingerprint"),
    );

    // The axes first, so a stack arrives complete.
    //
    // A pass never reads a pass. `classification_axis` holds whatever decided
    // a value, and on any run after the first that includes answers an earlier
    // run's pass wrote, so taking the table as it stands would let the vote
    // count its own guesses as evidence. Measured on the live corpus: with the
    // pass's answers in the reference, sorting the same archive again from two
    // different ingest histories agrees on 14 of 9,014 answers, and without
    // them on 31,880 of 31,880 (spec section 7.4). The evidence row says which
    // it was: `pass` is null exactly when a rule or a person decided.
    //
    // Read as two queries rather than one anti-join: what a pass wrote is a
    // small set, most runs have none of it, and a correlated subquery over
    // every axis of every stack costs more than the set does.
    let mut by_a_pass: HashSet<(i64, String)> = HashSet::new();
    let sql_pass = format!(
        "SELECT stack_id, axis FROM {} WHERE pass IS NOT NULL",
        store.qualified("classification_evidence")
    );
    for r in store.query(&sql_pass, &[])? {
        by_a_pass.insert((r.int(0)?, r.text(1)?.to_string()));
    }

    let mut decided: HashMap<i64, Vec<String>> = HashMap::new();
    let sql_axes = format!(
        "SELECT stack_id, axis, value FROM {}",
        store.qualified("classification_axis")
    );
    for r in store.query(&sql_axes, &[])? {
        let name = r.text(1)?;
        let Some(a) = pack.axes.iter().position(|x| x.name == name) else {
            continue;
        };
        let id = r.int(0)?;
        if !by_a_pass.is_empty() && by_a_pass.contains(&(id, name.to_string())) {
            continue;
        }
        let value = r.opt_text(2)?.unwrap_or("").to_string();
        decided
            .entry(id)
            .or_insert_with(|| vec![String::new(); pack.axes.len()])[a] = value;
    }

    let empty: Vec<String> = vec![String::new(); pack.axes.len()];
    for r in store.query(&sql, &[])? {
        let id = r.int(0)?;
        let cells: Vec<String> = (0..needed.len())
            .map(|i| crate::classify::cell_text(r.get(i + 1)).unwrap_or_default())
            .collect();
        let axes = decided.get(&id).unwrap_or(&empty);
        corpus.push(
            id,
            |f| {
                needed
                    .iter()
                    .position(|n| *n == f)
                    .map(|i| cells[i].clone())
                    .unwrap_or_default()
            },
            |a| axes[a].clone(),
        );
    }
    Ok(corpus)
}

/// Run every pass of this phase, in the order the pack declares them.
pub fn run(
    store: &mut Store,
    pack: &Pack,
    settings: &crate::job::Settings,
    cancel: &nils_digest::Cancel,
    phase: Phase,
    job_id: i64,
) -> Result<Vec<Ran>, Error> {
    let wanted: Vec<&Pass> = pack.passes.iter().filter(|p| p.phase == phase).collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let corpus = read_corpus(store, pack, settings.modality.as_deref())?;
    let mut out = Vec::new();
    for pass in wanted {
        if cancel.stop() {
            break;
        }
        let Some(vote) = pass.vote() else { continue };
        out.push(run_one(store, pack, pass, vote, &corpus, job_id)?);
    }
    Ok(out)
}

fn run_one(
    store: &mut Store,
    pack: &Pack,
    pass: &Pass,
    vote: &Vote,
    corpus: &Corpus,
    job_id: i64,
) -> Result<Ran, Error> {
    let mut ran = Ran {
        pass: pass.name.clone(),
        kind: pass.kind_name().to_string(),
        reference: pass.reference.scope.clone(),
        ..Ran::default()
    };
    let (answers, pool, _pools) = run_vote(pack, pass, vote, corpus, false);
    ran.pool = pool;
    ran.targets = answers.len() as i64;

    let now = now_iso();
    let mut axis_rows: Vec<Vec<Param>> = Vec::new();
    let mut evidence: Vec<Vec<Param>> = Vec::new();
    let mut reviews: Vec<Vec<Param>> = Vec::new();
    for a in &answers {
        *ran.by_method.entry(a.outcome.method.clone()).or_insert(0) += 1;
        if a.writes.is_empty() {
            continue;
        }
        ran.decided += 1;
        let stack_id = corpus.ids[a.at];
        let confidence = a.outcome.confidence();
        let cited = format!(
            "{} of {} neighbours in the {} pool of {}",
            a.outcome.matches, a.outcome.neighbours, a.outcome.partition, a.outcome.pool
        );
        for (axis, stored) in &a.writes {
            // An axis that already says something keeps it: a pass fills a
            // gap, it does not overrule the rules.
            let current = corpus.axis_of(a.at, *axis);
            let fills = current.is_empty() || vote.write_when.iter().any(|w| w == current);
            if !fills {
                continue;
            }
            let name = pack.axes[*axis].name.as_str();
            axis_rows.push(vec![
                Param::Int(stack_id),
                Param::from(name),
                Param::from(stored.as_str()),
                Param::Double(confidence),
                Param::from("vote"),
            ]);
            if pass.emit.evidence {
                evidence.push(vec![
                    Param::Int(stack_id),
                    Param::from(name),
                    Param::from(stored.as_str()),
                    Param::from("vote"),
                    Param::Double(confidence),
                    Param::from(pass.name.as_str()),
                    Param::from(a.outcome.method.as_str()),
                    Param::from("neighbours"),
                    Param::from(cited.as_str()),
                    Param::from(pass.name.as_str()),
                    Param::from(pass.reference.scope.as_str()),
                ]);
            }
            if confidence < pass.emit.review_below || pass.emit.review_all_touched {
                reviews.push(vec![
                    Param::from(format!("{name}:vote")),
                    Param::from("stack"),
                    Param::from(serde_json::json!({"stack_id": stack_id}).to_string()),
                    Param::from(
                        serde_json::json!({
                            "axis": name,
                            "value": stored,
                            "confidence": confidence,
                            "pass": pass.name,
                            "method": a.outcome.method,
                            "matches": a.outcome.matches,
                            "neighbours": a.outcome.neighbours,
                            "reference": pass.reference.scope,
                            "job": job_id,
                        })
                        .to_string(),
                    ),
                    Param::from("open"),
                    Param::from(now.as_str()),
                ]);
                ran.review_items += 1;
            }
        }
    }

    // --- written in one transaction, like any other window
    if !axis_rows.is_empty() {
        store.begin()?;
        let write = (|| -> Result<(), nils_registry::store::Error> {
            store.insert(
                &Insert::new(
                    table("classification_axis"),
                    &["stack_id", "axis", "value", "confidence", "tier"],
                )
                .on_conflict(nils_registry::dialect::Conflict::Update {
                    target: &["stack_id", "axis"],
                    set: &["value", "confidence", "tier"],
                }),
                &axis_rows,
            )?;
            if !evidence.is_empty() {
                store.insert(
                    &Insert::new(
                        table("classification_evidence"),
                        &[
                            "stack_id",
                            "axis",
                            "value",
                            "tier",
                            "confidence",
                            "rule_set",
                            "rule",
                            "source",
                            "matched",
                            "pass",
                            "reference",
                        ],
                    ),
                    &evidence,
                )?;
            }
            if !reviews.is_empty() {
                store.insert(
                    &Insert::new(
                        table("review_item"),
                        &["kind", "scope", "ref", "evidence", "status", "created_at"],
                    ),
                    &reviews,
                )?;
            }
            Ok(())
        })();
        match write {
            Ok(()) => store.commit()?,
            Err(e) => {
                store.rollback().ok();
                return Err(Error::Store(e));
            }
        }
    }
    Ok(ran)
}
