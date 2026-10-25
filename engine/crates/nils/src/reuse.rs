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

/// A kept result the unit reuses: the run it came from and the outcome the
/// unit records.
pub(crate) struct Reused {
    pub run: i64,
    pub outcome: Value,
}

/// Whether a derivative of a run belongs to this unit: its stack, or its
/// session or subject.
fn of_unit(d: &derivative::Derivative, of: &Of<'_>) -> bool {
    match (
        d.scope.as_str(),
        d.stack_id,
        d.subject_id,
        d.session_day.as_deref(),
    ) {
        ("stack", Some(s), _, _) => of.stacks.contains(&s),
        ("session", _, Some(s), Some(day)) => {
            of.subject_id == Some(s) && of.session_day == Some(day)
        }
        ("subject", _, Some(s), _) => of.subject_id == Some(s) && of.session_day.is_none(),
        _ => false,
    }
}

/// The kept result whose result key is `result_key`, where it may be
/// reused; none where no run kept one, or every kept one is refused (a
/// person rejected it, its pipeline version was replaced as wrong, its
/// files changed). The refusals are returned in the outcome's `considered`
/// for the run's words.
pub(crate) fn kept_for(
    store: &mut Store,
    of: &Of<'_>,
    result_key: &str,
) -> Result<Option<Reused>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let candidates = rows::kept_by_result_key(store, result_key).map_err(err)?;
    let places: std::collections::BTreeMap<i64, String> = nils_registry::place::list(store)
        .map_err(err)?
        .into_iter()
        .map(|p| (p.id, p.path))
        .collect();
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
        // every file as it was registered: the sha256 of each
        let mut changed: Option<String> = None;
        for d in &made {
            let Some(root) = places.get(&d.place_id) else {
                changed = Some(format!("the place {} of {} is gone", d.place_id, d.path));
                break;
            };
            let file = std::path::Path::new(root).join(&d.path);
            match std::fs::read(&file) {
                Ok(bytes) => {
                    // the registry keeps the bare hex, the engine writes it
                    // with its prefix: compared without either
                    let bare = |s: &str| s.trim_start_matches("sha256:").to_string();
                    let got = bare(&nils_pipeline::sha256(&bytes));
                    if got != bare(&d.sha256) {
                        changed = Some(format!("{} fails its sha256", d.path));
                        break;
                    }
                }
                Err(e) => {
                    changed = Some(format!("{}: {e}", d.path));
                    break;
                }
            }
        }
        if let Some(why) = changed {
            considered.push(json!({"run": run, "unit": unit, "reused": false, "why": why}));
            continue;
        }
        let files: Vec<&str> = made.iter().map(|d| d.path.as_str()).collect();
        return Ok(Some(Reused {
            run,
            outcome: json!({
                "status": "reused",
                "reused_from": run,
                "words": format!("reused from run {run} ({})", p.label()),
                "pipeline": p.label(),
                "derivatives": made.iter().map(|d| d.id).collect::<Vec<_>>(),
                "files": files,
                "metrics": {},
                "refused": [],
                "considered": considered,
            }),
        }));
    }
    Ok(None)
}
