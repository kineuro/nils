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

/// Why the stored pack of a catalog row cannot be read by this engine
/// (a newer contract, say), where it has one: `pack_of` reads that row as
/// having none, which a run must never take for the truth.
pub(crate) fn pack_error(p: &Pipeline) -> Option<String> {
    if p.pack.is_null() {
        return None;
    }
    pack::from_value(p.pack.clone()).err().map(|e| {
        format!(
            "{} has a pack (digest {}) that this engine cannot read: {e}",
            p.label(),
            p.pack_digest.as_deref().unwrap_or("unrecorded")
        )
    })
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
                    .parse::<i64>()
                    .map(|n| json!(n))
                    .or_else(|_| other.parse::<f64>().map(|n| json!(n)))
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

/// How far a value sits from the middle of its stratum, and by which
/// spread (spec section 5.8). The robust z reads the median and the MAD;
/// where more than half the values are identical the MAD is 0, and the
/// mean absolute deviation stands in; where that is 0 as well every value
/// is the same, and any value that differs from the median is flagged
/// whatever its distance. Answers `(median, spread, z, how)`; `z` is
/// infinite (and `how` says so) in the last case.
pub(crate) fn standardise(xs: &[f64], n: f64) -> Option<(f64, f64, f64, &'static str)> {
    if xs.is_empty() {
        return None;
    }
    let median = |v: &[f64]| {
        let mut s = v.to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let k = s.len();
        if k % 2 == 1 {
            s[k / 2]
        } else {
            (s[k / 2 - 1] + s[k / 2]) / 2.0
        }
    };
    let med = median(xs);
    let dev: Vec<f64> = xs.iter().map(|x| (x - med).abs()).collect();
    let mad = 1.4826 * median(&dev);
    if mad > 0.0 {
        return Some((med, mad, (n - med) / mad, "the MAD"));
    }
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    // 1.2533 turns a mean absolute deviation into the scale of a standard
    // deviation, as 1.4826 does for the MAD
    let mean_dev = 1.2533 * xs.iter().map(|x| (x - mean).abs()).sum::<f64>() / xs.len() as f64;
    if mean_dev > 0.0 {
        return Some((
            med,
            mean_dev,
            (n - med) / mean_dev,
            "the mean absolute deviation (the MAD is 0)",
        ));
    }
    let z = if n > med {
        f64::INFINITY
    } else if n < med {
        f64::NEG_INFINITY
    } else {
        0.0
    };
    Some((
        med,
        0.0,
        z,
        "any difference from the median (every value is the same)",
    ))
}

/// One measure of a cohort check's metric, whose it is and where it sits.
#[derive(Debug, Clone)]
struct Seen {
    number: f64,
    key: String,
    stack: Option<i64>,
    from: String,
}

/// A cohort check over the units of a pipeline name whose measures of the
/// metric the registry holds (spec section 5.8): within each stratum of at
/// least `min_sessions` sessions, a robust z (the median and the MAD) above
/// `z` flags the unit; below the size the check reads "not yet: N of M
/// sessions". A session counts once: its latest kept value across the runs
/// of the name that share its comparability key. A unit's stratum is read
/// from the stack its measure names, else from the main scan of the session
/// the unit read (the first of the pipeline's roles that has a stack; else
/// its first input stack), which the unit recorded when its run began.
/// Answers, per unit of `units` (as `(run_id, unit)`), the check's standing:
/// `{metric, status: flagged | not yet | within, words, z, stratum,
/// stratum_from, sessions, min_sessions}`.
pub(crate) fn cohort_check(
    store: &mut Store,
    pipeline: &str,
    check: &pack::Check,
    units: &[(i64, String)],
) -> Vec<Value> {
    let d = store.dialect();
    let day = d.text_of(
        nils_registry::schema::table("measure")
            .column("session_day")
            .expect("session_day"),
    );
    let sql = format!(
        "SELECT run_id, unit_id, stack_id, subject_id, {day}, number, scope FROM {} WHERE pipeline = {} AND name = {} \
         AND number IS NOT NULL ORDER BY id",
        store.qualified("measure"),
        d.param(1, nils_registry::schema::Type::Text),
        d.param(2, nils_registry::schema::Type::Text),
    );
    let measured = store
        .query(
            &sql,
            &[Param::from(pipeline), Param::from(check.metric.as_str())],
        )
        .unwrap_or_default();
    let run_ids: Vec<i64> = measured
        .iter()
        .filter_map(|r| r.int(0).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let records: BTreeMap<(i64, String), (Option<String>, Value)> =
        rows::unit_records(store, &run_ids)
            .unwrap_or_default()
            .into_iter()
            .map(|(run, unit, key, pack)| ((run, unit), (key, pack)))
            .collect();
    // every measure with its owner, its key and the stack that says its
    // stratum; later rows replace earlier ones of the same session and key
    let mut own: BTreeMap<(i64, String), Seen> = BTreeMap::new();
    let mut latest: BTreeMap<(String, String), Seen> = BTreeMap::new();
    for r in &measured {
        let (Ok(run), Ok(unit), Ok(n)) = (r.int(0), r.text(1), r.double(5)) else {
            continue;
        };
        let (stack, subject, session) = (
            r.opt_int(2).ok().flatten(),
            r.opt_int(3).ok().flatten(),
            r.opt_text(4).ok().flatten(),
        );
        let (key, pack) = records
            .get(&(run, unit.to_string()))
            .cloned()
            .unwrap_or((None, Value::Null));
        let key = key.unwrap_or_default();
        let (stack, from) = match (
            stack,
            pack["stratum_stack"]["stack"].as_i64(),
            unit.strip_prefix("stack-")
                .and_then(|s| s.parse::<i64>().ok()),
        ) {
            (Some(s), _, _) => (Some(s), "the measure's stack".to_string()),
            (None, Some(s), _) => (
                Some(s),
                pack["stratum_stack"]["from"]
                    .as_str()
                    .unwrap_or("the unit's input stack")
                    .to_string(),
            ),
            (None, None, Some(s)) => (Some(s), "the unit's stack".to_string()),
            _ => (None, "no stack recorded".to_string()),
        };
        // what a value is of: a stack's own for a stack-level pipeline, a
        // session's (or a subject's) otherwise
        let who = match (r.text(6).unwrap_or_default(), subject, session, stack) {
            ("stack", _, _, Some(k)) => format!("stack:{k}"),
            (_, Some(s), Some(t), _) => format!("session:{s}:{t}"),
            (_, Some(s), None, _) => format!("subject:{s}"),
            (_, None, _, Some(k)) => format!("stack:{k}"),
            _ => format!("unit:{run}:{unit}"),
        };
        let seen = Seen {
            number: n,
            key: key.clone(),
            stack,
            from,
        };
        latest.insert((who, key), seen.clone());
        own.insert((run, unit.to_string()), seen);
    }
    let mut stratum_cache: BTreeMap<Option<i64>, String> = BTreeMap::new();
    let mut stratum_for = |store: &mut Store, s: &Seen| -> String {
        stratum_cache
            .entry(s.stack)
            .or_insert_with(|| stratum_of(store, s.stack, &check.stratum))
            .clone()
    };
    let mut by_stratum: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for s in latest.values() {
        let stratum = stratum_for(store, s);
        by_stratum
            .entry((s.key.clone(), stratum))
            .or_default()
            .push(s.number);
    }
    units
        .iter()
        .map(|(run, unit)| {
            let Some(seen) = own.get(&(*run, unit.clone())) else {
                return json!({
                    "metric": check.metric, "status": "unchecked", "check": check.text(),
                    "words": format!("{} was not measured for this unit", check.metric),
                });
            };
            let stratum = stratum_for(store, seen);
            let n = seen.number;
            let xs = &by_stratum[&(seen.key.clone(), stratum.clone())];
            let sessions = xs.len();
            if sessions < check.min_sessions {
                return json!({
                    "metric": check.metric, "status": "not yet", "check": check.text(),
                    "words": format!("not yet: {sessions} of {} sessions", check.min_sessions),
                    "stratum": stratum, "stratum_from": seen.from,
                    "sessions": sessions, "min_sessions": check.min_sessions,
                });
            }
            let (med, spread, z, how) = standardise(xs, n).unwrap_or((0.0, 0.0, 0.0, "the MAD"));
            let above = match check.op.as_str() {
                ">=" | ">" => z < -check.z,
                "<=" | "<" => z > check.z,
                _ => z.abs() > check.z,
            };
            let z_text = if z.is_finite() {
                format!("robust z {z:.1}")
            } else {
                "it differs from the median, which all the other values equal".to_string()
            };
            json!({
                "metric": check.metric, "status": if above { "flagged" } else { "within" }, "check": check.text(),
                "value": n, "z": if z.is_finite() { json!((z * 100.0).round() / 100.0) } else { Value::Null },
                "median": med, "mad": spread, "spread_by": how,
                "stratum": stratum, "stratum_from": seen.from,
                "sessions": sessions, "min_sessions": check.min_sessions,
                "words": if above {
                    format!("{} is {} ({z_text}, by {how}, against its stratum of {sessions} sessions; the check flags above {})",
                        check.metric, nils_pipeline::descriptor::number_text(n), nils_pipeline::descriptor::number_text(check.z))
                } else {
                    format!("within its stratum of {sessions} sessions ({z_text})")
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
    // units that carry no key (a run from before the pack) are reported
    // apart, never as a key of their own
    let mut unkeyed_units: BTreeSet<String> = BTreeSet::new();
    let mut unkeyed_measures = 0usize;
    for m in &measures {
        let Some(key) = key_of.get(&m.unit_id).cloned() else {
            unkeyed_units.insert(m.unit_id.clone());
            unkeyed_measures += 1;
            continue;
        };
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
        "unkeyed": unkeyed_units.len(),
        "unkeyed_measures": unkeyed_measures,
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
    let unkeyed: BTreeSet<&str> = measures
        .iter()
        .filter(|m| !key_of.contains_key(&m.unit_id))
        .map(|m| m.unit_id.as_str())
        .collect();
    if !unkeyed.is_empty() {
        out.push_str(&format!(
            "# unkeyed: {} (units with no comparability key, listed with an empty key)\n",
            unkeyed.len()
        ));
    }
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
            key_of.get(&m.unit_id).map(String::as_str).unwrap_or(""),
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

    // ---- the review fixes of 2026-10-19 ----

    #[test]
    fn a_numeric_switch_on_the_command_line_is_the_integer_the_pack_offers() {
        let pack = pack::parse(
            r#"nils_pack: 1
pipeline: {name: t, descriptor_sha256: "sha256:0000000000000000000000000000000000000000000000000000000000000000"}
goals: {goal: [fast, standard], workers: [2, 4], ratio: [0.5, 1.5]}
"#,
        )
        .unwrap();
        let ask =
            PersonAsk::of_args(None, &["workers=4".into(), "ratio=1.5".into()], false).unwrap();
        assert_eq!(ask.switches["workers"], json!(4));
        assert_eq!(ask.switches["ratio"], json!(1.5));
        let person = ask
            .person(&pack, "anna", &[])
            .expect("the door accepts it, so must the flag");
        assert_eq!(person.choices["workers"], json!(4));
        assert!(
            PersonAsk::of_args(None, &["workers=3".into()], false)
                .unwrap()
                .person(&pack, "anna", &[])
                .is_err()
        );
    }

    fn registry_store() -> Store {
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        store
    }

    fn put_stack(store: &mut Store, id: i64, field: f64) {
        store
            .batch(&format!(
                "INSERT INTO stack_fingerprint (stack_id, series_id, study_id, subject_id, modality, \
                 orientation, n_instances, stack_index, stacks_in_series, job_id, epoch, magnetic_field_strength, manufacturer) \
                 VALUES ({id}, {id}, {id}, {id}, 'MR', 'axial', 1, 0, 1, 1, 1, {field}, 'SIEMENS')"
            ))
            .unwrap();
    }

    /// A unit row of run `run`, with its key and the pack record it kept.
    fn put_unit(store: &mut Store, run: i64, unit: &str, key: Option<&str>, pack: &Value) {
        store
            .batch(&format!(
                "INSERT INTO pipeline_unit (run_id, unit, position, state, attempts, pack, comparability_key) \
                 VALUES ({run}, '{unit}', 0, 'over', 1, '{}', {})",
                pack.to_string().replace('\'', "''"),
                key.map_or("NULL".to_string(), |k| format!("'{k}'"))
            ))
            .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn put_measure(
        store: &mut Store,
        run: i64,
        unit: &str,
        subject: i64,
        day: &str,
        stack: Option<i64>,
        name: &str,
        number: f64,
    ) {
        store
            .batch(&format!(
                "INSERT INTO measure (run_id, pipeline_id, pipeline, source, scope, subject_id, session_day, \
                 stack_id, unit_id, name, type, number, created_at) VALUES ({run}, 1, 'samseg-lesions', 'metrics', \
                 'session', {subject}, '{day}', {}, '{unit}', '{name}', 'number', {number}, '2026-10-19T10:00:00Z')",
                stack.map_or("NULL".to_string(), |s| s.to_string())
            ))
            .unwrap();
    }

    fn cohort(min_sessions: usize) -> pack::Check {
        let p = pack::parse(&format!(
            r#"nils_pack: 1
pipeline: {{name: samseg-lesions, descriptor_sha256: "sha256:0000000000000000000000000000000000000000000000000000000000000000"}}
checks:
  - {{metric: intracranial, op: "<=", action: flag, cohort: true, stratum: [site, field_strength], min_sessions: {min_sessions}, source: policy}}
"#
        ))
        .unwrap();
        p.checks[0].clone()
    }

    fn stratum_pack(stack: i64) -> Value {
        json!({"stratum_stack": {"stack": stack, "from": "the t1w stack"}})
    }

    #[test]
    fn a_session_unit_is_stratified_by_the_stack_its_unit_read() {
        let mut store = registry_store();
        let check = cohort(5);
        // ten sessions, five at 1.5 T and five at 3 T, one unit each; a
        // session unit's measure has no stack of its own
        let mut units = Vec::new();
        for (i, (field, value)) in [
            (1.5, 10.0),
            (1.5, 10.5),
            (1.5, 11.0),
            (1.5, 11.5),
            (1.5, 30.0),
            (3.0, 20.0),
            (3.0, 20.5),
            (3.0, 21.0),
            (3.0, 21.5),
            (3.0, 22.0),
        ]
        .into_iter()
        .enumerate()
        {
            let i = i as i64 + 1;
            put_stack(&mut store, i, field);
            let unit = format!("sub-{i}_ses-a");
            put_unit(&mut store, 1, &unit, Some("sha256:k"), &stratum_pack(i));
            put_measure(
                &mut store,
                1,
                &unit,
                i,
                "2026-01-01",
                None,
                "intracranial",
                value,
            );
            units.push((1, unit));
        }
        let out = cohort_check(&mut store, "samseg-lesions", &check, &units);
        for (i, c) in out.iter().enumerate() {
            let field = if i < 5 { "1.5" } else { "3" };
            assert_eq!(c["stratum"], format!("site=here,field={field}"), "{c}");
            assert_eq!(
                c["sessions"], 5,
                "two strata of five, not one unknown of ten: {c}"
            );
            assert_eq!(c["stratum_from"], "the t1w stack", "{c}");
        }
        assert_eq!(out[4]["status"], "flagged", "{}", out[4]);
        assert_eq!(out[5]["status"], "within", "{}", out[5]);
        // a unit that kept no stack falls in the unknown stratum, and says so
        put_unit(&mut store, 1, "sub-99_ses-a", Some("sha256:k"), &json!({}));
        put_measure(
            &mut store,
            1,
            "sub-99_ses-a",
            99,
            "2026-01-01",
            None,
            "intracranial",
            5.0,
        );
        let out = cohort_check(
            &mut store,
            "samseg-lesions",
            &check,
            &[(1, "sub-99_ses-a".into())],
        );
        assert_eq!(out[0]["stratum"], "site=here,field=unknown", "{}", out[0]);
        assert_eq!(out[0]["stratum_from"], "no stack recorded", "{}", out[0]);
    }

    #[test]
    fn a_session_run_again_counts_once_and_a_key_is_its_own_population() {
        let mut store = registry_store();
        let check = cohort(3);
        put_stack(&mut store, 1, 3.0);
        put_stack(&mut store, 2, 3.0);
        // two sessions, each run three times under one key: two sessions
        let mut units = Vec::new();
        for run in 1..=3 {
            for (s, stack) in [(1, 1), (2, 2)] {
                let unit = format!("sub-{s}_ses-a");
                put_unit(
                    &mut store,
                    run,
                    &unit,
                    Some("sha256:k"),
                    &stratum_pack(stack),
                );
                put_measure(
                    &mut store,
                    run,
                    &unit,
                    s,
                    "2026-01-01",
                    None,
                    "intracranial",
                    10.0 + run as f64,
                );
                units.push((run, unit));
            }
        }
        let out = cohort_check(&mut store, "samseg-lesions", &check, &units);
        for c in &out {
            assert_eq!(c["status"], "not yet", "six values are two sessions: {c}");
            assert_eq!(c["words"], "not yet: 2 of 3 sessions", "{c}");
        }
        // a third session opens the guard, still counting each once
        put_stack(&mut store, 3, 3.0);
        put_unit(
            &mut store,
            3,
            "sub-3_ses-a",
            Some("sha256:k"),
            &stratum_pack(3),
        );
        put_measure(
            &mut store,
            3,
            "sub-3_ses-a",
            3,
            "2026-01-01",
            None,
            "intracranial",
            11.0,
        );
        let out = cohort_check(&mut store, "samseg-lesions", &check, &units);
        assert!(out.iter().all(|c| c["sessions"] == 3), "{out:?}");
        assert!(out.iter().all(|c| c["status"] == "within"), "{out:?}");
        // the same session under another key is a session of that key's own
        // population, not of the first
        put_unit(
            &mut store,
            4,
            "sub-1_ses-a",
            Some("sha256:other"),
            &stratum_pack(1),
        );
        put_measure(
            &mut store,
            4,
            "sub-1_ses-a",
            1,
            "2026-01-01",
            None,
            "intracranial",
            12.0,
        );
        let out = cohort_check(
            &mut store,
            "samseg-lesions",
            &check,
            &[(4, "sub-1_ses-a".into()), (3, "sub-3_ses-a".into())],
        );
        assert_eq!(out[0]["sessions"], 1, "{}", out[0]);
        assert_eq!(out[1]["sessions"], 3, "{}", out[1]);
    }

    #[test]
    fn a_stratum_of_identical_values_still_flags_an_outlier() {
        // the MAD is 0 where more than half the values are identical: the
        // mean absolute deviation stands in
        let mut xs = vec![5.0; 9];
        xs.push(50.0);
        let (med, spread, z, how) = standardise(&xs, 50.0).unwrap();
        assert_eq!(med, 5.0);
        assert!(spread > 0.0 && z > 3.0, "{spread} {z}");
        assert!(how.starts_with("the mean absolute deviation"), "{how}");
        let (_, _, z, how) = standardise(&xs, 5.0).unwrap();
        assert!(z.abs() < 1.0, "{z}");
        assert!(how.starts_with("the mean absolute deviation"), "{how}");
        // a spread the MAD sees is the MAD's
        let (_, _, _, how) = standardise(&[1.0, 2.0, 3.0, 4.0, 5.0], 5.0).unwrap();
        assert_eq!(how, "the MAD");
        // nothing varies: any value that differs from the median flags
        let (_, spread, z, how) = standardise(&[5.0, 5.0, 5.0], 7.0).unwrap();
        assert!(spread == 0.0 && z == f64::INFINITY, "{spread} {z}");
        assert!(how.starts_with("any difference from the median"), "{how}");
        let (_, _, z, _) = standardise(&[5.0, 5.0, 5.0], 5.0).unwrap();
        assert_eq!(z, 0.0);

        // through the check: the outlier flags, and the record says by what
        let mut store = registry_store();
        let check = cohort(5);
        let mut units = Vec::new();
        for i in 1..=10_i64 {
            put_stack(&mut store, i, 3.0);
            let unit = format!("sub-{i}_ses-a");
            put_unit(&mut store, 1, &unit, Some("sha256:k"), &stratum_pack(i));
            put_measure(
                &mut store,
                1,
                &unit,
                i,
                "2026-01-01",
                None,
                "intracranial",
                if i == 10 { 50.0 } else { 5.0 },
            );
            units.push((1, unit));
        }
        let out = cohort_check(&mut store, "samseg-lesions", &check, &units);
        assert_eq!(out[9]["status"], "flagged", "{}", out[9]);
        assert!(
            out[9]["spread_by"]
                .as_str()
                .unwrap()
                .starts_with("the mean absolute deviation"),
            "{}",
            out[9]
        );
        assert!(
            out[9]["words"]
                .as_str()
                .unwrap()
                .contains("by the mean absolute deviation"),
            "{}",
            out[9]
        );
        assert_eq!(out[0]["status"], "within", "{}", out[0]);
    }

    #[test]
    fn units_with_no_key_are_reported_apart_at_both_doors() {
        let mut store = registry_store();
        put_unit(
            &mut store,
            7,
            "sub-1_ses-a",
            Some("sha256:aaaa"),
            &json!({}),
        );
        put_unit(&mut store, 7, "sub-2_ses-a", None, &json!({}));
        put_unit(&mut store, 7, "sub-3_ses-a", None, &json!({}));
        put_measure(
            &mut store,
            7,
            "sub-1_ses-a",
            1,
            "2026-01-01",
            None,
            "volume",
            10.0,
        );
        put_measure(
            &mut store,
            7,
            "sub-2_ses-a",
            2,
            "2026-01-01",
            None,
            "volume",
            20.0,
        );
        put_measure(
            &mut store,
            7,
            "sub-3_ses-a",
            3,
            "2026-01-01",
            None,
            "volume",
            30.0,
        );
        let doc = numbers_by_key(&mut store, 7).unwrap();
        assert_eq!(doc["keys"].as_array().unwrap().len(), 1, "{doc}");
        assert_eq!(doc["keys"][0]["comparability_key"], "sha256:aaaa", "{doc}");
        assert_eq!(doc["keys"][0]["measures"]["volume"]["count"], 1, "{doc}");
        assert_eq!(doc["unkeyed"], 2, "{doc}");
        assert_eq!(doc["unkeyed_measures"], 2, "{doc}");
        assert_eq!(
            doc["pooled"], true,
            "one key is not a pool across keys: {doc}"
        );
        assert_eq!(doc["measures"], 3, "{doc}");
        let csv = numbers_csv(&mut store, 7).unwrap();
        assert!(csv.starts_with("# unkeyed: 2 ("), "{csv}");
        assert!(!csv.contains("none"), "{csv}");
        assert_eq!(csv.matches(",volume,").count(), 3, "{csv}");
        assert!(csv.contains("7,sub-2_ses-a,,volume,20,"), "{csv}");
        // two real keys and one unkeyed: the pooling words count the keys
        put_unit(
            &mut store,
            7,
            "sub-4_ses-a",
            Some("sha256:bbbb"),
            &json!({}),
        );
        put_measure(
            &mut store,
            7,
            "sub-4_ses-a",
            4,
            "2026-01-01",
            None,
            "volume",
            40.0,
        );
        let doc = numbers_by_key(&mut store, 7).unwrap();
        assert!(
            doc["words"]
                .as_str()
                .unwrap()
                .starts_with("these results hold 2 comparability keys ("),
            "{doc}"
        );
        let csv = numbers_csv(&mut store, 7).unwrap();
        assert!(
            csv.contains("# these results hold 2 comparability keys ("),
            "{csv}"
        );
        assert!(csv.contains("# unkeyed: 2 ("), "{csv}");
    }
}
