// SPDX-License-Identifier: AGPL-3.0-only

//! The pipeline pack in the engine (record 57 P1 to P4; Wave 7b A1): the
//! registry's side of `nils_pipeline::pack`. The catalog keeps a version as
//! the pair of descriptor and pack; this module loads a pack against the
//! words the registry holds, reads a unit's facts from the registry (the
//! fingerprint rows, the classification axes, the post-contrast label, the
//! clinical imports, the host), judges every unit of a run at pre-flight
//! and at the run's start with `nils_pipeline::pack::rules`, computes the
//! two keys of each unit's results, holds the pack's checks (fixed and
//! cohort) at registration, and answers per comparability key wherever a
//! measure is read.

use std::collections::{BTreeMap, BTreeSet};

use nils_pipeline::descriptor::Descriptor;
use nils_pipeline::pack::facts::{Facts, Val, Vocabulary};
use nils_pipeline::pack::rules::{Judgement, Person};
use nils_pipeline::pack::{self, PipelinePack};
use nils_registry::Registry;
use nils_registry::pipeline::{self as rows, Pipeline};
use nils_registry::store::{Param, Store};
use serde_json::{Map, Value, json};

/// The pack of a catalog row, where it has one.
pub(crate) fn pack_of(p: &Pipeline) -> Option<PipelinePack> {
    if p.pack.is_null() {
        return None;
    }
    pack::from_value(p.pack.clone()).ok()
}

/// The vocabulary a pack is checked against here: the crate's, widened by
/// the words the registry's classification holds for `base` and
/// `body_part`, so a newer MRI pack's word is not refused.
pub(crate) fn vocabulary(store: &mut Store, d: &Descriptor, pack: &PipelinePack) -> Vocabulary {
    let mut v = pack.vocabulary(d);
    for axis in ["base", "body_part"] {
        let sql = format!(
            "SELECT DISTINCT value FROM {} WHERE axis = {} AND value IS NOT NULL",
            store.qualified("classification_axis"),
            store.dialect().param(1, nils_registry::schema::Type::Text)
        );
        let words: Vec<String> = store
            .query(&sql, &[Param::from(axis)])
            .unwrap_or_default()
            .iter()
            .filter_map(|r| r.text(0).ok().map(str::to_string))
            .collect();
        if !words.is_empty() {
            v = v.widened(axis, &words);
        }
    }
    v
}

/// Load a pack's text against its descriptor, under the registry's words.
pub(crate) fn load(store: &mut Store, d: &Descriptor, text: &str) -> Result<PipelinePack, String> {
    let parsed = pack::parse(text)?;
    let vocab = vocabulary(store, d, &parsed);
    parsed.check(d, &vocab)?;
    Ok(parsed)
}

/// What the host offers, as facts (`host.*`).
#[derive(Debug, Clone, Default)]
pub(crate) struct HostFacts {
    pub cores: Option<u32>,
    pub memory_gb: Option<f64>,
    pub gpu: Option<bool>,
    pub gpu_memory_gb: Option<f64>,
    pub runtime: Option<String>,
}

/// The host's facts as this engine reads them: the machine's cores and
/// memory, whether the runtime offers a card the lane leases, and the
/// runtime's name.
pub(crate) fn host_facts(registry: &mut Registry) -> HostFacts {
    let (cores, mib) = nils_pipeline::lane::host();
    let detected = crate::pipelines::detect_cached(registry);
    let (lane, _) = crate::preflight::budget(registry);
    let card = detected.runtime.as_ref().and_then(|r| r.gpu.clone());
    HostFacts {
        cores: Some(cores),
        memory_gb: Some((mib as f64 / 102.4).round() / 10.0),
        gpu: Some(card.is_some() && lane.card.is_some()),
        gpu_memory_gb: None,
        runtime: detected.runtime.as_ref().map(|r| r.kind.name().to_string()),
    }
}

impl HostFacts {
    fn into_facts(self, f: &mut Facts) {
        f.set("host.cores", Val::number(self.cores.map(f64::from)));
        f.set("host.memory_gb", Val::number(self.memory_gb));
        f.set("host.gpu", self.gpu.map_or(Val::Unknown, Val::Bool));
        f.set("host.gpu_memory_gb", Val::number(self.gpu_memory_gb));
        f.set("host.runtime", Val::text(self.runtime.as_deref()));
    }
}

/// One unit as the facts provider sees it: its stacks by role, and whose
/// session it is.
#[derive(Debug, Clone, Default)]
pub(crate) struct UnitOf {
    pub by_role: BTreeMap<String, Vec<i64>>,
    pub subject_id: Option<i64>,
    pub session_day: Option<String>,
}

/// A stack's facts as the registry holds them (spec section 5.4), read for
/// many stacks at once.
#[derive(Debug, Clone, Default)]
struct StackRow {
    dimension: Option<String>,
    slice_thickness: Option<f64>,
    spacing_between: Option<f64>,
    pixel_row: Option<f64>,
    pixel_col: Option<f64>,
    field: Option<f64>,
    manufacturer: Option<String>,
    base: Option<String>,
    body_part: Option<String>,
    post_contrast: Option<String>,
}

fn stack_rows(store: &mut Store, stacks: &[i64]) -> BTreeMap<i64, StackRow> {
    let mut out: BTreeMap<i64, StackRow> = BTreeMap::new();
    if stacks.is_empty() {
        return out;
    }
    let ids: Vec<String> = stacks.iter().map(i64::to_string).collect();
    let ids = ids.join(", ");
    let sql = format!(
        "SELECT stack_id, mr_acquisition_type, slice_thickness, spacing_between_slices, \
         pixel_spacing_row, pixel_spacing_col, magnetic_field_strength, manufacturer \
         FROM {} WHERE stack_id IN ({ids})",
        store.qualified("stack_fingerprint")
    );
    for r in store.query(&sql, &[]).unwrap_or_default() {
        let Ok(id) = r.int(0) else { continue };
        out.insert(
            id,
            StackRow {
                dimension: r.opt_text(1).ok().flatten().map(str::to_string),
                slice_thickness: r.opt_double(2).ok().flatten(),
                spacing_between: r.opt_double(3).ok().flatten(),
                pixel_row: r.opt_double(4).ok().flatten(),
                pixel_col: r.opt_double(5).ok().flatten(),
                field: r.opt_double(6).ok().flatten(),
                manufacturer: r.opt_text(7).ok().flatten().map(str::to_string),
                ..StackRow::default()
            },
        );
    }
    let sql = format!(
        "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({ids}) \
         AND axis IN ('base', 'body_part', 'post_contrast') AND value IS NOT NULL",
        store.qualified("classification_axis")
    );
    for r in store.query(&sql, &[]).unwrap_or_default() {
        let (Ok(id), Ok(axis), Ok(value)) = (r.int(0), r.text(1), r.text(2)) else {
            continue;
        };
        let row = out.entry(id).or_default();
        match axis {
            "base" => row.base = Some(value.to_string()),
            "body_part" => row.body_part = Some(value.to_string()),
            "post_contrast" => row.post_contrast = Some(value.to_string()),
            _ => {}
        }
    }
    out
}

fn dimension_of(text: Option<&str>) -> Val {
    match text.map(str::trim) {
        Some("2D") => Val::Text("2D".into()),
        Some("3D") => Val::Text("3D".into()),
        _ => Val::Unknown,
    }
}

/// A stack's facts laid into `f` under `role`.
fn stack_facts(f: &mut Facts, role: &str, row: Option<&StackRow>) {
    let r = row.cloned().unwrap_or_default();
    let set = |f: &mut Facts, name: &str, v: Val| f.set(&format!("{role}.{name}"), v);
    set(f, "base", Val::text(r.base.as_deref()));
    set(f, "dimension", dimension_of(r.dimension.as_deref()));
    let through = r.spacing_between.or(r.slice_thickness);
    let voxels: Vec<f64> = [r.pixel_row, r.pixel_col, through]
        .into_iter()
        .flatten()
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    let (vmax, vmin) = if voxels.len() == 3 {
        (
            voxels.iter().cloned().fold(f64::MIN, f64::max),
            voxels.iter().cloned().fold(f64::MAX, f64::min),
        )
    } else {
        (f64::NAN, f64::NAN)
    };
    set(f, "voxel_max_mm", Val::number(Some(vmax)));
    set(f, "voxel_min_mm", Val::number(Some(vmin)));
    set(f, "slice_thickness_mm", Val::number(r.slice_thickness));
    set(f, "field_strength_t", Val::number(r.field));
    set(f, "post_contrast", Val::text(r.post_contrast.as_deref()));
    set(
        f,
        "vendor",
        r.manufacturer
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map_or(Val::Unknown, |m| {
                Val::Text(nils_pipeline::pack::facts::vendor_of(m).to_string())
            }),
    );
    set(f, "body_part", Val::text(r.body_part.as_deref()));
    // the registry records neither (spec section 5.4): unknown until a
    // derivative or a step says
    set(f, "skull_stripped", Val::Unknown);
    set(f, "defaced", Val::Unknown);
}

/// The Dice the alignment check of a session wrote, where a run of any
/// pipeline measured `flair_t1_brain_dice` for it: aligned at 0.85 or more
/// (the check's own threshold), else not.
fn aligned_from_check(store: &mut Store, subject_id: i64, day: &str) -> Val {
    let d = store.dialect();
    let sql = format!(
        "SELECT number FROM {} WHERE name = 'flair_t1_brain_dice' AND scope = 'session' \
         AND subject_id = {} AND session_day = {} AND number IS NOT NULL ORDER BY id DESC LIMIT 1",
        store.qualified("measure"),
        d.param(1, nils_registry::schema::Type::Int),
        d.param(2, nils_registry::schema::Type::Date),
    );
    match store.query_opt(&sql, &[Param::Int(subject_id), Param::from(day)]) {
        Ok(Some(r)) => r
            .opt_double(0)
            .ok()
            .flatten()
            .map_or(Val::Unknown, |dice| Val::Bool(dice >= 0.85)),
        _ => Val::Unknown,
    }
}

/// A subject's diagnosis from the clinical imports: the disease's name of
/// its standing disease row, where one is.
fn diagnosis_of(store: &mut Store, subject_id: i64) -> Val {
    let d = store.dialect();
    let sql = format!(
        "SELECT di.name FROM {} sd JOIN {} di ON di.id = sd.disease_id \
         WHERE sd.subject_id = {} AND sd.superseded_by IS NULL ORDER BY sd.id DESC LIMIT 1",
        store.qualified("subject_disease"),
        store.qualified("disease"),
        d.param(1, nils_registry::schema::Type::Int),
    );
    match store.query_opt(&sql, &[Param::Int(subject_id)]) {
        Ok(Some(r)) => Val::text(r.opt_text(0).ok().flatten()),
        _ => Val::Unknown,
    }
}

/// The facts of many units, read from the registry a few queries at a time:
/// each unit's stacks by role, its session's alignment where a check said,
/// its subject's diagnosis, and the host's.
pub(crate) fn facts_for(store: &mut Store, units: &[UnitOf], host: &HostFacts) -> Vec<Facts> {
    let all: Vec<i64> = units
        .iter()
        .flat_map(|u| u.by_role.values().flatten().copied())
        .collect::<BTreeSet<i64>>()
        .into_iter()
        .collect();
    let mut rows: BTreeMap<i64, StackRow> = BTreeMap::new();
    for chunk in all.chunks(500) {
        rows.extend(stack_rows(store, chunk));
    }
    let mut diagnosis: BTreeMap<i64, Val> = BTreeMap::new();
    units
        .iter()
        .map(|u| {
            let mut f = Facts::default();
            for (role, stacks) in &u.by_role {
                if stacks.is_empty() {
                    continue;
                }
                f.roles.insert(role.clone());
                // one stack a role; where a role has several, the first
                stack_facts(&mut f, role, rows.get(&stacks[0]));
            }
            if let (Some(s), Some(day)) = (u.subject_id, u.session_day.as_deref())
                && f.roles.contains("flair")
                && f.roles.contains("t1w")
            {
                f.set_aligned("flair", "t1w", aligned_from_check(store, s, day));
            }
            if let Some(s) = u.subject_id {
                let v = diagnosis
                    .entry(s)
                    .or_insert_with(|| diagnosis_of(store, s))
                    .clone();
                f.set("diagnosis", v);
            }
            // the registry holds no age it may tell here (record 55 K7:
            // only where detail allows); unknown until a clinical import
            // carries one the facts may read
            f.set("age_years", Val::Unknown);
            host.clone().into_facts(&mut f);
            f
        })
        .collect()
}

/// What a person asked of a run with a pack: the goal, the switches, and
/// whether a unit whose hard need the registry cannot tell runs anyway.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PersonAsk {
    pub goal: Option<String>,
    pub switches: BTreeMap<String, Value>,
    pub let_run: bool,
}

impl PersonAsk {
    /// From a door's body: `goal`, `switches` {name: value}, `let_run`.
    pub(crate) fn of_body(doc: &Value) -> Result<PersonAsk, String> {
        let goal = match &doc["goal"] {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            _ => return Err("goal is text: fast, standard or research".into()),
        };
        let switches = match &doc["switches"] {
            Value::Null => BTreeMap::new(),
            Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => return Err("switches is a mapping of the pack's switches to their values".into()),
        };
        let let_run = match &doc["let_run"] {
            Value::Null => false,
            Value::Bool(b) => *b,
            _ => return Err("let_run is true or false".into()),
        };
        Ok(PersonAsk {
            goal,
            switches,
            let_run,
        })
    }

    /// From the command line: `--goal`, `--switch name=value`, `--let-run`.
    pub(crate) fn of_args(
        goal: Option<&str>,
        switches: &[String],
        let_run: bool,
    ) -> Result<PersonAsk, String> {
        let mut out = PersonAsk {
            goal: goal.map(str::to_string),
            switches: BTreeMap::new(),
            let_run,
        };
        for w in switches {
            let (k, v) = w
                .split_once('=')
                .ok_or_else(|| format!("--switch takes name=value, not {w}"))?;
            let v = match v.trim() {
                "true" => json!(true),
                "false" => json!(false),
                other => other
                    .parse::<f64>()
                    .map(|n| json!(n))
                    .unwrap_or_else(|_| json!(other)),
            };
            out.switches.insert(k.trim().to_string(), v);
        }
        Ok(out)
    }

    /// As a run's args record it.
    pub(crate) fn doc(&self) -> Value {
        json!({"goal": self.goal, "switches": self.switches, "let_run": self.let_run})
    }

    /// The person at the run, checked against the pack: a goal or switch
    /// the pack does not offer is refused, and so is a knob a person may
    /// not set.
    pub(crate) fn person(
        &self,
        pack: &PipelinePack,
        user: &str,
        given: &[(String, String)],
    ) -> Result<Person, String> {
        let mut choices: BTreeMap<String, Value> = BTreeMap::new();
        if let Some(g) = &self.goal {
            choices.insert("goal".into(), pack.choice("goal", &json!(g))?);
        }
        for (k, v) in &self.switches {
            choices.insert(k.clone(), pack.choice(k, v)?);
        }
        let given: BTreeSet<String> = given.iter().map(|(k, _)| k.clone()).collect();
        nils_pipeline::pack::rules::person_may_set(pack, &given)?;
        Ok(Person {
            user: user.to_string(),
            choices,
            given,
            proposed: BTreeSet::new(),
            let_run: self.let_run,
        })
    }
}

/// Every unit judged by the pack: one judgement a unit, in the units'
/// order.
pub(crate) fn judge_units(
    store: &mut Store,
    pack: &PipelinePack,
    d: &Descriptor,
    resolved: &Map<String, Value>,
    units: &[UnitOf],
    host: &HostFacts,
    person: &Person,
) -> Vec<Judgement> {
    facts_for(store, units, host)
        .iter()
        .map(|f| nils_pipeline::pack::rules::judge(pack, d, resolved, f, person))
        .collect()
}

/// The pre-flight's lines for judged units: per unit `{unit, status, need,
/// words, variant, caveats, knobs {value, by, rule}}`, and the counts.
pub(crate) fn judged_doc(names: &[String], judged: &[Judgement]) -> Value {
    use nils_pipeline::pack::rules::Status;
    let per_unit: Vec<Value> = names
        .iter()
        .zip(judged)
        .map(|(name, j)| {
            json!({
                "unit": name,
                "status": j.status.name(),
                "need": j.need,
                "words": j.words,
                "cannot_tell": j.cannot_tell,
                "let_run_by": j.let_run_by,
                "variant": j.variant.as_ref().map(|v| json!({"variant": v.variant, "pipeline": v.pipeline, "rule": v.rule, "why": v.why})),
                "caveats": j.caveats.iter().map(|c| c.words.clone()).collect::<Vec<_>>(),
                "knobs": j.knobs_brief(),
            })
        })
        .collect();
    let count = |s: Status| judged.iter().filter(|j| j.status == s).count();
    let mut by_words: BTreeMap<String, usize> = BTreeMap::new();
    for j in judged.iter().filter(|j| j.status == Status::Skipped) {
        *by_words
            .entry(j.words.clone().unwrap_or_default())
            .or_default() += 1;
    }
    let mut skipped_by_why: Vec<Value> = by_words
        .into_iter()
        .map(|(why, units)| json!({"why": why, "units": units}))
        .collect();
    skipped_by_why.sort_by(|a, b| b["units"].as_u64().cmp(&a["units"].as_u64()));
    json!({
        "units": per_unit,
        "ready": count(Status::Ready),
        "skipped": count(Status::Skipped),
        "routed": count(Status::Routed),
        "cannot_tell": judged.iter().filter(|j| j.cannot_tell.is_some()).count(),
        "with_caveats": judged.iter().filter(|j| !j.caveats.is_empty()).count(),
        "skipped_by_why": skipped_by_why,
    })
}

/// Where units run together in one container, every unit's knobs are the
/// run's: a knob the rules set differently across units cannot be honoured,
/// and the run is refused with the knob named.
pub(crate) fn together_disagree(judged: &[Judgement]) -> Option<String> {
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for j in judged
        .iter()
        .filter(|j| j.status == nils_pipeline::pack::rules::Status::Ready)
    {
        for (k, c) in &j.knobs {
            seen.entry(k.clone())
                .or_default()
                .insert(c.value.to_string());
        }
    }
    let differing: Vec<String> = seen
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(k, _)| k)
        .collect();
    if differing.is_empty() {
        return None;
    }
    Some(format!(
        "the pack's rules set {} differently for different units, and this pipeline runs its units together in one container; run it with units apart, or set {} by hand",
        differing.join(", "),
        differing.join(", ")
    ))
}

/// The identities of a unit's input files as the registry holds them (each
/// file's path, size and time), which the result key hashes.
pub(crate) fn input_identities(store: &mut Store, stacks: &[i64]) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in stacks.chunks(500) {
        let ids: Vec<String> = chunk.iter().map(i64::to_string).collect();
        let sql = format!(
            "SELECT f.path, f.size, f.mtime_ns FROM {} i JOIN {} f ON f.id = i.source_file_id \
             WHERE i.stack_id IN ({})",
            store.qualified("instance"),
            store.qualified("source_file"),
            ids.join(", ")
        );
        for r in store.query(&sql, &[]).unwrap_or_default() {
            if let (Ok(p), Ok(s), Ok(m)) = (r.text(0), r.int(1), r.int(2)) {
                out.push(nils_pipeline::sha256(format!("{p}\n{s}\n{m}").as_bytes()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The comparability keys of the units whose results a unit read through
/// its derivative inputs: by the runs that made them.
pub(crate) fn upstream_keys(store: &mut Store, made_by_runs: &[i64]) -> Vec<String> {
    rows::unit_keys(store, made_by_runs)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, _, k)| k)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The two keys of one unit's results.
pub(crate) struct KeysOf<'a> {
    pub pipeline: &'a Pipeline,
    pub pack: Option<&'a PipelinePack>,
    pub judgement: Option<&'a Judgement>,
    pub params: &'a Map<String, Value>,
    pub roles: &'a BTreeSet<String>,
    pub converter: Option<(String, String)>,
    pub upstream: Vec<String>,
    pub inputs: Vec<String>,
}

pub(crate) fn keys(k: &KeysOf<'_>) -> (String, String, Value) {
    let mut m = nils_pipeline::pack::keys::Members {
        tool: k.pipeline.name.clone(),
        tool_version: k.pipeline.tool_version.clone(),
        image: k.pipeline.image_digest.clone(),
        pack: k.pipeline.pack_digest.clone(),
        variant: k
            .judgement
            .and_then(|j| j.variant.as_ref().map(|v| v.variant.clone())),
        breaks: BTreeMap::new(),
        contrasts: k.roles.iter().cloned().collect(),
        facts: BTreeMap::new(),
        converter: k.converter.clone(),
        upstream: k.upstream.clone(),
    };
    if let Some(p) = k.pack {
        for knob in p.breaking_knobs() {
            let v = k
                .judgement
                .and_then(|j| j.knobs.get(&knob).map(|c| c.value.clone()))
                .or_else(|| k.params.get(&knob).cloned())
                .unwrap_or(Value::Null);
            m.breaks.insert(knob, v);
        }
        if let Some(j) = k.judgement {
            for f in &p.comparability_facts {
                m.facts.insert(f.clone(), j.facts.get(f).to_json());
            }
        }
    }
    let (ck, members) = nils_pipeline::pack::keys::comparability(&m);
    let knobs: Map<String, Value> = k
        .judgement
        .map(Judgement::params)
        .unwrap_or_else(|| k.params.clone());
    let (rk, _) = nils_pipeline::pack::keys::result(&ck, &knobs, &k.inputs);
    (ck, rk, members)
}

/// What the pack's fixed checks say of a unit's metrics: the flags (kept
/// results, the check named with its value and threshold), the failures,
/// and the measures a `drop` check drops.
pub(crate) struct Held {
    pub flags: Vec<Value>,
    pub failed: Vec<Value>,
    pub dropped: Vec<String>,
}

pub(crate) fn hold_checks(
    pack: &PipelinePack,
    metrics: &Value,
    measured: &BTreeMap<String, f64>,
) -> Held {
    let mut held = Held {
        flags: Vec::new(),
        failed: Vec::new(),
        dropped: Vec::new(),
    };
    for c in pack.checks.iter().filter(|c| !c.cohort) {
        let value = metrics[&c.metric]
            .as_f64()
            .filter(|v| v.is_finite())
            .or_else(|| measured.get(&c.metric).copied());
        let Some(v) = value else { continue };
        if c.holds(v) {
            continue;
        }
        let doc = json!({
            "metric": c.metric, "value": v, "threshold": c.value, "op": c.op,
            "check": c.text(), "action": c.action.name(), "words": c.words, "source": c.source,
        });
        match c.action {
            pack::Action::Fail => held.failed.push(doc),
            pack::Action::Flag => held.flags.push(doc),
            pack::Action::Drop => {
                held.dropped.push(c.metric.clone());
                held.flags.push(doc);
            }
        }
    }
    held
}

/// The words a flagged unit says: `flair_t1_brain_dice is 0.8, and the
/// check is flair_t1_brain_dice >= 0.85 (FLAIR and T1 may not be aligned)`.
pub(crate) fn flag_words(flags: &[Value]) -> String {
    flags
        .iter()
        .map(|f| {
            let mut s = format!(
                "{} is {}, and the check is {}",
                f["metric"].as_str().unwrap_or_default(),
                nils_pipeline::descriptor::number_text(f["value"].as_f64().unwrap_or(f64::NAN)),
                f["check"].as_str().unwrap_or_default()
            );
            if let Some(w) = f["words"].as_str() {
                s.push_str(&format!(" ({w})"));
            }
            s
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The stratum of a unit for a cohort check: the registry (one site), the
/// vendor, the field strength, the scanning sequence and the post-contrast
/// state of its first stack, as the check's `stratum` names them.
pub(crate) fn stratum_of(store: &mut Store, stack: Option<i64>, members: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let (mut vendor, mut field, mut sequence, mut post) = (
        "unknown".to_string(),
        "unknown".to_string(),
        "unknown".to_string(),
        "unknown".to_string(),
    );
    if let Some(s) = stack {
        let sql = format!(
            "SELECT manufacturer, magnetic_field_strength, scanning_sequence FROM {} WHERE stack_id = {}",
            store.qualified("stack_fingerprint"),
            store.dialect().param(1, nils_registry::schema::Type::Int)
        );
        if let Ok(Some(r)) = store.query_opt(&sql, &[Param::Int(s)]) {
            if let Ok(Some(m)) = r.opt_text(0) {
                vendor = nils_pipeline::pack::facts::vendor_of(m).to_string();
            }
            if let Ok(Some(f)) = r.opt_double(1) {
                field = nils_pipeline::descriptor::number_text(f);
            }
            if let Ok(Some(q)) = r.opt_text(2) {
                sequence = q.trim().to_string();
            }
        }
        let sql = format!(
            "SELECT value FROM {} WHERE stack_id = {} AND axis = 'post_contrast' AND value IS NOT NULL",
            store.qualified("classification_axis"),
            store.dialect().param(1, nils_registry::schema::Type::Int)
        );
        if let Ok(Some(r)) = store.query_opt(&sql, &[Param::Int(s)])
            && let Ok(Some(v)) = r.opt_text(0)
        {
            post = v.to_string();
        }
    }
    for m in members {
        match m.as_str() {
            "site" => parts.push("site=here".into()),
            "vendor" => parts.push(format!("vendor={vendor}")),
            "field_strength" => parts.push(format!("field={field}")),
            "sequence" => parts.push(format!("sequence={sequence}")),
            "post_contrast" => parts.push(format!("post_contrast={post}")),
            _ => {}
        }
    }
    parts.join(",")
}

/// A cohort check over the units of a pipeline name whose measures of the
/// metric the registry holds (spec section 5.8): within each stratum of at
/// least `min_sessions` sessions, a robust z (the median and the MAD) above
/// `z` flags the unit; below the size the check reads "not yet: N of M
/// sessions". Answers, per unit of `units` (as `(run_id, unit)`), the
/// check's standing: `{metric, status: flagged | not yet | within, words,
/// z, stratum, sessions, min_sessions}`.
pub(crate) fn cohort_check(
    store: &mut Store,
    pipeline: &str,
    check: &pack::Check,
    units: &[(i64, String, Option<i64>)],
) -> Vec<Value> {
    // every measure of the metric of this pipeline name, by unit, with its
    // stratum: one value a unit (its newest)
    let d = store.dialect();
    let sql = format!(
        "SELECT run_id, unit_id, stack_id, subject_id, number FROM {} WHERE pipeline = {} AND name = {} \
         AND number IS NOT NULL ORDER BY id",
        store.qualified("measure"),
        d.param(1, nils_registry::schema::Type::Text),
        d.param(2, nils_registry::schema::Type::Text),
    );
    let mut values: BTreeMap<(i64, String), (Option<i64>, f64)> = BTreeMap::new();
    for r in store
        .query(
            &sql,
            &[Param::from(pipeline), Param::from(check.metric.as_str())],
        )
        .unwrap_or_default()
    {
        if let (Ok(run), Ok(unit), Ok(stack), Ok(n)) =
            (r.int(0), r.text(1), r.opt_int(2), r.double(4))
        {
            values.insert((run, unit.to_string()), (stack, n));
        }
    }
    // a session's stack for its stratum: the measure's stack, else the
    // first stack of the unit the caller named
    let mut stratum_cache: BTreeMap<Option<i64>, String> = BTreeMap::new();
    let mut by_stratum: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut of_unit: BTreeMap<(i64, String), (String, f64)> = BTreeMap::new();
    let first_stack: BTreeMap<(i64, String), Option<i64>> = units
        .iter()
        .map(|(r, u, s)| ((*r, u.clone()), *s))
        .collect();
    for ((run, unit), (stack, n)) in &values {
        let stack = stack.or_else(|| first_stack.get(&(*run, unit.clone())).copied().flatten());
        let stratum = stratum_cache
            .entry(stack)
            .or_insert_with(|| stratum_of(store, stack, &check.stratum))
            .clone();
        by_stratum.entry(stratum.clone()).or_default().push(*n);
        of_unit.insert((*run, unit.clone()), (stratum, *n));
    }
    let robust = |xs: &[f64]| -> Option<(f64, f64)> {
        if xs.is_empty() {
            return None;
        }
        let mut s = xs.to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = |v: &[f64]| {
            let n = v.len();
            if n % 2 == 1 {
                v[n / 2]
            } else {
                (v[n / 2 - 1] + v[n / 2]) / 2.0
            }
        };
        let med = median(&s);
        let mut dev: Vec<f64> = s.iter().map(|x| (x - med).abs()).collect();
        dev.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Some((med, 1.4826 * median(&dev)))
    };
    units
        .iter()
        .map(|(run, unit, _)| {
            let Some((stratum, n)) = of_unit.get(&(*run, unit.clone())) else {
                return json!({
                    "metric": check.metric, "status": "unchecked", "check": check.text(),
                    "words": format!("{} was not measured for this unit", check.metric),
                });
            };
            let xs = &by_stratum[stratum];
            let sessions = xs.len();
            if sessions < check.min_sessions {
                return json!({
                    "metric": check.metric, "status": "not yet", "check": check.text(),
                    "words": format!("not yet: {sessions} of {} sessions", check.min_sessions),
                    "stratum": stratum, "sessions": sessions, "min_sessions": check.min_sessions,
                });
            }
            let (med, mad) = robust(xs).unwrap_or((0.0, 0.0));
            let z = if mad > 0.0 { (n - med) / mad } else { 0.0 };
            let above = match check.op.as_str() {
                ">=" | ">" => z < -check.z,
                "<=" | "<" => z > check.z,
                _ => z.abs() > check.z,
            };
            json!({
                "metric": check.metric, "status": if above { "flagged" } else { "within" }, "check": check.text(),
                "value": n, "z": (z * 100.0).round() / 100.0, "median": med, "mad": mad,
                "stratum": stratum, "sessions": sessions, "min_sessions": check.min_sessions,
                "words": if above {
                    format!("{} is {} (robust z {:.1} against its stratum of {sessions} sessions; the check flags above {})",
                        check.metric, nils_pipeline::descriptor::number_text(*n), z, nils_pipeline::descriptor::number_text(check.z))
                } else {
                    format!("within its stratum of {sessions} sessions (robust z {:.1})", z)
                },
            })
        })
        .collect()
}

/// A run's numbers per comparability key (the run's numbers door): every
/// measure of the run grouped by the key of the unit it came from, each
/// group with its units and, per measure name, the count, the sum and the
/// mean; and, where the run holds more than one key, the pooling words and
/// no total across keys.
pub(crate) fn numbers_by_key(store: &mut Store, run_id: i64) -> Result<Value, String> {
    let units = rows::units(store, run_id).map_err(|e| e.to_string())?;
    let key_of: BTreeMap<String, String> = units
        .iter()
        .filter_map(|u| u.comparability_key.clone().map(|k| (u.unit.clone(), k)))
        .collect();
    let measures = nils_registry::measure::of_run(store, run_id).map_err(|e| e.to_string())?;
    /// The measures of one comparability key: the units they came from,
    /// the numbers by measure name, and each name's unit of measure.
    #[derive(Default)]
    struct Group {
        units: BTreeSet<String>,
        numbers: BTreeMap<String, Vec<f64>>,
        unit_of: BTreeMap<String, Option<String>>,
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for m in &measures {
        let key = key_of
            .get(&m.unit_id)
            .cloned()
            .unwrap_or_else(|| "none".to_string());
        let g = groups.entry(key).or_default();
        g.units.insert(m.unit_id.clone());
        if let Some(n) = m.number {
            g.numbers.entry(m.name.clone()).or_default().push(n);
            g.unit_of.entry(m.name.clone()).or_insert(m.unit.clone());
        }
    }
    let keys: Vec<String> = groups.keys().cloned().collect();
    let words = (keys.len() > 1).then(|| nils_pipeline::pack::keys::pooling_words(&keys));
    let caveats: BTreeMap<String, Vec<Value>> = units
        .iter()
        .filter(|u| !u.pack["caveats"].as_array().is_none_or(Vec::is_empty))
        .map(|u| {
            (
                u.unit.clone(),
                u.pack["caveats"].as_array().cloned().unwrap_or_default(),
            )
        })
        .collect();
    Ok(json!({
        "run": run_id,
        "keys": groups.iter().map(|(k, g)| json!({
            "comparability_key": k,
            "short": nils_pipeline::pack::keys::short(k),
            "units": g.units.len(),
            "measures": g.numbers.iter().map(|(name, xs)| (name.clone(), json!({
                "count": xs.len(),
                "sum": xs.iter().sum::<f64>(),
                "mean": xs.iter().sum::<f64>() / xs.len() as f64,
                "unit": g.unit_of.get(name).cloned().flatten(),
            }))).collect::<Map<_, _>>(),
            "caveats": g.units.iter().filter_map(|u| caveats.get(u).map(|c| json!({"unit": u, "caveats": c}))).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "pooled": keys.len() <= 1,
        "words": words,
        "measures": measures.len(),
    }))
}

/// The same numbers as rows of text: one row a unit and measure, with its
/// comparability key, and never a total across keys (the export).
pub(crate) fn numbers_csv(store: &mut Store, run_id: i64) -> Result<String, String> {
    let units = rows::units(store, run_id).map_err(|e| e.to_string())?;
    let key_of: BTreeMap<String, String> = units
        .iter()
        .filter_map(|u| u.comparability_key.clone().map(|k| (u.unit.clone(), k)))
        .collect();
    let measures = nils_registry::measure::of_run(store, run_id).map_err(|e| e.to_string())?;
    let keys: BTreeSet<String> = key_of.values().cloned().collect();
    let mut out = String::new();
    if keys.len() > 1 {
        out.push_str(&format!(
            "# {}\n",
            nils_pipeline::pack::keys::pooling_words(&keys.iter().cloned().collect::<Vec<_>>())
        ));
    }
    out.push_str("run,unit,comparability_key,name,value,unit_of_measure\n");
    let cell = |s: &str| {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    for m in &measures {
        let value = match (m.number, &m.text) {
            (Some(n), _) => nils_pipeline::descriptor::number_text(n),
            (None, Some(t)) => cell(t),
            _ => String::new(),
        };
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            run_id,
            cell(&m.unit_id),
            key_of.get(&m.unit_id).map(String::as_str).unwrap_or("none"),
            cell(&m.name),
            value,
            m.unit.as_deref().map(cell).unwrap_or_default()
        ));
    }
    Ok(out)
}

/// The comparability keys a pipeline's measures hold in the registry, by
/// pipeline name: what the ask door reads before it pools a measure.
pub(crate) fn keys_of_pipeline(store: &mut Store, pipeline: &str) -> Vec<String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT DISTINCT pu.comparability_key FROM {} pu JOIN {} pr ON pr.id = pu.run_id \
         JOIN {} pl ON pl.id = pr.pipeline_id WHERE pl.name = {} AND pu.comparability_key IS NOT NULL \
         AND EXISTS (SELECT 1 FROM {} nm WHERE nm.run_id = pu.run_id AND nm.unit_id = pu.unit)",
        store.qualified("pipeline_unit"),
        store.qualified("pipeline_run"),
        store.qualified("pipeline"),
        d.param(1, nils_registry::schema::Type::Text),
        store.qualified("measure"),
    );
    store
        .query(&sql, &[Param::from(pipeline)])
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.text(0).ok().map(str::to_string))
        .collect()
}

/// The pipelines whose measures an ask aggregates (`sum`, `avg`, `min`,
/// `max`, `distinct`, `count` or `list` over `measure.<pipeline>.<name>`,
/// or an answer at the aggregate level with such a column), and whether
/// each is grouped by its key (`measure.<pipeline>.key`).
pub(crate) fn pooled_pipelines(ask: &nils_ask::ast::Ask) -> Vec<(String, bool)> {
    use nils_ask::ast::{Arg, Clause};
    fn fields(c: &Clause, out: &mut Vec<String>) {
        if c.op == "field"
            && let Some(Arg::Text(path)) = c.args.first()
        {
            out.push(path.clone());
        }
        for a in &c.args {
            if let Arg::Clause(inner) = a {
                fields(inner, out);
            }
        }
    }
    fn pipeline_of(path: &str) -> Option<(String, String)> {
        let rest = path
            .strip_prefix("measure.")
            .or_else(|| path.split_once(".measure.").map(|(_, r)| r))?;
        let (pipeline, name) = rest.split_once('.')?;
        Some((pipeline.to_string(), name.to_string()))
    }
    const AGGREGATES: [&str; 7] = ["count", "distinct", "min", "max", "sum", "avg", "list"];
    let mut pooled: BTreeSet<String> = BTreeSet::new();
    let aggregate_level = ask.out.level == nils_ask::ast::Level::Aggregate;
    for c in &ask.out.columns {
        let mut paths = Vec::new();
        fields(c, &mut paths);
        let aggregates = AGGREGATES.contains(&c.op.as_str()) || aggregate_level;
        if !aggregates {
            continue;
        }
        for p in paths {
            if let Some((pipeline, name)) = pipeline_of(&p)
                && name != "key"
            {
                pooled.insert(pipeline);
            }
        }
    }
    for m in &ask.out.measures {
        for v in m.0.values() {
            if let Some(of) = v["of"].as_str()
                && let Some((pipeline, name)) = pipeline_of(of)
                && name != "key"
            {
                pooled.insert(pipeline);
            }
        }
    }
    let mut grouped: BTreeSet<String> = BTreeSet::new();
    for set in ask.sets.values() {
        if let Some(g) = &set.group {
            for c in &g.by {
                let mut paths = Vec::new();
                fields(c, &mut paths);
                for p in paths {
                    if let Some((pipeline, name)) = pipeline_of(&p)
                        && name == "key"
                    {
                        grouped.insert(pipeline);
                    }
                }
            }
        }
    }
    pooled
        .into_iter()
        .map(|p| (p.clone(), grouped.contains(&p)))
        .collect()
}

/// Why an ask is refused before it runs: it would pool a pipeline's
/// measures across more than one comparability key without grouping by the
/// key (`measure.<pipeline>.key`). None where it may run.
pub(crate) fn pooling_refusal(store: &mut Store, ask: &nils_ask::ast::Ask) -> Option<String> {
    for (pipeline, grouped) in pooled_pipelines(ask) {
        if grouped {
            continue;
        }
        let keys = keys_of_pipeline(store, &pipeline);
        if keys.len() > 1 {
            return Some(format!(
                "{}: group by measure.{pipeline}.key to answer per key",
                nils_pipeline::pack::keys::pooling_words(&keys)
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_person_s_ask_reads_from_a_body_and_from_flags() {
        let a = PersonAsk::of_body(
            &json!({"goal": "fast", "switches": {"lesions": "off"}, "let_run": true}),
        )
        .unwrap();
        assert_eq!(a.goal.as_deref(), Some("fast"));
        assert_eq!(a.switches["lesions"], "off");
        assert!(a.let_run);
        let b = PersonAsk::of_args(Some("fast"), &["lesions=off".into()], true).unwrap();
        assert_eq!(a, b);
        assert!(PersonAsk::of_body(&json!({"goal": 3})).is_err());
        assert!(PersonAsk::of_args(None, &["lesions".into()], false).is_err());
        assert_eq!(
            PersonAsk::of_args(None, &["threads=4".into()], false)
                .unwrap()
                .switches["threads"],
            4.0
        );
    }

    #[test]
    fn the_pack_s_checks_flag_fail_or_drop() {
        let text = r#"nils_pack: 1
pipeline: {name: t, descriptor_sha256: "sha256:0000000000000000000000000000000000000000000000000000000000000000"}
checks:
  - {metric: dice, op: ">=", value: 0.85, action: flag, source: policy, words: "may not be aligned"}
  - {metric: volume, op: ">=", value: 900000, action: fail, source: policy}
  - {metric: fber, op: "!=", value: -1, action: drop, source: policy}
  - {metric: euler, op: "<=", action: flag, cohort: true, source: policy}
"#;
        let pack = pack::parse(text).unwrap();
        let measured: BTreeMap<String, f64> = [("fber".to_string(), -1.0)].into_iter().collect();
        let held = hold_checks(&pack, &json!({"dice": 0.8, "volume": 500000.0}), &measured);
        assert_eq!(held.flags.len(), 2, "{:?}", held.flags);
        assert_eq!(held.flags[0]["check"], "dice >= 0.85");
        assert_eq!(held.flags[0]["action"], "flag");
        assert_eq!(held.failed.len(), 1);
        assert_eq!(held.failed[0]["metric"], "volume");
        assert_eq!(held.dropped, vec!["fber".to_string()]);
        assert_eq!(
            flag_words(&held.flags[..1]),
            "dice is 0.8, and the check is dice >= 0.85 (may not be aligned)"
        );
        let fine = hold_checks(
            &pack,
            &json!({"dice": 0.95, "volume": 1200000.0}),
            &BTreeMap::new(),
        );
        assert!(fine.flags.is_empty() && fine.failed.is_empty() && fine.dropped.is_empty());
    }

    #[test]
    fn the_ask_s_pooled_pipelines_are_found_with_their_grouping() {
        let ask: nils_ask::ast::Ask = serde_json::from_value(json!({
            "ast_version": 1,
            "sets": {"s": {"grain": "session"}},
            "out": {"set": "s", "level": "record", "columns": [
                ["sum", {}, ["field", {}, "measure.synthseg.total_intracranial"]],
                ["field", {}, "measure.samseg-lesions.lesions"],
            ]}
        }))
        .unwrap();
        assert_eq!(
            pooled_pipelines(&ask),
            vec![("synthseg".to_string(), false)]
        );
        let ask: nils_ask::ast::Ask = serde_json::from_value(json!({
            "ast_version": 1,
            "sets": {"s": {"grain": "session"}, "g": {"grain": "group", "group": {"of": "s", "by": [["field", {}, "measure.synthseg.key"]]}}},
            "out": {"set": "g", "level": "aggregate", "columns": [
                ["avg", {}, ["field", {}, "measure.synthseg.total_intracranial"]],
            ]}
        }))
        .unwrap();
        assert_eq!(pooled_pipelines(&ask), vec![("synthseg".to_string(), true)]);
    }
}
