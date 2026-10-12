// SPDX-License-Identifier: AGPL-3.0-only

//! What a run made, unit by unit (2026-10-12; Nima, trying the desk: "done-
//! fail should be per unit of job not the whole thing"). A run keeps its
//! status (done, partial, failed, cancelled, interrupted, running); what a
//! person reads of it is its units summed: how many made what they were for,
//! how many failed and for what, how many were skipped and what they lacked,
//! in a few words ("3 of 4 made · 1 skipped: no FLAIR picked"), and whether
//! it made nothing at all, as a run every unit of which was skipped did,
//! done though it is ("nothing made · 4 of 4 skipped: no FLAIR picked").
//!
//! A run answers it as `made`, and so does the job that ran it, from its
//! result; a pipeline job whose run was taken up again by a later job says
//! which (`taken_up`), so that nobody is asked to mend what a later job went
//! on past. A run that ended before its skipped units' reasons were summed
//! (`summary.skipped_by_why`) has them read from its units when it is shown.

use std::collections::BTreeMap;

use nils_registry::job;
use nils_registry::pipeline as rows;
use nils_registry::store::Store;
use serde_json::{Value, json};

/// How many of a run's units are read for the reasons its skipped units
/// lacked, where its summary did not sum them (a run from before).
const READ_MOST: usize = 2_000;

/// How many reasons a line names; the rest are counted.
const LINE_REASONS: usize = 3;

/// The statuses of a unit that failed.
const FAILED: [&str; 2] = ["failed", "unreported"];

/// A failure's cause in a few words, as a run's line says why units failed.
pub(crate) fn cause_words(cause: &str) -> String {
    match cause {
        "dcm2niix" => "no dcm2niix".into(),
        "runtime" => "the container runtime failed".into(),
        "subuid" => "too few subordinate ids".into(),
        "image" => "the image did not pull".into(),
        "gpu_driver" => "no GPU driver".into(),
        "memory" => "out of memory".into(),
        "disk" => "the disk is full".into(),
        "gpu_memory" => "out of GPU memory".into(),
        "lane" => "beyond the lane".into(),
        "time" => "out of time".into(),
        "input" => "an input refused".into(),
        "pipeline" => "the pipeline's own error".into(),
        other => other.replace('_', " "),
    }
}

/// What a skipped unit lacked, each reason once in a few words
/// ([`crate::preflight::brief`]): from the reasons it recorded, else from
/// its error.
pub(crate) fn lacks(outcome: &Value) -> Vec<String> {
    let long: Vec<String> = match outcome["missing"].as_array() {
        Some(list) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        None => outcome["error"]
            .as_str()
            .and_then(|e| e.strip_prefix("missing an input: "))
            .map(|e| e.split("; ").map(str::to_string).collect())
            .unwrap_or_default(),
    };
    let mut out: Vec<String> = Vec::new();
    for w in long {
        let b = crate::preflight::brief(&w);
        if !out.contains(&b) {
            out.push(b);
        }
    }
    out
}

/// Reasons with their counts, the most first, as `[{why, units}]`.
fn ranked(by: BTreeMap<String, usize>) -> Value {
    let mut list: Vec<(String, usize)> = by.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    json!(
        list.into_iter()
            .map(|(why, n)| json!({"why": why, "units": n}))
            .collect::<Vec<_>>()
    )
}

/// The skipped units among these outcomes, by what they lacked, the most
/// first: `[{why, units}]`, a unit counted under each reason it has.
pub(crate) fn skipped_by_why<'a>(outcomes: impl Iterator<Item = &'a Value>) -> Value {
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for o in outcomes.filter(|o| o["status"] == "skipped") {
        let said = lacks(o);
        if said.is_empty() {
            *by.entry("skipped".into()).or_default() += 1;
        }
        for why in said {
            *by.entry(why).or_default() += 1;
        }
    }
    ranked(by)
}

/// A run's skipped units by what they lacked, read from its units, for a
/// run whose summary did not sum them: at most [`READ_MOST`] units are read.
pub(crate) fn skipped_of_run(store: &mut Store, run_id: i64) -> Value {
    let units = rows::units(store, run_id).unwrap_or_default();
    skipped_by_why(units.iter().take(READ_MOST).map(|u| &u.outcome))
}

/// One count of a summary's units: none where it was withheld (null), and
/// 0 where it is not said at all, as a summary from before says nothing of
/// what it did not count.
fn counted(units: &Value, key: &str) -> Option<u64> {
    match units.get(key) {
        Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().unwrap_or(0)),
        None => Some(0),
    }
}

/// The counts a line is made of.
struct Counts {
    total: u64,
    made: Option<u64>,
    failed: Option<u64>,
    skipped: Option<u64>,
    left: u64,
}

/// The reasons of one status on a line: ": why", or with their counts where
/// there are several, ": why (3), why (1)", at most [`LINE_REASONS`] named.
fn reasons_words(why: &[Value], status: &str) -> String {
    let mine: Vec<&Value> = why.iter().filter(|w| w["status"] == status).collect();
    let said = |w: &Value| w["why"].as_str().unwrap_or_default().to_string();
    match mine.as_slice() {
        [] => String::new(),
        [one] => format!(": {}", said(one)),
        many => {
            let mut words: Vec<String> = many
                .iter()
                .take(LINE_REASONS)
                .map(|w| match w["units"].as_u64() {
                    Some(n) => format!("{} ({n})", said(w)),
                    None => said(w),
                })
                .collect();
            if many.len() > LINE_REASONS {
                words.push(format!("{} more", many.len() - LINE_REASONS));
            }
            format!(": {}", words.join(", "))
        }
    }
}

/// The line a person reads: the units summed, never a bare done. "4 of 4
/// made"; "3 of 4 made · 1 skipped: no FLAIR picked"; "nothing made · 4 of
/// 4 skipped: no FLAIR picked"; a count withheld below detail quasi is
/// "fewer than five".
fn line(c: &Counts, why: &[Value]) -> String {
    if c.total == 0 {
        return "no units".into();
    }
    let mut parts = vec![match c.made {
        Some(0) => "nothing made".to_string(),
        Some(m) => format!("{m} of {} made", c.total),
        None => format!("{} units", c.total),
    }];
    // after "nothing made" the first count says of how many
    let mut of_total = c.made == Some(0);
    for (status, n) in [("failed", c.failed), ("skipped", c.skipped)] {
        let reasons = reasons_words(why, status);
        match n {
            Some(0) => continue,
            Some(n) if of_total => {
                parts.push(format!("{n} of {} {status}{reasons}", c.total));
                of_total = false;
            }
            Some(n) => parts.push(format!("{n} {status}{reasons}")),
            None => parts.push(format!("fewer than five {status}{reasons}")),
        }
    }
    if c.left > 0 {
        parts.push(format!("{} left", c.left));
    }
    parts.join(" · ")
}

/// The document of what a run made.
fn doc(c: &Counts, why: Vec<Value>) -> Value {
    let nothing = c.total > 0 && c.made == Some(0) && c.left == 0;
    json!({
        "units": c.total,
        "made": c.made,
        "failed": c.failed,
        "skipped": c.skipped,
        "left": c.left,
        "nothing": nothing,
        "line": line(c, &why),
        "why": why,
    })
}

/// What a run made, from its summary's units, the reasons its units failed
/// (`failures_by_cause`) and those its skipped units lacked (`skipped`, or
/// the summary's `skipped_by_why`): null for a run that counted no units
/// yet (one running, or one that stopped before its units ended).
pub(crate) fn of_summary(summary: &Value, skipped: Option<&Value>) -> Value {
    let units = &summary["units"];
    let Some(total) = units["total"].as_u64() else {
        return Value::Null;
    };
    let made = counted(units, "succeeded");
    let failed = match (counted(units, "failed"), counted(units, "unreported")) {
        (Some(a), Some(b)) => Some(a + b),
        _ => None,
    };
    let skipped_n = counted(units, "skipped");
    let left = match (made, failed, skipped_n) {
        (Some(m), Some(f), Some(s)) => total.saturating_sub(m + f + s),
        _ => 0,
    };
    let mut why: Vec<Value> = Vec::new();
    for f in summary["failures_by_cause"]
        .as_array()
        .into_iter()
        .flatten()
    {
        why.push(json!({
            "status": "failed",
            "why": cause_words(f["cause"].as_str().unwrap_or("pipeline")),
            "units": f["units"],
        }));
    }
    let skipped_list = skipped.or_else(|| summary.get("skipped_by_why").filter(|v| v.is_array()));
    for s in skipped_list.and_then(Value::as_array).into_iter().flatten() {
        why.push(json!({"status": "skipped", "why": s["why"], "units": s["units"]}));
    }
    doc(
        &Counts {
            total,
            made,
            failed,
            skipped: skipped_n,
            left,
        },
        why,
    )
}

/// What a run made, from its units as they stand: each over with its
/// status, the rest left to run; null for a run that kept no units (one
/// from before record 49).
pub(crate) fn of_units(units: &[rows::Unit]) -> Value {
    if units.is_empty() {
        return Value::Null;
    }
    let (mut made, mut failed, mut skipped, mut left) = (0u64, 0u64, 0u64, 0u64);
    let mut causes: BTreeMap<String, usize> = BTreeMap::new();
    for u in units {
        if u.state != "over" {
            left += 1;
            continue;
        }
        match u.outcome["status"].as_str().unwrap_or("failed") {
            "succeeded" => made += 1,
            "skipped" => skipped += 1,
            s if FAILED.contains(&s) => {
                failed += 1;
                let f = crate::failures::of_unit(&u.outcome);
                *causes
                    .entry(cause_words(f["cause"].as_str().unwrap_or("pipeline")))
                    .or_default() += 1;
            }
            _ => failed += 1,
        }
    }
    let mut why: Vec<Value> = Vec::new();
    for f in ranked(causes).as_array().into_iter().flatten() {
        why.push(json!({"status": "failed", "why": f["why"], "units": f["units"]}));
    }
    for s in skipped_by_why(
        units
            .iter()
            .filter(|u| u.state == "over")
            .map(|u| &u.outcome),
    )
    .as_array()
    .into_iter()
    .flatten()
    {
        why.push(json!({"status": "skipped", "why": s["why"], "units": s["units"]}));
    }
    doc(
        &Counts {
            total: units.len() as u64,
            made: Some(made),
            failed: Some(failed),
            skipped: Some(skipped),
            left,
        },
        why,
    )
}

/// What a run made as a door shows it below detail quasi: from its summary
/// as [`crate::pipelines::summary_totals_only`] holds it, a count of 1 to 4
/// failed units (and the made count with it) withheld, the reasons kept,
/// since they are the engine's own words and name no unit.
pub(crate) fn held(made: &Value, summary: &Value) -> Value {
    if made.is_null() {
        return Value::Null;
    }
    let skipped: Vec<Value> = made["why"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["status"] == "skipped")
        .map(|w| json!({"why": w["why"], "units": w["units"]}))
        .collect();
    of_summary(summary, Some(&json!(skipped)))
}

/// The run a pipeline job ran or takes up again: its result's, the run it
/// claimed, or the run its command line names with --resume.
fn run_of(doc: &Value) -> Option<i64> {
    doc["result"]["run"]
        .as_i64()
        .or_else(|| doc["args"]["run"].as_i64())
        .or_else(|| doc["args"]["resume"].as_i64())
        .or_else(|| resumed_by(&doc["args"]))
}

/// The run a job's command line takes up again (`run --resume <run>`).
fn resumed_by(args: &Value) -> Option<i64> {
    let words: Vec<&str> = args["queued"]
        .as_array()
        .or_else(|| args["argv"].as_array())?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let at = words.iter().position(|w| *w == "--resume")?;
    words.get(at + 1)?.parse().ok()
}

/// Whether a job's document is of a pipeline run.
fn a_run(doc: &Value) -> bool {
    doc["kind"] == "pipeline" || doc["result"]["run"].is_i64()
}

/// Pipeline jobs as the doors and `nils jobs` show them (2026-10-12): what
/// each made, from its result (`made`), and, where its run was taken up
/// again by a later job, that job and its state (`taken_up`), a job queued
/// to take it up included before it claims the run.
pub(crate) fn jobs(store: &mut Store, docs: &mut [Value]) {
    if !docs.iter().any(a_run) {
        return;
    }
    // the runs a job not over yet takes up again, by run: the newest
    let mut queued: BTreeMap<i64, (i64, String)> = BTreeMap::new();
    for j in job::list(store, false, 200).unwrap_or_default() {
        if j.kind != "pipeline" {
            continue;
        }
        if let Some(run) = j.args["resume"].as_i64().or_else(|| resumed_by(&j.args)) {
            let e = queued
                .entry(run)
                .or_insert((j.id, j.state.name().to_string()));
            if j.id > e.0 {
                *e = (j.id, j.state.name().to_string());
            }
        }
    }
    let mut runs: BTreeMap<i64, Option<rows::Run>> = BTreeMap::new();
    for doc in docs.iter_mut().filter(|d| a_run(d)) {
        let (Some(id), Some(run_id)) = (doc["id"].as_i64(), run_of(doc)) else {
            continue;
        };
        let run = runs
            .entry(run_id)
            .or_insert_with(|| rows::run(store, run_id).ok().flatten())
            .clone();
        let current = run.as_ref().is_some_and(|r| r.job_id == Some(id));
        // what it made, as its result says it
        if doc["result"]["summary"]["units"].is_object() {
            let summed = doc["result"]["summary"].get("skipped_by_why").cloned();
            let skipped = match summed {
                Some(v) if v.is_array() => Some(v),
                _ if current
                    && doc["result"]["summary"]["units"]["skipped"]
                        .as_u64()
                        .is_some_and(|n| n > 0) =>
                {
                    Some(skipped_of_run(store, run_id))
                }
                _ => None,
            };
            doc["made"] = of_summary(&doc["result"]["summary"], skipped.as_ref());
        } else {
            doc["made"] = Value::Null;
        }
        // a later job that took its run up again
        let later = run
            .as_ref()
            .and_then(|r| r.job_id)
            .filter(|j| *j > id)
            .map(|j| {
                let state = job::show(store, j)
                    .ok()
                    .flatten()
                    .map(|x| x.state.name().to_string())
                    .unwrap_or_default();
                (j, state)
            })
            .or_else(|| queued.get(&run_id).filter(|(j, _)| *j > id).cloned());
        doc["taken_up"] = match later {
            Some((j, state)) => json!({"job": j, "state": state, "run": run_id}),
            None => Value::Null,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_reads_as_its_units_summed_never_a_bare_done() {
        let skipped = json!([{"why": "no FLAIR picked", "units": 4}]);
        let nothing = of_summary(
            &json!({"units": {"total": 4, "succeeded": 0, "failed": 0, "skipped": 4, "unreported": 0, "missing_input": 4}}),
            Some(&skipped),
        );
        assert_eq!(nothing["nothing"], true, "{nothing}");
        assert_eq!(
            nothing["line"],
            "nothing made · 4 of 4 skipped: no FLAIR picked"
        );
        assert_eq!(nothing["made"], 0);
        let partly = of_summary(
            &json!({"units": {"total": 4, "succeeded": 3, "failed": 0, "skipped": 1},
                    "skipped_by_why": [{"why": "no FLAIR picked", "units": 1}]}),
            None,
        );
        assert_eq!(partly["nothing"], false);
        assert_eq!(partly["line"], "3 of 4 made · 1 skipped: no FLAIR picked");
        let all = of_summary(&json!({"units": {"total": 4, "succeeded": 4}}), None);
        assert_eq!(all["line"], "4 of 4 made");
        // units that failed say for what, the most first, and a count held
        // below detail quasi reads fewer than five
        let failed = of_summary(
            &json!({"units": {"total": 9, "succeeded": 0, "failed": 2, "unreported": 1, "skipped": 6},
                    "failures_by_cause": [{"kind": "resource", "cause": "memory", "units": 2},
                                          {"kind": "pipeline", "cause": "pipeline", "units": 1}],
                    "skipped_by_why": [{"why": "no FLAIR picked", "units": 4}, {"why": "no T1w picked", "units": 2}]}),
            None,
        );
        assert_eq!(
            failed["line"],
            "nothing made · 3 of 9 failed: out of memory (2), the pipeline's own error (1) · 6 skipped: no FLAIR picked (4), no T1w picked (2)"
        );
        let held = of_summary(
            &json!({"units": {"total": 20, "succeeded": null, "failed": null, "skipped": 0}}),
            None,
        );
        assert_eq!(held["line"], "20 units · fewer than five failed");
        assert_eq!(held["nothing"], false);
        // nothing counted yet
        assert!(of_summary(&json!({"phase": "run"}), None).is_null());
    }

    #[test]
    fn a_skipped_unit_says_what_it_lacked_in_a_few_words() {
        let o = json!({"status": "skipped", "error": "missing an input: no flair is picked for it among the selection's stacks",
                       "missing": ["no flair is picked for it among the selection's stacks", "no t1w is picked for it among the selection's stacks"]});
        assert_eq!(lacks(&o), vec!["no FLAIR picked", "no T1w picked"]);
        // a unit from before that kept only its error
        let old = json!({"status": "skipped", "error": "missing an input: no flair is picked for it among the selection's stacks"});
        assert_eq!(lacks(&old), vec!["no FLAIR picked"]);
        let outcomes = [o.clone(), old, json!({"status": "succeeded"})];
        assert_eq!(
            skipped_by_why(outcomes.iter()),
            json!([{"why": "no FLAIR picked", "units": 2}, {"why": "no T1w picked", "units": 1}])
        );
    }

    #[test]
    fn a_job_names_the_run_it_takes_up_again() {
        let queued = json!({"queued": ["run", "--resume", "7", "--pack-dir", "/packs"]});
        assert_eq!(resumed_by(&queued), Some(7));
        assert_eq!(resumed_by(&json!({"queued": ["run", "n4@1"]})), None);
        let doc = json!({"kind": "pipeline", "args": {"run": 3}, "result": null});
        assert_eq!(run_of(&doc), Some(3));
        assert!(a_run(&doc));
        assert!(!a_run(&json!({"kind": "digest", "result": null})));
    }
}
