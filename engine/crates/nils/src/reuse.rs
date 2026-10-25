// SPDX-License-Identifier: AGPL-3.0-only

//! Results kept and reused (Wave 7b A7, section 8.5, record 57 P8; G6): a
//! step whose result key matches a kept result is not run again; the unit
//! records "reused from run N" and links the result. A result is not reused
//! when a person rejected it in a check (job type 3), when a newer version
//! replaced its pipeline as wrong (`replaces`), or when its files fail
//! their sha256.

use nils_registry::Store;
use nils_registry::derivative;
use nils_registry::pipeline::{self as rows, Pipeline};
use serde_json::{Value, json};

/// The unit a run is about to run, as the reuse looks for its kept result.
pub(crate) struct Of<'a> {
    pub run_id: i64,
    pub pipeline: &'a Pipeline,
    pub unit: &'a str,
    pub stacks: &'a [i64],
    pub subject_id: Option<i64>,
    pub session_day: Option<&'a str>,
}

/// Whether a derivative of a run belongs to this unit: its stack, or its
/// session or subject.
fn of_unit(d: &derivative::Derivative, of: &Of<'_>) -> bool {
    belongs(d, of.stacks, of.subject_id, of.session_day)
}

/// Whether a derivative belongs to a unit given by its stacks, its subject
/// and its session's day (none for a subject's or a stack's unit): a stack
/// derivative by its stack, a session's by subject and day, a subject's by
/// the subject of a subject unit. The reuse and the provenance door agree
/// on it (after review, finding 1 of the second review).
pub(crate) fn belongs(
    d: &derivative::Derivative,
    stacks: &[i64],
    subject_id: Option<i64>,
    session_day: Option<&str>,
) -> bool {
    match (
        d.scope.as_str(),
        d.stack_id,
        d.subject_id,
        d.session_day.as_deref(),
    ) {
        ("stack", Some(s), _, _) => stacks.contains(&s),
        ("session", _, Some(s), Some(day)) => subject_id == Some(s) && session_day == Some(day),
        ("subject", _, Some(s), _) => subject_id == Some(s) && session_day.is_none(),
        _ => false,
    }
}

/// A kept result the unit reuses: the run it came from and the outcome the
/// unit records.
pub(crate) struct Reused {
    pub run: i64,
    pub outcome: Value,
}

/// Why kept results were not reused, each `{run, unit, reused: false,
/// why}`: what the unit records under `reuse.considered` where none is
/// reused, and what a reused unit's outcome carries of the ones passed
/// over (after review, finding 8 of the second review).
pub(crate) type Refusals = Vec<Value>;

/// A kept result a unit may reuse once its files prove to be as they were
/// registered (after review, finding 9): found without reading a file, so
/// the run's start reads none; the scheduler checks the files in its pool,
/// in parallel, where the unit would otherwise start a container.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub run: i64,
    pipeline: String,
    derivatives: Vec<i64>,
    /// Each kept file: its registered path, where it is, and its sha256 as
    /// registered (bare hex).
    files: Vec<(String, std::path::PathBuf, String)>,
    considered: Vec<Value>,
}

impl Candidate {
    /// Every file as it was registered: the sha256 of each, read through
    /// now (streamed, never the file whole in memory).
    pub(crate) fn verify(&self) -> Result<(), String> {
        for (path, file, sha) in &self.files {
            match nils_pipeline::files::sha256_file(file) {
                Ok((_, got)) => {
                    // the registry keeps the bare hex, the engine writes it
                    // with its prefix: compared without either
                    if bare(&got) != *sha {
                        return Err(format!("{path} fails its sha256"));
                    }
                }
                Err(e) => return Err(format!("{path}: {e}")),
            }
        }
        Ok(())
    }

    /// The refusal a unit records when this candidate's files failed.
    pub(crate) fn refused(&self, why: &str) -> Refusals {
        let mut out = self.considered.clone();
        out.push(json!({"run": self.run, "reused": false, "why": why}));
        out
    }

    /// The outcome the unit records once the files proved good.
    pub(crate) fn outcome(&self) -> Value {
        json!({
            "status": "reused",
            "reused_from": self.run,
            "words": format!("reused from run {} ({})", self.run, self.pipeline),
            "pipeline": self.pipeline,
            "derivatives": self.derivatives,
            "files": self.files.iter().map(|(p, ..)| p.as_str()).collect::<Vec<_>>(),
            "metrics": {},
            "refused": [],
            "considered": self.considered,
        })
    }
}

fn bare(s: &str) -> String {
    s.trim_start_matches("sha256:").to_string()
}

/// The registry's places by id, read once for a run.
pub(crate) fn places(store: &mut Store) -> Result<std::collections::BTreeMap<i64, String>, String> {
    Ok(nils_registry::place::list(store)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|p| (p.id, p.path))
        .collect())
}

/// The kept result whose result key is `result_key`, where it may be
/// reused, its files not yet read; else the refusals, one for every kept
/// one passed over (a person rejected it, its pipeline version was replaced
/// as wrong, a place of its files is gone), empty where no run kept one.
/// The refusals go into the outcome's `considered` for the run's words.
pub(crate) fn candidate(
    store: &mut Store,
    places: &std::collections::BTreeMap<i64, String>,
    of: &Of<'_>,
    result_key: &str,
) -> Result<Result<Candidate, Refusals>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let candidates = rows::kept_by_result_key(store, result_key).map_err(err)?;
    let mut considered: Vec<Value> = Vec::new();
    for (run, _unit_row, unit, pipeline_id) in candidates {
        if run == of.run_id {
            continue;
        }
        if unit != of.unit {
            // the same key under another unit's name: a unit is named by
            // what it reads, so this is another unit's result
            considered.push(json!({
                "run": run, "unit": unit, "reused": false,
                "why": format!("the key is of unit {unit}, not {}", of.unit),
            }));
            continue;
        }
        // a newer version that replaces the result's pipeline as wrong
        let p = match rows::get(store, pipeline_id).map_err(err)? {
            Some(p) => p,
            None => continue,
        };
        if p.name != of.pipeline.name {
            considered.push(json!({
                "run": run, "unit": unit, "reused": false,
                "why": format!("the result is {}'s, not {}'s", p.label(), of.pipeline.label()),
            }));
            continue;
        }
        let standing = crate::versions::of_one(store, &p).map_err(err)?;
        if standing.replaced() {
            considered.push(json!({
                "run": run, "unit": unit, "reused": false,
                "why": format!(
                    "{} was replaced{}",
                    p.label(),
                    standing.replaced_by.as_ref().map_or(String::new(), |(_, l)| format!(" by {l}")),
                ),
            }));
            continue;
        }
        let made: Vec<derivative::Derivative> = derivative::list(
            store,
            &derivative::Filter {
                run_id: Some(run),
                limit: 10_000,
                ..Default::default()
            },
        )
        .map_err(err)?
        .into_iter()
        .filter(|d| d.withdrawn_at.is_none() && of_unit(d, of))
        .collect();
        if made.is_empty() {
            considered.push(json!({
                "run": run, "unit": unit, "reused": false,
                "why": "the run kept no file of this unit",
            }));
            continue;
        }
        if let Some(d) = made
            .iter()
            .find(|d| d.check_status.as_deref() == Some("rejected"))
        {
            considered.push(json!({
                "run": run, "unit": unit, "reused": false,
                "why": format!("a person rejected its result {} ({}) in a check", d.id, d.path),
            }));
            continue;
        }
        let mut files = Vec::new();
        let mut gone: Option<String> = None;
        for d in &made {
            let Some(root) = places.get(&d.place_id) else {
                gone = Some(format!("the place {} of {} is gone", d.place_id, d.path));
                break;
            };
            files.push((
                d.path.clone(),
                std::path::Path::new(root).join(&d.path),
                bare(&d.sha256),
            ));
        }
        if let Some(why) = gone {
            considered.push(json!({"run": run, "unit": unit, "reused": false, "why": why}));
            continue;
        }
        return Ok(Ok(Candidate {
            run,
            pipeline: p.label(),
            derivatives: made.iter().map(|d| d.id).collect(),
            files,
            considered,
        }));
    }
    Ok(Err(considered))
}

/// [`candidate`] with its files checked here and now: the kept result the
/// unit reuses, or why none is (the refusals, the sha256 failure among
/// them). Where the newest candidate's files fail, no older one is looked
/// at: a result whose files changed is one to run again.
pub(crate) fn kept_for(
    store: &mut Store,
    places: &std::collections::BTreeMap<i64, String>,
    of: &Of<'_>,
    result_key: &str,
) -> Result<Result<Reused, Refusals>, String> {
    let c = match candidate(store, places, of, result_key)? {
        Ok(c) => c,
        Err(refusals) => return Ok(Err(refusals)),
    };
    match c.verify() {
        Ok(()) => Ok(Ok(Reused {
            run: c.run,
            outcome: c.outcome(),
        })),
        Err(why) => Ok(Err(c.refused(&why))),
    }
}
