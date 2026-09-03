// SPDX-License-Identifier: AGPL-3.0-only

//! Running a pack's passes (`docs/specs/wave2-fingerprint-and-classify.md`,
//! §7).
//!
//! The per-stack rules have run and every stack has a verdict. A pass is the
//! part that reads more than one stack: it builds the **reference** the pack
//! declared, asks its kind for an answer, and writes the answer down with the
//! evidence that made it.
//!
//! v0 does this against the live table, in whatever order the database
//! returned it, and keeps nothing about the result. So the answer a stack got
//! depended on what had been sorted before it, sorting one cohort could change
//! another, and nothing about a stack explained its own result. Here the
//! reference is named, the vote is written down, and a tie says so.

use std::collections::HashMap;

use nils_pack::Pack;
use nils_pack::expr::{Ctx, Subject};
use nils_pack::pass::{Cond, Key, Pass, Phase, Pool, Vote, What, key_of, take};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::now_iso;

use crate::classify::FIELDS;
use crate::job::Error;

/// One stack, as a pass sees it: what the fingerprint says and what the rules
/// decided. A pass may read those two things and nothing else, which is what
/// the loader enforces when it compiles a pass's expressions.
struct Row<'a> {
    fields: &'a [Option<String>],
    /// Per axis of the pack, the value stored for this stack.
    axes: &'a [Option<String>],
}

impl Ctx for Row<'_> {
    fn pred(&self, _parser: usize, _pred: usize) -> bool {
        unreachable!("a pass may not name a parser predicate")
    }
    fn subject(&self, _parser: usize) -> Subject<'_> {
        unreachable!("a pass may not name a parser")
    }
    fn flag(&self, _flag: usize) -> bool {
        unreachable!("a pass may not name a flag")
    }
    fn num(&self, field: usize) -> Option<f64> {
        self.fields
            .get(field)
            .and_then(|v| v.as_deref())
            .and_then(|s| s.parse().ok())
    }
    fn present(&self, field: usize) -> bool {
        self.fields
            .get(field)
            .and_then(|v| v.as_deref())
            .is_some_and(|s| !s.is_empty())
    }
    fn text(&self, field: usize) -> &str {
        self.fields
            .get(field)
            .and_then(|v| v.as_deref())
            .unwrap_or("")
    }
    fn re(&self, _idx: usize) -> &nils_pack::expr::Regex {
        unreachable!("a pass's expressions carry no patterns of their own")
    }
    fn axis_is(&self, axis: usize, value: &str) -> bool {
        self.axes
            .get(axis)
            .and_then(|v| v.as_deref())
            .is_some_and(|v| v == value)
    }
    fn axis_empty(&self, axis: usize) -> bool {
        self.axes
            .get(axis)
            .and_then(|v| v.as_deref())
            .is_none_or(str::is_empty)
    }
}

fn holds(c: &Cond, row: &Row<'_>) -> bool {
    let value = match c.what {
        What::Field(i) => row.fields.get(i).and_then(|v| v.as_deref()),
        What::Axis(i) => row.axes.get(i).and_then(|v| v.as_deref()),
    };
    let value = value.filter(|v| !v.is_empty());
    if let Some(want) = c.present
        && value.is_some() != want
    {
        return false;
    }
    if let Some(v) = value {
        if !c.is.is_empty() && !c.is.iter().any(|w| w == v) {
            return false;
        }
        if c.not.iter().any(|w| w == v) {
            return false;
        }
    } else if !c.is.is_empty() {
        return false;
    }
    true
}

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

/// Everything a pass phase needs from the registry, read once.
struct Corpus {
    /// Per stack, in stack order: the fingerprint's fields.
    fields: Vec<Vec<Option<String>>>,
    /// Per stack, per axis of the pack: what the rules decided.
    axes: Vec<Vec<Option<String>>>,
    ids: Vec<i64>,
}

fn read_corpus(store: &mut Store, pack: &Pack, modality: Option<&str>) -> Result<Corpus, Error> {
    let t = table("stack_fingerprint");
    let dialect = store.dialect();
    let columns: Vec<String> = std::iter::once("stack_id".to_string())
        .chain(FIELDS.iter().map(|(_, c)| {
            let column = t
                .column(c)
                .unwrap_or_else(|| panic!("stack_fingerprint.{c} is not a column"));
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
    let mut corpus = Corpus {
        fields: Vec::new(),
        axes: Vec::new(),
        ids: Vec::new(),
    };
    let mut at: HashMap<i64, usize> = HashMap::new();
    for r in store.query(&sql, &[])? {
        let id = r.int(0)?;
        at.insert(id, corpus.ids.len());
        corpus.ids.push(id);
        corpus.fields.push(
            (0..FIELDS.len())
                .map(|i| crate::classify::cell_text(r.get(i + 1)))
                .collect(),
        );
        corpus.axes.push(vec![None; pack.axes.len()]);
    }

    let sql = format!(
        "SELECT stack_id, axis, value FROM {}",
        store.qualified("classification_axis")
    );
    for r in store.query(&sql, &[])? {
        let name = r.text(1)?;
        let (Some(&i), Some(a)) = (
            at.get(&r.int(0)?),
            pack.axes.iter().position(|x| x.name == name),
        ) else {
            continue;
        };
        corpus.axes[i][a] = r.opt_text(2)?.map(str::to_string);
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
        out.push(run_vote(store, pack, pass, vote, &corpus, job_id)?);
    }
    Ok(out)
}

/// The value's index in its axis's vocabulary, by what a row stores.
fn value_index(pack: &Pack, axis: usize, stored: &str) -> Option<usize> {
    let a = &pack.axes[axis];
    (0..a.values.len()).find(|i| a.stored(*i) == stored)
}

fn run_vote(
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

    // --- the reference, as the pack declared it
    let mut pools: HashMap<String, Pool> = HashMap::new();
    let mut global = Pool::default();
    for i in 0..corpus.ids.len() {
        let row = Row {
            fields: &corpus.fields[i],
            axes: &corpus.axes[i],
        };
        if !pass.reference.filter.iter().all(|c| holds(c, &row)) {
            continue;
        }
        let answer: Option<Vec<usize>> = vote
            .vote_on
            .iter()
            .map(|a| {
                corpus.axes[i][*a]
                    .as_deref()
                    .and_then(|v| value_index(pack, *a, v))
            })
            .collect();
        let Some(answer) = answer else { continue };
        let values: Vec<Option<f64>> = vote.dims.iter().map(|d| row.num(d.field)).collect();
        let key = key_of(vote, &values);
        if let Some(p) = pass.reference.partition_by {
            let name = corpus.axes[i][p].clone().unwrap_or_default();
            pools.entry(name).or_default().add(key, answer.clone());
        }
        global.add(key, answer);
    }
    ran.pool = global.len();

    // --- and the stacks it is for
    let now = now_iso();
    let mut axis_rows: Vec<Vec<Param>> = Vec::new();
    let mut evidence: Vec<Vec<Param>> = Vec::new();
    let mut reviews: Vec<Vec<Param>> = Vec::new();
    for i in 0..corpus.ids.len() {
        let row = Row {
            fields: &corpus.fields[i],
            axes: &corpus.axes[i],
        };
        if let Some(t) = &pass.target
            && !t.eval(None, &row)
        {
            continue;
        }
        ran.targets += 1;

        let values: Vec<Option<f64>> = vote.dims.iter().map(|d| row.num(d.field)).collect();
        let key: Key = key_of(vote, &values);
        let sequence = row.text(vote.compat.subject_field).to_string();
        let partition = pass
            .reference
            .partition_by
            .map(|p| corpus.axes[i][p].clone().unwrap_or_default());

        // Its own pool first, then the whole one, exactly as the pack says.
        let mut outcome = match &partition {
            Some(name) if !pass.reference.fallback_except.iter().any(|e| e == name) => {
                let pool = pools.get(name).unwrap_or(&global);
                take(vote, pool, &key, &sequence, &pack.axes, &pack.regexes, name)
            }
            _ => take(
                vote,
                &global,
                &key,
                &sequence,
                &pack.axes,
                &pack.regexes,
                "global",
            ),
        };
        if outcome.answer.is_none()
            && pass.reference.fallback
            && outcome.partition != "global"
            && pass.reference.fallback_when.contains(&outcome.method)
        {
            outcome = take(
                vote,
                &global,
                &key,
                &sequence,
                &pack.axes,
                &pack.regexes,
                "global",
            );
        }
        *ran.by_method.entry(outcome.method.clone()).or_insert(0) += 1;

        let Some(answer) = &outcome.answer else {
            continue;
        };
        ran.decided += 1;
        let confidence = outcome.confidence();
        let cited = format!(
            "{} of {} neighbours in the {} pool of {}",
            outcome.matches, outcome.neighbours, outcome.partition, outcome.pool
        );
        for a in &vote.writes {
            let at = vote
                .vote_on
                .iter()
                .position(|x| x == a)
                .expect("an axis written is an axis voted on");
            let stored = pack.axes[*a].stored(answer[at]).to_string();
            // An axis that already says something keeps it: a pass fills a
            // gap, it does not overrule the rules.
            let current = corpus.axes[i][*a].as_deref().unwrap_or("");
            let fills = current.is_empty() || vote.write_when.iter().any(|w| w == current);
            if !fills {
                continue;
            }
            axis_rows.push(vec![
                Param::Int(corpus.ids[i]),
                Param::from(pack.axes[*a].name.as_str()),
                Param::from(stored.to_string()),
                Param::Double(confidence),
                Param::from("vote"),
            ]);
            if pass.emit.evidence {
                evidence.push(vec![
                    Param::Int(corpus.ids[i]),
                    Param::from(pack.axes[*a].name.as_str()),
                    Param::from(stored.to_string()),
                    Param::from("vote"),
                    Param::Double(confidence),
                    Param::from(pass.name.as_str()),
                    Param::from(outcome.method.as_str()),
                    Param::from("neighbours"),
                    Param::from(cited.as_str()),
                    Param::from(pass.name.as_str()),
                    Param::from(pass.reference.scope.as_str()),
                ]);
            }
            if confidence < pass.emit.review_below || pass.emit.review_all_touched {
                reviews.push(vec![
                    Param::from(format!("{}:vote", pack.axes[*a].name)),
                    Param::from("stack"),
                    Param::from(serde_json::json!({"stack_id": corpus.ids[i]}).to_string()),
                    Param::from(
                        serde_json::json!({
                            "axis": pack.axes[*a].name,
                            "value": stored,
                            "confidence": confidence,
                            "pass": pass.name,
                            "method": outcome.method,
                            "matches": outcome.matches,
                            "neighbours": outcome.neighbours,
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
    let _ = Type::Int;
    Ok(ran)
}
