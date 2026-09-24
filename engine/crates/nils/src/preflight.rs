// SPDX-License-Identifier: AGPL-3.0-only

//! The pre-flight of a run (record 49 A3): before anything runs, what a run
//! of a pipeline over a selection would do, from the registry alone. The
//! units it would have, the units that lack an input and why, the stacks of
//! the selection that would reach no unit, the time it would take (from the
//! pipeline's own past runs here, else from its descriptor), whether it
//! wants a GPU and whether this engine has one, and what a unit needs of
//! the cores and memory against the lane's budget.
//!
//! It is `POST /api/pipelines/{name}/preflight` and `nils run --preflight`.
//! It starts nothing and writes nothing but the handle a selection is frozen
//! into, which a run would freeze the same way (and which is reused while
//! the registry has not moved).
//!
//! The units are counted the way the runner makes them: in the stacks
//! layout one a stack; in the bids layout one a session (or a participant)
//! of the picks the release takes, since the runner's input is a release
//! with the picks applied (record 43 R4). A session is missing an input
//! when it has no pick of a role the descriptor names under
//! `x-nils.input.roles`; a stack when the registry holds no file of it, or
//! no derivative a typed input needs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nils_pipeline::descriptor::{self, Descriptor, Gpu, Layout, Level};
use nils_registry::home::Home;
use nils_registry::pipeline as rows;
use nils_registry::store::Store;
use nils_registry::{Param, Registry};
use serde_json::{Value, json};

use crate::serve::Reply;
use crate::{Exit, usage};

/// The door, as the capabilities list it.
pub(crate) const DOOR: &str = "POST /api/pipelines/{name}/preflight";

/// Where the registry keeps the lane's budget (record 49 R6): the cores and
/// the memory NILS's runs may use together. Unset, the ruling's 48 cores and
/// 512 GB, each no more than the host has.
pub(crate) const LANE_CORES_KEY: &str = "pipeline_lane_cores";
pub(crate) const LANE_MEMORY_KEY: &str = "pipeline_lane_memory_gb";
pub(crate) const LANE_CORES: f64 = 48.0;
pub(crate) const LANE_MEMORY_GB: f64 = 512.0;

/// What a unit is taken to need where its descriptor says nothing: one core
/// and 2 GB, as the lane takes it (record 49 A1).
const DEFAULT_CORES: f64 = 1.0;
const DEFAULT_MEMORY_GB: f64 = 2.0;

/// How many of a pipeline's past runs the estimate reads.
const PAST_RUNS: usize = 20;

/// What the pre-flight is asked.
pub(crate) struct Asked<'a> {
    pub pipeline: &'a str,
    pub handle: i64,
    pub selection: Option<&'a str>,
    pub params: &'a [(String, String)],
}

/// The cores and the memory of this host.
fn host() -> (f64, Option<f64>) {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as f64)
        .unwrap_or(1.0);
    let memory = std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| {
        t.lines()
            .find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<f64>().ok())
            .map(|kb| (kb / 1024.0 / 1024.0 * 10.0).round() / 10.0)
    });
    (cores, memory)
}

/// The lane's budget: the setting, else the ruling's cap, each no more than
/// the host has.
pub(crate) fn budget(registry: &mut Registry) -> Value {
    let (host_cores, host_memory) = host();
    let read = |registry: &mut Registry, key: &str| {
        registry
            .meta_value(key)
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0)
    };
    let set_cores = read(registry, LANE_CORES_KEY);
    let set_memory = read(registry, LANE_MEMORY_KEY);
    let cores = set_cores.unwrap_or(LANE_CORES).min(host_cores);
    let memory = match host_memory {
        Some(h) => set_memory.unwrap_or(LANE_MEMORY_GB).min(h),
        None => set_memory.unwrap_or(LANE_MEMORY_GB),
    };
    json!({
        "cores": cores, "memory_gb": memory,
        "source": if set_cores.is_some() || set_memory.is_some() { "setting" } else { "ruling" },
        "host": {"cores": host_cores, "memory_gb": host_memory},
    })
}

/// What one unit needs (`x-nils.needs`), with the lane's defaults.
fn needs(d: &Descriptor) -> (f64, f64) {
    let n = &d.document["x-nils"]["needs"];
    (
        n["cores"]
            .as_f64()
            .filter(|c| *c >= 1.0)
            .unwrap_or(DEFAULT_CORES),
        n["memory-gb"]
            .as_f64()
            .filter(|m| *m > 0.0)
            .unwrap_or(DEFAULT_MEMORY_GB),
    )
}

/// One unit the run would have.
#[derive(Debug, Clone, Default)]
struct Unit {
    subject_id: Option<i64>,
    session_day: Option<String>,
    stack_id: Option<i64>,
    stacks: Vec<i64>,
    roles: BTreeSet<String>,
    missing: Vec<String>,
}

impl Unit {
    fn name(&self) -> String {
        match (self.stack_id, self.subject_id, &self.session_day) {
            (Some(s), _, _) => format!("stack-{s}"),
            (None, Some(sub), Some(day)) => format!("subject {sub}, session of {day}"),
            (None, Some(sub), None) => format!("subject {sub}"),
            _ => "a unit".into(),
        }
    }
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The stacks layout: one unit a stack, missing where the registry holds no
/// file of it or no derivative a typed input needs.
fn stack_units(
    store: &mut Store,
    d: &Descriptor,
    stacks: &[i64],
    place_id: Option<i64>,
) -> Result<Vec<Unit>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let mut with_files: BTreeSet<i64> = BTreeSet::new();
    let mut subject: BTreeMap<i64, i64> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let ids = list(chunk);
        for sql in [
            format!(
                "SELECT DISTINCT i.stack_id FROM {} i JOIN {} f ON f.id = i.source_file_id \
                 WHERE i.stack_id IN ({ids})",
                store.qualified("instance"),
                store.qualified("source_file")
            ),
            format!(
                "SELECT DISTINCT fr.stack_id FROM {} fr WHERE fr.stack_id IN ({ids})",
                store.qualified("instance_frame")
            ),
        ] {
            for r in store.query(&sql, &[]).map_err(err)? {
                with_files.insert(r.int(0).map_err(err)?);
            }
        }
        let sql = format!(
            "SELECT st.id, se.subject_id FROM {} st JOIN {} se ON se.id = st.series_id \
             WHERE st.id IN ({ids})",
            store.qualified("stack"),
            store.qualified("series")
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            subject.insert(r.int(0).map_err(err)?, r.int(1).map_err(err)?);
        }
    }
    // the derivatives each typed input takes, stack by stack
    let mut derived: Vec<(String, String, BTreeSet<i64>)> = Vec::new();
    for t in d
        .inputs
        .iter()
        .filter(|t| t.ty.starts_with("derivative:") && !t.optional)
    {
        let kind = t.ty.trim_start_matches("derivative:");
        let have: BTreeSet<i64> = match place_id {
            Some(place) => nils_registry::derivative::of_stacks(store, kind, stacks, place)
                .map_err(err)?
                .into_iter()
                .filter_map(|r| r.stack_id)
                .collect(),
            None => BTreeSet::new(),
        };
        derived.push((t.id.clone(), kind.to_string(), have));
    }
    Ok(stacks
        .iter()
        .map(|&s| {
            let mut u = Unit {
                stack_id: Some(s),
                subject_id: subject.get(&s).copied(),
                stacks: vec![s],
                ..Unit::default()
            };
            if !subject.contains_key(&s) {
                u.missing
                    .push("the registry does not hold this stack".into());
            } else if !with_files.contains(&s) {
                u.missing
                    .push("the registry holds no file of it to read".into());
            }
            for (input, kind, have) in &derived {
                if !have.contains(&s) {
                    u.missing
                        .push(format!("no {kind} derivative of it for the input {input}"));
                }
            }
            u
        })
        .collect())
}

/// The bids layout: the units the release of the picks makes, one a session
/// (or a participant) that holds a live pick of a stack of the selection,
/// missing where it has no pick of a role the descriptor names. Answers the
/// units and the stacks no pick takes, which the input leaves out.
fn bids_units(
    store: &mut Store,
    d: &Descriptor,
    stacks: &[i64],
) -> Result<(Vec<Unit>, Vec<i64>), String> {
    let err = |e: nils_registry::Error| e.to_string();
    let mut units: BTreeMap<(i64, Option<String>), Unit> = BTreeMap::new();
    let mut picked: BTreeSet<i64> = BTreeSet::new();
    let d_ = store.dialect();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT ps.stack_id, p.subject_id, {}, p.role FROM {} ps JOIN {} p ON p.id = ps.pick_id \
             WHERE p.withdrawn_at IS NULL AND ps.stack_id IN ({}) ORDER BY ps.stack_id",
            d_.text_of_qualified(
                Some("p"),
                nils_registry::schema::table("pick")
                    .column("session_day")
                    .expect("pick.session_day")
            ),
            store.qualified("pick_stack"),
            store.qualified("pick"),
            list(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let stack = r.int(0).map_err(err)?;
            let subject = r.int(1).map_err(err)?;
            let day = r.text(2).map_err(err)?.to_string();
            let role = r.text(3).map_err(err)?.to_string();
            picked.insert(stack);
            let day = (d.level == Level::Session).then_some(day);
            let u = units.entry((subject, day.clone())).or_insert(Unit {
                subject_id: Some(subject),
                session_day: day,
                ..Unit::default()
            });
            if !u.stacks.contains(&stack) {
                u.stacks.push(stack);
            }
            u.roles.insert(role);
        }
    }
    let mut out: Vec<Unit> = units.into_values().collect();
    for u in out.iter_mut() {
        for role in &d.roles {
            if !u.roles.contains(role) {
                u.missing.push(format!(
                    "no {role} is picked for it among the selection's stacks"
                ));
            }
        }
    }
    let left: Vec<i64> = stacks
        .iter()
        .copied()
        .filter(|s| !picked.contains(s))
        .collect();
    Ok((out, left))
}

/// Seconds a unit took in the pipeline's past runs here (by name, any
/// version), the median of the newest that completed; and how many.
fn past_seconds(store: &mut Store, name: &str) -> Result<Option<(f64, usize)>, String> {
    let d = store.dialect();
    let t = nils_registry::schema::table("pipeline_run");
    let sql = format!(
        "SELECT {}, {}, {} FROM {} r JOIN {} p ON p.id = r.pipeline_id \
         WHERE p.name = {} AND r.status IN ('done', 'partial') AND r.finished_at IS NOT NULL \
         ORDER BY r.id DESC LIMIT {PAST_RUNS}",
        d.text_of_qualified(Some("r"), t.column("started_at").expect("started_at")),
        d.text_of_qualified(Some("r"), t.column("finished_at").expect("finished_at")),
        d.text_of_qualified(Some("r"), t.column("summary").expect("summary")),
        store.qualified("pipeline_run"),
        store.qualified("pipeline"),
        d.param(1, nils_registry::schema::Type::Text),
    );
    let mut per_unit: Vec<f64> = Vec::new();
    for r in store
        .query(&sql, &[Param::from(name)])
        .map_err(|e| e.to_string())?
    {
        let (Ok(a), Ok(b)) = (r.text(0), r.text(1)) else {
            continue;
        };
        let (Some(a), Some(b)) = (
            nils_registry::time::secs_of(a),
            nils_registry::time::secs_of(b),
        ) else {
            continue;
        };
        let summary: Value = r
            .opt_text(2)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let units = summary["units"]["total"].as_f64().unwrap_or(0.0);
        if units >= 1.0 && b >= a {
            per_unit.push((b - a) as f64 / units);
        }
    }
    if per_unit.is_empty() {
        return Ok(None);
    }
    per_unit.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    Ok(Some((per_unit[per_unit.len() / 2], per_unit.len())))
}

/// The pre-flight of one run, from the registry alone.
pub(crate) fn check(registry: &mut Registry, asked: &Asked<'_>) -> Result<Value, String> {
    let p = rows::resolve(registry.store(), asked.pipeline)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no pipeline {} in the catalog", asked.pipeline))?;
    let d = descriptor::from_value(p.descriptor.clone()).map_err(|e| {
        format!(
            "{}: the catalog's descriptor no longer checks: {e}",
            p.label()
        )
    })?;
    let mut blockers: Vec<String> = Vec::new();
    if p.state != "active" {
        blockers.push(format!("{} is {}", p.label(), p.state));
    }
    let params = match d.resolve(asked.params) {
        Ok(v) => Value::Object(v),
        Err(e) => {
            blockers.push(e);
            Value::Null
        }
    };
    let stacks =
        crate::pipelines::handle_stacks(registry.store(), asked.handle).map_err(|e| e.message)?;
    let place = crate::derivatives::working(registry.store()).ok();
    let (units, left) = match d.layout {
        Layout::Stacks => (
            stack_units(registry.store(), &d, &stacks, place.as_ref().map(|p| p.id))?,
            Vec::new(),
        ),
        Layout::Bids => bids_units(registry.store(), &d, &stacks)?,
    };
    if units.is_empty() {
        blockers.push(match d.layout {
            Layout::Bids => "no stack of the selection is picked, so the release of the input would hold no subject's images; nils pick run picks them".into(),
            Layout::Stacks => "the selection holds no stacks".into(),
        });
    }
    let missing: Vec<&Unit> = units.iter().filter(|u| !u.missing.is_empty()).collect();

    // the runtime, the working place and the GPU
    let detected = crate::pipelines::detect_cached(registry);
    let capability = crate::pipelines::capability(registry);
    if capability["enabled"] != true {
        blockers.push(
            capability["reason"]
                .as_str()
                .unwrap_or("pipelines are off here")
                .to_string(),
        );
    }
    let card = detected.runtime.as_ref().and_then(|r| r.gpu.clone());
    let device = match (d.gpu, &card) {
        (Gpu::None, _) | (Gpu::Optional, None) => Some("cpu".to_string()),
        (_, Some(g)) => Some(g.clone()),
        (Gpu::Required, None) => {
            blockers.push(format!(
                "{} needs a GPU, and this engine's runtime offers none",
                p.label()
            ));
            None
        }
    };

    // what a unit needs, against the lane
    let (cores, memory) = needs(&d);
    let mut budget = budget(registry);
    let (lane_cores, lane_memory) = (
        budget["cores"].as_f64().unwrap_or(1.0),
        budget["memory_gb"].as_f64().unwrap_or(0.0),
    );
    let fits = cores <= lane_cores && memory <= lane_memory;
    budget["fits"] = json!(fits);
    if !fits {
        blockers.push(format!(
            "a unit needs {} cores and {} GB, and the lane has {} cores and {} GB",
            descriptor::number_text(cores),
            descriptor::number_text(memory),
            descriptor::number_text(lane_cores),
            descriptor::number_text(lane_memory)
        ));
    }
    let apart = d.document["x-nils"]["units"].as_str() == Some("apart");
    let slots = if apart && fits {
        ((lane_cores / cores)
            .floor()
            .min((lane_memory / memory).floor()))
        .max(1.0)
    } else {
        1.0
    };

    // the time: the pipeline's own runs here first, its descriptor next
    let past = past_seconds(registry.store(), &p.name)?;
    let (per_unit, source, runs) = match (past, d.unit_minutes) {
        (Some((s, n)), _) => (Some(s), Some("runs"), n),
        (None, Some(m)) => (Some(m * 60.0), Some("descriptor"), 0),
        (None, None) => (None, None, 0),
    };
    let n = units.len() as f64;
    let seconds = per_unit.map(|s| ((n / slots).ceil() * s).round());

    let ready = blockers.is_empty();
    Ok(json!({
        "pipeline": p.label(),
        "pipeline_id": p.id,
        "handle": asked.handle,
        "selection": asked.selection,
        "params": params,
        "layout": d.layout.name(),
        "level": d.level.name(),
        "stacks": stacks.len(),
        "units": {
            "total": units.len(),
            "ready": units.len() - missing.len(),
            "missing": missing.len(),
        },
        "missing": missing.iter().map(|u| json!({
            "unit": u.name(), "subject_id": u.subject_id, "session_day": u.session_day,
            "stack_id": u.stack_id, "why": u.missing,
        })).collect::<Vec<_>>(),
        "roles": d.roles,
        "left_out": {
            "stacks": left.len(),
            "why": (!left.is_empty()).then_some(
                "no live pick takes these stacks, so the release of the input leaves them out"
            ),
        },
        "estimate": {
            "seconds_per_unit": per_unit.map(f64::round),
            "source": source,
            "runs": runs,
            "slots": slots,
            "units_apart": apart,
            "seconds": seconds,
        },
        "gpu": {"need": d.gpu.name(), "available": card, "device": device},
        "needs": {"cores": cores, "memory_gb": memory},
        "budget": budget,
        "checks": d.checks.iter().map(|c| c.text()).collect::<Vec<_>>(),
        "runtime": capability["runtime"],
        "place": capability["place"],
        "ready": ready,
        "blockers": blockers,
    }))
}

/// A pre-flight read at detail plain: the counts and the reasons, no unit
/// named by its subject or its session's day.
pub(crate) fn plain(v: &mut Value) {
    if let Some(list) = v["missing"].as_array_mut() {
        for m in list.iter_mut() {
            *m = json!({"why": m["why"].clone()});
        }
    }
}

/// `POST /api/pipelines/{name}/preflight`: `{select: "selection:<name>@<v>"}`
/// or `{handle: <id>}`, and `params` as an object or a list of `id=value`.
pub(crate) fn route(
    home: &Home,
    pack_dir: Option<&Path>,
    registry: &mut Registry,
    quasi: bool,
    post: bool,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let ["api", "pipelines", name, "preflight"] = segs else {
        return None;
    };
    if !post {
        return None;
    }
    Some((|| {
        let doc = crate::serve::json_body(body)?;
        let params: Vec<(String, String)> = match &doc["params"] {
            Value::Null => Vec::new(),
            Value::Object(o) => o
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                    )
                })
                .collect(),
            Value::Array(a) => a
                .iter()
                .map(|w| {
                    w.as_str()
                        .and_then(|w| w.split_once('='))
                        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
                        .ok_or_else(|| Reply::error(400, "params are id=value words"))
                })
                .collect::<Result<_, _>>()?,
            _ => {
                return Err(Reply::error(
                    400,
                    "params is an object or a list of id=value",
                ));
            }
        };
        let selection = doc["select"].as_str().map(str::to_string);
        let handle = match (&selection, doc["handle"].as_i64()) {
            (Some(spec), _) => {
                let pack = doc["pack"].as_str().unwrap_or("mri");
                crate::ask_cli::freeze_selection(
                    home,
                    spec,
                    nils_ask::ast::Grain::Stack,
                    pack_dir.map(PathBuf::from),
                    pack,
                )
                .map_err(|e| Reply::error(400, e.message))?
            }
            (None, Some(h)) => h,
            (None, None) => {
                return Err(Reply::error(
                    400,
                    "a pre-flight is over a selection: {select: \"selection:<name>@<v>\"} or {handle: <id>}",
                ));
            }
        };
        let mut v = check(
            registry,
            &Asked {
                pipeline: name,
                handle,
                selection: selection.as_deref(),
                params: &params,
            },
        )
        .map_err(|e| {
            if e.starts_with("no pipeline") {
                Reply::error(404, e)
            } else {
                Reply::error(400, e)
            }
        })?;
        if !quasi {
            plain(&mut v);
        }
        Ok(Reply::ok(v))
    })())
}

/// `nils run --preflight`: the pre-flight printed, nothing run.
pub(crate) fn command(home: &Home, args: &crate::pipelines::RunArgs) -> Result<(), Exit> {
    if args.resume.is_some() {
        return Err(usage(
            "a pre-flight is of a new run, not of one taken up again: leave out --resume",
        ));
    }
    let pipeline = args
        .pipeline
        .as_deref()
        .ok_or_else(|| usage("run <pipeline> --preflight: the pipeline to check"))?;
    let handle = match (&args.select, args.handle) {
        (Some(spec), _) => crate::ask_cli::freeze_selection(
            home,
            spec,
            nils_ask::ast::Grain::Stack,
            args.pack_dir.clone(),
            &args.pack,
        )?,
        (None, Some(h)) => h,
        (None, None) => {
            return Err(usage(
                "a pre-flight is over a frozen selection: --select selection:<name>@<v>, or --handle <id>",
            ));
        }
    };
    let params: Vec<(String, String)> = args
        .params
        .iter()
        .map(|w| {
            w.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.to_string()))
                .ok_or_else(|| usage(format!("--param {w}: write it as id=value")))
        })
        .collect::<Result<_, _>>()?;
    let mut registry = crate::open(home)?;
    let v = check(
        &mut registry,
        &Asked {
            pipeline,
            handle,
            selection: args.select.as_deref(),
            params: &params,
        },
    )
    .map_err(usage)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
        );
        return Ok(());
    }
    println!(
        "pre-flight of {} over {} ({} stacks, handle {})",
        v["pipeline"].as_str().unwrap_or_default(),
        v["selection"].as_str().unwrap_or("a handle"),
        v["stacks"],
        v["handle"]
    );
    println!(
        "  units            {} ({} ready, {} missing an input)",
        v["units"]["total"], v["units"]["ready"], v["units"]["missing"]
    );
    for m in v["missing"].as_array().into_iter().flatten().take(20) {
        let why: Vec<&str> = m["why"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        println!(
            "    {}: {}",
            m["unit"].as_str().unwrap_or_default(),
            why.join("; ")
        );
    }
    if v["left_out"]["stacks"].as_u64().unwrap_or(0) > 0 {
        println!(
            "  left out         {} stacks: {}",
            v["left_out"]["stacks"],
            v["left_out"]["why"].as_str().unwrap_or_default()
        );
    }
    let e = &v["estimate"];
    match e["seconds"].as_f64() {
        Some(s) => println!(
            "  time             about {} min ({} s a unit from {}, {} at once)",
            (s / 60.0).ceil(),
            e["seconds_per_unit"],
            e["source"].as_str().unwrap_or_default(),
            e["slots"]
        ),
        None => println!(
            "  time             unknown: it has not run here and its descriptor names no unit-minutes"
        ),
    }
    println!(
        "  GPU              needs {}, this engine has {}, it would run on {}",
        v["gpu"]["need"].as_str().unwrap_or_default(),
        v["gpu"]["available"].as_str().unwrap_or("none"),
        v["gpu"]["device"].as_str().unwrap_or("nothing")
    );
    println!(
        "  a unit needs     {} cores, {} GB; the lane has {} cores, {} GB ({})",
        v["needs"]["cores"],
        v["needs"]["memory_gb"],
        v["budget"]["cores"],
        v["budget"]["memory_gb"],
        v["budget"]["source"].as_str().unwrap_or_default()
    );
    if v["ready"] == true {
        println!(
            "ready: nils run {} starts it",
            v["pipeline"].as_str().unwrap_or_default()
        );
    } else {
        for b in v["blockers"].as_array().into_iter().flatten() {
            println!("not ready: {}", b.as_str().unwrap_or_default());
        }
    }
    Ok(())
}
