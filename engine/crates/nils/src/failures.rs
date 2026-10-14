// SPDX-License-Identifier: AGPL-3.0-only

//! A failure told by its kind (2026-10-12), at every door and command that
//! shows a run, its units or its job: `failure {kind, cause, advice, fix}`
//! beside the error, which is kept as it was written. A run and a unit
//! record theirs when they end; one that failed before this was recorded
//! is read from its error when it is shown. `nils_pipeline::failure` reads
//! an error for its cause.

use nils_pipeline::failure::{self, Cause, Kind, Said};
use serde_json::{Value, json};

/// The statuses of a unit that are failures: what a pipeline said failed,
/// and what it did not say anything of.
const FAILED: [&str; 2] = ["failed", "unreported"];

/// A unit's outcome given its failure where it has none: a unit that
/// failed or went unreported by the cause its error, its container's exit
/// and the end of its log say; one skipped for a missing input as input.
/// `log` is read only for a container that failed as a whole, with no
/// results to say more.
pub(crate) fn mark_unit(
    outcome: &mut Value,
    exit_code: Option<i64>,
    signal: Option<i32>,
    log: Option<&str>,
) {
    if !outcome["failure"].is_null() {
        return;
    }
    let status = outcome["status"].as_str().unwrap_or("failed");
    if status == "skipped" && outcome["missing"].is_array() {
        outcome["failure"] = Cause::Input.doc();
        return;
    }
    if !FAILED.contains(&status) {
        return;
    }
    let said = Said {
        error: outcome["error"].as_str().unwrap_or_default(),
        exit_code,
        signal,
        log,
    };
    outcome["failure"] = failure::cause_of(&said).doc();
}

/// A unit's failure as its row is shown: the one it recorded, else, for a
/// unit that failed before this was recorded, the one its error says.
pub(crate) fn of_unit(outcome: &Value) -> Value {
    if !outcome["failure"].is_null() {
        return outcome["failure"].clone();
    }
    let mut o = outcome.clone();
    mark_unit(&mut o, None, None, None);
    o["failure"].clone()
}

/// The failures of a run's units, counted by their cause, the most first:
/// `[{kind, cause, units}]`.
pub(crate) fn by_cause<'a>(outcomes: impl Iterator<Item = &'a Value>) -> Value {
    let mut counts: Vec<(Cause, usize)> = Vec::new();
    for o in outcomes {
        if !FAILED.contains(&o["status"].as_str().unwrap_or("failed")) {
            continue;
        }
        let cause = failure::recorded(&of_unit(o)).unwrap_or(Cause::Pipeline);
        match counts.iter_mut().find(|(c, _)| *c == cause) {
            Some((_, n)) => *n += 1,
            None => counts.push((cause, 1)),
        }
    }
    // the most first; of as many, a known cause before the pipeline's own
    counts.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then((a.0 == Cause::Pipeline).cmp(&(b.0 == Cause::Pipeline)))
            .then(a.0.name().cmp(b.0.name()))
    });
    json!(
        counts
            .iter()
            .map(|(c, n)| json!({"kind": c.kind().name(), "cause": c.name(), "units": n}))
            .collect::<Vec<_>>()
    )
}

/// The failure of a run that failed: the cause most of its units failed
/// for, where that is a known one (a dependency, a resource, an input);
/// else what its error and its exit say, the pipeline's own where nothing
/// is known. One unit's cause never names a run most of whose units
/// failed on their own.
pub(crate) fn of_run(error: &str, exit_code: Option<i64>, summary: &Value) -> Value {
    let known = summary["failures_by_cause"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|f| f["cause"].as_str())
        .and_then(Cause::parse)
        .filter(|c| c.kind() != Kind::Pipeline);
    known
        .unwrap_or_else(|| {
            failure::cause_of(&Said {
                error,
                exit_code,
                ..Said::default()
            })
        })
        .doc()
}

/// A run's failure as it is shown: the one it recorded, else, for a run
/// that failed before this was recorded, the one its error says; null for
/// a run that did not fail.
pub(crate) fn of_run_doc(doc: &Value) -> Value {
    if !doc["failure"].is_null() {
        return doc["failure"].clone();
    }
    if doc["status"] != "failed" {
        return Value::Null;
    }
    of_run(
        doc["error"].as_str().unwrap_or_default(),
        doc["exit_code"].as_i64(),
        &doc["summary"],
    )
}

/// A job's failure: a pipeline run's, as its result recorded it, else what
/// its error says, the pipeline's own for a pipeline job where nothing is
/// known; for a job of any other kind only a cause that is known, since
/// "the pipeline's own" says nothing of a digest or a release. Null for a
/// job that did not fail.
pub(crate) fn of_job(job: &Value) -> Value {
    if !job["result"]["failure"].is_null() {
        return job["result"]["failure"].clone();
    }
    if job["state"] != "failed" {
        return Value::Null;
    }
    let Some(error) = job["error"].as_str() else {
        return Value::Null;
    };
    // a run refused before it started names its cause in its words
    if let Some(cause) = failure::marked(error) {
        return cause.refusal_doc();
    }
    let said = Said::error(error);
    if job["kind"] == "pipeline" || job["result"]["run"].is_i64() {
        return failure::cause_of(&said).doc();
    }
    failure::known(&said).map_or(Value::Null, Cause::doc)
}

/// A job as the doors and `nils jobs` answer it: its row, and its failure.
pub(crate) fn job_doc(job: &nils_registry::job::Job) -> Value {
    let mut doc = job.as_json();
    doc["failure"] = of_job(&doc);
    doc
}

/// A failure as a person reads it on one line: `<kind> (<cause>): <advice>`.
pub(crate) fn line(f: &Value) -> Option<String> {
    let kind = f["kind"].as_str()?;
    Some(format!(
        "{kind} ({}): {}",
        f["cause"].as_str().unwrap_or(kind),
        f["advice"].as_str().unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_is_marked_by_its_cause_and_a_skipped_one_by_its_input() {
        let mut o = json!({"status": "failed", "error": "the container exited 137 and wrote no results.json"});
        mark_unit(&mut o, Some(137), None, None);
        assert_eq!(o["failure"]["cause"], "memory");
        let mut o =
            json!({"status": "skipped", "error": "missing an input: t1w", "missing": ["t1w"]});
        mark_unit(&mut o, None, None, None);
        assert_eq!(o["failure"]["kind"], "input");
        let mut o = json!({"status": "succeeded", "error": null});
        mark_unit(&mut o, Some(0), None, None);
        assert!(o["failure"].is_null());
        // a unit from before is read from its error when it is shown
        assert_eq!(
            of_unit(
                &json!({"status": "unreported", "error": "the pipeline's results say nothing of this unit"})
            )["kind"],
            "pipeline"
        );
    }

    #[test]
    fn a_run_fails_for_what_its_units_failed_for() {
        let outcomes = [
            json!({"status": "failed", "failure": Cause::Memory.doc()}),
            json!({"status": "failed", "failure": Cause::Memory.doc()}),
            json!({"status": "failed", "error": "refused on purpose"}),
            json!({"status": "succeeded"}),
        ];
        let counted = by_cause(outcomes.iter());
        assert_eq!(
            counted,
            json!([
                {"kind": "resource", "cause": "memory", "units": 2},
                {"kind": "pipeline", "cause": "pipeline", "units": 1},
            ])
        );
        let summary = json!({ "failures_by_cause": counted });
        assert_eq!(
            of_run("the container exited 1", Some(1), &summary)["cause"],
            "memory"
        );
        assert_eq!(
            of_run("the container exited 1", Some(1), &json!({}))["cause"],
            "pipeline"
        );
        // one unit's cause does not name a run most of whose units failed
        // on their own
        let mostly_own = [
            json!({"status": "failed", "failure": Cause::Memory.doc()}),
            json!({"status": "failed", "error": "refused on purpose"}),
            json!({"status": "failed", "error": "refused on purpose"}),
        ];
        let summary = json!({ "failures_by_cause": by_cause(mostly_own.iter()) });
        assert_eq!(
            of_run("the container exited 1", Some(1), &summary)["cause"],
            "pipeline"
        );
        // a run from before is read from its error
        let doc = json!({"status": "failed", "error": "the release of the input failed: dcm2niix is not runnable (No such file or directory (os error 2))", "failure": null});
        assert_eq!(of_run_doc(&doc)["cause"], "dcm2niix");
        assert!(of_run_doc(&json!({"status": "done"})).is_null());
    }

    #[test]
    fn a_job_says_its_run_s_failure_and_another_job_only_what_is_known() {
        let run_job = json!({"kind": "pipeline", "state": "failed", "error": "run 3 the container exited 2", "result": null});
        assert_eq!(of_job(&run_job)["kind"], "pipeline");
        let recorded = json!({"kind": "pipeline", "state": "failed", "error": "x", "result": {"run": 3, "failure": Cause::Disk.doc()}});
        assert_eq!(of_job(&recorded)["cause"], "disk");
        let release = json!({"kind": "release", "state": "failed", "error": "dcm2niix is not runnable (No such file or directory (os error 2))"});
        assert_eq!(of_job(&release)["cause"], "dcm2niix");
        let digest =
            json!({"kind": "digest", "state": "failed", "error": "the pack mri has no axis x"});
        assert!(of_job(&digest).is_null());
        assert!(of_job(&json!({"kind": "digest", "state": "done"})).is_null());
        assert_eq!(
            line(&Cause::Disk.doc()).unwrap(),
            format!("resource (disk): {}", Cause::Disk.advice())
        );
    }
}
