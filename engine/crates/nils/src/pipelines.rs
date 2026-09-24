// SPDX-License-Identifier: AGPL-3.0-only

//! Pipelines at the keyboard, the doors and the queue (record 43 S1 to S3).
//!
//! The catalog: `nils pipeline add <nils.job.yml>` checks a descriptor
//! (`contracts/job/v1`, through `nils-pipeline`) and keeps it as the name's
//! next version, refusing an image not pinned by its registry manifest
//! digest; `list` and `show` read it back.
//!
//! The runner: `nils run <pipeline> --select selection:<name>@<v>` is one run
//! over a frozen selection, a job of kind `pipeline` that the queue's worker
//! runs one at a time (R1), queued at `POST /api/jobs` under `pipelines:work`
//! at detail quasi, since a pipeline reads pixels. In order:
//!
//! 1. The descriptor, its parameters with the defaults filled, the models
//!    and the label set it reads, the working place and a runtime (rootless
//!    podman, apptainer, docker only by an operator's choice; none is the
//!    capability off, D1), and the GPU it asks for.
//! 2. The selection frozen to a handle, which the run pins.
//! 3. The input materialised under `<working>/runs/<run>/`: a release in
//!    the BIDS layout with the picks applied (R4), or `stacks.json` naming
//!    each stack's files under the source places, mounted read-only.
//! 4. The container: no network, the input and the typed inputs read-only,
//!    one output folder, `<working>/derivatives/<pipeline>/<run>/`, and a
//!    process that is not root on the host.
//! 5. After it exits: `results.json` (or, without one, each unit's files
//!    found by the declared templates), every file hashed by the engine and
//!    registered as a derivative naming the run, the proposals passed to
//!    the review spine's hook, a `pipeline:qc` review item for each unit
//!    that failed or went unreported, and the run closed with the digest of
//!    what it said.
//!
//! The doors read the catalog and the runs; the derivatives a run made are
//! served by the derivative doors, by bytes or by a shared path (S3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nils_pipeline::descriptor::{self, Descriptor, Gpu, Layout, Level, Units};
use nils_pipeline::lane;
use nils_pipeline::runtime::{self, Choice, Detected, ImageForm, Invocation, Mount, Runtime};
use nils_pipeline::secrets;
use nils_registry::derivative::{self, Belongs};
use nils_registry::home::Home;
use nils_registry::job;
use nils_registry::pipeline::{self as rows, Pipeline, Run};
use nils_registry::schema::Type;
use nils_registry::store::Store;
use nils_registry::{Param, Registry};
use serde_json::{Value, json};

use crate::serve::Reply;
use crate::{Exit, fail, usage};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 4] = [
    "GET /api/pipelines",
    "GET /api/pipelines/{id}",
    "GET /api/pipeline-runs",
    "GET /api/pipeline-runs/{id}",
];

/// Where the registry keeps the operator's choice of runtime.
pub(crate) const RUNTIME_KEY: &str = "pipeline_runtime";

/// Where a run's input, typed inputs and log go under the working place.
pub(crate) const RUNS: &str = "runs";

/// Where apptainer's copies of images are kept under the working place, by
/// digest (record 49 A2).
pub(crate) const IMAGES: &str = "images";

/// The lane's settings in the registry (record 49 A1, A2): its cores, its
/// GB of memory, the card a GPU unit leases (`none` for no card), and how
/// apptainer keeps an image.
pub(crate) const LANE_CORES_KEY: &str = "pipeline_lane_cores";
pub(crate) const LANE_MEMORY_KEY: &str = "pipeline_lane_memory_gb";
pub(crate) const GPU_CARD_KEY: &str = "pipeline_gpu_card";
pub(crate) const APPTAINER_IMAGE_KEY: &str = "pipeline_apptainer_image";

/// A secret's file, as the site set it, under this prefix and its id
/// (record 49 R3): the path, never the bytes.
pub(crate) const SECRET_PREFIX: &str = "pipeline_secret:";

/// How apptainer keeps an image here: `sif` until set.
pub(crate) fn image_form(registry: &mut Registry) -> ImageForm {
    registry
        .meta_value(APPTAINER_IMAGE_KEY)
        .ok()
        .flatten()
        .and_then(|v| ImageForm::parse(&v))
        .unwrap_or(ImageForm::Sif)
}

/// The operator's choice of runtime: `auto` until one is set.
pub(crate) fn choice(registry: &mut Registry) -> Choice {
    registry
        .meta_value(RUNTIME_KEY)
        .ok()
        .flatten()
        .and_then(|v| Choice::parse(&v))
        .unwrap_or(Choice::Auto)
}

/// What the look for a runtime found, kept half a minute, since the
/// capabilities are read often and asking podman is not free.
pub(crate) fn detect_cached(registry: &mut Registry) -> Detected {
    static CACHE: Mutex<Option<(Instant, Choice, Detected)>> = Mutex::new(None);
    let chosen = choice(registry);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, c, d)) = cache.as_ref()
        && *c == chosen
        && at.elapsed() < Duration::from_secs(30)
    {
        return d.clone();
    }
    let d = runtime::detect(chosen, std::env::var_os("PATH").as_deref());
    // a runtime that did not answer in time is not a finding to keep: the
    // next read looks again (wave 43's proof)
    *cache = (!d.unknown).then(|| (Instant::now(), chosen, d.clone()));
    d
}

/// What `GET /api/capabilities` says of pipelines: on when a runtime and a
/// working place are both there, off with the sentence that names the cure
/// when either is not (D1).
pub(crate) fn capability(registry: &mut Registry) -> Value {
    let d = detect_cached(registry);
    let extra = lane_doc(registry);
    capability_with(registry.store(), &d, Some(extra))
}

/// The capability with the lane (record 49): its budget, its card, how
/// apptainer keeps images, and the secrets the site set, by id alone.
fn capability_with(store: &mut Store, d: &Detected, extra: Option<Value>) -> Value {
    let mut v = capability_base(store, d);
    if let Some(Value::Object(more)) = extra {
        for (k, x) in more {
            v[k] = x;
        }
    }
    v
}

/// The secrets the site set, by id.
fn secret_ids(store: &mut Store) -> Vec<String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT key, value FROM {} WHERE key LIKE {} ORDER BY key",
        store.qualified("registry_meta"),
        d.param(1, Type::Text)
    );
    store
        .query(&sql, &[Param::from(format!("{SECRET_PREFIX}%"))])
        .unwrap_or_default()
        .iter()
        .filter(|r| r.text(1).is_ok_and(|v| !v.trim().is_empty()))
        .filter_map(|r| {
            r.text(0)
                .ok()
                .and_then(|k| k.strip_prefix(SECRET_PREFIX))
                .map(str::to_string)
        })
        .collect()
}

/// What the lane adds to the capability.
fn lane_doc(registry: &mut Registry) -> Value {
    let lane = lane_of(registry);
    let form = image_form(registry);
    json!({
        "lane": lane.doc(),
        "apptainer_image": form.name(),
        "secrets": secret_ids(registry.store()),
    })
}

fn capability_base(store: &mut Store, d: &Detected) -> Value {
    let place = crate::derivatives::working(store);
    let catalog = rows::list(store)
        .map(|l| l.iter().filter(|p| p.state == "active").count())
        .unwrap_or(0);
    let reason = d.reason.clone().or_else(|| {
        place
            .as_ref()
            .err()
            .map(|m| m.replacen("derivatives are off", "pipelines are off", 1))
    });
    json!({
        "enabled": d.runtime.is_some() && place.is_ok(),
        "reason": reason,
        "runtime": d.runtime.as_ref().map(|r| json!({
            "name": r.kind.name(), "version": r.version, "gpu": r.gpu,
        })),
        "choice": d.choice.name(),
        "looked": d.looked.iter().map(|(k, s)| json!({"runtime": k, "found": s})).collect::<Vec<_>>(),
        "place": place.ok().map(|p| p.name),
        "pipelines": catalog,
        "contract": nils_pipeline::CONTRACT,
        "grants": {"see": "pipelines:see", "run": "pipelines:work", "run_detail": "quasi"},
    })
}

/// A catalog entry as the doors and `--json` answer it.
pub(crate) fn pipeline_doc(p: &Pipeline) -> Value {
    let x = &p.descriptor["x-nils"];
    json!({
        "id": p.id, "name": p.name, "version": p.version, "label": p.label(),
        "tool_version": p.tool_version, "description": p.descriptor["description"],
        "image": p.image, "image_digest": p.image_digest,
        "descriptor_digest": p.descriptor_digest, "layout": p.layout, "level": p.level,
        "state": p.state, "added_by": p.added_by, "added_at": p.added_at,
        "origin": p.origin, "starter": p.origin.as_deref() == Some(rows::STARTER),
        "checks": x["qc"], "roles": x["input"]["roles"],
        "parameters": p.descriptor["inputs"], "inputs": x["inputs"], "outputs": x["outputs"],
        "needs": x["needs"], "proposals": x["proposals"],
        "descriptor": p.descriptor,
    })
}

/// A run as the doors and `--json` answer it: the row, the pipeline by its
/// label, and the derivatives it made.
pub(crate) fn run_doc(store: &mut Store, r: &Run) -> Value {
    let mut doc = runs_docs(store, std::slice::from_ref(r))
        .pop()
        .unwrap_or(Value::Null);
    // record 49 A1: each unit as the lane scheduled it
    let units = rows::units(store, r.id).unwrap_or_default();
    let mut by_state: BTreeMap<String, usize> = BTreeMap::new();
    for u in &units {
        *by_state.entry(u.state.clone()).or_default() += 1;
    }
    doc["unit_states"] = json!(by_state);
    doc["units_run"] = json!(
        units
            .iter()
            .map(|u| json!({
                "unit": u.unit, "state": u.state, "status": u.outcome["status"],
                "attempts": u.attempts, "device": u.device, "gpu_card": u.gpu_card,
                "cores": u.cores, "memory_mb": u.memory_mb, "exit_code": u.exit_code,
                "started_at": u.started_at, "finished_at": u.finished_at,
            }))
            .collect::<Vec<_>>()
    );
    doc
}

/// The most runs one page of `GET /api/pipeline-runs` answers.
pub(crate) const RUNS_PAGE_MAX: usize = 500;

/// Many runs as the doors answer them, the catalog and the derivatives
/// they made read once for all of them, not once per run.
pub(crate) fn runs_docs(store: &mut Store, runs: &[Run]) -> Vec<Value> {
    let labels: BTreeMap<i64, String> = rows::list(store)
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.id, p.label()))
        .collect();
    let mut made: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for chunk in runs.chunks(500) {
        let list = chunk
            .iter()
            .map(|r| r.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT run_id, id FROM {} WHERE run_id IN ({list}) ORDER BY id",
            store.qualified("derivative")
        );
        for r in store.query(&sql, &[]).unwrap_or_default() {
            if let (Ok(run), Ok(id)) = (r.int(0), r.int(1)) {
                made.entry(run).or_default().push(id);
            }
        }
    }
    runs.iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
            v["pipeline"] = json!(labels.get(&r.pipeline_id));
            v["derivatives"] = json!(made.get(&r.id).cloned().unwrap_or_default());
            v
        })
        .collect()
}

/// A run read at detail plain (record 43 review): a unit named by its
/// subject or session, `sub-<s>` or `sub-<s>_ses-<t>`, is quasi
/// identifying, so every such text in the document is left out.
pub(crate) fn plain(v: &mut Value) {
    match v {
        Value::String(t) if t.starts_with("sub-") => *v = Value::Null,
        Value::Array(items) => items.iter_mut().for_each(plain),
        Value::Object(map) => map.values_mut().for_each(plain),
        _ => {}
    }
}

/// The fewest scans a count below detail quasi stands for (record 49 R4b,
/// the ask's own k): a count of 1 to 4 is withheld, none is still none.
const SCANS_K: usize = nils_ask::validate::MEASURE_K as usize;

/// What says that something was left out below detail quasi.
const WITHHELD: &str = "withheld below detail quasi";

/// A count of scans below detail quasi, under `key`: the count, or null
/// and marked withheld when it stands for 1 to 4 scans.
fn held(key: &str, n: usize) -> Value {
    if n == 0 || n >= SCANS_K {
        json!({ key: n })
    } else {
        json!({ key: null, "withheld": true })
    }
}

/// Counts under their names, each held to [`SCANS_K`], as a list.
fn held_list(name: &str, key: &str, counts: BTreeMap<String, usize>) -> Vec<Value> {
    counts
        .into_iter()
        .map(|(what, n)| {
            let mut v = held(key, n);
            v[name] = json!(what);
            v
        })
        .collect()
}

/// A run's summary read below detail quasi (record 49 R4 and R4b, after
/// the assistant's review): its checks and its failures as counts by check
/// and by reason, each count held to 5 scans, and no unit, no scan's value
/// and no error text a tool wrote. What was held is named in `withheld`.
pub(crate) fn summary_totals_only(s: &mut Value) {
    if !s.is_object() {
        return;
    }
    let mut withheld: Vec<&str> = Vec::new();
    // the breaches: a unit and its values become counts by check
    let mut by_check: BTreeMap<String, usize> = BTreeMap::new();
    let mut metric_of: BTreeMap<String, Value> = BTreeMap::new();
    for u in s["breaches"].as_array().into_iter().flatten() {
        for b in u["breaches"].as_array().into_iter().flatten() {
            let check = b["check"]
                .as_str()
                .or_else(|| b["metric"].as_str())
                .unwrap_or("a check")
                .to_string();
            *by_check.entry(check.clone()).or_default() += 1;
            metric_of
                .entry(check)
                .or_insert_with(|| b["metric"].clone());
        }
    }
    let mut checks = held_list("check", "units", by_check);
    for c in &mut checks {
        c["metric"] = metric_of
            .get(c["check"].as_str().unwrap_or_default())
            .cloned()
            .unwrap_or(Value::Null);
    }
    s["breaches"] = json!([]);
    s["breaches_by_check"] = json!(checks);
    if let Some(n) = s["numbers"]["checks"]["breaches"].as_u64()
        && held("n", n as usize)["withheld"] == true
    {
        s["numbers"]["checks"]["breaches"] = Value::Null;
        withheld.push("numbers.checks.breaches");
    }
    // a cell a table could not read is said as its value: counted by column
    if let Some(list) = s["numbers"]["tables"]["refused_values"].as_array() {
        let mut by_column: BTreeMap<String, usize> = BTreeMap::new();
        for v in list {
            let t = v.as_str().unwrap_or_default();
            // `output: column: value`, the value dropped
            let column = t.splitn(3, ": ").take(2).collect::<Vec<_>>().join(": ");
            *by_column.entry(column).or_default() += 1;
        }
        s["numbers"]["tables"]["refused_values"] = json!(held_list("column", "values", by_column));
    }
    // the failures: counted by reason, never a unit or a tool's words
    let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();
    for reason in ["failed", "unreported"] {
        let n = s["units"][reason].as_u64().unwrap_or(0) as usize;
        if n > 0 {
            by_reason.insert(reason.to_string(), n);
        }
    }
    let mut refused_units = std::collections::BTreeSet::new();
    if let Some(list) = s["refused_files"].as_array_mut() {
        // a refusal of a unit's file names the unit and its file; one of
        // the run's own, a model's output, names neither and stays
        list.retain(|r| match r["unit"].as_str() {
            Some(u) => {
                refused_units.insert(u.to_string());
                false
            }
            None => true,
        });
    }
    if !refused_units.is_empty() {
        by_reason.insert("refused".into(), refused_units.len());
    }
    // a count of failed units held, and the count of those that succeeded
    // with it, since the total less it says the same number
    let mut held_units = false;
    for reason in ["failed", "unreported"] {
        if let Some(n) = s["units"][reason].as_u64()
            && held("n", n as usize)["withheld"] == true
        {
            s["units"][reason] = Value::Null;
            held_units = true;
        }
    }
    if held_units && s["units"]["succeeded"].is_u64() {
        s["units"]["succeeded"] = Value::Null;
    }
    if held_units {
        withheld.push("units");
    }
    // the review items a run raised are one a unit: their ids are a count
    if let Some(ids) = s["review_items"].as_array() {
        let n = ids.len();
        s["review_items"] = json!(n);
        if held("n", n)["withheld"] == true {
            s["review_items"] = Value::Null;
            withheld.push("review_items");
        }
    }
    s["failures_by_reason"] = json!(held_list("reason", "units", by_reason));
    s["detail"] = json!("totals");
    s["withheld"] = json!(withheld);
}

/// A run's document read below detail quasi: its summary as
/// [`summary_totals_only`] says, no unit's own row, and no error text.
pub(crate) fn run_totals_only(doc: &mut Value) {
    summary_totals_only(&mut doc["summary"]);
    if doc.get("units_run").is_some() {
        doc["units_run"] = json!([]);
    }
    if doc["error"].is_string() {
        doc["error"] = json!(WITHHELD);
    }
    plain(doc);
}

/// A job read below detail quasi: a pipeline run's result and error as its
/// run's (record 49 R4, after review).
pub(crate) fn job_totals_only(job: &mut Value) {
    let a_run = job["kind"] == "pipeline" || job["result"]["run"].is_i64();
    if !a_run {
        return;
    }
    if job["result"]["summary"].is_object() {
        summary_totals_only(&mut job["result"]["summary"]);
    }
    if job["error"].is_string() {
        job["error"] = json!(WITHHELD);
    }
    plain(&mut job["result"]);
}

/// The `pipeline:qc` items of a review list read below detail quasi
/// (record 49 R4b, after review): never one a unit, since counting them
/// would say a small number, but one entry a run, an item status and a
/// check (a breach) or a reason (a failure), with its count of units held
/// to [`SCANS_K`]. Other kinds are left as they are, in their order; a
/// group stands where its first item stood.
pub(crate) fn qc_items_grouped(rows: Vec<Value>) -> Vec<Value> {
    type Key = (Option<i64>, String, String, String, Option<String>);
    let mut out: Vec<Result<Value, Key>> = Vec::new();
    let mut groups: BTreeMap<Key, (Value, usize)> = BTreeMap::new();
    for r in rows {
        if r["kind"] != nils_registry::review::PIPELINE_QC_KIND {
            out.push(Ok(r));
            continue;
        }
        let run = r["ref"]["run_id"].as_i64();
        let pipeline = r["ref"]["pipeline"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let status = r["status"].as_str().unwrap_or_default().to_string();
        let reason = r["evidence"]["status"]
            .as_str()
            .unwrap_or("failed")
            .to_string();
        let mut checks: Vec<Option<String>> = r["evidence"]["metrics"]["breaches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| {
                b["check"]
                    .as_str()
                    .or_else(|| b["metric"].as_str())
                    .map(|c| Some(c.to_string()))
            })
            .collect();
        checks.sort();
        checks.dedup();
        if reason != "breach" || checks.is_empty() {
            checks = vec![None];
        }
        for check in checks {
            let key: Key = (run, pipeline.clone(), status.clone(), reason.clone(), check);
            let g = groups.entry(key.clone()).or_insert_with(|| {
                out.push(Err(key.clone()));
                (r.clone(), 0)
            });
            g.1 += 1;
        }
    }
    out.into_iter()
        .map(|o| match o {
            Ok(v) => v,
            Err(key) => {
                let (first, n) = groups.get(&key).cloned().unwrap_or((Value::Null, 0));
                let mut v = held("units", n);
                v["kind"] = json!(nils_registry::review::PIPELINE_QC_KIND);
                v["scope"] = first["scope"].clone();
                v["status"] = json!(key.2);
                v["grouped"] = json!(true);
                v["ref"] = json!({"run_id": key.0, "pipeline": key.1});
                v["evidence"] = json!({"status": key.3, "check": key.4, "withheld": WITHHELD});
                v["group_key"] = key.0.map_or(Value::Null, |r| json!(format!("run:{r}")));
                v
            }
        })
        .collect()
}

/// A count of units under `key` in `map`, held to [`SCANS_K`]: null when
/// it stands for 1 to 4, and named under `withheld` beside it.
pub(crate) fn hold_count(map: &mut Value, key: &str) {
    if let Some(n) = map[key].as_u64()
        && held("n", n as usize)["withheld"] == true
    {
        map[key] = Value::Null;
    }
}

// ---------------------------------------------------------------- the doors

/// The reading doors: the catalog and the runs.
pub(crate) fn route(
    registry: &mut Registry,
    quasi: bool,
    get: bool,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Option<Result<Reply, Reply>> {
    if !get {
        return None;
    }
    let id_of = |s: &str, what: &str| -> Result<i64, Reply> {
        s.parse::<i64>()
            .map_err(|_| Reply::error(404, format!("a {what} is named by its id")))
    };
    let err = |e: nils_registry::Error| Reply::error(500, e.to_string());
    Some(match segs {
        ["api", "pipelines"] => (|| {
            let list = rows::list(registry.store()).map_err(err)?;
            let docs: Vec<Value> = list.iter().map(pipeline_doc).collect();
            Ok(Reply::ok(json!({
                "pipelines": docs,
                "capability": capability(registry),
            })))
        })(),
        ["api", "pipelines", id] => (|| {
            let id = id_of(id, "pipeline")?;
            let p = rows::get(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline {id}")))?;
            Ok(Reply::ok(pipeline_doc(&p)))
        })(),
        ["api", "pipeline-runs"] => (|| {
            let pipeline = query
                .get("pipeline")
                .map(|p| p.parse::<i64>())
                .transpose()
                .map_err(|_| Reply::error(400, "pipeline is a pipeline's id"))?;
            let limit = query
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(50)
                .clamp(1, RUNS_PAGE_MAX);
            let list = rows::runs(registry.store(), pipeline, limit).map_err(err)?;
            let mut docs = runs_docs(registry.store(), &list);
            if !quasi {
                docs.iter_mut().for_each(run_totals_only);
            }
            Ok(Reply::ok(json!({ "runs": docs, "limit": limit })))
        })(),
        ["api", "pipeline-runs", id] => (|| {
            let id = id_of(id, "run")?;
            let r = rows::run(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline run {id}")))?;
            let mut doc = run_doc(registry.store(), &r);
            if !quasi {
                run_totals_only(&mut doc);
            }
            Ok(Reply::ok(doc))
        })(),
        _ => return None,
    })
}

/// A `run` a door queues: the pipeline and the flags a caller may give,
/// and the deployment's pack directory; never a path a caller composes.
pub(crate) fn located(pack_dir: Option<&Path>, command: Vec<String>) -> Result<Vec<String>, Reply> {
    let mut out = vec!["run".to_string()];
    let mut named = 0;
    let mut resume = false;
    let mut it = command.into_iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--select" | "--handle" | "--param" | "--model" | "--labels" | "--pack"
            | "--threshold" | "--resume" => {
                resume |= arg == "--resume";
                let value = it
                    .next()
                    .ok_or_else(|| Reply::error(400, format!("run {arg} takes a value")))?;
                out.push(arg);
                out.push(value);
            }
            a if a.starts_with('-') => {
                return Err(Reply::error(
                    400,
                    format!(
                        "run takes --select, --handle, --param, --model, --labels, --threshold, --pack and --resume, not {a}"
                    ),
                ));
            }
            _ => {
                named += 1;
                if named > 1 {
                    return Err(Reply::error(400, "run names one pipeline"));
                }
                out.push(arg);
            }
        }
    }
    if named == 0 && !resume {
        return Err(Reply::error(400, "run <pipeline>: the pipeline to run"));
    }
    if let Some(d) = pack_dir {
        out.push("--pack-dir".into());
        out.push(d.display().to_string());
    }
    Ok(out)
}

// --------------------------------------------------------- the command line

/// `nils pipeline`: the catalog and the runtime.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum PipelineCommand {
    /// Add a descriptor (nils.job.yml, contracts/job/v1) to the catalog, as
    /// its name's next version; an image not pinned by its registry
    /// manifest digest is refused
    Add {
        /// The descriptor
        file: PathBuf,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The catalog, by name and version, and whether pipelines can run here
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// One pipeline: its image, parameters, inputs, outputs and needs
    Show {
        /// Its id, name@version, or a name for its newest version
        pipeline: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The container runtime pipelines run under: what was found, or the
    /// operator's choice. Docker is taken only when chosen here (D18)
    Runtime {
        /// auto (rootless podman, then apptainer), apptainer-first (apptainer,
        /// then podman), podman, apptainer, docker or off
        #[arg(long, value_name = "CHOICE")]
        set: Option<String>,
        /// How apptainer keeps an image it builds from the pinned digest:
        /// sif, or sandbox where there is no /dev/fuse (record 49 A2)
        #[arg(long, value_name = "FORM")]
        apptainer_image: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The pipeline lane (record 49): the cores and memory its units run
    /// within together, and the card a GPU unit leases
    Lane {
        /// The cores the lane's units hold together (48 until set)
        #[arg(long)]
        cores: Option<u32>,
        /// The GB of memory they hold together (512 until set); never more
        /// than this machine or its container offers
        #[arg(long)]
        memory_gb: Option<u64>,
        /// The card a GPU unit leases, by index, or none for no card (0
        /// until set)
        #[arg(long, value_name = "CARD")]
        gpu_card: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Secret inputs (record 49 R3): a file the site keeps, such as a
    /// licence, that a pipeline declaring it reads at run time
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
    /// A run's seeds (record 43): the values it suggests a person curate,
    /// and the stacks it suggests curating, which --save keeps as a
    /// selection a campaign starts from (nils campaign create --select)
    Seeds {
        /// The run, by id
        run: i64,
        /// Save the suggested stacks as the next version of this selection
        #[arg(long, value_name = "NAME")]
        save: Option<String>,
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        #[arg(long, default_value = "mri")]
        pack: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The starter catalog (record 49 A4): the analyses the engine seeds at
    /// its start, and what the catalog holds of each
    Starter {
        /// Seed what the catalog lacks now, as the engine does at its start
        #[arg(long)]
        seed: bool,
        /// Turn the seeding at the engine's start off
        #[arg(long, conflicts_with = "on")]
        off: bool,
        /// Turn it back on
        #[arg(long)]
        on: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The runs, newest first, or one run with its derivatives
    Runs {
        /// One run, by id
        id: Option<i64>,
        /// Only this pipeline's runs: its id, name@version or name
        #[arg(long, value_name = "PIPELINE")]
        pipeline: Option<String>,
        /// At most this many
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `nils pipeline secret`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum SecretCommand {
    /// Set the file a secret is read from; the engine keeps its path and
    /// never its bytes
    Set {
        /// The secret's id, as a descriptor declares it
        id: String,
        /// The file, an absolute path readable by the engine
        #[arg(long)]
        file: PathBuf,
    },
    /// Forget a secret's file
    Unset { id: String },
    /// The secrets set, by id, and whether each file can be read now
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `nils run`: one pipeline over a frozen selection.
#[derive(Debug, clap::Args)]
pub(crate) struct RunArgs {
    /// The pipeline: its id, name@version, or a name for its newest version
    #[arg(required_unless_present = "resume")]
    pub(crate) pipeline: Option<String>,
    /// Take up again a run that stopped, was cancelled or whose engine went
    /// away (record 49 A1): the units it finished are kept, and those in
    /// flight run again, with everything the run recorded
    #[arg(
        long,
        value_name = "RUN",
        conflicts_with_all = ["pipeline", "select", "handle", "params", "models", "labels", "threshold"]
    )]
    pub(crate) resume: Option<i64>,
    /// The selection to run over, frozen now into its stacks and pinned
    #[arg(long, value_name = "selection:NAME@V", conflicts_with = "handle")]
    pub(crate) select: Option<String>,
    /// A frozen list of stacks to run over, by handle id, pinned
    #[arg(long, value_name = "ID")]
    pub(crate) handle: Option<i64>,
    /// A parameter, as id=value (repeatable); the rest take their defaults
    #[arg(long = "param", value_name = "ID=VALUE")]
    pub(crate) params: Vec<String>,
    /// A registered model the pipeline reads: its id, digest or name@version (repeatable, in the order the descriptor declares its model inputs)
    #[arg(long = "model", value_name = "MODEL")]
    pub(crate) models: Vec<String>,
    /// A label set the pipeline reads, by id
    #[arg(long = "labels", value_name = "ID")]
    pub(crate) labels: Option<i64>,
    /// Stage the run's proposals at this probability or above: a model's
    /// card sets the threshold, and a run may raise it, never lower it
    #[arg(long, value_name = "P")]
    pub(crate) threshold: Option<f64>,
    /// The pack a selection is frozen and a bids input released under
    #[arg(long, default_value = "mri")]
    pub(crate) pack: String,
    #[arg(long, value_name = "DIR")]
    pub(crate) pack_dir: Option<PathBuf>,
    /// The DICOM to NIfTI converter a bids input is released with
    #[arg(long, default_value = "dcm2niix", value_name = "PATH")]
    pub(crate) dcm2niix: PathBuf,
    /// Run nothing: say what the run would do (record 49 A3), its units,
    /// the units missing an input and why, the time, the GPU, and what a
    /// unit needs against the lane's budget
    #[arg(long)]
    pub(crate) preflight: bool,
    /// Machine-readable output
    #[arg(long)]
    pub(crate) json: bool,
}

fn print(v: &Value) -> Result<(), Exit> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| fail(format!("will not serialize: {e}")))?
    );
    Ok(())
}

pub(crate) fn command(home: &Home, command: PipelineCommand) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    match command {
        PipelineCommand::Add { file, json } => {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| usage(format!("{}: {e}", file.display())))?;
            let d =
                descriptor::parse(&text).map_err(|e| usage(format!("{}: {e}", file.display())))?;
            let digest = d.digest();
            let who = crate::actor();
            let now = nils_registry::time::now_iso();
            let (p, fresh) = rows::add(
                registry.store(),
                &rows::New {
                    name: &d.name,
                    tool_version: &d.tool_version,
                    descriptor: &d.document,
                    descriptor_digest: &digest,
                    image: &d.image.reference,
                    image_digest: &d.image.digest,
                    layout: d.layout.name(),
                    level: d.level.name(),
                    added_by: &who,
                    added_at: &now,
                },
            )?;
            if fresh {
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineAdd,
                    json!({"pipeline": p.id, "name": p.name, "version": p.version}),
                    Some(json!({"descriptor": p.descriptor_digest, "image": p.image_digest})),
                )?;
            }
            if json {
                let mut v = pipeline_doc(&p);
                v["added"] = json!(fresh);
                return print(&v);
            }
            println!(
                "pipeline {}: {} ({}), image {}{}",
                p.id,
                p.label(),
                p.descriptor_digest,
                p.image,
                if fresh {
                    ""
                } else {
                    "; the catalog already held this descriptor"
                }
            );
            Ok(())
        }
        PipelineCommand::List { json } => {
            let list = rows::list(registry.store())?;
            let cap = {
                let d = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
                let extra = lane_doc(&mut registry);
                capability_with(registry.store(), &d, Some(extra))
            };
            if json {
                return print(&json!({
                    "pipelines": list.iter().map(pipeline_doc).collect::<Vec<_>>(),
                    "capability": cap,
                }));
            }
            println!(
                "{:>5}  {:<32} {:<8} {:<11} {:<7} image",
                "id", "pipeline", "layout", "level", "state"
            );
            for p in &list {
                println!(
                    "{:>5}  {:<32} {:<8} {:<11} {:<7} {}",
                    p.id,
                    p.label(),
                    p.layout,
                    p.level,
                    p.state,
                    p.image
                );
            }
            if cap["enabled"] == true {
                println!(
                    "{} pipelines; they run under {} {} into {}",
                    list.len(),
                    cap["runtime"]["name"].as_str().unwrap_or_default(),
                    cap["runtime"]["version"].as_str().unwrap_or_default(),
                    cap["place"].as_str().unwrap_or_default()
                );
            } else {
                println!(
                    "{} pipelines; {}",
                    list.len(),
                    cap["reason"].as_str().unwrap_or("pipelines are off")
                );
            }
            Ok(())
        }
        PipelineCommand::Show { pipeline, json } => {
            let p = rows::resolve(registry.store(), &pipeline)?
                .ok_or_else(|| usage(format!("no pipeline {pipeline} in the catalog")))?;
            let doc = pipeline_doc(&p);
            if json {
                return print(&doc);
            }
            println!("pipeline {}: {}", p.id, p.label());
            println!("  tool version     {}", p.tool_version);
            println!("  image            {}", p.image);
            println!("  descriptor       {}", p.descriptor_digest);
            println!("  input            {} layout, {} units", p.layout, p.level);
            println!(
                "  needs a GPU      {}",
                doc["needs"]["gpu"].as_str().unwrap_or("none")
            );
            for param in doc["parameters"].as_array().into_iter().flatten() {
                println!(
                    "  parameter        {} ({}), default {}",
                    param["id"].as_str().unwrap_or_default(),
                    param["type"].as_str().unwrap_or_default(),
                    param["default-value"]
                );
            }
            for o in doc["outputs"].as_array().into_iter().flatten() {
                println!(
                    "  output           {}: a {} at {}",
                    o["id"].as_str().unwrap_or_default(),
                    o["kind"].as_str().unwrap_or_default(),
                    o["path-template"].as_str().unwrap_or_default()
                );
            }
            println!("  added by         {} at {}", p.added_by, p.added_at);
            Ok(())
        }
        PipelineCommand::Runtime {
            set,
            apptainer_image,
            json,
        } => {
            if let Some(word) = apptainer_image {
                let form = ImageForm::parse(&word).ok_or_else(|| {
                    usage(format!(
                        "--apptainer-image is one of {}, not {word}",
                        ImageForm::WORDS.join(", ")
                    ))
                })?;
                registry.set_meta(APPTAINER_IMAGE_KEY, form.name())?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"apptainer_image": form.name()}),
                    None,
                )?;
            }
            if let Some(word) = set {
                let c = Choice::parse(&word).ok_or_else(|| {
                    usage(format!(
                        "--set is one of {}, not {word}",
                        Choice::WORDS.join(", ")
                    ))
                })?;
                registry.set_meta(RUNTIME_KEY, c.name())?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"choice": c.name()}),
                    None,
                )?;
            }
            let d = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
            let extra = lane_doc(&mut registry);
            let cap = capability_with(registry.store(), &d, Some(extra));
            if json {
                return print(&cap);
            }
            println!("choice   {}", d.choice.name());
            for (k, s) in &d.looked {
                println!("  {k:<10} {s}");
            }
            match &d.runtime {
                Some(r) => println!(
                    "runtime  {} {}{}",
                    r.kind.name(),
                    r.version,
                    r.gpu
                        .as_deref()
                        .map(|g| format!(", GPU {g}"))
                        .unwrap_or_else(|| ", no GPU".into())
                ),
                None => println!("runtime  none: {}", d.reason.as_deref().unwrap_or("")),
            }
            if cap["enabled"] != true
                && let Some(r) = cap["reason"].as_str()
                && d.runtime.is_some()
            {
                println!("{r}");
            }
            Ok(())
        }
        PipelineCommand::Lane {
            cores,
            memory_gb,
            gpu_card,
            json,
        } => {
            let mut changed = serde_json::Map::new();
            if let Some(c) = cores {
                if c == 0 {
                    return Err(usage("--cores is one or more"));
                }
                registry.set_meta(LANE_CORES_KEY, &c.to_string())?;
                changed.insert("cores".into(), json!(c));
            }
            if let Some(m) = memory_gb {
                if m == 0 {
                    return Err(usage("--memory-gb is one or more"));
                }
                registry.set_meta(LANE_MEMORY_KEY, &m.to_string())?;
                changed.insert("memory_gb".into(), json!(m));
            }
            if let Some(card) = gpu_card {
                let word = card.trim();
                if word != "none" && word.parse::<u32>().is_err() {
                    return Err(usage(format!(
                        "--gpu-card is a card's index, or none, not {word}"
                    )));
                }
                registry.set_meta(GPU_CARD_KEY, word)?;
                changed.insert("gpu_card".into(), json!(word));
            }
            if !changed.is_empty() {
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"lane": Value::Object(changed)}),
                    None,
                )?;
            }
            let lane = lane_of(&mut registry);
            if json {
                return print(&lane.doc());
            }
            println!(
                "lane     {} cores, {} GB of memory{}",
                lane.ledger.cores,
                lane.ledger.memory_mib / 1024,
                lane.why
                    .as_deref()
                    .map(|w| format!(" ({w})"))
                    .unwrap_or_default()
            );
            println!(
                "card     {}",
                lane.card
                    .map_or("none: GPU units do not run".to_string(), |c| format!(
                        "{c}, leased by free memory"
                    ))
            );
            Ok(())
        }
        PipelineCommand::Secret { command } => match command {
            SecretCommand::Set { id, file } => {
                if !id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                    || id.is_empty()
                {
                    return Err(usage(format!(
                        "a secret's id is lowercase letters, digits and _, not {id}"
                    )));
                }
                if !file.is_absolute() {
                    return Err(usage(format!(
                        "{} is not an absolute path; the engine reads the file where it runs",
                        file.display()
                    )));
                }
                // read now, so a file the engine cannot read is said now
                secrets::read(&id, &file).map_err(usage)?;
                let path = file.display().to_string();
                if path.contains(':') || path.contains(',') {
                    return Err(usage(format!(
                        "{path} holds a ':' or a ',', which a runtime's mount syntax splits on"
                    )));
                }
                registry.set_meta(&format!("{SECRET_PREFIX}{id}"), &path)?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"secret": id, "set": true}),
                    None,
                )?;
                println!("secret {id} is read from its file at each run that declares it");
                Ok(())
            }
            SecretCommand::Unset { id } => {
                registry.set_meta(&format!("{SECRET_PREFIX}{id}"), "")?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"secret": id, "set": false}),
                    None,
                )?;
                println!("secret {id} is no longer set");
                Ok(())
            }
            SecretCommand::List { json } => {
                let ids = secret_ids(registry.store());
                let mut docs = Vec::new();
                for id in ids {
                    let readable = secret_path(&mut registry, &id)
                        .is_some_and(|p| secrets::read(&id, Path::new(&p)).is_ok());
                    docs.push(json!({"id": id, "readable": readable}));
                }
                if json {
                    return print(&json!(docs));
                }
                for d in &docs {
                    println!(
                        "{:<32} {}",
                        d["id"].as_str().unwrap_or_default(),
                        if d["readable"] == true {
                            "set, readable"
                        } else {
                            "set, NOT readable now"
                        }
                    );
                }
                Ok(())
            }
        },
        PipelineCommand::Seeds {
            run,
            save,
            pack_dir,
            pack,
            json,
        } => {
            let doc = seeds_of(registry.store(), run).map_err(usage)?;
            let mut out = json!({
                "run": run, "seeds": doc["seeds"], "selection": doc["selection"],
            });
            let mut per: BTreeMap<String, usize> = BTreeMap::new();
            for s in doc["seeds"].as_array().into_iter().flatten() {
                let key = format!(
                    "{}={}",
                    s["axis"].as_str().unwrap_or_default(),
                    s["value"].as_str().unwrap_or_default()
                );
                *per.entry(key).or_default() += 1;
            }
            out["per_value"] = json!(per);
            if let Some(name) = save {
                let stacks: Vec<i64> = doc["selection"]["stacks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                    .collect();
                if stacks.is_empty() {
                    return Err(usage(format!("run {run} suggests no stacks to curate")));
                }
                drop(registry);
                let ask = json!({
                    "ast_version": 1,
                    "sets": {"seeded": {"grain": "stack", "where": [
                        ["in", {}, ["field", {}, "id"], stacks],
                    ]}},
                    "out": {"set": "seeded", "level": "record"},
                });
                let saved = crate::ask_cli::save_selection_doc(
                    home,
                    &name,
                    &ask,
                    pack_dir,
                    &pack,
                    Some(&format!("the stacks pipeline run {run} suggests curating")),
                )?;
                out["saved"] = saved;
            }
            if json {
                return print(&out);
            }
            println!(
                "run {run}: {} seed(s), {} stack(s) suggested for curation",
                doc["seeds"].as_array().map_or(0, Vec::len),
                doc["selection"]["stacks"].as_array().map_or(0, Vec::len)
            );
            for (k, n) in &per {
                println!("  {k:<32} {n}");
            }
            if let Some(s) = out.get("saved") {
                println!(
                    "saved as selection:{}@{}; nils campaign create --select selection:{}@{} makes a campaign of it",
                    s["name"].as_str().unwrap_or_default(),
                    s["version"],
                    s["name"].as_str().unwrap_or_default(),
                    s["version"]
                );
            }
            Ok(())
        }
        PipelineCommand::Starter {
            seed,
            off,
            on,
            json,
        } => {
            if off || on {
                let word = if off { "off" } else { "on" };
                registry.set_meta(crate::starter::SETTING, word)?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"starter": word}),
                    None,
                )?;
            }
            let added = if seed {
                crate::starter::seed(&mut registry).map_err(fail)?
            } else {
                Vec::new()
            };
            let enabled = crate::starter::enabled(&mut registry);
            let list = crate::starter::list(&mut registry).map_err(fail)?;
            if json {
                return print(&json!({
                    "seeding": if enabled { "on" } else { "off" },
                    "starters": list, "added": added,
                }));
            }
            println!(
                "seeding at the engine's start: {}",
                if enabled { "on" } else { "off" }
            );
            for s in &list {
                println!(
                    "  {:<24} {}",
                    s["name"].as_str().unwrap_or_default(),
                    s["state"].as_str().unwrap_or_default()
                );
            }
            for a in &added {
                println!("seeded {}", a["label"].as_str().unwrap_or_default());
            }
            Ok(())
        }
        PipelineCommand::Runs {
            id,
            pipeline,
            limit,
            json,
        } => {
            if let Some(id) = id {
                let r = rows::run(registry.store(), id)?
                    .ok_or_else(|| usage(format!("no pipeline run {id}")))?;
                let doc = run_doc(registry.store(), &r);
                if json {
                    return print(&doc);
                }
                print_run(&doc);
                return Ok(());
            }
            let pid = match pipeline {
                None => None,
                Some(name) => Some(
                    rows::resolve(registry.store(), &name)?
                        .ok_or_else(|| usage(format!("no pipeline {name} in the catalog")))?
                        .id,
                ),
            };
            let list = rows::runs(registry.store(), pid, limit)?;
            let docs = runs_docs(registry.store(), &list);
            if json {
                return print(&json!(docs));
            }
            println!(
                "{:>5}  {:<28} {:<9} {:<10} {:>6}  started",
                "run", "pipeline", "status", "runtime", "files"
            );
            for d in &docs {
                println!(
                    "{:>5}  {:<28} {:<9} {:<10} {:>6}  {}",
                    d["id"],
                    d["pipeline"].as_str().unwrap_or("-"),
                    d["status"].as_str().unwrap_or_default(),
                    d["runtime"].as_str().unwrap_or_default(),
                    d["derivatives"].as_array().map_or(0, Vec::len),
                    d["started_at"].as_str().unwrap_or_default()
                );
            }
            Ok(())
        }
    }
}

fn print_run(doc: &Value) {
    println!(
        "run {} of {}: {}",
        doc["id"],
        doc["pipeline"].as_str().unwrap_or("-"),
        doc["status"].as_str().unwrap_or_default()
    );
    println!(
        "  over             {} (handle {})",
        doc["selection"].as_str().unwrap_or("a handle"),
        doc["handle_id"]
    );
    println!("  parameters       {}", doc["params"]);
    println!(
        "  runtime          {} {} on {}, {}",
        doc["runtime"].as_str().unwrap_or_default(),
        doc["runtime_version"].as_str().unwrap_or_default(),
        doc["host"].as_str().unwrap_or_default(),
        doc["device"].as_str().unwrap_or_default()
    );
    let units = &doc["summary"]["units"];
    println!(
        "  units            {} succeeded, {} failed, {} skipped, {} unreported",
        units["succeeded"], units["failed"], units["skipped"], units["unreported"]
    );
    println!(
        "  derivatives      {}",
        doc["derivatives"].as_array().map_or(0, Vec::len)
    );
    if let Some(d) = doc["results_digest"].as_str() {
        println!("  results          {d}");
    }
    if let Some(e) = doc["error"].as_str() {
        println!("  error            {e}");
    }
}

// ------------------------------------------------------------------ the run

/// One work unit of a run.
#[derive(Debug, Clone)]
struct Unit {
    /// `sub-<s>`, `sub-<s>_ses-<t>` or `stack-<id>`.
    id: String,
    subject: Option<String>,
    session: Option<String>,
    stack_id: Option<i64>,
    series_id: Option<i64>,
    subject_id: Option<i64>,
    session_day: Option<String>,
    stacks: Vec<i64>,
}

impl Unit {
    /// What a derivative of the unit belongs to.
    fn belongs(&self) -> Option<Belongs> {
        let subject_id = Some(self.subject_id?);
        Some(match (self.stack_id, &self.session_day) {
            (Some(stack), _) => Belongs {
                scope: "stack".into(),
                stack_id: Some(stack),
                series_id: self.series_id,
                subject_id,
                session_day: None,
            },
            (None, Some(day)) => Belongs {
                scope: "session".into(),
                stack_id: None,
                series_id: None,
                subject_id,
                session_day: Some(day.clone()),
            },
            (None, None) => Belongs {
                scope: "subject".into(),
                stack_id: None,
                series_id: None,
                subject_id,
                session_day: None,
            },
        })
    }

    /// The values a path template's words take for the unit.
    fn vars(&self) -> Vec<(&'static str, String)> {
        let mut v = Vec::new();
        if let Some(s) = &self.subject {
            v.push(("subject", s.clone()));
        }
        if let Some(s) = &self.session {
            v.push(("session", s.clone()));
        }
        if let Some(s) = self.stack_id {
            v.push(("stack", s.to_string()));
        }
        v
    }

    fn doc(&self) -> Value {
        json!({
            "unit": self.id, "subject": self.subject, "session": self.session,
            "stack_id": self.stack_id, "subject_id": self.subject_id,
            "session_day": self.session_day, "stacks": self.stacks,
        })
    }
}

/// The detail a queued run was given; a run from the keyboard holds every
/// class. A pipeline reads pixels, which open at detail quasi.
fn detail_allows_pixels() -> Result<(), Exit> {
    if let Ok(d) = std::env::var("NILS_JOB_DETAIL") {
        let detail = crate::grants::Detail::parse(d.trim()).unwrap_or_default();
        if detail < crate::grants::Detail::Quasi {
            return Err(usage(
                "a pipeline reads the pixels, which are quasi-identifying; the run needs detail quasi",
            ));
        }
    }
    Ok(())
}

/// Parse `id=value` words.
fn pairs(words: &[String]) -> Result<Vec<(String, String)>, Exit> {
    words
        .iter()
        .map(|w| {
            w.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.to_string()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| usage(format!("--param {w}: write it as id=value")))
        })
        .collect()
}

/// A short random word, so two registries on one host never name a
/// container alike.
fn nonce() -> String {
    let mut b = [0u8; 4];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    hex::encode(b)
}

/// The stacks of a frozen handle.
pub(crate) fn handle_stacks(store: &mut Store, handle: i64) -> Result<Vec<i64>, Exit> {
    let h = nils_ask::handle::get(store, handle)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| usage(format!("no handle {handle}")))?;
    if h.grain != nils_ask::ast::Grain::Stack {
        return Err(usage(format!(
            "handle {handle} is a {} set; a run is over stacks, and --select freezes a wider selection into them",
            h.grain.name()
        )));
    }
    if !h.has_rows() || h.withdrawn_at.is_some() || h.truncated {
        return Err(usage(format!(
            "handle {handle} keeps no complete list of stacks; run its question again"
        )));
    }
    let mut stacks: Vec<i64> = nils_ask::handle::keys(store, handle)
        .map_err(|e| fail(e.to_string()))?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    stacks.sort_unstable();
    stacks.dedup();
    Ok(stacks)
}

/// Whether a release may write `out` for a run's input: the run is running
/// and `out` is its input folder under its working place. Otherwise a
/// release writes to an export place only (Wave 5 section 10.2).
pub(crate) fn release_target(store: &mut Store, run_id: i64, out: &Path) -> Result<(), String> {
    let r = rows::run(store, run_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("--into-run {run_id}: no such pipeline run"))?;
    if r.status != "running" {
        return Err(format!("--into-run {run_id}: the run is {}", r.status));
    }
    let place = r
        .place_id
        .and_then(|id| nils_registry::place::show(store, id).ok().flatten())
        .ok_or_else(|| format!("--into-run {run_id}: the run has no working place"))?;
    let want = Path::new(&place.path)
        .join(RUNS)
        .join(run_id.to_string())
        .join("input");
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canon(out) != canon(&want) {
        return Err(format!(
            "--into-run {run_id}: a run's input is released into its own folder, {}",
            want.display()
        ));
    }
    Ok(())
}

/// Wait for a child, beating the job's heart; a cancel kills it.
fn wait_beating(
    child: &mut std::process::Child,
    store: &mut Store,
    job_id: i64,
    progress: &Value,
) -> Result<(std::process::ExitStatus, bool), String> {
    let mut beaten = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok((status, false));
        }
        if beaten.elapsed() >= Duration::from_secs(5) {
            beaten = Instant::now();
            if let Ok(job::Asked::Cancel) = job::beat(store, job_id, Some(progress)) {
                let _ = child.kill();
                let status = child.wait().map_err(|e| e.to_string())?;
                return Ok((status, true));
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The input of a run, as materialised.
struct Materialised {
    units: Vec<Unit>,
    mounts: Vec<Mount>,
    release_id: Option<i64>,
    stopped: bool,
    /// What the run was given of the source places (record 43 review).
    scope: Value,
    /// For the stacks layout, each unit's entry of `stacks.json`, in the
    /// order of `units`, and the sources it lists: what a unit that runs
    /// apart is given of it (record 49 A1).
    entries: Vec<Value>,
    listed: Vec<Value>,
}

/// At most this many folders are bound one by one for a stacks input; a
/// selection whose files lie in more is given the source places' roots
/// instead, which the run records and its report says.
pub(crate) const MAX_BINDS: usize = 2000;

/// `stacks.json`: each stack's files, and the frames of a multi-frame
/// file; since record 43 also each stack's orientation, its `body_part` and
/// `technique` as the registry holds them now, and how many slices its
/// files hold, so an image seeds and picks slices without reading every
/// header. Each folder that holds a file of the selection is bound
/// read-only at `/source/<n>`, one bind per folder and never a whole source
/// place, unless the folders are more than [`MAX_BINDS`]. The registry is
/// read a few hundred stacks at a time, not stack by stack.
fn materialise_stacks(
    store: &mut Store,
    stacks: &[i64],
    input: &Path,
) -> Result<Materialised, String> {
    let d = store.dialect();
    let err = |e: nils_registry::Error| e.to_string();
    type Info = (i64, i64, String, Option<String>);
    let mut info: BTreeMap<i64, Info> = BTreeMap::new();
    let mut axes: BTreeMap<i64, BTreeMap<String, String>> = BTreeMap::new();
    // per stack: (source id, source root, path, frames and their count)
    type File = (i64, String, String, Option<(String, i64)>);
    let mut files_of: BTreeMap<i64, Vec<File>> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT st.id, st.series_id, se.subject_id, st.modality, st.orientation FROM {} st \
             JOIN {} se ON se.id = st.series_id WHERE st.id IN ({list})",
            store.qualified("stack"),
            store.qualified("series"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            info.insert(
                r.int(0).map_err(err)?,
                (
                    r.int(1).map_err(err)?,
                    r.int(2).map_err(err)?,
                    r.text(3).map_err(err)?.to_string(),
                    r.opt_text(4).map_err(err)?.map(str::to_string),
                ),
            );
        }
        // the axes an image reads, as the registry holds them now
        let sql = format!(
            "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({list}) \
             AND axis IN ('body_part', 'technique') AND value IS NOT NULL \
             ORDER BY stack_id, axis, value",
            store.qualified("classification_axis"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            axes.entry(r.int(0).map_err(err)?)
                .or_default()
                .entry(r.text(1).map_err(err)?.to_string())
                .or_insert(r.text(2).map_err(err)?.to_string());
        }
        // a multi-frame file's frames first, then the files whose every
        // frame is the stack's
        let framed = format!(
            "SELECT fr.stack_id, so.id, so.root, f.path, fr.frames, fr.n_frames FROM {} fr \
             JOIN {} i ON i.id = fr.instance_id JOIN {} f ON f.id = i.source_file_id \
             JOIN {} so ON so.id = f.source_id WHERE fr.stack_id IN ({list}) \
             ORDER BY fr.stack_id, f.path",
            store.qualified("instance_frame"),
            store.qualified("instance"),
            store.qualified("source_file"),
            store.qualified("source"),
        );
        for r in store.query(&framed, &[]).map_err(err)? {
            files_of.entry(r.int(0).map_err(err)?).or_default().push((
                r.int(1).map_err(err)?,
                r.text(2).map_err(err)?.to_string(),
                r.text(3).map_err(err)?.to_string(),
                Some((r.text(4).map_err(err)?.to_string(), r.int(5).map_err(err)?)),
            ));
        }
        let whole = format!(
            "SELECT i.stack_id, so.id, so.root, f.path FROM {} i \
             JOIN {} f ON f.id = i.source_file_id JOIN {} so ON so.id = f.source_id \
             WHERE i.stack_id IN ({list}) ORDER BY i.stack_id, f.path",
            store.qualified("instance"),
            store.qualified("source_file"),
            store.qualified("source"),
        );
        for r in store.query(&whole, &[]).map_err(err)? {
            files_of.entry(r.int(0).map_err(err)?).or_default().push((
                r.int(1).map_err(err)?,
                r.text(2).map_err(err)?.to_string(),
                r.text(3).map_err(err)?.to_string(),
                None,
            ));
        }
    }
    let _ = d;
    // one bind per folder that holds a file of the selection, or the roots
    let folder = |root: &str, path: &str| -> Result<String, String> {
        bind_folder(root, path).ok_or_else(|| {
            format!(
                "the source place {root} holds a ':' or a ',' in its path, which a runtime's mount syntax splits on"
            )
        })
    };
    let mut folders: BTreeMap<(i64, String), (String, String)> = BTreeMap::new();
    let mut roots: BTreeMap<i64, String> = BTreeMap::new();
    let (mut widened, mut at_root) = (0usize, 0usize);
    for files in files_of.values() {
        for (so, root, path, _) in files {
            roots.entry(*so).or_insert_with(|| root.clone());
            let dir = folder(root, path)?;
            if let std::collections::btree_map::Entry::Vacant(e) = folders.entry((*so, dir.clone()))
            {
                let own = Path::new(path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if dir != own {
                    widened += 1;
                }
                if dir.is_empty() {
                    at_root += 1;
                }
                e.insert((root.clone(), dir));
            }
        }
    }
    let by_folder = folders.len() <= MAX_BINDS;
    let mut index: BTreeMap<(i64, String), usize> = BTreeMap::new();
    let mut mounts = Vec::new();
    let mut listed = Vec::new();
    let bind = |host: PathBuf, n: usize, mounts: &mut Vec<Mount>, listed: &mut Vec<Value>| {
        let at = format!("/source/{n}");
        listed.push(json!({"id": n, "mount": at}));
        mounts.push(Mount {
            host,
            container: at,
            read_only: true,
        });
    };
    if by_folder {
        for (key, (root, dir)) in &folders {
            let n = index.len();
            index.insert(key.clone(), n);
            bind(Path::new(root).join(dir), n, &mut mounts, &mut listed);
        }
    } else {
        for (so, root) in &roots {
            let n = index.len();
            index.insert((*so, String::new()), n);
            bind(PathBuf::from(root), n, &mut mounts, &mut listed);
        }
    }
    let mut entries: Vec<Value> = Vec::new();
    let mut units: Vec<Unit> = Vec::new();
    for &stack in stacks {
        let Some((series_id, subject_id, modality, orientation)) = info.remove(&stack) else {
            return Err(format!(
                "stack {stack} of the selection is not in the registry"
            ));
        };
        let mut seen: std::collections::BTreeSet<(i64, String)> = Default::default();
        let mut files: Vec<Value> = Vec::new();
        let mut slices: i64 = 0;
        for (so, _, path, frames) in files_of.remove(&stack).unwrap_or_default() {
            if !seen.insert((so, path.clone())) {
                continue;
            }
            let (n, rel) = if by_folder {
                let root = &roots[&so];
                let dir = folder(root, &path)?;
                let n = index[&(so, dir.clone())];
                let rel = if dir.is_empty() {
                    path.clone()
                } else {
                    path.strip_prefix(&format!("{dir}/"))
                        .unwrap_or(&path)
                        .to_string()
                };
                (n, rel)
            } else {
                (index[&(so, String::new())], path.clone())
            };
            // a single-frame file is one slice; a multi-frame file the
            // frames that are the stack's
            slices += frames.as_ref().map_or(1, |(_, n)| *n);
            files.push(json!({"source": n, "path": rel, "frames": frames.map(|(f, _)| f)}));
        }
        if files.is_empty() {
            return Err(format!("stack {stack} has no files the registry can read"));
        }
        let unit = format!("stack-{stack}");
        let ax = axes.remove(&stack).unwrap_or_default();
        entries.push(json!({
            "unit": unit, "stack_id": stack, "series_id": series_id,
            "subject_id": subject_id, "modality": modality, "files": files,
            "orientation": orientation, "body_part": ax.get("body_part"),
            "technique": ax.get("technique"), "slices": slices,
        }));
        units.push(Unit {
            id: unit,
            subject: None,
            session: None,
            stack_id: Some(stack),
            series_id: Some(series_id),
            subject_id: Some(subject_id),
            session_day: None,
            stacks: vec![stack],
        });
    }
    let manifest =
        json!({"contract": nils_pipeline::CONTRACT, "sources": listed, "stacks": entries});
    std::fs::write(
        input.join("stacks.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("the run's input: {e}"))?;
    let scope = if by_folder {
        let mut s = json!({"sources": "folders", "binds": mounts.len()});
        // a folder bound wider than its own, and a source root bound whole,
        // are said (record 43 second review)
        if widened > 0 || at_root > 0 {
            s["widened"] = json!(widened);
            s["roots"] = json!(at_root);
            s["why"] = json!(format!(
                "{widened} folder(s) bound through an ancestor, since their names hold a ':' or a ',' that a runtime's mount syntax splits on, and {at_root} source root(s) bound whole, since files lie directly in them"
            ));
        }
        s
    } else {
        json!({
            "sources": "roots", "binds": mounts.len(), "folders": folders.len(),
            "why": format!(
                "the selection's files lie in {} folders, more than {MAX_BINDS} binds, so the source places' roots were given",
                folders.len()
            ),
        })
    };
    Ok(Materialised {
        units,
        mounts,
        release_id: None,
        stopped: false,
        scope,
        entries,
        listed,
    })
}

/// The bids layout: a release of the stacks with the picks applied into
/// the run's input folder, by the release machinery itself as a process of
/// its own, and the units read back from what it wrote.
#[allow(clippy::too_many_arguments)]
fn materialise_bids(
    home: &Home,
    registry: &mut Registry,
    run_id: i64,
    job_id: i64,
    level: Level,
    stacks: &[i64],
    run_dir: &Path,
    input: &Path,
    args: &RunArgs,
) -> Result<Materialised, String> {
    let chosen = run_dir.join("selection.txt");
    let list: String = stacks.iter().map(|s| format!("{s}\n")).collect();
    std::fs::write(&chosen, list).map_err(|e| format!("the run's folder: {e}"))?;
    let exe = std::env::current_exe().map_err(|e| format!("this binary: {e}"))?;
    let log = std::fs::File::create(run_dir.join("release.log"))
        .map_err(|e| format!("the run's folder: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--registry")
        .arg(home.dir())
        .args(["release", "--out"])
        .arg(input)
        .args(["--layout", "bids", "--picked", "--select"])
        .arg(&chosen)
        .args(["--name", &format!("pipeline-run-{run_id}")])
        .args([
            "--pack",
            &args.pack,
            "--json",
            "--into-run",
            &run_id.to_string(),
        ])
        .arg("--dcm2niix")
        .arg(&args.dcm2niix)
        // the release is a job of its own, never the run's row
        .env_remove(job::ADOPT_VAR)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(log));
    if let Some(dir) = &args.pack_dir {
        cmd.arg("--pack-dir").arg(dir);
    }
    let mut child = cmd.spawn().map_err(|e| format!("the release: {e}"))?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut s) = stdout {
            let _ = std::io::Read::read_to_string(&mut s, &mut text);
        }
        text
    });
    let (status, stopped) = wait_beating(
        &mut child,
        registry.store(),
        job_id,
        &json!({"run": run_id, "phase": "materialise", "layout": "bids"}),
    )?;
    let text = reader.join().unwrap_or_default();
    if stopped {
        return Ok(Materialised {
            units: Vec::new(),
            mounts: Vec::new(),
            release_id: None,
            stopped: true,
            scope: Value::Null,
            entries: Vec::new(),
            listed: Vec::new(),
        });
    }
    if !status.success() {
        let tail = std::fs::read_to_string(run_dir.join("release.log")).unwrap_or_default();
        let last = tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim_start_matches("nils: ")
            .to_string();
        return Err(format!("the release of the input failed: {last}"));
    }
    let report: Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("the release's report: {e}"))?;
    let release_id = report["release_id"]
        .as_i64()
        .ok_or("the release's report names no release")?;
    rows::set_input_release(registry.store(), run_id, release_id).map_err(|e| e.to_string())?;
    released(registry, release_id, level)
}

/// The units of a run's bids input, read back from the release that wrote
/// it: how a run materialises its input, and how a resumed run finds the
/// units of the input it released before (record 49 A1).
fn released(
    registry: &mut Registry,
    release_id: i64,
    level: Level,
) -> Result<Materialised, String> {
    let store = registry.store();
    let d = store.dialect();
    let err = |e: nils_registry::Error| e.to_string();
    let sql = format!(
        "SELECT rs.stack_id, rs.dir, se.subject_id, se.study_id FROM {} r \
         JOIN {} rs ON rs.dataset_id = r.dataset_id \
         JOIN {} st ON st.id = rs.stack_id JOIN {} se ON se.id = st.series_id \
         WHERE r.id = {} ORDER BY rs.stack_id",
        store.qualified("release"),
        store.qualified("release_stack"),
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    let placed = store.query(&sql, &[Param::Int(release_id)]).map_err(err)?;
    let labels = nils_session::labels_by_study(store, &nils_registry::session::Scheme::default())
        .unwrap_or_default();
    let mut units: BTreeMap<String, Unit> = BTreeMap::new();
    for r in &placed {
        let stack = r.int(0).map_err(err)?;
        let dir = r.text(1).map_err(err)?;
        let subject_id = r.int(2).map_err(err)?;
        let study_id = r.int(3).map_err(err)?;
        let mut parts = dir.split('/');
        let Some(subject) = parts.next().and_then(|p| p.strip_prefix("sub-")) else {
            // routed to sourcedata or derivatives: not a unit's input
            continue;
        };
        let session = parts.next().and_then(|p| p.strip_prefix("ses-"));
        let (id, session) = match (level, session) {
            (Level::Session, Some(s)) => (format!("sub-{subject}_ses-{s}"), Some(s.to_string())),
            _ => (format!("sub-{subject}"), None),
        };
        let day = if session.is_some() {
            labels.get(&study_id).map(|l| l.first.to_string())
        } else {
            None
        };
        let u = units.entry(id.clone()).or_insert(Unit {
            id,
            subject: Some(subject.to_string()),
            session,
            stack_id: None,
            series_id: None,
            subject_id: Some(subject_id),
            session_day: day,
            stacks: Vec::new(),
        });
        u.stacks.push(stack);
    }
    if units.is_empty() {
        return Err(
            "the release of the input holds no subject's images: the selection's stacks were not picked, or went to sourcedata; nils pick run picks them".into(),
        );
    }
    Ok(Materialised {
        units: units.into_values().collect(),
        mounts: Vec::new(),
        release_id: Some(release_id),
        stopped: false,
        scope: json!({"sources": "release"}),
        entries: Vec::new(),
        listed: Vec::new(),
    })
}

/// What became of one unit after the container exited.
struct Outcome {
    status: &'static str,
    error: Option<String>,
    metrics: Value,
    /// Files relative to the output folder.
    files: Vec<String>,
}

/// The lane a run is scheduled in (record 49 A1, A2): its budget, as set
/// and as this machine allows it, and the card a GPU unit leases.
#[derive(Debug, Clone)]
pub(crate) struct Lane {
    pub(crate) ledger: lane::Ledger,
    /// The cores and GB of memory set, before the machine had its say.
    pub(crate) set: (u32, u64),
    /// Why the budget is below the setting, where it is.
    pub(crate) why: Option<String>,
    /// The card a GPU unit leases; none where the lane uses no card.
    pub(crate) card: Option<u32>,
}

impl Lane {
    pub(crate) fn doc(&self) -> Value {
        json!({
            "cores": self.ledger.cores,
            "memory_gb": self.ledger.memory_mib / 1024,
            "set": {"cores": self.set.0, "memory_gb": self.set.1},
            "why": self.why,
            "gpu_card": self.card,
        })
    }
}

/// The lane as the registry's settings and this machine make it.
pub(crate) fn lane_of(registry: &mut Registry) -> Lane {
    let mut num = |key: &str| {
        registry
            .meta_value(key)
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    let cores = num(LANE_CORES_KEY).map_or(lane::DEFAULT_CORES, |c| c.clamp(1, 65_536) as u32);
    let memory = num(LANE_MEMORY_KEY).map_or(lane::DEFAULT_MEMORY_GB, |m| m.max(1));
    let card = match registry
        .meta_value(GPU_CARD_KEY)
        .ok()
        .flatten()
        .as_deref()
        .map(str::trim)
    {
        None | Some("") => Some(0),
        Some("none") => None,
        Some(n) => n.parse().ok(),
    };
    let (ledger, why) = lane::budget(cores, memory, lane::host());
    Lane {
        ledger,
        set: (cores, memory),
        why,
        card,
    }
}

/// What one scheduling unit of a pipeline asks of the lane.
fn ask_of(d: &Descriptor) -> lane::Ask {
    lane::Ask {
        cores: d.needs.cores,
        memory_mib: lane::mib_of_gb(d.needs.memory_gb),
    }
}

/// Refuse a pipeline whose unit could never fit in the lane.
fn fits(d: &Descriptor, p: &Pipeline, lane: &Lane) -> Result<(), Exit> {
    if lane.ledger.could(ask_of(d)) {
        return Ok(());
    }
    Err(usage(format!(
        "{} asks {} cores and {} GB of memory for each of its {}, and the pipeline lane has {} cores and {} GB{}; nils pipeline lane --cores <n> --memory-gb <n> sets it",
        p.label(),
        d.needs.cores,
        d.needs.memory_gb,
        if d.units == Units::Apart {
            "units"
        } else {
            "runs"
        },
        lane.ledger.cores,
        lane.ledger.memory_mib / 1024,
        lane.why
            .as_deref()
            .map(|w| format!(" ({w})"))
            .unwrap_or_default()
    )))
}

/// The device a run takes and whether it takes the GPU: a GPU where the
/// pipeline asks, the runtime offers one and the lane names a card.
fn device_for(
    d: &Descriptor,
    p: &Pipeline,
    rt: &runtime::Cli,
    lane: &Lane,
) -> Result<(String, bool), Exit> {
    let offered = rt.gpu().filter(|_| lane.card.is_some());
    match (d.gpu, offered) {
        (Gpu::None, _) | (Gpu::Optional, None) => Ok(("cpu".to_string(), false)),
        (Gpu::Optional | Gpu::Required, Some(g)) => Ok((g.to_string(), true)),
        (Gpu::Required, None) if rt.gpu().is_some() => Err(usage(format!(
            "{} needs a GPU, and the pipeline lane uses no card here; nils pipeline lane --gpu-card <n> names the one it leases",
            p.label()
        ))),
        (Gpu::Required, None) => Err(usage(format!(
            "{} needs a GPU, and {} here offers none{}",
            p.label(),
            rt.kind.name(),
            if rt.kind == runtime::Kind::Podman {
                " (podman passes one through a CDI specification, which this host has not got)"
            } else {
                ""
            }
        ))),
    }
}

/// A secret a run was given (record 49 R3): what the descriptor declares,
/// the file the site set, and what is looked for in what the run leaves.
#[derive(Debug, Clone)]
pub(crate) struct Given {
    secret: descriptor::Secret,
    path: PathBuf,
    held: secrets::Held,
}

/// The file the site set for a secret, where it set one.
pub(crate) fn secret_path(registry: &mut Registry, id: &str) -> Option<String> {
    registry
        .meta_value(&format!("{SECRET_PREFIX}{id}"))
        .ok()
        .flatten()
        .filter(|p| !p.trim().is_empty())
}

/// The secrets a pipeline reads, each read now from the file the site set;
/// one it needs and the site has not set, or that cannot be read, is
/// refused before anything runs.
fn secrets_for(registry: &mut Registry, d: &Descriptor, p: &Pipeline) -> Result<Vec<Given>, Exit> {
    let mut out = Vec::new();
    for s in &d.secrets {
        match secret_path(registry, &s.id) {
            None if s.optional => {}
            None => {
                return Err(usage(format!(
                    "{} reads the secret {}, which this site has not set: nils pipeline secret set {} --file <path>",
                    p.label(),
                    s.id,
                    s.id
                )));
            }
            Some(path) => {
                let held = secrets::read(&s.id, Path::new(&path)).map_err(usage)?;
                out.push(Given {
                    secret: s.clone(),
                    path: PathBuf::from(path),
                    held,
                });
            }
        }
    }
    Ok(out)
}

/// Whether a job is running now: not over, heard from within the freshness
/// a claim allows, and its process on this host still there.
fn job_alive(store: &mut Store, id: i64) -> bool {
    match job::show(store, id) {
        Ok(Some(j)) if !j.state.is_over() => {
            let gone = j.host.as_deref() == Some(job::hostname().as_str())
                && j.pid.is_some_and(|p| job::process_alive(p) == Some(false));
            let fresh = j
                .heartbeat_at
                .as_deref()
                .and_then(nils_registry::time::secs_of)
                .is_some_and(|s| {
                    nils_registry::time::now_secs().saturating_sub(s) < job::FRESH_SECS
                });
            fresh && !gone
        }
        _ => false,
    }
}

/// How often the lane takes a run up again by itself before it leaves the
/// run to a person, so a run that brings its engine down is not started
/// forever.
pub(crate) const AUTO_RESUMES: i64 = 3;

/// Record 49 A1: a run whose engine went away with units in flight (its
/// job is over, or no longer heard from, and the run still says running)
/// is marked interrupted and queued once to be taken up again, under what
/// the job that ran it recorded: its principal, its detail and its actor.
/// A run someone asked to stop is closed as cancelled instead. Answers how
/// many were queued.
pub(crate) fn take_up_interrupted(store: &mut Store) -> Result<usize, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let mut queued = 0;
    for r in rows::runs_in(store, "running").map_err(err)? {
        if r.job_id.is_some_and(|j| job_alive(store, j)) {
            continue;
        }
        let old = r.job_id.and_then(|j| job::show(store, j).ok().flatten());
        if old
            .as_ref()
            .is_some_and(|j| matches!(j.state, job::State::Cancelling | job::State::Cancelled))
        {
            let now = nils_registry::time::now_iso();
            rows::finish(
                store,
                r.id,
                &rows::Finish {
                    status: "cancelled",
                    finished_at: &now,
                    exit_code: None,
                    results_digest: None,
                    summary: &json!({"phase": "run"}),
                    error: Some("stopped with its engine; nils run --resume goes on"),
                },
            )
            .map_err(err)?;
            continue;
        }
        let why = format!(
            "the engine that ran it went away{}",
            r.job_id.map(|j| format!(" (job {j})")).unwrap_or_default()
        );
        if !rows::interrupt(store, r.id, &why).map_err(err)? {
            continue;
        }
        let resumes = r.resumes.unwrap_or(0);
        if resumes >= AUTO_RESUMES {
            let d = store.dialect();
            let sql = format!(
                "UPDATE {} SET error = {} WHERE id = {}",
                store.qualified("pipeline_run"),
                d.param(1, Type::Text),
                d.param(2, Type::Int)
            );
            store
                .execute(
                    &sql,
                    &[
                        Param::from(format!(
                            "{why}, after being taken up again {resumes} times; nils run --resume {} takes it up by hand",
                            r.id
                        )),
                        Param::Int(r.id),
                    ],
                )
                .map_err(err)?;
            continue;
        }
        let mut extra = old.map(|j| j.args).unwrap_or_else(|| json!({}));
        if let Some(map) = extra.as_object_mut() {
            for key in [
                "argv",
                "queued",
                "then",
                "chain_before",
                "chain_after",
                "run",
            ] {
                map.remove(key);
            }
        } else {
            extra = json!({});
        }
        // a run started at the keyboard recorded no detail, since the
        // keyboard holds every class; its resume is given the least a run
        // needs, quasi, and never more (a door's run keeps what it recorded)
        if extra.get("detail").is_none() && extra.get("roles").is_none() {
            extra["detail"] = json!("quasi");
        }
        let principal = extra["principal"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| r.principal.clone());
        let argv = vec!["run".to_string(), "--resume".to_string(), r.id.to_string()];
        job::enqueue_with(
            store,
            &argv,
            Some(&format!("run {} taken up again", r.id)),
            Some(&principal),
            extra,
        )
        .map_err(|e| e.to_string())?;
        queued += 1;
    }
    Ok(queued)
}

/// `nils run`.
pub(crate) fn run_command(home: &Home, args: RunArgs) -> Result<(), Exit> {
    if args.preflight {
        return crate::preflight::command(home, &args);
    }
    detail_allows_pixels()?;
    if let Some(run) = args.resume {
        return resume_command(home, &args, run);
    }
    let name = args.pipeline.clone().ok_or_else(|| {
        usage("run <pipeline>: the pipeline to run, or --resume <run> to take a run up again")
    })?;
    let mut registry = crate::open(home)?;
    let p = rows::resolve(registry.store(), &name)?.ok_or_else(|| {
        usage(format!(
            "no pipeline {name} in the catalog; nils pipeline list names them"
        ))
    })?;
    if p.state != "active" {
        return Err(usage(format!("{} is {}", p.label(), p.state)));
    }
    let d = descriptor::from_value(p.descriptor.clone()).map_err(|e| {
        fail(format!(
            "{}: the catalog's descriptor no longer checks: {e}",
            p.label()
        ))
    })?;
    let params = d.resolve(&pairs(&args.params)?).map_err(usage)?;

    // where it goes and what runs it; either absent is the capability off
    let place = crate::derivatives::working(registry.store())
        .map_err(|m| usage(m.replacen("derivatives are off", "pipelines are off", 1)))?;
    let detected = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
    let rt = detected
        .runtime
        .clone()
        .ok_or_else(|| usage(detected.reason.clone().unwrap_or_default()))?;
    // the lane it runs in: a unit that could never fit, a GPU it cannot
    // lease and a secret the site has not set are refused before anything
    let lane = lane_of(&mut registry);
    let (device, gpu) = device_for(&d, &p, &rt, &lane)?;
    fits(&d, &p, &lane)?;
    let given = secrets_for(&mut registry, &d, &p)?;

    // the typed inputs: models, a label set, derivatives of a kind
    let model_inputs: Vec<&descriptor::TypedInput> =
        d.inputs.iter().filter(|t| t.ty == "model").collect();
    if args.models.len() > model_inputs.len() {
        return Err(usage(format!(
            "{} reads {} model(s); {} were given",
            p.label(),
            model_inputs.len(),
            args.models.len()
        )));
    }
    if let Some(missing) = model_inputs
        .iter()
        .skip(args.models.len())
        .find(|t| !t.optional)
    {
        return Err(usage(format!(
            "{} needs a model for its input {}: --model <id|digest|name@version>",
            p.label(),
            missing.id
        )));
    }
    let mut models = Vec::new();
    for reference in &args.models {
        let m = nils_registry::model::resolve(registry.store(), reference)?
            .ok_or_else(|| usage(format!("no registered model answers to {reference}")))?;
        if m.state == "retired" {
            return Err(usage(format!("model {} is retired", m.label())));
        }
        // record 43: a run may raise a model's threshold, never lower it
        if let Some(t) = args.threshold {
            nils_registry::proposals::threshold(&m, Some(t)).map_err(|e| usage(e.to_string()))?;
        }
        models.push(m);
    }
    if let Some(t) = args.threshold
        && !(t > 0.0 && t <= 1.0)
    {
        return Err(usage(format!(
            "--threshold is a probability above 0 and at most 1, not {t}"
        )));
    }
    let label_input = d.inputs.iter().find(|t| t.ty == "label_set");
    let label_set = match (args.labels, label_input) {
        (Some(_), None) => {
            return Err(usage(format!("{} reads no label set", p.label())));
        }
        (None, Some(t)) if !t.optional => {
            return Err(usage(format!(
                "{} needs a label set for its input {}: --labels <id>",
                p.label(),
                t.id
            )));
        }
        (Some(id), Some(_)) => {
            let set = nils_registry::labels::get(registry.store(), id)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| usage(format!("no label set {id}")))?;
            if set.sealed {
                return Err(usage(format!(
                    "label set {id} was drawn from a sealed certification sample and is never training data (record 40 R3); a pipeline that reads a label set may train on it"
                )));
            }
            Some(set)
        }
        _ => None,
    };

    // the selection, frozen, and the stacks it holds
    let (handle_id, selection) = match (&args.select, args.handle) {
        (Some(spec), _) => {
            drop(registry);
            let h = crate::ask_cli::freeze_selection(
                home,
                spec,
                nils_ask::ast::Grain::Stack,
                args.pack_dir.clone(),
                &args.pack,
            )?;
            registry = crate::open(home)?;
            (h, Some(spec.clone()))
        }
        (None, Some(h)) => (h, None),
        (None, None) => {
            return Err(usage(
                "a run is over a frozen selection: --select selection:<name>@<v>, or --handle <id>",
            ));
        }
    };
    let stacks = handle_stacks(registry.store(), handle_id)?;
    if stacks.is_empty() {
        return Err(usage(
            "the selection holds no stacks; there is nothing to run",
        ));
    }

    // the job: one pipeline run at a time in the lane (R1, record 49 A1)
    let who = crate::actor();
    let actor = nils_registry::actor::current();
    let job_id = claim_run(
        registry.store(),
        &p,
        json!({
            "pipeline": p.label(), "select": args.select, "handle": handle_id,
            "params": args.params, "models": args.models, "labels": args.labels,
        }),
    )?;
    let params_value = Value::Object(params.clone());
    let model_ids: Vec<i64> = models.iter().map(|m| m.id).collect();
    let started = nils_registry::time::now_iso();
    let run_id = rows::start(
        registry.store(),
        &rows::NewRun {
            pipeline_id: p.id,
            job_id: Some(job_id),
            handle_id: Some(handle_id),
            selection: selection.as_deref(),
            params: &params_value,
            runtime: rt.kind.name(),
            runtime_version: &rt.version,
            host: &job::hostname(),
            device: &device,
            model_ids: &model_ids,
            label_set_id: label_set.as_ref().map(|s| s.id),
            place_id: Some(place.id),
            principal: &who,
            actor: Some(&actor),
            started_at: &started,
        },
    )?;
    rows::set_units(registry.store(), run_id, d.units.name(), args.threshold)?;
    let _ = job::set_arg(registry.store(), job_id, "run", json!(run_id));

    let outcome = execute(
        home,
        &mut registry,
        &Execution {
            pipeline: &p,
            descriptor: &d,
            params: &params,
            run_id,
            job_id,
            place: &place,
            runtime: &rt,
            gpu,
            device: &device,
            stacks: &stacks,
            models: &models,
            label_set: label_set.as_ref(),
            threshold: args.threshold,
            who: &who,
            actor: &actor,
            args: &args,
            lane: &lane,
            secrets: &given,
            resume: None,
        },
    );
    conclude(
        &mut registry,
        &Conclusion {
            pipeline: &p,
            run_id,
            job_id,
            handle_id,
            runtime: rt.kind.name(),
            device: &device,
            who: &who,
            json: args.json,
            resumed: false,
        },
        outcome,
    )
}

/// Claim the job a run is: one pipeline run at a time (record 43 R1).
fn claim_run(store: &mut Store, p: &Pipeline, args: Value) -> Result<i64, Exit> {
    job::claim(
        store,
        &job::Claim {
            kind: "pipeline",
            name: &p.label(),
            args,
        },
    )
    .map_err(|e| match e {
        job::Error::Busy { .. } => Exit {
            code: crate::BUSY,
            message: e.to_string(),
        },
        other => fail(other.to_string()),
    })
}

/// `nils run --resume <run>` (record 49 A1): a run that stopped, was
/// cancelled or whose engine went away is taken up again with everything
/// it recorded. The units it finished are kept, with their derivatives and
/// their outcomes; the units in flight run again from a clean folder.
fn resume_command(home: &Home, args: &RunArgs, run_id: i64) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    let r = rows::run(registry.store(), run_id)?
        .ok_or_else(|| usage(format!("no pipeline run {run_id}")))?;
    match r.status.as_str() {
        "done" | "partial" => {
            return Err(usage(format!(
                "run {run_id} is {}: nothing is left to run",
                r.status
            )));
        }
        "failed" if r.results_digest.is_some() => {
            return Err(usage(format!(
                "run {run_id} failed after its containers ended, and its failures are review items; start a new run"
            )));
        }
        "running" => {
            if let Some(j) = r.job_id
                && job_alive(registry.store(), j)
            {
                return Err(Exit {
                    code: crate::BUSY,
                    message: format!("run {run_id} is running, as job {j}"),
                });
            }
        }
        _ => {}
    }
    let p = rows::get(registry.store(), r.pipeline_id)?
        .ok_or_else(|| fail(format!("run {run_id} names a pipeline the catalog lost")))?;
    let d = descriptor::from_value(p.descriptor.clone()).map_err(|e| {
        fail(format!(
            "{}: the catalog's descriptor no longer checks: {e}",
            p.label()
        ))
    })?;
    let params = r
        .params
        .as_object()
        .cloned()
        .ok_or_else(|| fail(format!("run {run_id} recorded no parameters")))?;
    let place = r
        .place_id
        .and_then(|id| {
            nils_registry::place::show(registry.store(), id)
                .ok()
                .flatten()
        })
        .ok_or_else(|| usage(format!("run {run_id} has no working place to go on in")))?;
    let detected = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
    let rt = detected
        .runtime
        .clone()
        .ok_or_else(|| usage(detected.reason.clone().unwrap_or_default()))?;
    if rt.kind.name() != r.runtime {
        return Err(usage(format!(
            "run {run_id} ran under {}, and pipelines here run under {} now; its units would not be alike, so start a new run",
            r.runtime,
            rt.kind.name()
        )));
    }
    let lane = lane_of(&mut registry);
    let (device, gpu) = device_for(&d, &p, &rt, &lane)?;
    if (r.device == "cpu") != (device == "cpu") {
        return Err(usage(format!(
            "run {run_id} ran on {}, and would go on on {device}; start a new run",
            r.device
        )));
    }
    fits(&d, &p, &lane)?;
    let given = secrets_for(&mut registry, &d, &p)?;
    let mut models = Vec::new();
    for id in r
        .model_ids
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
    {
        models.push(
            nils_registry::model::get(registry.store(), id)?
                .ok_or_else(|| fail(format!("run {run_id} read model {id}, which is gone")))?,
        );
    }
    let label_set = match r.label_set_id {
        Some(id) => Some(
            nils_registry::labels::get(registry.store(), id)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| fail(format!("run {run_id} read label set {id}, which is gone")))?,
        ),
        None => None,
    };
    let handle_id = r
        .handle_id
        .ok_or_else(|| fail(format!("run {run_id} pinned no handle")))?;
    let stacks = handle_stacks(registry.store(), handle_id)?;
    let who = crate::actor();
    let actor = nils_registry::actor::current();
    let job_id = claim_run(
        registry.store(),
        &p,
        json!({"pipeline": p.label(), "resume": run_id, "handle": handle_id}),
    )?;
    rows::resume(registry.store(), run_id, job_id)?;
    let _ = job::set_arg(registry.store(), job_id, "run", json!(run_id));
    let outcome = execute(
        home,
        &mut registry,
        &Execution {
            pipeline: &p,
            descriptor: &d,
            params: &params,
            run_id,
            job_id,
            place: &place,
            runtime: &rt,
            gpu,
            device: &device,
            stacks: &stacks,
            models: &models,
            label_set: label_set.as_ref(),
            threshold: r.threshold,
            who: &who,
            actor: &actor,
            args,
            lane: &lane,
            secrets: &given,
            resume: Some(&r),
        },
    );
    conclude(
        &mut registry,
        &Conclusion {
            pipeline: &p,
            run_id,
            job_id,
            handle_id,
            runtime: rt.kind.name(),
            device: &device,
            who: &who,
            json: args.json,
            resumed: true,
        },
        outcome,
    )
}

/// What closing a run needs to say.
struct Conclusion<'a> {
    pipeline: &'a Pipeline,
    run_id: i64,
    job_id: i64,
    handle_id: i64,
    runtime: &'a str,
    device: &'a str,
    who: &'a str,
    json: bool,
    resumed: bool,
}

/// Close a run, audit it, finish its job and say how it ended.
fn conclude(
    registry: &mut Registry,
    c: &Conclusion<'_>,
    outcome: Result<Ended, String>,
) -> Result<(), Exit> {
    let run_id = c.run_id;
    let (status, summary, digest, exit_code, error) = match outcome {
        Ok(o) => o,
        Err(e) => ("failed", json!({}), None, None, Some(e)),
    };
    let finished = nils_registry::time::now_iso();
    rows::finish(
        registry.store(),
        run_id,
        &rows::Finish {
            status,
            finished_at: &finished,
            exit_code,
            results_digest: digest.as_deref(),
            summary: &summary,
            error: error.as_deref(),
        },
    )?;
    // record 49 A3: a run that loaded measures changed what the ask
    // answers, so it moves the epoch and the ask's catalog is built again
    let measures_loaded = summary["numbers"]["measures"].as_u64().unwrap_or(0) > 0;
    nils_registry::audit::record_judging(
        registry,
        &nils_registry::audit::Entry {
            principal: c.who,
            action: nils_registry::audit::Action::PipelineRun,
            scope: json!({
                "run": run_id, "pipeline": c.pipeline.label(), "handle": c.handle_id,
                "units": summary["units"], "derivatives": summary["derivatives"],
                "resumed": c.resumed,
            }),
            policy: None,
            job_id: Some(c.job_id),
            details: Some(json!({
                "status": status, "runtime": c.runtime, "device": c.device,
                "results": digest, "review_items": summary["review_items"],
                "measures": summary["numbers"]["measures"],
            })),
        },
        measures_loaded,
    )?;
    let job_state = match status {
        "done" | "partial" => job::State::Done,
        "cancelled" => job::State::Cancelled,
        _ => job::State::Failed,
    };
    let _ = job::set_result(
        registry.store(),
        c.job_id,
        &json!({"run": run_id, "status": status, "summary": summary, "results_digest": digest}),
    );
    job::finish(registry.store(), c.job_id, job_state, error.as_deref())
        .map_err(|e| fail(e.to_string()))?;
    let r = rows::run(registry.store(), run_id)?
        .ok_or_else(|| fail(format!("no pipeline run {run_id}")))?;
    let doc = run_doc(registry.store(), &r);
    if c.json {
        print(&doc)?;
    } else {
        print_run(&doc);
    }
    match status {
        // a partial run completed: its failed units are review items
        "done" | "partial" => Ok(()),
        "cancelled" => Err(Exit {
            code: crate::STOPPED,
            message: format!(
                "run {run_id} was cancelled; the units it finished are kept, and nils run --resume {run_id} goes on"
            ),
        }),
        _ => Err(fail(format!(
            "run {run_id} {}",
            error.unwrap_or_else(|| "failed".into())
        ))),
    }
}

/// What a run is given once it is checked.
struct Execution<'a> {
    pipeline: &'a Pipeline,
    descriptor: &'a Descriptor,
    params: &'a serde_json::Map<String, Value>,
    run_id: i64,
    job_id: i64,
    place: &'a nils_registry::place::Place,
    runtime: &'a runtime::Cli,
    gpu: bool,
    device: &'a str,
    stacks: &'a [i64],
    models: &'a [nils_registry::model::Model],
    label_set: Option<&'a nils_registry::labels::LabelSet>,
    /// The caller's threshold for the run's proposals, which only raises a
    /// model card's (record 43).
    threshold: Option<f64>,
    who: &'a str,
    actor: &'a Value,
    args: &'a RunArgs,
    /// The lane's budget and card (record 49 A1, A2).
    lane: &'a Lane,
    /// The secrets it was given (record 49 R3).
    secrets: &'a [Given],
    /// The run as it stood, where this takes it up again.
    resume: Option<&'a Run>,
}

/// How a run ended: its status, its summary, the digest of what it said,
/// the container's exit code and the error.
type Ended = (
    &'static str,
    Value,
    Option<String>,
    Option<i64>,
    Option<String>,
);

/// One container the lane schedules: the whole run where its units run
/// together, one unit where they run apart.
struct Batch {
    /// Indices into the run's units.
    units: Vec<usize>,
    /// The unit's id where it runs apart.
    key: Option<String>,
    /// The folder it writes, and the same under the working place.
    out: PathBuf,
    rel_out: String,
    log: PathBuf,
}

/// A batch whose container runs.
struct Running {
    batch: Batch,
    child: std::process::Child,
    inv: Invocation,
    gpu_mib: Option<u64>,
}

/// What every batch of a run shares.
struct Shared {
    run_dir: PathBuf,
    input: PathBuf,
    inputs: PathBuf,
    /// Models and a label set, each at /inputs/<id>.
    typed: Vec<Mount>,
    /// The folders of the derivative inputs under the run's inputs.
    derivative_dirs: Vec<(String, PathBuf)>,
    manifest: Value,
    local_image: Option<PathBuf>,
    /// Whether the runtime holds a container to its cores and its memory
    /// here (record 49, after review).
    limits: (bool, bool),
}

/// A unit's id as a folder name: the characters a unit's id is made of.
fn folder_word(id: &str) -> Result<&str, String> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !id.starts_with('.');
    if ok {
        Ok(id)
    } else {
        Err(format!(
            "the unit {id} is not a name a folder can take, so its units cannot run apart"
        ))
    }
}

/// Materialise, schedule the containers in the lane, take in what each
/// left as it ends, and close with what the run as a whole said.
fn execute(home: &Home, registry: &mut Registry, x: &Execution<'_>) -> Result<Ended, String> {
    let p = x.pipeline;
    let d = x.descriptor;
    let working = PathBuf::from(&x.place.path);
    let run_dir = working.join(RUNS).join(x.run_id.to_string());
    let input = run_dir.join("input");
    let inputs = run_dir.join("inputs");
    let rel_out = format!("{}/{}/{}", derivative::TREE, p.name, x.run_id);
    let out = working.join(&rel_out);
    let err = |e: nils_registry::Error| e.to_string();
    match x.resume {
        None => {
            for dir in [&input, &inputs] {
                std::fs::create_dir_all(dir).map_err(|e| format!("the run's folder: {e}"))?;
            }
            if out.exists() {
                return Err(format!(
                    "the output folder {rel_out} is there already; a run writes a folder of its own"
                ));
            }
            std::fs::create_dir_all(&out).map_err(|e| format!("the output folder: {e}"))?;
            rows::set_output(registry.store(), x.run_id, &rel_out).map_err(err)?;
        }
        Some(r) => {
            let before = rows::units(registry.store(), x.run_id).map_err(err)?;
            stop_left(x, &before, &out);
            // together, the units are all in flight or all past their
            // container: the first run again from a clean folder, the second
            // taken in from what the container left
            let past = |u: &rows::Unit| u.state == "over" || u.state == "registering";
            if d.units == Units::Together && !before.iter().any(past) && out.exists() {
                std::fs::remove_dir_all(&out).map_err(|e| format!("the output folder: {e}"))?;
            }
            // a bids input a release did not finish is released again
            if d.layout == Layout::Bids && r.input_release_id.is_none() && input.exists() {
                std::fs::remove_dir_all(&input).map_err(|e| format!("the run's input: {e}"))?;
            }
            for dir in [&input, &inputs, &out] {
                std::fs::create_dir_all(dir).map_err(|e| format!("the run's folder: {e}"))?;
            }
            if r.output.is_none() {
                rows::set_output(registry.store(), x.run_id, &rel_out).map_err(err)?;
            }
        }
    }
    let _ = job::beat(
        registry.store(),
        x.job_id,
        Some(&json!({"run": x.run_id, "phase": "materialise", "layout": d.layout.name()})),
    );

    let released_before = x.resume.and_then(|r| r.input_release_id);
    let m = match (d.layout, released_before) {
        (Layout::Stacks, _) => materialise_stacks(registry.store(), x.stacks, &input)?,
        (Layout::Bids, Some(release)) => released(registry, release, d.level)?,
        (Layout::Bids, None) => materialise_bids(
            home, registry, x.run_id, x.job_id, d.level, x.stacks, &run_dir, &input, x.args,
        )?,
    };
    if m.stopped {
        return Ok((
            "cancelled",
            json!({"phase": "materialise"}),
            None,
            None,
            None,
        ));
    }

    // the typed inputs, each read-only, and the manifest
    let mut typed: Vec<Mount> = Vec::new();
    let model_inputs: Vec<&descriptor::TypedInput> =
        d.inputs.iter().filter(|t| t.ty == "model").collect();
    let mut models_doc: Vec<Value> = Vec::new();
    for (model, t) in x.models.iter().zip(&model_inputs) {
        // a model a run fitted is that run's derivative of kind model
        // (record 43): its folder is the input, read-only
        let artifact = model_artifact(registry.store(), model.id, x.place.id)?;
        if let Some(rel) = &artifact {
            let host = working.join(rel);
            if let Some(dir) = host.parent().filter(|d| d.is_dir()) {
                typed.push(Mount {
                    host: dir.to_path_buf(),
                    container: format!("/inputs/{}", t.id),
                    read_only: true,
                });
            }
        }
        models_doc.push(json!({
            "input": t.id, "model_id": model.id, "name": model.name, "version": model.version,
            "digest": model.digest, "kind": model.kind, "card": model.card,
            "encoder_model_ids": model.encoder_model_ids,
            "artifact": artifact.as_deref().and_then(|a| Path::new(a).file_name())
                .map(|f| format!("/inputs/{}/{}", t.id, f.to_string_lossy())),
        }));
    }
    let mut label_doc = Value::Null;
    if let (Some(set), Some(t)) = (x.label_set, d.inputs.iter().find(|t| t.ty == "label_set")) {
        label_doc = set.as_json();
        label_doc["input"] = json!(t.id);
        let dir = set
            .place_id
            .and_then(|id| {
                nils_registry::place::show(registry.store(), id)
                    .ok()
                    .flatten()
            })
            .zip(set.path.as_deref())
            .map(|(pl, rel)| Path::new(&pl.path).join(rel));
        if let Some(dir) = dir {
            let dir = if dir.is_file() {
                dir.parent().map(Path::to_path_buf).unwrap_or(dir)
            } else {
                dir
            };
            if dir.is_dir() {
                typed.push(Mount {
                    host: dir,
                    container: format!("/inputs/{}", t.id),
                    read_only: true,
                });
            }
        }
    }
    // the derivatives a run takes, each linked into its input's own folder
    // under the run's inputs (record 43 review): the container sees those
    // files and no other, and no bind reaches the derivatives tree
    let mut derivative_doc = serde_json::Map::new();
    let mut derivative_dirs: Vec<(String, PathBuf)> = Vec::new();
    let taken = derivative_inputs(registry.store(), d, x.stacks, x.place.id)?;
    for t in d.inputs.iter().filter(|t| t.ty.starts_with("derivative:")) {
        let kind = t.ty.trim_start_matches("derivative:");
        let rows = taken.get(&t.id).cloned().unwrap_or_default();
        if rows.is_empty() && !t.optional {
            return Err(format!(
                "{} needs {kind} derivatives of the selection's stacks for its input {}, and none is registered",
                p.label(),
                t.id
            ));
        }
        let into = inputs.join(&t.id);
        std::fs::create_dir_all(&into).map_err(|e| format!("the run's inputs: {e}"))?;
        derivative_dirs.push((t.id.clone(), into.clone()));
        let mut listed = Vec::new();
        let mut linked: std::collections::BTreeSet<String> = Default::default();
        for row in rows {
            let rel = link_input(&working, &row.path, &into)?;
            // two rows of one file are one input
            if !linked.insert(rel.clone()) {
                continue;
            }
            listed.push(json!({
                "id": row.id, "kind": row.kind, "stack_id": row.stack_id,
                "subject_id": row.subject_id, "model_id": row.model_id,
                "preprocess_version": row.preprocess_version,
                "path": rel, "sha256": row.sha256,
            }));
        }
        derivative_doc.insert(t.id.clone(), json!(listed));
    }
    let manifest = json!({
        "contract": nils_pipeline::CONTRACT,
        "run": x.run_id,
        "pipeline": {"name": p.name, "version": p.version, "descriptor_digest": p.descriptor_digest, "image": p.image},
        "params": Value::Object(x.params.clone()),
        "level": d.level.name(),
        "layout": d.layout.name(),
        "units": m.units.iter().map(Unit::doc).collect::<Vec<_>>(),
        "models": models_doc,
        "label_set": label_doc,
        "derivatives": derivative_doc,
        // the secrets it reads, by id and where it sees them: never a path
        // on the host or a byte of the file (record 49 R3)
        "secrets": x.secrets.iter().map(|g| json!({"id": g.secret.id, "mount": g.secret.mount})).collect::<Vec<_>>(),
    });
    std::fs::write(
        inputs.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("the run's folder: {e}"))?;

    // the units, as the lane keeps them: a unit over stays over; one in
    // flight when the run stopped runs again
    let names: Vec<String> = m.units.iter().map(|u| u.id.clone()).collect();
    for u in rows::ensure_units(registry.store(), x.run_id, &names).map_err(err)? {
        if u.state == "running" {
            rows::unit_requeued(registry.store(), u.id).map_err(err)?;
        }
    }
    let unit_rows = rows::units(registry.store(), x.run_id).map_err(err)?;
    let row_of: BTreeMap<String, (i64, String)> = unit_rows
        .iter()
        .map(|u| (u.unit.clone(), (u.id, u.state.clone())))
        .collect();
    let state_of = |id: &str| row_of.get(id).map_or("queued", |(_, s)| s.as_str());

    // the image apptainer runs, built once from the pinned digest
    let local_image = if x.runtime.kind == runtime::Kind::Apptainer {
        match ensure_image(registry, x, &working, &run_dir)? {
            Some(path) => Some(path),
            None => {
                return Ok(("cancelled", json!({"phase": "image"}), None, None, None));
            }
        }
    } else {
        None
    };
    let shared = Shared {
        run_dir: run_dir.clone(),
        input: input.clone(),
        inputs: inputs.clone(),
        typed,
        derivative_dirs,
        manifest,
        local_image,
        limits: runtime::limits_here(x.runtime),
    };

    // the batches: those whose container ended and whose taking in a stop
    // cut short are taken in again now; the rest still to run
    let batch_of = |units: Vec<usize>| -> Result<Batch, String> {
        Ok(match d.units {
            Units::Together => Batch {
                units,
                key: None,
                out: out.clone(),
                rel_out: rel_out.clone(),
                log: run_dir.join("log.txt"),
            },
            Units::Apart => {
                let key = folder_word(&m.units[units[0]].id)?.to_string();
                Batch {
                    units,
                    out: out.join(&key),
                    rel_out: format!("{rel_out}/{key}"),
                    log: run_dir.join("units").join(&key).join("log.txt"),
                    key: Some(key),
                }
            }
        })
    };
    let group = |want: &dyn Fn(&str) -> bool| -> Vec<Vec<usize>> {
        let idx: Vec<usize> = (0..m.units.len())
            .filter(|i| want(state_of(&m.units[*i].id)))
            .collect();
        match d.units {
            Units::Together if idx.is_empty() => Vec::new(),
            Units::Together => vec![idx],
            Units::Apart => idx.into_iter().map(|i| vec![i]).collect(),
        }
    };
    let mut taken_files: std::collections::BTreeSet<PathBuf> = Default::default();
    for units in group(&|s| s == "registering") {
        let code = units
            .first()
            .and_then(|i| unit_rows.iter().find(|u| u.unit == m.units[*i].id))
            .and_then(|u| u.exit_code)
            .and_then(|c| i32::try_from(c).ok());
        let b = batch_of(units)?;
        take_in(registry, x, &m, &b, code, &row_of, &mut taken_files)?;
    }
    let mut pending: std::collections::VecDeque<Batch> = Default::default();
    for units in group(&|s| s == "queued" || s == "running") {
        pending.push_back(batch_of(units)?);
    }

    let mut running: Vec<Running> = Vec::new();
    let scheduled = schedule(
        registry,
        x,
        &m,
        &shared,
        &row_of,
        &mut pending,
        &mut running,
        &mut taken_files,
    );
    // whatever ended the schedule, no container outlives it: a stop, a
    // cancel and an error each give their units back to the queue
    if !running.is_empty() {
        let mut pulse = Pulse::new(x.job_id, json!({"run": x.run_id, "phase": "stop"}));
        for mut r in running.drain(..) {
            x.runtime.stop(&r.inv);
            let _ = runtime::kill(&mut r.child);
            let _ = r.child.wait();
            // record 49 R3: a unit stopped mid-way is swept as one that
            // ended; where the sweep cannot vouch for its folder, the
            // folder goes, since a resume runs the unit again from a clean
            // one
            match sweep_batch(registry, x, &r.batch, &mut pulse) {
                Ok((_, shut)) if shut.is_empty() => {}
                _ if r.batch.key.is_some() => {
                    let _ = std::fs::remove_dir_all(&r.batch.out);
                    let _ = std::fs::remove_file(&r.batch.log);
                }
                _ => {}
            }
            for i in &r.batch.units {
                if let Some((id, _)) = row_of.get(&m.units[*i].id) {
                    let _ = rows::unit_requeued(registry.store(), *id);
                }
            }
        }
    }
    match scheduled? {
        Scheduled::Cancelled => {
            let over = rows::units(registry.store(), x.run_id)
                .map_err(err)?
                .iter()
                .filter(|u| u.state == "over")
                .count();
            return Ok((
                "cancelled",
                json!({"phase": "run", "units": {"total": m.units.len(), "over": over}}),
                None,
                None,
                None,
            ));
        }
        Scheduled::Done => {}
    }
    finalize(registry, x, &m, &working, &out, &rel_out, &mut taken_files)
}

/// How the schedule ended.
enum Scheduled {
    Done,
    Cancelled,
}

/// Run the batches within the lane's budget, a GPU batch only under a lease
/// on the lane's card, and take in each as it ends. Those still running
/// when it returns are the caller's to stop.
#[allow(clippy::too_many_arguments)]
fn schedule(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    shared: &Shared,
    row_of: &BTreeMap<String, (i64, String)>,
    pending: &mut std::collections::VecDeque<Batch>,
    running: &mut Vec<Running>,
    taken_files: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<Scheduled, String> {
    let d = x.descriptor;
    let ask = ask_of(d);
    let gpu_need = x.gpu.then(|| lane::mib_of_gb(d.needs.gpu_memory_gb));
    let smi = runtime::which("nvidia-smi", std::env::var_os("PATH").as_deref());
    let mut ledger = x.lane.ledger;
    let total = m.units.len();
    let mut waiting: Option<String> = None;
    let mut card_asked: Option<Instant> = None;
    let mut beaten: Option<Instant> = None;
    let host = job::hostname();
    // the one card a GPU unit leases and is given: the lane's, 0 where it
    // names none (record 49, after review)
    let card = x.lane.card.unwrap_or(0);
    loop {
        // start what fits
        while !pending.is_empty() {
            if !ledger.fits(ask) {
                waiting = Some(format!(
                    "the lane's budget: {} of {} cores and {} of {} MiB held",
                    ledger.held_cores, ledger.cores, ledger.held_memory_mib, ledger.memory_mib
                ));
                break;
            }
            let mut gpu_mib = None;
            if let Some(need) = gpu_need {
                if card_asked.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
                    break;
                }
                card_asked = Some(Instant::now());
                let held: u64 = running.iter().filter_map(|r| r.gpu_mib).sum();
                let free = match &smi {
                    Some(smi) => lane::gpu_free_mib(smi, card),
                    None => Err("nvidia-smi is not on the search path".into()),
                };
                match free {
                    Ok(free) if lane::lease_fits(free, held, need) => gpu_mib = Some(need),
                    Ok(free) => {
                        waiting = Some(format!(
                            "card {card}: {free} MiB free, {held} MiB held by this run's units, and a unit needs {need} MiB"
                        ));
                        break;
                    }
                    Err(e) => {
                        waiting = Some(format!("card {card}: {e}"));
                        break;
                    }
                }
            }
            let Some(b) = pending.pop_front() else { break };
            let mut inv = invocation_of(x, m, &b, shared)?;
            inv.card = gpu_mib.map(|_| card);
            if let Some(dir) = b.log.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("the run's folder: {e}"))?;
            }
            let child = runtime::spawn(x.runtime, &inv, &b.log)
                .map_err(|e| format!("{} could not start the container: {e}", x.runtime.name()))?;
            ledger.take(ask);
            let now = nils_registry::time::now_iso();
            for i in &b.units {
                if let Some((id, _)) = row_of.get(&m.units[*i].id) {
                    rows::unit_started(
                        registry.store(),
                        *id,
                        &rows::UnitStart {
                            cores: i64::from(ask.cores),
                            memory_mb: ask.memory_mib as i64,
                            gpu_card: gpu_mib.map(|_| i64::from(card)),
                            gpu_memory_mb: gpu_mib.map(|g| g as i64),
                            device: if gpu_mib.is_some() { x.device } else { "cpu" },
                            container: &inv.name,
                            pid: Some(i64::from(child.id())),
                            host: &host,
                            started_at: &now,
                        },
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            running.push(Running {
                batch: b,
                child,
                inv,
                gpu_mib,
            });
            waiting = None;
        }
        // the heart, and a cancel
        if beaten.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            beaten = Some(Instant::now());
            let in_flight: usize = running.iter().map(|r| r.batch.units.len()).sum();
            let queued: usize = pending.iter().map(|b| b.units.len()).sum();
            let progress = json!({
                "run": x.run_id, "phase": "run", "units": total,
                "over": total - in_flight - queued, "running": in_flight, "queued": queued,
                "waiting": waiting,
                "lane": {
                    "cores": ledger.cores, "held_cores": ledger.held_cores,
                    "memory_mb": ledger.memory_mib, "held_memory_mb": ledger.held_memory_mib,
                },
            });
            if let Ok(job::Asked::Cancel) = job::beat(registry.store(), x.job_id, Some(&progress)) {
                return Ok(Scheduled::Cancelled);
            }
        }
        // take in what ended
        let mut i = 0;
        while i < running.len() {
            match running[i].child.try_wait() {
                Ok(Some(status)) => {
                    let r = running.remove(i);
                    ledger.give(ask);
                    take_in(registry, x, m, &r.batch, status.code(), row_of, taken_files)?;
                }
                Ok(None) => i += 1,
                Err(e) => return Err(format!("a container of the run: {e}")),
            }
        }
        if running.is_empty() && pending.is_empty() {
            return Ok(Scheduled::Done);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The container of one batch: its folders made fresh where it runs one
/// unit apart, its mounts, its command line and who it runs as.
fn invocation_of(
    x: &Execution<'_>,
    m: &Materialised,
    b: &Batch,
    shared: &Shared,
) -> Result<Invocation, String> {
    let d = x.descriptor;
    let p = x.pipeline;
    let mut mounts: Vec<Mount>;
    let participants: Vec<String>;
    let mut env: Vec<(String, String)> = vec![
        ("NILS_RUN_ID".into(), x.run_id.to_string()),
        ("NILS_PIPELINE".into(), p.label()),
        ("NILS_IMAGE_DIGEST".into(), p.image_digest.clone()),
        ("NILS_CORES".into(), d.needs.cores.to_string()),
        ("NILS_MEMORY_GB".into(), format!("{}", d.needs.memory_gb)),
    ];
    match &b.key {
        None => {
            mounts = vec![
                Mount {
                    host: shared.input.clone(),
                    container: "/input".into(),
                    read_only: true,
                },
                Mount {
                    host: shared.inputs.clone(),
                    container: "/inputs".into(),
                    read_only: true,
                },
                Mount {
                    host: b.out.clone(),
                    container: "/output".into(),
                    read_only: false,
                },
            ];
            mounts.extend(m.mounts.iter().cloned());
            mounts.extend(shared.typed.iter().cloned());
            let mut s: Vec<String> = m.units.iter().filter_map(|u| u.subject.clone()).collect();
            s.sort();
            s.dedup();
            participants = s;
        }
        Some(key) => {
            let i = b.units[0];
            let u = &m.units[i];
            let udir = shared.run_dir.join("units").join(key);
            if udir.exists() {
                std::fs::remove_dir_all(&udir).map_err(|e| format!("the unit's folder: {e}"))?;
            }
            if b.out.exists() {
                std::fs::remove_dir_all(&b.out)
                    .map_err(|e| format!("the unit's output folder: {e}"))?;
            }
            let (uin, uinputs) = (udir.join("input"), udir.join("inputs"));
            for dir in [&uin, &uinputs, &b.out] {
                std::fs::create_dir_all(dir).map_err(|e| format!("the unit's folder: {e}"))?;
            }
            let mut sources: Vec<Mount> = Vec::new();
            match d.layout {
                Layout::Stacks => {
                    let entry = m.entries.get(i).cloned().unwrap_or(Value::Null);
                    let used: std::collections::BTreeSet<i64> = entry["files"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|f| f["source"].as_i64())
                        .collect();
                    let listed: Vec<Value> = m
                        .listed
                        .iter()
                        .filter(|l| l["id"].as_i64().is_some_and(|n| used.contains(&n)))
                        .cloned()
                        .collect();
                    let doc = json!({"contract": nils_pipeline::CONTRACT, "sources": listed, "stacks": [entry]});
                    std::fs::write(
                        uin.join("stacks.json"),
                        serde_json::to_vec_pretty(&doc).unwrap_or_default(),
                    )
                    .map_err(|e| format!("the unit's input: {e}"))?;
                    sources = m
                        .mounts
                        .iter()
                        .filter(|mt| {
                            mt.container
                                .strip_prefix("/source/")
                                .and_then(|n| n.parse::<i64>().ok())
                                .is_some_and(|n| used.contains(&n))
                        })
                        .cloned()
                        .collect();
                }
                Layout::Bids => unit_input(&shared.input, &uin, u, d.level)?,
            }
            let mut manifest = shared.manifest.clone();
            manifest["units"] = json!([u.doc()]);
            manifest["unit"] = json!(u.id);
            std::fs::write(
                uinputs.join("manifest.json"),
                serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
            )
            .map_err(|e| format!("the unit's folder: {e}"))?;
            mounts = vec![
                Mount {
                    host: uin,
                    container: "/input".into(),
                    read_only: true,
                },
                Mount {
                    host: uinputs,
                    container: "/inputs".into(),
                    read_only: true,
                },
                Mount {
                    host: b.out.clone(),
                    container: "/output".into(),
                    read_only: false,
                },
            ];
            mounts.extend(sources);
            mounts.extend(shared.typed.iter().cloned());
            for (id, dir) in &shared.derivative_dirs {
                mounts.push(Mount {
                    host: dir.clone(),
                    container: format!("/inputs/{id}"),
                    read_only: true,
                });
            }
            participants = u.subject.iter().cloned().collect();
            env.push(("NILS_UNIT".into(), u.id.clone()));
        }
    }
    // a secret, read-only, in this pipeline's containers alone
    for g in x.secrets {
        mounts.push(Mount {
            host: g.path.clone(),
            container: g.secret.mount.clone(),
            read_only: true,
        });
        if let Some(var) = &g.secret.env {
            env.push((var.clone(), g.secret.mount.clone()));
        }
    }
    let argv = d.argv(x.params, &participants)?;
    // who the process is: for podman the engine's own account, which
    // `--userns keep-id` maps to itself, never the group of a shared
    // (setgid) working place, which keep-id does not map and which would
    // put the outputs on a sub-gid or keep the container from starting;
    // for docker, whose daemon maps nothing, the output folder's owner
    let user = {
        #[cfg(unix)]
        {
            if x.runtime.name() == "podman" {
                Some(this_account())
            } else {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(&b.out).ok().map(|m| (m.uid(), m.gid()))
            }
        }
        #[cfg(not(unix))]
        {
            None
        }
    };
    Ok(Invocation {
        name: format!("nils-run-{}-{}", x.run_id, nonce()),
        image: p.image.clone(),
        argv,
        mounts,
        gpu: x.gpu,
        card: None,
        local_image: shared.local_image.clone(),
        env,
        user,
        // the unit's declared cores and memory, enforced where the runtime
        // can; the lane's budget counts them either way
        cpus: shared.limits.0.then_some(d.needs.cores),
        memory_mib: shared.limits.1.then(|| lane::mib_of_gb(d.needs.memory_gb)),
    })
}

/// A bids unit's own input: the dataset's top-level files, and only its
/// subject's folder, or of that only the top-level files and its session's
/// folder, linked from the run's release (a copy across filesystems).
fn unit_input(input: &Path, into: &Path, u: &Unit, level: Level) -> Result<(), String> {
    let e = |e: std::io::Error| format!("the unit's input: {e}");
    for entry in std::fs::read_dir(input).map_err(e)? {
        let entry = entry.map_err(e)?;
        if entry.file_type().map_err(e)?.is_file() {
            link_file(&entry.path(), &into.join(entry.file_name()))?;
        }
    }
    let Some(subject) = &u.subject else {
        return Err(format!("the unit {} names no subject", u.id));
    };
    let sub = format!("sub-{subject}");
    match (level, &u.session) {
        (Level::Session, Some(session)) => {
            let from = input.join(&sub);
            std::fs::create_dir_all(into.join(&sub)).map_err(e)?;
            for entry in std::fs::read_dir(&from).map_err(e)? {
                let entry = entry.map_err(e)?;
                if entry.file_type().map_err(e)?.is_file() {
                    link_file(&entry.path(), &into.join(&sub).join(entry.file_name()))?;
                }
            }
            let ses = format!("ses-{session}");
            link_tree(&from.join(&ses), &into.join(&sub).join(&ses))
        }
        _ => link_tree(&input.join(&sub), &into.join(&sub)),
    }
}

/// Link a folder's regular files, folder by folder; a link is left out.
fn link_tree(from: &Path, to: &Path) -> Result<(), String> {
    let e = |e: std::io::Error| format!("the unit's input: {e}");
    std::fs::create_dir_all(to).map_err(e)?;
    for entry in std::fs::read_dir(from).map_err(e)? {
        let entry = entry.map_err(e)?;
        let kind = entry.file_type().map_err(e)?;
        if kind.is_dir() {
            link_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            link_file(&entry.path(), &to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// A hard link, or a copy where the two are not on one filesystem.
fn link_file(from: &Path, to: &Path) -> Result<(), String> {
    match std::fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("the unit's input: {e}")),
        Err(e) => Err(format!("the unit's input: {e}")),
    }
}

/// Stop what a run that went away left running: each unit in flight, by its
/// container's name, and its process where it is this host's and is still
/// the unit's (its command line names the container or the unit's folder).
fn stop_left(x: &Execution<'_>, units: &[rows::Unit], out: &Path) {
    let host = job::hostname();
    for u in units.iter().filter(|u| u.state == "running") {
        if let Some(name) = &u.container {
            x.runtime.stop(&Invocation {
                name: name.clone(),
                image: String::new(),
                argv: Vec::new(),
                mounts: Vec::new(),
                gpu: false,
                card: None,
                local_image: None,
                env: Vec::new(),
                user: None,
                cpus: None,
                memory_mib: None,
            });
        }
        let Some(pid) = u.pid.filter(|_| u.host.as_deref() == Some(host.as_str())) else {
            continue;
        };
        if job::process_alive(pid) != Some(true) {
            continue;
        }
        let line = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let line = String::from_utf8_lossy(&line);
        let folder = out.join(&u.unit).display().to_string();
        let ours = u.container.as_deref().is_some_and(|n| line.contains(n))
            || line.contains(&folder)
            || (x.descriptor.units == Units::Together && line.contains(&out.display().to_string()));
        if ours {
            // the runtime's process led a group of its own
            if let Ok(leader) = u32::try_from(pid) {
                runtime::kill_group(leader);
            }
        }
    }
}

/// The image apptainer runs (record 49 A2): built once from the pinned
/// digest into the working place's image folder, as a SIF file or a
/// sandbox folder, and kept there by the digest. Answers none when the run
/// was cancelled while it built.
fn ensure_image(
    registry: &mut Registry,
    x: &Execution<'_>,
    working: &Path,
    run_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    let form = image_form(registry);
    let dir = working.join(IMAGES);
    std::fs::create_dir_all(&dir).map_err(|e| format!("the image folder: {e}"))?;
    let target = runtime::local_image(&dir, &x.pipeline.image_digest, form);
    // a cached copy is used only as the engine built it (record 49, after
    // review): what fails its recorded digest, or is not what the engine
    // builds, is built again
    if runtime::image_verified(&target, form) {
        return Ok(Some(target));
    }
    if std::fs::symlink_metadata(&target).is_ok() {
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_dir_all(&target);
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err(format!(
                "the cached image {} is not what the engine built and cannot be removed",
                target.display()
            ));
        }
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let part = dir.join(format!(".{name}.part-{}", nonce()));
    let log = std::fs::File::create(run_dir.join("image.log"))
        .map_err(|e| format!("the run's folder: {e}"))?;
    let err_log = log
        .try_clone()
        .map_err(|e| format!("the run's folder: {e}"))?;
    let _ = job::beat(
        registry.store(),
        x.job_id,
        Some(&json!({"run": x.run_id, "phase": "image", "image": x.pipeline.image})),
    );
    let mut child = std::process::Command::new(&x.runtime.program)
        .args(runtime::build_argv(&x.pipeline.image, &part, form))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(err_log))
        .spawn()
        .map_err(|e| format!("apptainer could not start a build: {e}"))?;
    let (status, stopped) = wait_beating(
        &mut child,
        registry.store(),
        x.job_id,
        &json!({"run": x.run_id, "phase": "image", "image": x.pipeline.image}),
    )?;
    let clean = |p: &Path| {
        let _ = std::fs::remove_dir_all(p);
        let _ = std::fs::remove_file(p);
    };
    if stopped {
        clean(&part);
        return Ok(None);
    }
    if !status.success() || !part.exists() {
        clean(&part);
        return Err(format!(
            "apptainer could not build {} from its digest; its log is {RUNS}/{}/image.log",
            x.pipeline.image, x.run_id
        ));
    }
    if runtime::image_verified(&target, form) {
        // another run built it meanwhile
        clean(&part);
    } else {
        clean(&target);
        std::fs::rename(&part, &target).map_err(|e| format!("the image folder: {e}"))?;
        runtime::record_image(&target, form).map_err(|e| format!("the image folder: {e}"))?;
    }
    Ok(Some(target))
}

/// What a run's `results.json` in a folder says, read only where it lies
/// inside the folder, never through a link the container planted (record
/// 43 review); or why it cannot be read; or neither, where there is none.
fn read_results(out: &Path) -> (Option<nils_pipeline::Results>, Option<String>) {
    let results_path = out.join(nils_pipeline::results::FILE);
    if std::fs::symlink_metadata(&results_path).is_err() {
        return (None, None);
    }
    match nils_pipeline::files::read_inside(out, nils_pipeline::results::FILE)
        .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
        .and_then(|t| nils_pipeline::results::parse(&t))
    {
        Ok(r) => (Some(r), None),
        Err(e) => (None, Some(e)),
    }
}

/// What one unit's container said of it, or what its templates find.
fn outcome_of(
    u: &Unit,
    reported: &Option<nils_pipeline::Results>,
    unreadable: &Option<String>,
    exit_code: Option<i64>,
    unit_outputs: &[descriptor::Output],
    run_outputs: &[descriptor::Output],
    out: &Path,
) -> Outcome {
    let is_run_file = |rel: &str| nils_pipeline::files::which_output(run_outputs, rel).is_some();
    if let Some(e) = unreadable {
        return Outcome {
            status: "failed",
            error: Some(e.clone()),
            metrics: json!({}),
            files: Vec::new(),
        };
    }
    if let Some(r) = reported {
        return match r
            .units
            .iter()
            .find(|e| nils_pipeline::results::normalise(&e.unit_id) == u.id)
        {
            None => Outcome {
                status: "unreported",
                error: Some("the pipeline's results say nothing of this unit".into()),
                metrics: json!({}),
                files: Vec::new(),
            },
            Some(e) => {
                let mut files: Vec<String> = e
                    .derivatives
                    .iter()
                    .filter(|f| !is_run_file(f))
                    .cloned()
                    .collect();
                if e.status == nils_pipeline::results::Status::Succeeded && files.is_empty() {
                    files = found_for(unit_outputs, out, u);
                }
                Outcome {
                    status: e.status.name(),
                    error: e.error.clone(),
                    metrics: e.metrics.clone(),
                    files: if e.status == nils_pipeline::results::Status::Succeeded {
                        files
                    } else {
                        Vec::new()
                    },
                }
            }
        };
    }
    if exit_code != Some(0) {
        return Outcome {
            status: "failed",
            error: Some(format!(
                "the container exited {} and wrote no results.json",
                exit_code.map_or("by a signal".to_string(), |c| c.to_string())
            )),
            metrics: json!({}),
            files: Vec::new(),
        };
    }
    let files = found_for(unit_outputs, out, u);
    if files.is_empty() {
        Outcome {
            status: "failed",
            error: Some("no declared output was found for this unit".into()),
            metrics: json!({}),
            files,
        }
    } else {
        Outcome {
            status: "succeeded",
            error: None,
            metrics: json!({}),
            files,
        }
    }
}

/// Take in what one batch left: its units marked registering, the secrets
/// swept out, then each unit's files hashed by the engine and registered
/// naming the run, and the unit marked over with its outcome. Taking in is
/// idempotent (a file registered already is found, not registered again),
/// so a resume that finds a unit registering takes it in again and runs
/// nothing.
/// A job's heart kept beating through long work that is not a container:
/// hashing and sweeping what one left (record 49, after review), so a long
/// intake is never taken for a stale job.
struct Pulse {
    job_id: i64,
    last: Instant,
    progress: Value,
}

impl Pulse {
    fn new(job_id: i64, progress: Value) -> Pulse {
        Pulse {
            job_id,
            last: Instant::now(),
            progress,
        }
    }

    /// Beat, at most every five seconds.
    fn tick(&mut self, store: &mut Store) {
        if self.last.elapsed() >= Duration::from_secs(5) {
            self.last = Instant::now();
            let _ = job::beat(store, self.job_id, Some(&self.progress));
        }
    }
}

/// The run's own folder beside its output, which no container sees: where
/// the sweep writes the copies it renames over what it rewrites.
fn scratch_of(x: &Execution<'_>, b: &Batch) -> PathBuf {
    let run_rel = match &b.key {
        Some(_) => b
            .rel_out
            .rsplit_once('/')
            .map_or(b.rel_out.as_str(), |(r, _)| r),
        None => b.rel_out.as_str(),
    };
    PathBuf::from(&x.place.path).join(format!("{run_rel}.nils"))
}

/// Sweep what a batch's container left of the secrets it was given (record
/// 49 R3): its output folder, its `results.json` and its log. Answers what
/// the sweep did, as the unit's row keeps it, and what it could not read.
fn sweep_batch(
    registry: &mut Registry,
    x: &Execution<'_>,
    b: &Batch,
    pulse: &mut Pulse,
) -> Result<(Vec<Value>, Vec<String>), String> {
    let mut swept: Vec<Value> = Vec::new();
    let mut shut: Vec<String> = Vec::new();
    if x.secrets.is_empty() {
        return Ok((swept, shut));
    }
    let held: Vec<secrets::Held> = x.secrets.iter().map(|g| g.held.clone()).collect();
    let scratch = scratch_of(x, b);
    let results_path = b.out.join(nils_pipeline::results::FILE);
    let done = secrets::sweep(
        &b.out,
        &held,
        &[results_path.as_path()],
        &scratch,
        &mut || pulse.tick(registry.store()),
    )
    .map_err(|e| format!("the sweep for secrets: {e}"))?;
    for s in done.done {
        match s {
            secrets::Swept::Removed(rel) => swept.push(json!({
                "file": rel.to_string_lossy(),
                "why": "it held a secret input, and was removed",
            })),
            secrets::Swept::Unscannable(rel, why) => swept.push(json!({
                "file": rel.to_string_lossy(),
                "why": format!("it could not be read inside to sweep it for the secret inputs ({why}), and was removed"),
            })),
            secrets::Swept::Redacted(rel) => {
                swept.push(json!({"file": rel.to_string_lossy(), "redacted": true}));
            }
        }
    }
    shut.extend(
        done.unreadable
            .iter()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    secrets::redact_file(&b.log, &held, &scratch).map_err(|e| format!("the run's log: {e}"))?;
    Ok((swept, shut))
}

fn take_in(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    b: &Batch,
    code: Option<i32>,
    row_of: &BTreeMap<String, (i64, String)>,
    taken_files: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<(), String> {
    let d = x.descriptor;
    let exit_code = code.map(i64::from);
    let now = nils_registry::time::now_iso();
    for &i in &b.units {
        if let Some((id, _)) = row_of.get(&m.units[i].id) {
            rows::unit_registering(registry.store(), *id, exit_code).map_err(|e| e.to_string())?;
        }
    }
    // record 49 R3: nothing a container left is read before what holds a
    // secret is swept from it
    let mut pulse = Pulse::new(x.job_id, json!({"run": x.run_id, "phase": "intake"}));
    let (swept, shut) = sweep_batch(registry, x, b, &mut pulse)?;
    let (reported, unreadable) = read_results(&b.out);
    let unit_outputs: Vec<descriptor::Output> =
        d.outputs.iter().filter(|o| !o.run_level).cloned().collect();
    let run_outputs: Vec<descriptor::Output> =
        d.outputs.iter().filter(|o| o.run_level).cloned().collect();
    // a container that failed as a whole, with no results.json to say more
    let whole = reported.is_none() && (exit_code != Some(0) || unreadable.is_some());
    let cards: Vec<Value> = reported
        .as_ref()
        .map(|r| r.models.clone())
        .unwrap_or_default();
    let log_rel = b
        .log
        .strip_prefix(&x.place.path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{RUNS}/{}/log.txt", x.run_id));
    for (n, &i) in b.units.iter().enumerate() {
        let u = &m.units[i];
        let o = outcome_of(
            u,
            &reported,
            &unreadable,
            exit_code,
            &unit_outputs,
            &run_outputs,
            &b.out,
        );
        let Some((row_id, _)) = row_of.get(&u.id) else {
            continue;
        };
        let mut doc = if shut.is_empty() {
            match register_unit(
                registry,
                x,
                u,
                o,
                &b.out,
                &unit_outputs,
                &cards,
                taken_files,
                &now,
                &mut pulse,
            ) {
                Ok(doc) => doc,
                Err(e) => {
                    json!({"status": "failed", "error": e, "metrics": {}, "files": [], "refused": []})
                }
            }
        } else {
            // record 49 R3: what could not be read could not be swept for
            // the secret, so nothing of the batch is vouched for
            json!({
                "status": "failed",
                "error": format!(
                    "{} of what the container left could not be read, even given back to the engine, so it could not be swept for the secret inputs; nothing of it is registered",
                    shut.len()
                ),
                "metrics": {}, "files": [],
                "refused": shut.iter().map(|p| json!({
                    "unit": u.id, "file": p,
                    "why": "it could not be read to sweep it for the secret inputs",
                })).collect::<Vec<_>>(),
            })
        };
        doc["whole"] = json!(whole);
        if let Some(e) = &unreadable {
            doc["unreadable"] = json!(e);
        }
        doc["log"] = json!(log_rel);
        doc["out"] = json!(b.rel_out);
        if n == 0 && !swept.is_empty() {
            doc["swept"] = json!(swept);
        }
        rows::unit_over(registry.store(), *row_id, exit_code, &doc, &now)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Whether the run registered a file at this path already: a resume that
/// takes a unit in again finds it rather than registering it twice.
fn registered_already(store: &mut Store, run_id: i64, path: &str) -> Result<Option<i64>, String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE run_id = {} AND path = {}",
        store.qualified("derivative"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    store
        .query_opt(&sql, &[Param::Int(run_id), Param::from(path)])
        .and_then(|r| r.map(|r| r.int(0)).transpose())
        .map_err(|e| e.to_string())
}

/// Hash and register one unit's files; answers the unit's outcome as its
/// row keeps it.
#[allow(clippy::too_many_arguments)]
fn register_unit(
    registry: &mut Registry,
    x: &Execution<'_>,
    u: &Unit,
    mut o: Outcome,
    out: &Path,
    unit_outputs: &[descriptor::Output],
    cards: &[Value],
    taken_files: &mut std::collections::BTreeSet<PathBuf>,
    now: &str,
    pulse: &mut Pulse,
) -> Result<Value, String> {
    let working = PathBuf::from(&x.place.path);
    let mut hashed: Vec<Value> = Vec::new();
    let mut refused: Vec<Value> = Vec::new();
    let mut tables: Vec<Value> = Vec::new();
    let (mut registered, mut bytes_total, mut embedded, mut cached) =
        (0usize, 0u64, 0usize, 0usize);
    let Some(belongs) = u.belongs() else {
        return Ok(json!({
            "status": "failed", "error": "the unit belongs to no subject the registry holds",
            "metrics": o.metrics, "files": [], "refused": [],
        }));
    };
    let vars = u.vars();
    let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    for rel in &o.files {
        // a unit's file is one its own templates find, never another's
        let Some(declared) = nils_pipeline::files::unit_output(unit_outputs, rel, &pairs) else {
            refused.push(json!({
                "unit": u.id, "file": rel,
                "why": "not a file this unit's declared outputs name",
            }));
            continue;
        };
        let file = match nils_pipeline::files::inside(out, rel) {
            Ok(f) => f,
            Err(e) => {
                refused.push(json!({"unit": u.id, "file": rel, "why": e}));
                continue;
            }
        };
        if !taken_files.insert(file.clone()) {
            refused.push(json!({
                "unit": u.id, "file": rel,
                "why": "the same file as another output of the run, by another name",
            }));
            continue;
        }
        let (bytes, sha) =
            nils_pipeline::files::sha256_file_ticking(&file, &mut || pulse.tick(registry.store()))
                .map_err(|e| format!("{rel}: {e}"))?;
        let kind = declared.kind.as_str();
        let media = nils_pipeline::files::media_type(rel, declared.media_type.as_deref());
        // the row names the file where it is, never a link to it
        let path = place_path(&working, &file)?;
        if kind == nils_registry::embedding::KIND {
            match register_embedding(
                registry,
                x,
                u,
                &declared.encoders,
                &file,
                &path,
                bytes,
                &sha,
                cards,
                now,
            ) {
                Ok(nils_registry::embedding::Registered::New(_)) => {
                    embedded += 1;
                    registered += 1;
                    bytes_total += bytes;
                }
                Ok(nils_registry::embedding::Registered::Kept { .. }) => cached += 1,
                Err(why) => {
                    refused.push(json!({"unit": u.id, "file": rel, "why": why}));
                    continue;
                }
            }
        } else {
            let id = match registered_already(registry.store(), x.run_id, &path)? {
                // taken in before a resume: the row stands
                Some(id) => id,
                None => derivative::insert_of_run(
                    registry.store(),
                    &derivative::New {
                        kind,
                        belongs: &belongs,
                        place_id: x.place.id,
                        path: &path,
                        bytes: bytes as i64,
                        sha256: &sha,
                        media_type: &media,
                        registered_by: x.who,
                        actor: Some(x.actor),
                        model_id: None,
                        run_id: None,
                        preprocess_version: None,
                        supersedes_id: None,
                        created_at: now,
                    },
                    x.run_id,
                )
                .map_err(|e| e.to_string())?,
            };
            registered += 1;
            bytes_total += bytes;
            // a table's rows are the unit's measures (record 49 A3), read
            // when the run is closed, so a resume reads what it kept
            if declared.table.is_some() {
                tables.push(
                    json!({"output": declared.id, "file": rel, "path": path, "derivative": id}),
                );
            }
        }
        hashed.push(json!({"path": rel, "sha256": sha}));
    }
    hashed.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    o.files = hashed
        .iter()
        .filter_map(|h| h["path"].as_str().map(str::to_string))
        .collect();
    Ok(json!({
        "status": o.status, "error": o.error, "metrics": o.metrics,
        "files": hashed, "refused": refused, "registered": registered,
        "bytes": bytes_total, "embedded": embedded, "cached": cached, "tables": tables,
    }))
}

/// Close a run over what its units left: the run's own files where its
/// units ran together and the container completed, the seeds, the
/// proposals, the review items, the digest and the summary.
fn finalize(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    working: &Path,
    out: &Path,
    rel_out: &str,
    taken_files: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<Ended, String> {
    let p = x.pipeline;
    let d = x.descriptor;
    let now = nils_registry::time::now_iso();
    let unit_rows = rows::units(registry.store(), x.run_id).map_err(|e| e.to_string())?;
    let by_name: BTreeMap<&str, &rows::Unit> =
        unit_rows.iter().map(|u| (u.unit.as_str(), u)).collect();
    let apart = d.units == Units::Apart;
    let mut digest_units: Vec<Value> = Vec::new();
    let mut refused: Vec<Value> = Vec::new();
    let (mut registered, mut bytes_total, mut embedded, mut cached) =
        (0usize, 0u64, 0usize, 0usize);
    let mut statuses: Vec<(usize, String, Option<String>, Value, bool)> = Vec::new();
    for (i, u) in m.units.iter().enumerate() {
        let Some(row) = by_name.get(u.id.as_str()) else {
            continue;
        };
        let o = &row.outcome;
        let status = o["status"].as_str().unwrap_or("failed").to_string();
        digest_units.push(json!({"unit": u.id, "status": status, "files": o["files"]}));
        refused.extend(o["refused"].as_array().cloned().unwrap_or_default());
        for s in o["swept"].as_array().into_iter().flatten() {
            if s["why"].is_string() {
                refused.push(json!({
                    "unit": if apart { u.id.as_str() } else { "run" },
                    "file": s["file"], "why": s["why"],
                }));
            }
        }
        let n = |k: &str| o[k].as_u64().unwrap_or(0);
        registered += n("registered") as usize;
        bytes_total += n("bytes");
        embedded += n("embedded") as usize;
        cached += n("cached") as usize;
        statuses.push((
            i,
            status,
            o["error"].as_str().map(str::to_string),
            o["metrics"].clone(),
            o["whole"].as_bool().unwrap_or(false),
        ));
    }
    digest_units.sort_by(|a, b| a["unit"].as_str().cmp(&b["unit"].as_str()));
    // record 49 A3: the rows of the run's tables, as measures, and what
    // reading them came to; a unit's own tables are read here from where
    // it registered them, so a run taken up again reads what it kept
    let mut measured: Vec<crate::measures::Pending> = Vec::new();
    let mut notes = crate::measures::Notes::default();
    for u in &m.units {
        let Some(row) = by_name.get(u.id.as_str()) else {
            continue;
        };
        let Some(belongs) = u.belongs() else {
            continue;
        };
        for t in row.outcome["tables"].as_array().into_iter().flatten() {
            let (Some(output), Some(path), Some(id)) = (
                t["output"].as_str(),
                t["path"].as_str(),
                t["derivative"].as_i64(),
            ) else {
                continue;
            };
            let Some(declared) = d.outputs.iter().find(|o| o.id == output) else {
                continue;
            };
            let read = std::fs::read(working.join(path))
                .map_err(|e| e.to_string())
                .and_then(|b| {
                    crate::measures::unit_table(
                        declared,
                        &b,
                        &u.id,
                        &belongs,
                        id,
                        &mut notes,
                        &mut measured,
                    )
                });
            if let Err(why) = read {
                refused.push(json!({
                    "unit": u.id, "file": t["file"],
                    "why": format!("its rows could not be read: {why}"),
                }));
            }
        }
    }
    // the run's exit: together, its one container's; apart, the first that
    // did not exit 0, or 0
    let exit_code = if apart {
        unit_rows
            .iter()
            .map(|u| u.exit_code)
            .find(|c| *c != Some(0))
            .unwrap_or(Some(0))
    } else {
        unit_rows.first().and_then(|u| u.exit_code)
    };
    let whole_units = statuses.iter().filter(|s| s.4).count();
    let unreadable: Option<String> = unit_rows
        .iter()
        .find_map(|u| u.outcome["unreadable"].as_str().map(str::to_string));
    // together: the container failed as a whole; apart: every unit's did
    let whole = !statuses.is_empty() && whole_units == statuses.len();
    let completed = if apart { !whole } else { exit_code == Some(0) };

    // what the containers said, read again from where each wrote it
    let folders: Vec<PathBuf> = if apart {
        m.units
            .iter()
            .filter(|u| {
                by_name
                    .get(u.id.as_str())
                    .is_some_and(|r| r.state == "over")
            })
            .map(|u| out.join(&u.id))
            .collect()
    } else {
        vec![out.to_path_buf()]
    };
    let reports: Vec<nils_pipeline::Results> =
        folders.iter().filter_map(|f| read_results(f).0).collect();

    // the run's own files (record 43): a model it fitted, registered from
    // its card, and any other run-level output; only a run that completed
    // with its units together
    let run_outputs: Vec<descriptor::Output> =
        d.outputs.iter().filter(|o| o.run_level).cloned().collect();
    let mut run_files: Vec<Value> = Vec::new();
    let mut models_made: Vec<Value> = Vec::new();
    if !apart && exit_code == Some(0) {
        for o in &run_outputs {
            let found = nils_pipeline::files::found(out, &o.template, &[]);
            if o.kind == "model" {
                match found.as_slice() {
                    [rel] => match register_model(registry, x, o, out, rel, &now) {
                        Ok((model, id, sha)) => {
                            registered += 1;
                            run_files.push(json!({"path": rel, "sha256": sha}));
                            models_made.push(json!({
                                "output": o.id, "model": model.named(), "derivative": id,
                                "encoders": model.encoder_model_ids, "trained_on": model.trained_on,
                            }));
                        }
                        Err(why) => refused.push(json!({"output": o.id, "file": rel, "why": why})),
                    },
                    [] => refused.push(json!({"output": o.id, "why": "the run wrote no model"})),
                    many => refused.push(json!({
                        "output": o.id,
                        "why": format!("{} files match; a model output is one file", many.len()),
                    })),
                }
                continue;
            }
            for rel in found {
                let file = match nils_pipeline::files::inside(out, &rel) {
                    Ok(f) => f,
                    Err(why) => {
                        refused.push(json!({"output": o.id, "file": rel, "why": why}));
                        continue;
                    }
                };
                if !taken_files.insert(file.clone()) {
                    refused.push(json!({
                        "output": o.id, "file": rel,
                        "why": "the same file as another output of the run, by another name",
                    }));
                    continue;
                }
                let (bytes, sha) =
                    nils_pipeline::files::sha256_file(&file).map_err(|e| format!("{rel}: {e}"))?;
                let media = nils_pipeline::files::media_type(&rel, o.media_type.as_deref());
                let id = derivative::insert_of_run(
                    registry.store(),
                    &derivative::New {
                        kind: &o.kind,
                        belongs: &Belongs::run(),
                        place_id: x.place.id,
                        path: &place_path(working, &file)?,
                        bytes: bytes as i64,
                        sha256: &sha,
                        media_type: &media,
                        registered_by: x.who,
                        actor: Some(x.actor),
                        model_id: None,
                        run_id: None,
                        preprocess_version: None,
                        supersedes_id: None,
                        created_at: &now,
                    },
                    x.run_id,
                )
                .map_err(|e| e.to_string())?;
                registered += 1;
                bytes_total += bytes;
                // a run's table: each row's unit by its unit column (record
                // 49 A3), and only a unit that succeeded
                if o.table.is_some() {
                    let units: Vec<(String, Option<Belongs>)> = statuses
                        .iter()
                        .map(|(i, status, ..)| {
                            let u = &m.units[*i];
                            (
                                u.id.clone(),
                                (status == "succeeded").then(|| u.belongs()).flatten(),
                            )
                        })
                        .collect();
                    let read = std::fs::read(&file)
                        .map_err(|e| e.to_string())
                        .and_then(|b| {
                            crate::measures::run_table(o, &b, &units, id, &mut notes, &mut measured)
                        });
                    if let Err(why) = read {
                        refused.push(json!({
                            "output": o.id, "file": rel,
                            "why": format!("its rows could not be read: {why}"),
                        }));
                    }
                }
                run_files.push(json!({"path": rel, "sha256": sha}));
            }
        }
    }
    run_files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));

    // the seeds and the selection it suggests (record 43): kept whole as the
    // run's one derivative of kind seeds, apart from any proposal
    let mut seeds_doc = Value::Null;
    let mut seeds_digest = Value::Null;
    let all_seeds: Vec<&nils_pipeline::results::Seed> =
        reports.iter().flat_map(|r| r.seeds.iter()).collect();
    let any_selection = reports.iter().any(|r| r.selection.is_some());
    if !all_seeds.is_empty() || any_selection {
        let ours: std::collections::BTreeSet<i64> = x.stacks.iter().copied().collect();
        let taken: Vec<&Value> = all_seeds
            .iter()
            .filter(|s| ours.contains(&s.stack_id))
            .map(|s| &s.entry)
            .collect();
        let outside = all_seeds.len() - taken.len();
        let mut selection: Vec<i64> = reports
            .iter()
            .flat_map(|r| r.selection.iter().flatten().copied())
            .filter(|s| ours.contains(s))
            .collect();
        selection.sort_unstable();
        selection.dedup();
        let doc = json!({
            "contract": nils_pipeline::CONTRACT, "run": x.run_id, "pipeline": p.label(),
            "seeds": taken, "selection": {"stacks": selection},
        });
        let text = nils_pipeline::canonical(&doc);
        // into a folder of the engine's own beside the output, which the
        // container never saw, created new and never through a link
        let own = working.join(format!("{rel_out}.nils"));
        match std::fs::symlink_metadata(&own) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "{rel_out}.nils is there already and is not a folder"
                ));
            }
            Err(_) => {
                std::fs::create_dir(&own).map_err(|e| format!("the run's own folder: {e}"))?
            }
        }
        let rel = format!("{rel_out}.nils/{}", nils_pipeline::results::SEEDS_FILE);
        let at = working.join(&rel);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_file()) {
            // a resume writes what the run says now
            let _ = std::fs::remove_file(&at);
        }
        nils_pipeline::files::write_new(&at, text.as_bytes()).map_err(|e| format!("{rel}: {e}"))?;
        let sha = hex_of(&nils_pipeline::sha256(text.as_bytes()));
        let id = derivative::insert_of_run(
            registry.store(),
            &derivative::New {
                kind: "seeds",
                belongs: &Belongs::run(),
                place_id: x.place.id,
                path: &rel,
                bytes: text.len() as i64,
                sha256: &sha,
                media_type: "application/json",
                registered_by: x.who,
                actor: Some(x.actor),
                model_id: None,
                run_id: None,
                preprocess_version: None,
                supersedes_id: None,
                created_at: &now,
            },
            x.run_id,
        )
        .map_err(|e| e.to_string())?;
        registered += 1;
        bytes_total += text.len() as u64;
        seeds_doc = json!({
            "seeds": taken.len(), "outside": outside, "selection": selection.len(),
            "derivative": id,
        });
        seeds_digest = json!(sha);
    }

    let proposals: Vec<Value> = reports
        .iter()
        .flat_map(|r| r.proposals.iter().cloned())
        .collect();
    let results_digest = nils_pipeline::sha256(
        nils_pipeline::canonical(&json!({
            "units": digest_units, "proposals": proposals, "run": run_files, "seeds": seeds_digest,
        }))
        .as_bytes(),
    );

    // the units no one can vouch for become review items; a container
    // that failed as a whole, with no results.json to say more, is one
    // item of the run, not one per unit (wave 43's proof: three failed
    // runs left 2,023 items, each run for a single cause), and so is a run
    // whose every unit's container did
    let mut items: Vec<i64> = Vec::new();
    let label = p.label();
    if whole && !m.units.is_empty() {
        let error = statuses
            .first()
            .and_then(|s| s.2.clone())
            .unwrap_or_else(|| "the run failed".into());
        let log = unit_rows
            .first()
            .and_then(|u| u.outcome["log"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{RUNS}/{}/log.txt", x.run_id));
        let id = nils_registry::review::raise_pipeline_qc(
            registry.store(),
            &nils_registry::review::PipelineQc {
                run_id: x.run_id,
                pipeline: &label,
                unit: "run",
                stack_id: None,
                subject_id: None,
                session_day: None,
                status: "failed",
                error: Some(&error),
                metrics: &json!({
                    "units": m.units.len(), "exit_code": exit_code, "log": log,
                }),
                job_id: Some(x.job_id),
            },
            &now,
        )
        .map_err(|e| e.to_string())?;
        items.push(id);
    }
    for (i, status, error, metrics, _) in &statuses {
        if whole || (status != "failed" && status != "unreported") {
            continue;
        }
        let u = &m.units[*i];
        let id = nils_registry::review::raise_pipeline_qc(
            registry.store(),
            &nils_registry::review::PipelineQc {
                run_id: x.run_id,
                pipeline: &label,
                unit: &u.id,
                stack_id: u.stack_id,
                subject_id: u.subject_id,
                session_day: u.session_day.as_deref(),
                status: if status == "unreported" {
                    "unreported"
                } else {
                    "failed"
                },
                error: error.as_deref(),
                metrics,
                job_id: Some(x.job_id),
            },
            &now,
        )
        .map_err(|e| e.to_string())?;
        items.push(id);
    }

    // a file the engine refused, a unit's or the run's own, is a review
    // item too (record 43 review)
    for r in &refused {
        let unit = r["unit"].as_str().unwrap_or("run");
        let u = m.units.iter().find(|u| u.id == unit);
        let id = nils_registry::review::raise_pipeline_qc(
            registry.store(),
            &nils_registry::review::PipelineQc {
                run_id: x.run_id,
                pipeline: &label,
                unit,
                stack_id: u.and_then(|u| u.stack_id),
                subject_id: u.and_then(|u| u.subject_id),
                session_day: u.and_then(|u| u.session_day.as_deref()),
                status: "refused",
                error: r["why"].as_str(),
                metrics: &json!({"file": r["file"], "output": r["output"]}),
                job_id: Some(x.job_id),
            },
            &now,
        )
        .map_err(|e| e.to_string())?;
        if !items.contains(&id) {
            items.push(id);
        }
    }

    // record 49 A3: the declared checks, held against each unit that
    // succeeded, a breach one pipeline:qc item naming the metric and its
    // value; a unit that has an item already keeps it
    let mut breached: Vec<Value> = Vec::new();
    if !whole {
        let already: std::collections::BTreeSet<String> = statuses
            .iter()
            .filter(|(_, status, ..)| status == "failed" || status == "unreported")
            .map(|(i, ..)| m.units[*i].id.clone())
            .chain(
                refused
                    .iter()
                    .filter_map(|r| r["unit"].as_str().map(str::to_string)),
            )
            .collect();
        for (i, status, _, metrics, _) in &statuses {
            if status != "succeeded" || d.checks.is_empty() {
                continue;
            }
            let u = &m.units[*i];
            let mine: BTreeMap<String, f64> = measured
                .iter()
                .filter(|p| p.unit_id == u.id)
                .filter_map(|p| Some((p.name.clone(), p.number?)))
                .collect();
            let (breaches, taken, unchecked) = crate::measures::hold(&d.checks, metrics, &mine);
            notes.unchecked += unchecked;
            if let Some(b) = u.belongs() {
                for (metric, value) in taken {
                    measured.push(crate::measures::Pending {
                        unit_id: u.id.clone(),
                        belongs: b.clone(),
                        derivative_id: None,
                        source: nils_registry::measure::METRICS.into(),
                        name: metric,
                        ty: "number".into(),
                        number: Some(value),
                        text: None,
                        unit: None,
                    });
                }
            }
            if breaches.is_empty() {
                continue;
            }
            notes.breaches += breaches.len();
            breached.push(json!({"unit": u.id, "breaches": breaches}));
            if already.contains(&u.id) {
                continue;
            }
            let words = crate::measures::breach_words(&breaches);
            let id = nils_registry::review::raise_pipeline_qc(
                registry.store(),
                &nils_registry::review::PipelineQc {
                    run_id: x.run_id,
                    pipeline: &label,
                    unit: &u.id,
                    stack_id: u.stack_id,
                    subject_id: u.subject_id,
                    session_day: u.session_day.as_deref(),
                    status: "breach",
                    error: Some(&words),
                    metrics: &json!({"breaches": breaches, "metrics": metrics}),
                    job_id: Some(x.job_id),
                },
                &now,
            )
            .map_err(|e| e.to_string())?;
            if !items.contains(&id) {
                items.push(id);
            }
        }
    }
    let measures_written =
        crate::measures::write(registry.store(), x.run_id, p.id, &p.name, &measured, &now)?;

    // the proposals on the axes it declares, through the review spine
    // (record 43 S6): grouped model items, staged at the model's threshold
    let (declared, undeclared): (Vec<Value>, Vec<Value>) =
        proposals.iter().cloned().partition(|p| {
            p["axis"]
                .as_str()
                .is_some_and(|a| d.proposals.iter().any(|x| x == a))
        });
    // a run speaks for its own stacks, and for the models it was given or
    // made (record 43 review)
    let ours: std::collections::BTreeSet<i64> = x.stacks.iter().copied().collect();
    let speaks_for: std::collections::BTreeSet<i64> = x
        .models
        .iter()
        .map(|m| m.id)
        .chain(models_made.iter().filter_map(|m| m["model"]["id"].as_i64()))
        .collect();
    let mut proposed = json!({
        "given": proposals.len(), "declared": declared.len(),
        "undeclared": undeclared.len(), "taken": 0,
    });
    if !declared.is_empty() {
        let taken =
            nils_registry::proposals::parse(&json!({ "proposals": declared })).and_then(|parsed| {
                nils_registry::proposals::ingest(
                    registry,
                    &nils_registry::proposals::Run {
                        id: x.run_id,
                        job_id: Some(x.job_id),
                        principal: x.who,
                        stacks: Some(&ours),
                        models: Some(&speaks_for),
                    },
                    &parsed,
                    x.threshold,
                )
            });
        match taken {
            Ok(done) => {
                proposed["taken"] = json!(done.members);
                proposed["ingested"] = done.to_json();
            }
            Err(e) => proposed["refused"] = json!(e.to_string()),
        }
    }

    let count = |s: &str| statuses.iter().filter(|x| x.1 == s).count();
    let first_log = unit_rows
        .first()
        .and_then(|u| u.outcome["log"].as_str().map(str::to_string));
    let summary = json!({
        "units": {
            "total": m.units.len(), "succeeded": count("succeeded"), "failed": count("failed"),
            "skipped": count("skipped"), "unreported": count("unreported"),
        },
        "derivatives": registered,
        "bytes": bytes_total,
        "embeddings": {"registered": embedded, "kept": cached},
        "run_files": run_files,
        "models": models_made,
        "seeds": seeds_doc,
        "refused_files": refused,
        "review_items": items,
        "proposals": proposed,
        "results": match (reports.is_empty(), unreadable) {
            (false, _) if apart => json!("results.json, one per unit"),
            (false, _) => json!("results.json"),
            (true, Some(e)) => json!(format!("unreadable: {e}")),
            (true, None) => json!("none: found by the declared templates"),
        },
        // record 49 A3: the run's tables, measures and checks
        "numbers": notes.summary(measures_written, d.checks.len()),
        "breaches": breached,
        "input_release_id": m.release_id,
        // what the container was given of the sources: one bind per folder
        // of the selection's files, or the roots past the limit, and why
        "scope": m.scope,
        "log": if apart { format!("{RUNS}/{}/units/<unit>/log.txt", x.run_id) } else { first_log.unwrap_or_else(|| format!("{RUNS}/{}/log.txt", x.run_id)) },
        // record 49: how its units ran, in which lane, what it was taken up
        // again, and the secrets it read, by id alone
        "units_mode": d.units.name(),
        "lane": x.lane.doc(),
        "needs": {"cores": d.needs.cores, "memory_gb": d.needs.memory_gb, "gpu_memory_gb": x.gpu.then_some(d.needs.gpu_memory_gb)},
        "resumed": x.resume.is_some(),
        "secrets": x.secrets.iter().map(|g| g.secret.id.clone()).collect::<Vec<_>>(),
    });
    if completed {
        // record 43: a run that completed with units that failed or went
        // unreported, or a file it made that was refused, is partial; the
        // units are review items
        let short = count("failed") + count("unreported") > 0 || !refused.is_empty();
        let status = if short { "partial" } else { "done" };
        Ok((status, summary, Some(results_digest), exit_code, None))
    } else {
        let log = if apart {
            format!("{RUNS}/{}/units/<unit>/log.txt", x.run_id)
        } else {
            format!("{RUNS}/{}/log.txt", x.run_id)
        };
        let mut error = format!(
            "the container exited {}; its log is {log} in the working place {}",
            exit_code.map_or("by a signal".to_string(), |c| c.to_string()),
            x.place.name
        );
        // 125 is podman's and docker's own failure, as when the image is
        // not in the store it looked in: say which store (wave 43's proof:
        // a scratch HOME has an empty store of its own)
        if exit_code == Some(125) && matches!(x.runtime.name(), "podman" | "docker") {
            error.push_str(&format!(
                "; 125 is {}'s own failure, as when it cannot find or pull {}: it looked in the image store {}{}",
                x.runtime.name(),
                p.image,
                x.runtime.store().as_deref().unwrap_or("it could not name"),
                std::env::var("HOME")
                    .map(|h| format!(" (HOME is {h}; a HOME of its own has a store of its own)"))
                    .unwrap_or_default()
            ));
        }
        Ok((
            "failed",
            summary,
            Some(results_digest),
            exit_code,
            Some(error),
        ))
    }
}

/// The engine process's own uid and gid, the ids podman's `--userns
/// keep-id` maps to themselves.
#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "getuid and getgid read this process and cannot fail"
)]
fn this_account() -> (u32, u32) {
    // SAFETY: neither call takes a pointer, touches memory, or can fail.
    unsafe { (libc::getuid(), libc::getgid()) }
}

/// A run's seeds, as the runner kept them: the document of its derivative
/// of kind seeds, read back from the working place and checked against the
/// digest the row holds.
fn seeds_of(store: &mut Store, run: i64) -> Result<Value, String> {
    let r = rows::run(store, run)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no pipeline run {run}"))?;
    let rows = derivative::list(
        store,
        &derivative::Filter {
            kind: Some("seeds"),
            run_id: Some(r.id),
            limit: 1,
            ..derivative::Filter::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let row = rows
        .into_iter()
        .find(|d| d.withdrawn_at.is_none())
        .ok_or_else(|| format!("run {run} suggested no seeds"))?;
    let place = nils_registry::place::show(store, row.place_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("the place of derivative {} is gone", row.id))?;
    let bytes = std::fs::read(Path::new(&place.path).join(&row.path))
        .map_err(|e| format!("the seeds of run {run}: {e}"))?;
    if hex_of(&nils_pipeline::sha256(&bytes)) != row.sha256 {
        return Err(format!(
            "the seeds file of run {run} is not the one registered (derivative {})",
            row.id
        ));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("the seeds of run {run}: {e}"))
}

/// Where the artifact of a model a run fitted lies under the working place:
/// the newest live derivative of kind model naming it there.
fn model_artifact(
    store: &mut Store,
    model_id: i64,
    place_id: i64,
) -> Result<Option<String>, String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT path FROM {} WHERE kind = 'model' AND model_id = {} AND place_id = {} \
         AND withdrawn_at IS NULL ORDER BY id DESC",
        store.qualified("derivative"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(model_id), Param::Int(place_id)])
        .map_err(|e| e.to_string())?
        .map(|r| r.text(0).map(str::to_string))
        .transpose()
        .map_err(|e| e.to_string())
}

/// The folder, relative to its source root, a file is bound through: its
/// own, or where that path holds a ':' or a ',', which the runtimes' mount
/// syntax splits on, its nearest ancestor that holds neither; the root
/// itself, `""`, for a file directly in it. None when the root's own path
/// holds one.
fn bind_folder(root: &str, path: &str) -> Option<String> {
    let bad = |p: &str| p.contains(':') || p.contains(',');
    if bad(root) {
        return None;
    }
    let mut dir = Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    loop {
        let text = dir.to_string_lossy().into_owned();
        if !bad(&text) {
            return Some(text);
        }
        dir = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    }
}

/// The live derivatives of the working place each derivative input of a
/// descriptor takes, over the selection's stacks, read a few hundred
/// stacks at a time.
fn derivative_inputs(
    store: &mut Store,
    d: &Descriptor,
    stacks: &[i64],
    place_id: i64,
) -> Result<BTreeMap<String, Vec<derivative::Derivative>>, String> {
    let mut out: BTreeMap<String, Vec<derivative::Derivative>> = BTreeMap::new();
    for t in d.inputs.iter().filter(|t| t.ty.starts_with("derivative:")) {
        let kind = t.ty.trim_start_matches("derivative:");
        let rows =
            derivative::of_stacks(store, kind, stacks, place_id).map_err(|e| e.to_string())?;
        out.insert(t.id.clone(), rows);
    }
    Ok(out)
}

/// Link one derivative into a run's input folder at its path under the
/// derivatives tree, by a hard link (a copy where the two are not on one
/// filesystem). The file is taken only where it resolves inside the
/// working place's derivatives tree. Answers the path under the input.
fn link_input(working: &Path, path: &str, into: &Path) -> Result<String, String> {
    let rel = path
        .strip_prefix(&format!("{}/", derivative::TREE))
        .ok_or_else(|| format!("{path} is not under the derivatives tree"))?;
    let tree = std::fs::canonicalize(working.join(derivative::TREE))
        .map_err(|e| format!("the derivatives tree: {e}"))?;
    let real = std::fs::canonicalize(working.join(path)).map_err(|e| format!("{path}: {e}"))?;
    if !real.starts_with(&tree) || !real.is_file() {
        return Err(format!("{path} is not a file of the derivatives tree"));
    }
    if Path::new(rel)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("{path} is not a plain path"));
    }
    let at = into.join(rel);
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("the run's inputs: {e}"))?;
    }
    match std::fs::hard_link(&real, &at) {
        Ok(()) => {}
        // linked already, by an earlier row of the same file: nothing to do;
        // anything else in its place is refused, never copied over
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !same_file(&real, &at) {
                return Err(format!(
                    "{path}: the run's inputs hold another file at {rel} already"
                ));
            }
        }
        // only across filesystems is it copied, onto nothing
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let bytes = std::fs::read(&real).map_err(|e| format!("{path}: {e}"))?;
            nils_pipeline::files::write_new(&at, &bytes).map_err(|e| format!("{path}: {e}"))?;
        }
        Err(e) => return Err(format!("{path}: {e}")),
    }
    Ok(rel.to_string())
}

/// A file's path under the working place, from where it really is: the row
/// never names a link (record 43 review).
fn place_path(working: &Path, file: &Path) -> Result<String, String> {
    let root = std::fs::canonicalize(working).map_err(|e| format!("the working place: {e}"))?;
    let real = std::fs::canonicalize(file).map_err(|e| format!("{}: {e}", file.display()))?;
    real.strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| format!("{} is outside the working place", file.display()))
}

/// Whether two paths are one file on disk.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
    }
}

/// `sha256:<hex>` to its hex, as the derivative rows keep a digest.
fn hex_of(digest: &str) -> String {
    digest.strip_prefix("sha256:").unwrap_or(digest).to_string()
}

/// Register an embedding a run wrote under the cache's key (record 43 S4):
/// its header names the stack, the encoder and the preprocessing; the
/// encoder is the registry's, or registered from the card the results
/// carry for it. Answers what the cache did, or why the file is refused.
#[allow(clippy::too_many_arguments)]
fn register_embedding(
    registry: &mut Registry,
    x: &Execution<'_>,
    u: &Unit,
    declared: &[String],
    file: &Path,
    path: &str,
    bytes: u64,
    sha: &str,
    cards: &[Value],
    now: &str,
) -> Result<nils_registry::embedding::Registered, String> {
    use nils_registry::embedding;
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    // read whole, every value finite, before anything is registered
    let h = embedding::decode(&data)?.header;
    if u.stack_id != Some(h.stack_id) {
        return Err(format!(
            "the embedding names stack {}, and the unit is {}",
            h.stack_id, u.id
        ));
    }
    // only an encoder the run was given, or its descriptor declares
    let given = x.models.iter().any(|m| m.digest == h.encoder);
    if !given && !declared.contains(&h.encoder) {
        return Err(format!(
            "the encoder {} is one the run was neither given nor declares for this output",
            h.encoder
        ));
    }
    let encoder = match nils_registry::model::by_digest(registry.store(), &h.encoder)
        .map_err(|e| e.to_string())?
    {
        Some(m) => m,
        None => {
            let card = cards
                .iter()
                .find(|c| c["digest"] == h.encoder.as_str() && c["kind"] == "encoder")
                .ok_or_else(|| {
                    format!(
                        "the encoder {} is not registered, and the results carry no card for it",
                        h.encoder
                    )
                })?;
            let text = |k: &str| card[k].as_str().filter(|s| !s.is_empty());
            embedding::register_encoder(
                registry,
                &embedding::Encoder {
                    name: text("name").ok_or("the encoder's card names no name")?,
                    version: text("version").ok_or("the encoder's card names no version")?,
                    weights_digest: &h.encoder,
                    image_digest: text("image_digest"),
                },
                x.who,
            )
            .map_err(|e| e.to_string())?
        }
    };
    embedding::register(
        registry,
        &embedding::New {
            stack_id: h.stack_id,
            encoder_id: encoder.id,
            preprocess_version: &h.preprocess_version,
            place_id: x.place.id,
            path,
            bytes: bytes as i64,
            sha256: sha,
            registered_by: x.who,
            actor: Some(x.actor),
            run_id: Some(x.run_id),
            created_at: now,
        },
    )
    .map_err(|e| e.to_string())
}

/// Register the model a run fitted (record 43): the artifact hashed by the
/// engine, the card beside it naming that digest, trained on the label set
/// the run was given, its encoders the card's, in state registered. The
/// artifact becomes the run's derivative of kind model, which a later run
/// that reads the model mounts. Answers the model, the derivative's id and
/// the artifact's digest.
fn register_model(
    registry: &mut Registry,
    x: &Execution<'_>,
    o: &descriptor::Output,
    out: &Path,
    rel: &str,
    now: &str,
) -> Result<(nils_registry::model::Model, i64, String), String> {
    let file = nils_pipeline::files::inside(out, rel)?;
    let (bytes, sha) =
        nils_pipeline::files::sha256_file(&file).map_err(|e| format!("{rel}: {e}"))?;
    let card_rel = o.card.as_deref().ok_or("a model output names its card")?;
    let card_file = nils_pipeline::files::inside(out, card_rel)?;
    let text =
        std::fs::read_to_string(&card_file).map_err(|e| format!("the card {card_rel}: {e}"))?;
    let mut card: Value =
        serde_json::from_str(&text).map_err(|e| format!("the card {card_rel}: {e}"))?;
    let digest = format!("sha256:{sha}");
    if card["digest"].as_str() != Some(digest.as_str()) {
        return Err(format!(
            "the card names the digest {}, and the artifact is {digest}",
            card["digest"]
        ));
    }
    // trained on the label set it was given, and no other
    let bare = |d: &str| d.strip_prefix("sha256:").unwrap_or(d).to_string();
    match (x.label_set, card["trained_on"]["label_set"].as_str()) {
        (Some(set), Some(named)) if bare(named) != bare(&set.digest) => {
            return Err(format!(
                "the card says it was trained on {named}, and the run gave it label set {} ({})",
                set.id, set.digest
            ));
        }
        (Some(set), _) => {
            if !card["trained_on"].is_object() {
                card["trained_on"] = json!({});
            }
            card["trained_on"]["label_set"] = json!(format!("sha256:{}", bare(&set.digest)));
            card["trained_on"]["name"] = json!(set.name);
            card["trained_on"]["rows"] = json!(set.rows);
        }
        (None, Some(named)) => {
            return Err(format!(
                "the card says it was trained on {named}, and the run was given no label set"
            ));
        }
        (None, None) => {}
    }
    let model =
        nils_registry::model::register(registry, &card, x.who).map_err(|e| e.to_string())?;
    nils_registry::model::set_job(registry.store(), model.id, x.job_id)
        .map_err(|e| e.to_string())?;
    let media = nils_pipeline::files::media_type(rel, o.media_type.as_deref());
    let id = derivative::insert_of_run(
        registry.store(),
        &derivative::New {
            kind: "model",
            belongs: &Belongs::run(),
            place_id: x.place.id,
            path: &place_path(Path::new(&x.place.path), &file)?,
            bytes: bytes as i64,
            sha256: &sha,
            media_type: &media,
            registered_by: x.who,
            actor: Some(x.actor),
            model_id: Some(model.id),
            run_id: None,
            preprocess_version: None,
            supersedes_id: None,
            created_at: now,
        },
        x.run_id,
    )
    .map_err(|e| e.to_string())?;
    Ok((model, id, sha))
}

/// A unit's files as the declared templates find them.
fn found_for(outputs: &[descriptor::Output], out: &Path, u: &Unit) -> Vec<String> {
    let vars = u.vars();
    let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut files: Vec<String> = outputs
        .iter()
        .flat_map(|o| nils_pipeline::files::found(out, &o.template, &pairs))
        .collect();
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The review of record 43: at detail plain a run's document says no
    /// unit by its subject or session, and keeps everything else.
    #[test]
    fn a_run_read_at_plain_detail_names_no_subject_or_session() {
        let mut doc = json!({
            "id": 3, "selection": "selection:every@1",
            "summary": {"refused_files": [
                {"unit": "sub-P1_ses-20220115", "why": "not its own"},
                {"unit": "stack-12", "why": "a link out"},
                {"unit": "sub-P2", "file": "sub-P2/x.nii.gz"},
            ], "units": {"total": 3}},
        });
        plain(&mut doc);
        let refused = doc["summary"]["refused_files"].as_array().unwrap();
        assert!(refused[0]["unit"].is_null() && refused[2]["file"].is_null());
        assert_eq!(refused[1]["unit"], "stack-12");
        assert_eq!(doc["summary"]["units"]["total"], 3);
        assert!(!doc.to_string().contains("sub-"), "{doc}");
    }

    /// Record 49 R4 and R4b, after the assistant's review: below detail
    /// quasi a run's checks and failures are counts by check and by reason,
    /// a count of 1 to 4 scans withheld, and no unit, value or tool's words.
    #[test]
    fn a_run_read_below_quasi_says_counts_by_check_and_reason_and_no_scan() {
        let breach = |unit: &str, check: &str, metric: &str, value: f64| {
            json!({"unit": unit, "breaches": [
                {"metric": metric, "value": value, "check": check, "op": ">=", "threshold": 8},
            ]})
        };
        let mut breaches: Vec<Value> = (1..=6)
            .map(|i| breach(&format!("stack-{i}"), "snr >= 8", "snr", 3.25 + i as f64))
            .collect();
        breaches.push(breach("sub-P7_ses-1", "holes <= 200", "holes", 251.5));
        let mut doc = json!({
            "id": 9, "status": "partial",
            "error": "the tool said: sub-P9 has no /data/raw/P9/x.dcm",
            "unit_states": {"over": 12},
            "units_run": [{"unit": "stack-1", "state": "over", "status": "failed", "exit_code": 3}],
            "summary": {
                "units": {"total": 12, "succeeded": 10, "failed": 2, "skipped": 0, "unreported": 0},
                "numbers": {
                    "tables": {"refused_values": ["volumes: brain_volume: 12.75x", "volumes: brain_volume: 13.5x"]},
                    "checks": {"declared": 2, "breaches": 7, "unchecked": 0},
                },
                "breaches": breaches,
                "refused_files": [
                    {"unit": "stack-11", "file": "stack-11/x.nii.gz", "why": "a link out"},
                    {"output": "model", "why": "the run wrote no model"},
                ],
                "review_items": [1, 2, 3],
            },
        });
        run_totals_only(&mut doc);
        let s = &doc["summary"];
        assert_eq!(s["breaches"], json!([]), "{doc}");
        assert_eq!(
            s["breaches_by_check"],
            json!([
                {"check": "holes <= 200", "metric": "holes", "units": null, "withheld": true},
                {"check": "snr >= 8", "metric": "snr", "units": 6},
            ]),
            "{doc}"
        );
        assert_eq!(
            s["numbers"]["checks"]["breaches"], 7,
            "7 breaches stand for 7 scans"
        );
        assert_eq!(
            s["failures_by_reason"],
            json!([
                {"reason": "failed", "units": null, "withheld": true},
                {"reason": "refused", "units": null, "withheld": true},
            ]),
            "{doc}"
        );
        // a count held is held where the run's totals would say it again
        assert!(s["units"]["failed"].is_null() && s["units"]["succeeded"].is_null());
        assert_eq!(s["units"]["total"], 12);
        assert_eq!(
            s["numbers"]["tables"]["refused_values"],
            json!([{"column": "volumes: brain_volume", "values": null, "withheld": true}])
        );
        assert_eq!(
            s["refused_files"],
            json!([{"output": "model", "why": "the run wrote no model"}])
        );
        assert!(
            s["review_items"].is_null(),
            "3 items stand for 3 units: {doc}"
        );
        assert_eq!(doc["units_run"], json!([]));
        assert_eq!(doc["unit_states"]["over"], 12);
        assert_eq!(s["detail"], "totals");
        let text = doc.to_string();
        for leak in [
            "stack-",
            "sub-",
            "3.25",
            "4.25",
            "251.5",
            "12.75",
            "/data/raw",
        ] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }

        // a job's result is its run's summary; a pipeline:qc item says its
        // status and run alone
        let mut job = json!({
            "kind": "pipeline", "error": "stack-3 failed: /data/raw/P3",
            "result": {"run": 9, "summary": {"breaches": [breach("stack-3", "snr >= 8", "snr", 4.5)],
                        "numbers": {"checks": {"breaches": 1}}}},
        });
        job_totals_only(&mut job);
        assert!(job["result"]["summary"]["numbers"]["checks"]["breaches"].is_null());
        assert!(!job.to_string().contains("stack-3") && !job.to_string().contains("4.5"));
        // below detail quasi a review list says a run's items one a check
        // or a reason with its count, never one a unit
        let qc = |id: i64, unit: &str, status: &str, check: Option<&str>| {
            json!({
                "id": id, "kind": "pipeline:qc", "scope": "run", "status": "open",
                "group_key": format!("run:9|unit:{unit}"),
                "ref": {"run_id": 9, "pipeline": "volumes@1", "unit": unit, "stack_id": id},
                "evidence": {"status": status, "error": "snr is 4.5, and the check is snr >= 8",
                             "metrics": {"breaches": check.map_or(json!([]), |c| json!([{"check": c, "value": 4.5}]))}},
            })
        };
        let mut rows =
            vec![json!({"id": 1, "kind": "classify:conflict", "evidence": {"axis": "a"}})];
        rows.extend((0..6).map(|i| qc(10 + i, &format!("stack-{i}"), "breach", Some("snr >= 8"))));
        rows.push(qc(20, "sub-P1", "breach", Some("holes <= 200")));
        rows.push(qc(21, "sub-P2", "failed", None));
        let grouped = qc_items_grouped(rows);
        assert_eq!(grouped.len(), 4, "{grouped:?}");
        assert_eq!(grouped[0]["evidence"]["axis"], "a");
        let of = |check: Option<&str>, reason: &str| {
            grouped
                .iter()
                .find(|g| {
                    g["evidence"]["check"].as_str() == check && g["evidence"]["status"] == reason
                })
                .cloned()
                .unwrap()
        };
        assert_eq!(of(Some("snr >= 8"), "breach")["units"], 6);
        assert_eq!(of(Some("holes <= 200"), "breach")["withheld"], true);
        assert!(of(None, "failed")["units"].is_null());
        assert_eq!(
            of(None, "failed")["ref"],
            json!({"run_id": 9, "pipeline": "volumes@1"})
        );
        let text = json!(grouped).to_string();
        for leak in ["stack-", "sub-", "4.5", "the check is"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
        assert!(grouped[1..].iter().all(|g| g.get("id").is_none()), "{text}");
    }

    /// The second review of record 43: a folder whose name the runtimes'
    /// mount syntax would split is bound through an ancestor, not refused,
    /// and a file directly in its source root binds that root.
    #[test]
    fn a_folder_a_mount_would_misread_is_bound_through_its_parent() {
        assert_eq!(bind_folder("/data", "P1/1/f.dcm").as_deref(), Some("P1/1"));
        assert_eq!(bind_folder("/data", "P1/a:b/f.dcm").as_deref(), Some("P1"));
        assert_eq!(bind_folder("/data", "x,y/a:b/f.dcm").as_deref(), Some(""));
        assert_eq!(bind_folder("/data", "f.dcm").as_deref(), Some(""));
        assert_eq!(bind_folder("/da:ta", "P1/f.dcm"), None);
    }

    /// The second review of record 43: linking an input twice, or a file
    /// already linked, never copies a file onto its own link, which would
    /// empty it.
    #[test]
    fn an_input_linked_twice_keeps_its_bytes() {
        let root = std::env::temp_dir().join(format!("nils-link-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let file = root.join("derivatives/p/1/stack-1/x.emb");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"the embedding").unwrap();
        let into = root.join("runs/2/inputs/emb");
        std::fs::create_dir_all(&into).unwrap();
        for _ in 0..2 {
            let rel = link_input(&root, "derivatives/p/1/stack-1/x.emb", &into).unwrap();
            assert_eq!(rel, "p/1/stack-1/x.emb");
        }
        assert_eq!(std::fs::read(&file).unwrap(), b"the embedding");
        assert_eq!(
            std::fs::read(into.join("p/1/stack-1/x.emb")).unwrap(),
            b"the embedding"
        );
        // another file already at the link's place is refused, never overwritten
        let other = root.join("derivatives/p/1/stack-2/x.emb");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"another").unwrap();
        std::fs::write(
            into.join("p/1/stack-2/x.emb")
                .parent()
                .map(|p| {
                    std::fs::create_dir_all(p).unwrap();
                    p.join("x.emb")
                })
                .unwrap(),
            b"planted",
        )
        .unwrap();
        assert!(link_input(&root, "derivatives/p/1/stack-2/x.emb", &into).is_err());
        assert_eq!(std::fs::read(&other).unwrap(), b"another");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Record 49 A1: the door queues a resume as it queues a run, and still
    /// takes no flag of a caller's own.
    #[test]
    fn a_door_queues_a_resume_and_nothing_else() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let pack = Path::new("/packs");
        assert_eq!(
            located(Some(pack), words(&["run", "--resume", "7"])).ok(),
            Some(words(&["run", "--resume", "7", "--pack-dir", "/packs"]))
        );
        assert!(located(None, words(&["run"])).is_err());
        assert!(located(None, words(&["run", "--resume"])).is_err());
        assert!(located(None, words(&["run", "--resume", "7", "--dcm2niix", "/x"])).is_err());
    }

    /// Record 49: a unit's id becomes a folder only where it is a plain word.
    #[test]
    fn a_unit_s_folder_is_a_plain_word() {
        assert!(folder_word("sub-P1_ses-20220115").is_ok());
        assert!(folder_word("stack-12").is_ok());
        for bad in ["", "..", ".hidden", "a/b", "sub-1 x"] {
            assert!(folder_word(bad).is_err(), "{bad}");
        }
    }
}
