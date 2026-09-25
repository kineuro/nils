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
//! the registry has not moved), and the session cache the release reads,
//! which the release would build the same way.
//!
//! The units are counted the way the runner makes them: in the stacks
//! layout one a stack; in the bids layout one a session (or a participant)
//! of the release's own tree, since the runner's input is a release with
//! the picks applied (record 43 R4) and its units are that tree's folders.
//! The release's plan says where each picked stack goes, so a stack it
//! keeps in `sourcedata/` or holds is left out as the run leaves it out. A
//! session is missing an input when no stack picked for a role the
//! descriptor names under `x-nils.input.roles` is released under that
//! role's own BIDS suffix (a `t1w` pick the release names `FLAIR` is no
//! T1w), and the run skips it; a stack when the registry holds no file of
//! it, or no derivative a typed input needs.

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

/// How many of a pipeline's past runs the estimate reads.
const PAST_RUNS: usize = 20;

/// What the pre-flight is asked.
pub(crate) struct Asked<'a> {
    pub pipeline: &'a str,
    /// The pack the run's release names its stacks by; without it the bids
    /// units are counted from the picks alone.
    pub pack: Option<&'a nils_pack::Pack>,
    pub handle: i64,
    pub selection: Option<&'a str>,
    pub params: &'a [(String, String)],
}

/// The lane's budget (record 49 A1, R6), as the lane itself reads it: the
/// setting of `nils pipeline lane`, else the ruling's 48 cores and 512 GB,
/// each no more than this machine or the container the engine runs in
/// offers; and the card a GPU unit would lease.
pub(crate) fn budget(registry: &mut Registry) -> (crate::pipelines::Lane, Value) {
    let lane = crate::pipelines::lane_of(registry);
    let mut doc = lane.doc();
    doc["memory_gb"] = json!(lane.ledger.memory_mib as f64 / 1024.0);
    (lane, doc)
}

/// One unit the run would have.
#[derive(Debug, Clone, Default)]
struct Unit {
    /// The run's own name for it, `sub-<s>_ses-<t>` or `sub-<s>`, where the
    /// release's plan gave it one.
    id: Option<String>,
    subject_id: Option<i64>,
    session_day: Option<String>,
    stack_id: Option<i64>,
    stacks: Vec<i64>,
    roles: BTreeSet<String>,
    missing: Vec<String>,
}

impl Unit {
    fn name(&self) -> String {
        if let Some(id) = &self.id {
            return id.clone();
        }
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
    place_ids: &[i64],
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
        // where the runner looks: the lane's output place, then its
        // scratch place (record 49 R7)
        let mut have: BTreeSet<i64> = BTreeSet::new();
        for place in place_ids {
            have.extend(
                nils_registry::derivative::of_stacks(store, kind, stacks, *place)
                    .map_err(err)?
                    .into_iter()
                    .filter_map(|r| r.stack_id),
            );
        }
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

/// What the live picks say of one stack: the roles it was picked for, and
/// the subject and the session's day the pick names.
pub(crate) struct Picked {
    pub(crate) roles: BTreeSet<String>,
    subject: i64,
    day: String,
}

/// The stacks the selection leaves out of a run's input, each with why.
type LeftOut = Vec<(i64, String)>;

/// The live picks among `stacks`, by stack.
pub(crate) fn pick_roles(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeMap<i64, Picked>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let mut out: BTreeMap<i64, Picked> = BTreeMap::new();
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
            out.entry(stack)
                .or_insert_with(|| Picked {
                    roles: BTreeSet::new(),
                    subject,
                    day,
                })
                .roles
                .insert(role);
        }
    }
    Ok(out)
}

/// A unit's id as the runner names it from where the release put a stack:
/// `sub-<s>_ses-<t>` at the session level, `sub-<s>` at the subject level;
/// none for a stack outside a subject's folder.
pub(crate) fn unit_of_dir(dir: &str, level: Level) -> Option<(String, String, Option<String>)> {
    let mut parts = dir.split('/');
    let subject = parts.next()?.strip_prefix("sub-")?;
    let session = parts.next().and_then(|p| p.strip_prefix("ses-"));
    Some(match (level, session) {
        (Level::Session, Some(s)) => (
            format!("sub-{subject}_ses-{s}"),
            subject.to_string(),
            Some(s.to_string()),
        ),
        _ => (format!("sub-{subject}"), subject.to_string(), None),
    })
}

/// What a unit lacks of the roles a descriptor names (record 49 A3), from
/// what the release makes of the stacks picked for each role in it: the
/// BIDS suffix of each in the raw tree, none for one the release puts
/// elsewhere. A role is there when a stack picked for it is released under
/// the role's own suffix (`t1w` as `T1w`); a role the standard spells no
/// suffix for is there when a stack picked for it is in the raw tree. The
/// pre-flight and the runner both judge a unit by this, so a session whose
/// T1w pick the release writes as a FLAIR is missing its T1w before the
/// run, and is not run.
pub(crate) fn roles_missing(
    roles: &[String],
    held: &BTreeMap<String, Vec<Option<String>>>,
) -> Vec<String> {
    let mut out = Vec::new();
    for role in roles {
        let got = held.get(role).map(Vec::as_slice).unwrap_or(&[]);
        if got.is_empty() {
            out.push(format!(
                "no {role} is picked for it among the selection's stacks"
            ));
            continue;
        }
        let raw: Vec<&str> = got.iter().filter_map(|s| s.as_deref()).collect();
        let want = nils_release::run::role_suffix(role);
        let ok = match want {
            Some(w) => raw.contains(&w),
            None => !raw.is_empty(),
        };
        if ok {
            continue;
        }
        let mut as_: Vec<&str> = raw.clone();
        as_.sort_unstable();
        as_.dedup();
        out.push(match (want, as_.is_empty()) {
            (_, true) => format!(
                "its {role} pick is released outside the session's BIDS folders, so its input holds none"
            ),
            (Some(w), false) => format!(
                "its {role} pick is released as {}, not {w}, so its input holds no {w}",
                as_.join(" and ")
            ),
            (None, false) => unreachable!("a role with no suffix is there when anything is raw"),
        });
    }
    out
}

/// The bids layout: the units the release of the picks makes, one a session
/// (or a participant) that holds a live pick of a stack of the selection,
/// missing where it has no pick of a role the descriptor names. Answers the
/// units and the stacks no pick takes, which the input leaves out, with why.
///
/// With the pack, the units are the release's own (record 49 A3, found on
/// the group's install): the plan of `nils release --layout bids --picked`
/// says where each picked stack goes, so a session the release merges, a
/// stack it routes to `sourcedata/` or holds, and a pick it names by
/// another suffix are counted as the run will meet them.
fn bids_units(
    registry: &mut Registry,
    d: &Descriptor,
    stacks: &[i64],
    pack: Option<&nils_pack::Pack>,
) -> Result<(Vec<Unit>, LeftOut), String> {
    let picks = pick_roles(registry.store(), stacks)?;
    let unpicked = "no live pick takes it, so the release of the input leaves it out".to_string();
    let mut left: LeftOut = stacks
        .iter()
        .filter(|s| !picks.contains_key(s))
        .map(|s| (*s, unpicked.clone()))
        .collect();
    let mut held: BTreeMap<String, BTreeMap<String, Vec<Option<String>>>> = BTreeMap::new();
    let mut units: BTreeMap<String, Unit> = BTreeMap::new();
    match pack {
        Some(pack) => {
            let picked: Vec<i64> = picks.keys().copied().collect();
            let plan = nils_release::run::plan_picked(
                registry,
                pack,
                &nils_registry::session::Scheme::default(),
                &picked,
            )
            .map_err(|e| format!("the plan of the input's release: {e}"))?;
            let planned: BTreeSet<i64> = plan.iter().map(|p| p.stack).collect();
            for s in picked.iter().filter(|s| !planned.contains(s)) {
                left.push((
                    *s,
                    "the release leaves it out: the registry holds no file of it to write, or its disposition is excluded".into(),
                ));
            }
            for p in &plan {
                let Some((id, _, _)) = p.dir.as_deref().and_then(|dir| unit_of_dir(dir, d.level))
                else {
                    left.push((
                        p.stack,
                        p.why
                            .clone()
                            .unwrap_or_else(|| format!("the release puts it in {}", p.route)),
                    ));
                    continue;
                };
                let Picked { roles, day, .. } = &picks[&p.stack];
                let u = units.entry(id.clone()).or_insert(Unit {
                    id: Some(id.clone()),
                    subject_id: Some(p.subject_id),
                    session_day: (d.level == Level::Session).then(|| day.clone()),
                    ..Unit::default()
                });
                u.stacks.push(p.stack);
                let suffix = (p.route == "raw").then(|| p.suffix.clone()).flatten();
                for role in roles {
                    u.roles.insert(role.clone());
                    held.entry(id.clone())
                        .or_default()
                        .entry(role.clone())
                        .or_default()
                        .push(suffix.clone());
                }
            }
        }
        // no pack to plan with: the picks alone, as the pick names them
        None => {
            for (
                stack,
                Picked {
                    roles,
                    subject,
                    day,
                },
            ) in &picks
            {
                let day = (d.level == Level::Session).then(|| day.clone());
                let key = format!("{subject}/{}", day.clone().unwrap_or_default());
                let u = units.entry(key.clone()).or_insert(Unit {
                    subject_id: Some(*subject),
                    session_day: day,
                    ..Unit::default()
                });
                u.stacks.push(*stack);
                for role in roles {
                    u.roles.insert(role.clone());
                    held.entry(key.clone())
                        .or_default()
                        .entry(role.clone())
                        .or_default()
                        .push(nils_release::run::role_suffix(role).map(str::to_string));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (key, mut u) in units {
        u.missing = roles_missing(&d.roles, held.get(&key).unwrap_or(&BTreeMap::new()));
        out.push(u);
    }
    left.sort();
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
    let places = crate::pipelines::run_places(registry.store()).ok();
    let mut place_ids: Vec<i64> = places.iter().map(|p| p.output.id).collect();
    if let Some(p) = &places
        && p.scratch.id != p.output.id
    {
        place_ids.push(p.scratch.id);
    }
    let (units, left) = match d.layout {
        Layout::Stacks => (
            stack_units(registry.store(), &d, &stacks, &place_ids)?,
            LeftOut::new(),
        ),
        Layout::Bids => bids_units(registry, &d, &stacks, asked.pack)?,
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
    // the GPU as a run would take it: the runtime's, and only where the
    // lane names a card to lease
    let (lane, mut budget) = budget(registry);
    let card = detected.runtime.as_ref().and_then(|r| r.gpu.clone());
    let offered = card.clone().filter(|_| lane.card.is_some());
    let device = match (d.gpu, &offered) {
        (Gpu::None, _) | (Gpu::Optional, None) => Some("cpu".to_string()),
        (_, Some(g)) => Some(g.clone()),
        (Gpu::Required, None) => {
            blockers.push(if card.is_some() {
                format!(
                    "{} needs a GPU, and the pipeline lane uses no card here; nils pipeline lane --gpu-card <n> names the one it leases",
                    p.label()
                )
            } else {
                format!(
                    "{} needs a GPU, and this engine's runtime offers none",
                    p.label()
                )
            });
            None
        }
    };

    // what a unit needs, against the lane
    let ask = nils_pipeline::lane::Ask {
        cores: d.needs.cores,
        memory_mib: nils_pipeline::lane::mib_of_gb(d.needs.memory_gb),
    };
    let (cores, memory) = (f64::from(d.needs.cores), d.needs.memory_gb);
    let fits = lane.ledger.could(ask);
    budget["fits"] = json!(fits);
    if !fits {
        blockers.push(format!(
            "a unit needs {} cores and {} GB, and the lane has {} cores and {} GB{}",
            d.needs.cores,
            descriptor::number_text(memory),
            lane.ledger.cores,
            descriptor::number_text(lane.ledger.memory_mib as f64 / 1024.0),
            lane.why
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default()
        ));
    }
    let apart = d.units == descriptor::Units::Apart;
    let slots = if apart && fits {
        f64::from(lane.ledger.cores / ask.cores.max(1))
            .min((lane.ledger.memory_mib / ask.memory_mib.max(1)) as f64)
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
            "why": left_why(&left),
        },
        "estimate": {
            "seconds_per_unit": per_unit.map(f64::round),
            "source": source,
            "runs": runs,
            "slots": slots,
            "units_apart": apart,
            "seconds": seconds,
        },
        "gpu": {
            "need": d.gpu.name(), "available": card, "device": device, "card": lane.card,
            "gpu_memory_gb": (device.as_deref().is_some_and(|d| d != "cpu")).then_some(d.needs.gpu_memory_gb),
        },
        "needs": {"cores": cores, "memory_gb": memory},
        "budget": budget,
        "checks": d.checks.iter().map(|c| c.text()).collect::<Vec<_>>(),
        "runtime": capability["runtime"],
        "place": capability["place"],
        "scratch": capability["scratch"],
        "ready": ready,
        "blockers": blockers,
    }))
}

/// Why the stacks left out are, each reason once with its count.
fn left_why(left: &[(i64, String)]) -> Value {
    if left.is_empty() {
        return Value::Null;
    }
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, why) in left {
        *by.entry(why.as_str()).or_insert(0) += 1;
    }
    json!(
        by.iter()
            .map(|(w, n)| format!("{n}: {w}"))
            .collect::<Vec<_>>()
            .join("; ")
    )
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
        let pack = pack_dir
            .map(Path::to_path_buf)
            .or_else(|| crate::pack_dir(home, None).ok())
            .and_then(|dir| {
                nils_pack::load(&dir.join(doc["pack"].as_str().unwrap_or("mri")), None).ok()
            });
        let mut v = check(
            registry,
            &Asked {
                pipeline: name,
                pack: pack.as_ref(),
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
    let pack = crate::pack_dir(home, args.pack_dir.clone())
        .ok()
        .and_then(|dir| crate::ask_cli::load_pack(&dir, &args.pack).ok());
    let mut registry = crate::open(home)?;
    let v = check(
        &mut registry,
        &Asked {
            pipeline,
            pack: pack.as_ref(),
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
        "  a unit needs     {} cores, {} GB; the lane has {} cores, {} GB{}",
        v["needs"]["cores"],
        v["needs"]["memory_gb"],
        v["budget"]["cores"],
        v["budget"]["memory_gb"],
        v["budget"]["why"]
            .as_str()
            .map(|w| format!(" ({w})"))
            .unwrap_or_default()
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
