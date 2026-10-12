// SPDX-License-Identifier: AGPL-3.0-only

//! Job type 3, check a result, at the door (record 55 H3; the build spec of
//! 2026-10-11, sections 5, 6, 8.3 to 8.5). The mechanism is the registry's
//! (`nils_registry::check` and the campaign it is); what lives here is what
//! a door adds to it: the results a check is made of (a run's output, named
//! ones, or a selection's newest), the panels each item shows, resolved
//! through the visit's main scans and frozen, the preview that writes
//! nothing, the item as the reader reads it, and a result checked directly.
//!
//! - **Sources.** `{run, output, which: all | unchecked | breached, sample?,
//!   seed?}`, `{derivatives: [ids]}` (at most 5,000), or `{selection,
//!   pipeline, output}`. A result whose link is `none` is left out and
//!   counted with why, never refused one by one; a result an open check
//!   holds is left out and counted; one whose parts are not read yet holds
//!   the making back (409) and its job is queued once.
//! - **Sealed means sealed** (record 48; record 55, the rulings of
//!   2026-10-12, item 6: a check never holds a sealed scan's result). A
//!   result made from a stack of a sample sealed now is left out before
//!   anything else is counted, and counted, never named. A check made
//!   before that holds one withholds its item from every rater and
//!   adjudicator without sealed:see: no claim offers it, the item door and
//!   an answer refuse it, and its close writes nothing of it.
//! - **Panels.** The scan the result was made on first, then each declared
//!   role through the visit's main scans (the picks the run's release read,
//!   or the pack's), skipping a role whose pick is the made-on scan and one
//!   whose pick is in another scanner frame; a visit without a pick for a
//!   role shows one panel fewer, counted by role.
//! - **No dates.** No door here carries a visit's day, a study or
//!   acquisition time, a subject code or a birth date; an item is its place
//!   in the campaign.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nils_ask::ast::Grain;
use nils_registry::Registry;
use nils_registry::campaign::{self, Items, New};
use nils_registry::check::{self as reg, CheckItem, Panel};
use nils_registry::derivative::{self, Derivative};
use nils_registry::schema::Type;
use nils_registry::store::{Param, Store};
use serde_json::{Value, json};

use crate::campaigns::campaign_err;
use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply};

/// The most results a source may name by id (section 8.3).
pub(crate) const DERIVATIVES_MAX: usize = 5_000;

/// The most items one check holds (section 8.3).
pub(crate) const ITEMS_MAX: usize = 50_000;

/// The parts an item's read carries inline; the rest through the parts door.
pub(crate) const PARTS_INLINE: usize = 2_000;

/// The panels a check shows when it names none: the made-on scan, then T1w
/// and FLAIR (section 5).
pub(crate) const DEFAULT_PANELS: [&str; 3] = [reg::MADE_ON, "t1w", "flair"];

/// What `GET /api/capabilities` says of campaigns: the questions, what a
/// close writes, and a check's limits and presets, so the desk detects them.
pub(crate) fn capability() -> Value {
    json!({
        "questions": campaign::KINDS,
        "closes_into": campaign::CLOSES_INTO,
        "check": {
            "grains": reg::GRAINS,
            "verdicts": reg::VERDICTS,
            "needs": reg::NEEDS,
            "reasons_max": reg::REASONS_MAX,
            "words_max": reg::WORDS_MAX,
            "points_max": reg::POINTS_MAX,
            "note_max": reg::NOTE_MAX,
            "panels_max": reg::PANELS_MAX,
            "panels_default": DEFAULT_PANELS,
            "derivatives_max": DERIVATIVES_MAX,
            "items_max": ITEMS_MAX,
            "parts_inline": PARTS_INLINE,
            "sources": ["run", "derivatives", "selection"],
            "which": ["all", "unchecked", "breached"],
            "presets": reg::presets(),
            "grants": {
                "make": ["campaigns:work", "pipelines:see"], "item": "campaigns:see",
                "direct": "review:work",
            },
            "doors": {
                "preview": "/api/campaigns/preview",
                "item": "/api/campaigns/{id}/items/{item}/check",
                "direct": "/api/derivatives/{id}/check",
                "withdraw": "/api/derivatives/{id}/check/withdraw",
            },
        },
    })
}

/// The word a role is shown as on a panel's label.
pub(crate) fn role_word(role: &str) -> String {
    match role {
        "t1w" => "T1w".into(),
        "t2w" => "T2w".into(),
        "flair" => "FLAIR".into(),
        "pd" | "pdw" => "PD".into(),
        "swi" => "SWI".into(),
        "dwi" => "DWI".into(),
        other => other.to_string(),
    }
}

fn bad(m: impl Into<String>) -> Reply {
    Reply::error(400, m)
}

// ------------------------------------------------------------ the plan

/// What a check's body comes to, before anything is written.
struct Plan {
    question: Value,
    part_set: String,
    grain: String,
    roles: Vec<String>,
    /// The source as given, with what was drawn (the sample's seed).
    source: Value,
    /// The results the source named, before any was left out.
    named: usize,
    /// Record 55, the rulings of 2026-10-12, item 6: how many of them were
    /// made from a stack of a sample sealed now and were left out first,
    /// never which; every count after is of the rest.
    left_out_sealed: usize,
    checked_already: usize,
    held: usize,
    pending: Vec<Derivative>,
    not_drawable: BTreeMap<String, usize>,
    /// The results to check, in order, each with its panels.
    chosen: Vec<(Derivative, Vec<Panel>, i64)>,
    panels_missing: BTreeMap<String, usize>,
    panels_other_frame: BTreeMap<String, usize>,
    handle: Option<i64>,
}

impl Plan {
    fn parts(&self) -> i64 {
        self.chosen.iter().map(|(_, _, n)| *n).sum()
    }

    fn items(&self) -> usize {
        if self.grain == "part" {
            self.parts() as usize
        } else {
            self.chosen.len()
        }
    }
}

/// The part sets a result is read as: its row's, where its parts are read,
/// else its output's declared ones, else one set, whole.
fn sets_known(store: &mut Store, d: &Derivative) -> Vec<(String, String)> {
    if let Some(list) = d.part_sets.as_array() {
        return list
            .iter()
            .filter_map(|s| {
                Some((
                    s["key"].as_str()?.to_string(),
                    s["kind"].as_str()?.to_string(),
                ))
            })
            .collect();
    }
    crate::derivatives::sets_of(store, d)
        .into_iter()
        .map(|s| (s.key, s.kind))
        .collect()
}

/// How many parts a result's set holds, as its row says.
fn count_of(d: &Derivative, set: &str) -> i64 {
    d.part_sets
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["key"] == set)
        .and_then(|s| s["count"].as_i64())
        .unwrap_or(0)
}

/// Why a result cannot be drawn, in words, or none where it can.
fn not_drawable(d: &Derivative) -> Option<String> {
    if d.grid_link.as_deref() != Some("none") && d.grid_stack_id.is_some() {
        return None;
    }
    let note = d
        .part_sets
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|s| s["note"].as_str().map(str::to_string));
    Some(match (note, d.grid_stack_id) {
        (Some(n), _) if d.grid.is_null() => n,
        (_, None) => "no scan is known whose grid it is on".to_string(),
        _ => "its scan is in another scanner frame".to_string(),
    })
}

/// The results of a run of one declared output, each a mask that is not
/// withdrawn, oldest first.
fn of_run_output(store: &mut Store, run: i64, output: &str) -> Result<Vec<Derivative>, Reply> {
    let desc = crate::derivatives::descriptor_of_run(store, run)
        .ok_or_else(|| Reply::error(404, format!("source.run: no pipeline run {run}")))?;
    let Some(o) = desc.outputs.iter().find(|o| o.id == output) else {
        return Err(bad(format!(
            "source.output: run {run}'s pipeline declares no output {output}; it declares {}",
            desc.outputs
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };
    if o.kind != "mask" {
        return Err(bad(format!(
            "source.output: {output} is {}; a check is of a mask",
            nils_registry::a_or_an(&o.kind)
        )));
    }
    let mut rows = derivative::list(
        store,
        &derivative::Filter {
            kind: Some("mask"),
            run_id: Some(run),
            limit: 1_000_000,
            ..derivative::Filter::default()
        },
    )
    .map_err(|e| Reply::error(500, e.to_string()))?;
    rows.retain(|d| {
        d.withdrawn_at.is_none()
            && crate::derivatives::output_in(&desc, &d.path).as_deref() == Some(output)
    });
    rows.sort_by_key(|d| d.id);
    Ok(rows)
}

/// The declared part sets of a run's output: key and kind; one set, whole,
/// where it declares none.
fn declared_sets(store: &mut Store, run: i64, output: &str) -> Vec<(String, String)> {
    let declared: Vec<(String, String)> = crate::derivatives::descriptor_of_run(store, run)
        .and_then(|desc| {
            desc.outputs.iter().find(|o| o.id == output).map(|o| {
                o.parts
                    .iter()
                    .map(|p| (p.key.clone(), p.kind.clone()))
                    .collect()
            })
        })
        .unwrap_or_default();
    if declared.is_empty() {
        vec![(
            nils_pipeline::descriptor::WHOLE.to_string(),
            "whole".to_string(),
        )]
    } else {
        declared
    }
}

/// The stacks, and the subjects with their day, of the units a run's
/// declared checks flagged.
type Breached = (BTreeSet<i64>, BTreeSet<(i64, Option<String>)>);

/// The main scans of visits, by (subject, day, role): the stacks picked.
type Picks = BTreeMap<(i64, String, String), Vec<i64>>;

/// The part sets an output declares, each its key and kind.
type Sets = Vec<(String, String)>;

/// The units a run's declared checks flagged: `pipeline:qc` items of the
/// run with status `breach`, each as its stack, or its subject and day.
fn breached_units(store: &mut Store, run: i64) -> Result<Breached, Reply> {
    let d = store.dialect();
    let t = nils_registry::schema::table("review_item");
    let sql = format!(
        "SELECT {}, {} FROM {} WHERE kind = {} AND group_key LIKE {}",
        d.text_of(t.column("ref").expect("ref")),
        d.text_of(t.column("evidence").expect("evidence")),
        store.qualified("review_item"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let mut stacks = BTreeSet::new();
    let mut visits = BTreeSet::new();
    for r in store.query(
        &sql,
        &[
            Param::from(nils_registry::review::PIPELINE_QC_KIND),
            Param::from(format!("run:{run}|%")),
        ],
    )? {
        let parse = |t: Option<&str>| {
            t.and_then(|t| serde_json::from_str::<Value>(t).ok())
                .unwrap_or(Value::Null)
        };
        let (reference, evidence) = (parse(r.opt_text(0)?), parse(r.opt_text(1)?));
        if evidence["status"] != "breach" {
            continue;
        }
        if let Some(s) = reference["stack_id"].as_i64() {
            stacks.insert(s);
        } else if let Some(subject) = reference["subject_id"].as_i64() {
            visits.insert((
                subject,
                reference["session_day"]
                    .as_str()
                    .map(|d| d.chars().take(10).collect()),
            ));
        }
    }
    Ok((stacks, visits))
}

/// A stack's frame and visit: its series' frame of reference, and its
/// subject and session's day where the session cache knows them.
#[derive(Debug, Clone, Default)]
struct Visit {
    frame: Option<String>,
    subject: Option<i64>,
    day: Option<String>,
}

fn visits_of(store: &mut Store, stacks: &[i64]) -> Result<BTreeMap<i64, Visit>, Reply> {
    let d = store.dialect();
    let mut out: BTreeMap<i64, Visit> = BTreeMap::new();
    let mut study_of: BTreeMap<i64, i64> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let list = ids(chunk);
        let sql = format!(
            "SELECT st.id, se.frame_of_reference_uid, se.subject_id, se.study_id FROM {} st \
             JOIN {} se ON se.id = st.series_id WHERE st.id IN ({list})",
            store.qualified("stack"),
            store.qualified("series"),
        );
        for r in store.query(&sql, &[])? {
            let id = r.int(0)?;
            out.insert(
                id,
                Visit {
                    frame: r
                        .opt_text(1)?
                        .map(str::trim)
                        .filter(|f| !f.is_empty())
                        .map(str::to_string),
                    subject: r.opt_int(2)?,
                    day: None,
                },
            );
            if let Some(study) = r.opt_int(3)? {
                study_of.insert(id, study);
            }
        }
    }
    let studies: Vec<i64> = study_of
        .values()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut day_of: BTreeMap<(i64, i64), String> = BTreeMap::new();
    let first = d.text_of_qualified(
        Some("sc"),
        nils_registry::schema::table("session_cache")
            .column("first")
            .expect("first"),
    );
    for chunk in studies.chunks(500) {
        let sql = format!(
            "SELECT cs.study_id, sc.subject_id, {first} FROM {} cs JOIN {} sc ON sc.id = cs.session_id \
             WHERE cs.study_id IN ({})",
            store.qualified("session_cache_study"),
            store.qualified("session_cache"),
            ids(chunk),
        );
        for r in store.query(&sql, &[])? {
            if let Some(f) = r.opt_text(2)? {
                day_of
                    .entry((r.int(0)?, r.int(1)?))
                    .or_insert_with(|| f.chars().take(10).collect());
            }
        }
    }
    for (stack, v) in out.iter_mut() {
        if let (Some(study), Some(subject)) = (study_of.get(stack), v.subject) {
            v.day = day_of.get(&(*study, subject)).cloned();
        }
    }
    Ok(out)
}

fn ids(list: &[i64]) -> String {
    list.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The main scans of these subjects' visits for some roles, as a reading
/// of the picks says them: by (subject, day, role), the stacks picked.
fn picks_of(
    store: &mut Store,
    subjects: &[i64],
    roles: &[String],
    of: &nils_registry::pick_truth::Of,
) -> Result<Picks, Reply> {
    let mut out: Picks = BTreeMap::new();
    if roles.is_empty() {
        return Ok(out);
    }
    let d = store.dialect();
    // one place of truth (2026-10-11): the picks as every reader reads
    // them, through the resolver's relation
    let picks = of.picks(store);
    let quoted = roles
        .iter()
        .map(|r| format!("'{}'", r.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let day = d.text_of_qualified(
        Some("p"),
        nils_registry::schema::table("pick")
            .column("session_day")
            .expect("pick.session_day"),
    );
    for chunk in subjects.chunks(500) {
        let sql = format!(
            "SELECT p.subject_id, {day}, p.role, ps.stack_id FROM {picks} p JOIN {} ps ON ps.pick_id = p.id \
             WHERE p.role IN ({quoted}) AND p.subject_id IN ({}) \
             ORDER BY ps.stack_id",
            store.qualified("pick_stack"),
            ids(chunk),
        );
        for r in store.query(&sql, &[])? {
            let key = (
                r.int(0)?,
                r.text(1)?.chars().take(10).collect::<String>(),
                r.text(2)?.to_string(),
            );
            let list = out.entry(key).or_default();
            let s = r.int(3)?;
            if !list.contains(&s) {
                list.push(s);
            }
        }
    }
    Ok(out)
}

/// The roles the served pack's picks know.
fn known_roles(doors: &Doors) -> Option<BTreeSet<String>> {
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack)?;
    Some(
        pack.picks
            .iter()
            .flat_map(|m| m.roles.iter().cloned())
            .collect(),
    )
}

/// The panels a body asks for, held to section 5: the made-on scan first
/// (added where it is not named), each role once and one the pack knows, at
/// most eight.
fn roles_of(doors: &Doors, v: &Value) -> Result<Vec<String>, Reply> {
    let named: Vec<String> = match v {
        Value::Null => DEFAULT_PANELS.iter().map(|s| s.to_string()).collect(),
        Value::Array(list) => list
            .iter()
            .map(|r| {
                r.as_str()
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| bad("panels: a list of roles, made_on first"))
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(bad("panels: a list of roles, made_on first")),
    };
    let mut roles: Vec<String> = Vec::new();
    for r in named {
        if r == reg::MADE_ON {
            continue;
        }
        if roles.contains(&r) {
            return Err(bad(format!("panels names {r} twice")));
        }
        roles.push(r);
    }
    if roles.len() + 1 > reg::PANELS_MAX {
        return Err(bad(format!(
            "panels: at most {}, the made-on scan included; {} asked",
            reg::PANELS_MAX,
            roles.len() + 1
        )));
    }
    if let Some(known) = known_roles(doors)
        && let Some(r) = roles.iter().find(|r| !known.contains(*r))
    {
        return Err(bad(format!(
            "panels: {r} is not a role of the pack's picks: {}",
            known.iter().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(roles)
}

/// Read a check's body into what it would make, writing nothing but the
/// freeze of a selection a source names.
fn plan(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    doc: &Value,
) -> Result<Plan, Reply> {
    let question = doc["question"].clone();
    let q = campaign::Question::parse(&question).map_err(campaign_err)?;
    let campaign::Question::Check {
        part_set, grain, ..
    } = &q
    else {
        return Err(bad("question.kind: a check's question is check"));
    };
    let (part_set, grain) = (part_set.clone(), grain.clone());
    let roles = roles_of(doors, &doc["panels"])?;
    let source = &doc["source"];
    let mut recorded = source.clone();
    let mut handle = None;
    // the results the source names, and the sets they are read as
    let (mut results, sets): (Vec<Derivative>, Option<Vec<(String, String)>>) = if let Some(run) =
        source["run"].as_i64()
    {
        let output = source["output"]
            .as_str()
            .ok_or_else(|| bad("source.output: the run's output the results are"))?;
        let rows = of_run_output(registry.store(), run, output)?;
        (rows, Some(declared_sets(registry.store(), run, output)))
    } else if let Some(list) = source["derivatives"].as_array() {
        if list.len() > DERIVATIVES_MAX {
            return Err(bad(format!(
                "source.derivatives: at most {DERIVATIVES_MAX}; {} named",
                list.len()
            )));
        }
        let mut rows = Vec::new();
        let mut seen = BTreeSet::new();
        for v in list {
            let id = v
                .as_i64()
                .ok_or_else(|| bad("source.derivatives: derivative ids"))?;
            if !seen.insert(id) {
                continue;
            }
            let d = derivative::get(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| bad(format!("source.derivatives: no derivative {id}")))?;
            if d.kind != "mask" {
                return Err(bad(format!(
                    "source.derivatives: derivative {id} is {}; a check is of a mask",
                    nils_registry::a_or_an(&d.kind)
                )));
            }
            if d.withdrawn_at.is_some() {
                return Err(bad(format!(
                    "source.derivatives: derivative {id} is withdrawn"
                )));
            }
            rows.push(d);
        }
        (rows, None)
    } else if let Some(spec) = source["selection"].as_str() {
        let (pipeline, output) = (
            source["pipeline"]
                .as_str()
                .ok_or_else(|| bad("source.pipeline: the pipeline whose results are checked"))?,
            source["output"]
                .as_str()
                .ok_or_else(|| bad("source.output: the pipeline's output the results are"))?,
        );
        // a preview freezes the selection too: the handle is the ask's
        // record of what the caller read, as every freeze leaves one
        let (h, keys) =
            crate::campaigns::freeze_at_door(doors, registry, ask, caller, spec, Grain::Stack)?;
        handle = Some(h);
        let stacks: Vec<i64> = keys.iter().map(|(k, _)| *k).collect();
        let (rows, sets) = of_selection(registry.store(), &stacks, pipeline, output)?;
        (rows, Some(sets))
    } else {
        return Err(bad(
            "source: {run, output, which}, {derivatives: [ids]} or {selection, pipeline, output}",
        ));
    };
    // record 55, the rulings of 2026-10-12, item 6: a check never holds a
    // sealed scan's result. The results made from a stack of a sample sealed
    // now, as the derivative doors read what each was made from, are left
    // out first and counted, never named, so no count after (checked,
    // breached, held, drawable) says anything of them
    let named = results.len();
    let sealed = crate::derivatives::sealed_among(registry.store(), &results)
        .map_err(|e| Reply::error(500, e))?;
    results.retain(|d| !sealed.contains(&d.id));
    let left_out_sealed = named - results.len();
    // the part set and the grain, against what the output declares or what
    // each result is read as
    let check_set = |sets: &[(String, String)], whose: &str| -> Result<(), Reply> {
        let Some((_, kind)) = sets.iter().find(|(k, _)| *k == part_set) else {
            return Err(bad(format!(
                "question.part_set: {whose} is read as {}; it has no set {part_set}",
                sets.iter()
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        if grain == "part" && kind != "components" {
            return Err(bad(format!(
                "question.grain: part is for a set of components; {part_set} is {kind}"
            )));
        }
        Ok(())
    };
    match &sets {
        Some(s) => check_set(s, "the output")?,
        None => {
            // a result that cannot be drawn is counted below, whatever
            // set it was read as
            for d in results
                .iter()
                .filter(|d| d.grid_link.as_deref() != Some("none"))
            {
                let s = sets_known(registry.store(), d);
                check_set(&s, &format!("result {}", d.id))?;
            }
        }
    }
    let checked_already = results.iter().filter(|d| d.check_status.is_some()).count();
    // which of them
    match source["which"].as_str() {
        None | Some("all") => {}
        Some("unchecked") => results.retain(|d| d.check_status.is_none()),
        Some("breached") => {
            let run = source["run"]
                .as_i64()
                .ok_or_else(|| bad("source.which: breached is of a run's results"))?;
            let (stacks, visits) = breached_units(registry.store(), run)?;
            results.retain(|d| {
                d.stack_id.is_some_and(|s| stacks.contains(&s))
                    || d.subject_id.is_some_and(|s| {
                        visits.contains(&(
                            s,
                            d.session_day.as_ref().map(|x| x.chars().take(10).collect()),
                        )) || visits.contains(&(s, None))
                    })
            });
        }
        Some(other) => {
            return Err(bad(format!(
                "source.which: all, unchecked or breached, not {other}"
            )));
        }
    }
    // left out: held by an open check, parts not read yet, not drawable
    let held_set = reg::held_among(
        registry.store(),
        &results.iter().map(|d| d.id).collect::<Vec<_>>(),
    )
    .map_err(campaign_err)?;
    let held = results.iter().filter(|d| held_set.contains(&d.id)).count();
    results.retain(|d| !held_set.contains(&d.id));
    let mut pending = Vec::new();
    let mut not_drawable_by: BTreeMap<String, usize> = BTreeMap::new();
    let mut drawable = Vec::new();
    for d in results {
        if d.part_sets.is_null() || d.grid_link.is_none() {
            pending.push(d);
        } else if let Some(why) = not_drawable(&d) {
            *not_drawable_by.entry(why).or_default() += 1;
        } else {
            drawable.push(d);
        }
    }
    // a sample, drawn by the seed and recorded
    if let Some(n) = source["sample"].as_u64() {
        let seed = match source["seed"].as_str() {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => {
                use ring::rand::SecureRandom;
                let mut b = [0u8; 16];
                ring::rand::SystemRandom::new()
                    .fill(&mut b)
                    .map_err(|_| Reply::error(500, "no seed could be drawn"))?;
                hex::encode(b)
            }
        };
        drawable.sort_by_key(|d| crate::campaigns::sha256(format!("{seed}:{}", d.id).as_bytes()));
        drawable.truncate(n as usize);
        drawable.sort_by_key(|d| d.id);
        recorded["seed"] = json!(seed);
    } else if !source["sample"].is_null() {
        return Err(bad("source.sample: a number of results"));
    }
    // the panels of each, through the visits' main scans
    let (chosen, panels_missing, panels_other_frame) =
        panels_for(registry.store(), &drawable, &roles, &part_set)?;
    Ok(Plan {
        question,
        part_set,
        grain,
        roles,
        source: recorded,
        named,
        left_out_sealed,
        checked_already,
        held,
        pending,
        not_drawable: not_drawable_by,
        chosen,
        panels_missing,
        panels_other_frame,
        handle,
    })
}

/// The newest result of a pipeline's output for each scan, visit or subject
/// of a selection's stacks: not withdrawn, superseded by none.
fn of_selection(
    store: &mut Store,
    stacks: &[i64],
    pipeline: &str,
    output: &str,
) -> Result<(Vec<Derivative>, Sets), Reply> {
    let versions: Vec<i64> = nils_registry::pipeline::list(store)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .into_iter()
        .filter(|p| p.name == pipeline)
        .map(|p| p.id)
        .collect();
    if versions.is_empty() {
        return Err(bad(format!("source.pipeline: no pipeline {pipeline}")));
    }
    let sql = format!(
        "SELECT id FROM {} WHERE pipeline_id IN ({}) ORDER BY id",
        store.qualified("pipeline_run"),
        ids(&versions)
    );
    let runs: Vec<i64> = store
        .query(&sql, &[])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    let visits = visits_of(store, stacks)?;
    let wanted: BTreeSet<i64> = stacks.iter().copied().collect();
    let sessions: BTreeSet<(i64, String)> = visits
        .values()
        .filter_map(|v| Some((v.subject?, v.day.clone()?)))
        .collect();
    let subjects: BTreeSet<i64> = visits.values().filter_map(|v| v.subject).collect();
    let mut all: Vec<Derivative> = Vec::new();
    let mut sets = None;
    for run in runs {
        let desc = crate::derivatives::descriptor_of_run(store, run);
        let Some(desc) = desc else { continue };
        if !desc
            .outputs
            .iter()
            .any(|o| o.id == output && o.kind == "mask")
        {
            continue;
        }
        if sets.is_none() {
            sets = Some(declared_sets(store, run, output));
        }
        for d in derivative::list(
            store,
            &derivative::Filter {
                kind: Some("mask"),
                run_id: Some(run),
                limit: 1_000_000,
                ..derivative::Filter::default()
            },
        )
        .map_err(|e| Reply::error(500, e.to_string()))?
        {
            if crate::derivatives::output_in(&desc, &d.path).as_deref() == Some(output) {
                all.push(d);
            }
        }
    }
    let Some(sets) = sets else {
        return Err(bad(format!(
            "source.output: no version of {pipeline} declares a mask output {output}"
        )));
    };
    let superseded: BTreeSet<i64> = all.iter().filter_map(|d| d.supersedes_id).collect();
    let mut newest: BTreeMap<String, Derivative> = BTreeMap::new();
    for d in all {
        if d.withdrawn_at.is_some() || superseded.contains(&d.id) {
            continue;
        }
        let day = d
            .session_day
            .as_ref()
            .map(|x| x.chars().take(10).collect::<String>());
        let unit = match (d.stack_id, d.subject_id, day) {
            (Some(s), ..) if d.scope == "stack" && wanted.contains(&s) => format!("stack:{s}"),
            (_, Some(s), Some(day)) if sessions.contains(&(s, day.clone())) => {
                format!("session:{s}:{day}")
            }
            (_, Some(s), None) if d.scope == "subject" && subjects.contains(&s) => {
                format!("subject:{s}")
            }
            _ => continue,
        };
        match newest.get(&unit) {
            Some(held) if held.id > d.id => {}
            _ => {
                newest.insert(unit, d);
            }
        }
    }
    let mut rows: Vec<Derivative> = newest.into_values().collect();
    rows.sort_by_key(|d| d.id);
    Ok((rows, sets))
}

/// Each result's panels, through the visits' main scans: the made-on scan,
/// then each role's pick of the visit in the made-on scan's frame. Answers
/// the results with their panels and their part count, and per role the
/// visits that had no pick, and those whose pick is in another frame.
type Panelled = (
    Vec<(Derivative, Vec<Panel>, i64)>,
    BTreeMap<String, usize>,
    BTreeMap<String, usize>,
);

fn panels_for(
    store: &mut Store,
    results: &[Derivative],
    roles: &[String],
    set: &str,
) -> Result<Panelled, Reply> {
    let made_on: Vec<i64> = results
        .iter()
        .filter_map(|d| d.grid_stack_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let visits = visits_of(store, &made_on)?;
    let subjects: Vec<i64> = visits
        .values()
        .filter_map(|v| v.subject)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // the picks read: what the run's release read, as it recorded it;
    // for a result of no release, the main scans of no scope, as every
    // reader of picks reads them (one place of truth, 2026-10-11)
    let mut readings: BTreeMap<Option<i64>, nils_registry::pick_truth::Of> = BTreeMap::new();
    for d in results {
        if readings.contains_key(&d.run_id) {
            continue;
        }
        let release = d.run_id.and_then(|run| {
            nils_registry::pipeline::run(store, run)
                .ok()
                .flatten()
                .and_then(|r| r.input_release_id)
        });
        let of = match release {
            Some(release) => crate::pipelines::release_reading(store, release)
                .map_err(|e| Reply::error(500, e))?,
            None => nils_registry::pick_truth::Of::own(store)?,
        };
        readings.insert(d.run_id, of);
    }
    let mut picks: BTreeMap<Option<i64>, Picks> = BTreeMap::new();
    let mut picked: BTreeSet<i64> = BTreeSet::new();
    for (run, of) in &readings {
        let p = picks_of(store, &subjects, roles, of)?;
        picked.extend(p.values().flatten().copied());
        picks.insert(*run, p);
    }
    let frames = reg::frames_of(store, &picked.into_iter().collect::<Vec<_>>())?;
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut other_frame: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::with_capacity(results.len());
    for d in results {
        let Some(grid) = d.grid_stack_id else {
            continue;
        };
        let link = d.grid_link.clone().unwrap_or_else(|| "world".into());
        let mut panels = vec![Panel {
            panel: 0,
            role: reg::MADE_ON.to_string(),
            stack: grid,
            link,
        }];
        let v = visits.get(&grid).cloned().unwrap_or_default();
        let of_run = picks.get(&d.run_id);
        for role in roles {
            let stacks: Vec<i64> = match (v.subject, &v.day, of_run) {
                (Some(s), Some(day), Some(p)) => p
                    .get(&(s, day.clone(), role.clone()))
                    .cloned()
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            // the made-on scan is panel 0 already
            if stacks.contains(&grid) {
                continue;
            }
            if stacks.is_empty() {
                *missing.entry(role.clone()).or_default() += 1;
                continue;
            }
            let same = stacks
                .iter()
                .find(|s| v.frame.is_some() && frames.get(s).cloned().flatten() == v.frame);
            match same {
                Some(s) if !panels.iter().any(|p| p.stack == *s) => {
                    panels.push(Panel {
                        panel: panels.len() as i64,
                        role: role.clone(),
                        stack: *s,
                        link: "world".to_string(),
                    });
                }
                Some(_) => {}
                None => {
                    *other_frame.entry(role.clone()).or_default() += 1;
                    *missing.entry(role.clone()).or_default() += 1;
                }
            }
        }
        out.push((d.clone(), panels, count_of(d, set)));
    }
    Ok((out, missing, other_frame))
}

/// What is not built yet of a plan's pictures and layers: the panels'
/// scans without a picture, and (result, scan) pairs without the set's
/// layer.
fn to_build(store: &mut Store, p: &Plan) -> (usize, usize, usize, Option<String>) {
    let Ok(w) = crate::pyramid::working_place_cached(store) else {
        let stacks: BTreeSet<i64> = p
            .chosen
            .iter()
            .flat_map(|(_, ps, _)| ps.iter().map(|x| x.stack))
            .collect();
        let pairs: usize = p.chosen.iter().map(|(_, ps, _)| ps.len()).sum();
        return (stacks.len(), pairs, pairs, None);
    };
    let root = Path::new(&w.path);
    let stacks: BTreeSet<i64> = p
        .chosen
        .iter()
        .flat_map(|(_, ps, _)| ps.iter().map(|x| x.stack))
        .collect();
    let pictures = stacks
        .iter()
        .filter(|s| {
            !crate::pyramid::dir(root, **s)
                .join("manifest.json")
                .exists()
        })
        .count();
    let mut pairs = 0;
    let mut layers = 0;
    for (d, ps, _) in &p.chosen {
        for x in ps {
            pairs += 1;
            if !crate::mask::layer_dir(root, x.stack, &d.sha256, &p.part_set)
                .join("manifest.json")
                .exists()
            {
                layers += 1;
            }
        }
    }
    (pictures, pairs, layers, Some(w.name))
}

/// The command that reads a result's parts: its run's, or its own.
fn parts_command(d: &Derivative) -> Vec<String> {
    match d.run_id {
        Some(run) => vec![
            "derivative".into(),
            "parts".into(),
            "--run".into(),
            run.to_string(),
        ],
        None => vec!["derivative".into(), "parts".into(), d.id.to_string()],
    }
}

/// Queue the job that reads the parts of the results waiting for it, once:
/// one per run, and one per result without a run. A result whose job
/// failed is not waited for again: it is counted as one that cannot be
/// drawn, with why, and left out (the review of E1, item 2). Answers the
/// jobs queued or running.
fn queue_parts(store: &mut Store, caller: &Caller, p: &mut Plan) -> Vec<i64> {
    let commands: BTreeSet<Vec<String>> = p.pending.iter().map(parts_command).collect();
    let mut jobs = Vec::new();
    let mut failed: BTreeMap<Vec<String>, i64> = BTreeMap::new();
    for c in commands {
        let name = format!("parts: {}", c[2..].join(" "));
        match crate::derivatives::made_by_job(
            store,
            caller,
            c.clone(),
            &name,
            "the parts of a result",
        ) {
            Ok(r) => jobs.extend(r.body["job"].as_i64()),
            Err(r) if r.status == 422 => {
                failed.insert(c, r.body["job"].as_i64().unwrap_or_default());
            }
            Err(_) => {}
        }
    }
    let pending = std::mem::take(&mut p.pending);
    for d in pending {
        match failed.get(&parts_command(&d)) {
            Some(job) => {
                *p.not_drawable
                    .entry(format!("its parts could not be read; job {job} says why"))
                    .or_default() += 1;
            }
            None => p.pending.push(d),
        }
    }
    jobs
}

/// What a plan counts, as the preview answers it (section 8.3).
fn counts(store: &mut Store, p: &Plan, parts_jobs: &[i64]) -> Value {
    let (pictures, pairs, layers, place) = to_build(store, p);
    let panels: Vec<usize> = p.chosen.iter().map(|(_, ps, _)| ps.len()).collect();
    json!({
        "results": p.named,
        "items": p.items(),
        "parts": p.parts(),
        "drawable": p.chosen.len(),
        "not_drawable": p.not_drawable.iter().map(|(why, n)| json!({"why": why, "count": n})).collect::<Vec<_>>(),
        "left_out_sealed": p.left_out_sealed,
        "checked_already": p.checked_already,
        "held": p.held,
        "parts_pending": p.pending.len(),
        "parts_jobs": parts_jobs,
        "panels_missing": p.panels_missing,
        "panels_other_frame": p.panels_other_frame,
        "panels": {
            "roles": std::iter::once(reg::MADE_ON.to_string()).chain(p.roles.iter().cloned()).collect::<Vec<_>>(),
            "most": panels.iter().max(), "fewest": panels.iter().min(),
        },
        "pictures_to_build": pictures,
        "layers_to_build": layers,
        "layers_asked": pairs,
        "place": place,
        "grain": p.grain,
        "part_set": p.part_set,
    })
}

fn make_grant(caller: &Caller) -> Result<(), Reply> {
    caller.allowed("making a check", Need::One("pipelines:see"), Detail::Plain)
}

/// `POST /api/campaigns/preview`: what a check's body would make, written
/// nowhere. Results whose parts are not read yet are counted, and their
/// job is queued once.
pub(crate) fn preview(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    doc: &Value,
) -> Result<Reply, Reply> {
    make_grant(caller)?;
    let started = std::time::Instant::now();
    let mut p = plan(doors, registry, ask, caller, doc)?;
    if p.items() > ITEMS_MAX {
        return Err(bad(format!(
            "source: {} items; a check holds at most {ITEMS_MAX}",
            p.items()
        )));
    }
    let jobs = queue_parts(registry.store(), caller, &mut p);
    let mut v = counts(registry.store(), &p, &jobs);
    v["seconds"] = json!((started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0);
    if !p.source["seed"].is_null() {
        v["seed"] = p.source["seed"].clone();
    }
    Ok(Reply::ok(v))
}

/// The content hash of a check: what each item shows, the result by its
/// digest, its part and its panels, under the question.
fn content_hash(question: &Value, items: &[(String, &CheckItem)]) -> String {
    let doc = json!({
        "question": question,
        "items": items.iter().map(|(sha, it)| json!({
            "result": sha, "set": it.part_set, "part": it.part_label,
            "panels": it.panels.iter().map(|p| json!([p.role, p.stack, p.link])).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    crate::campaigns::sha256(doc.to_string().as_bytes())
}

/// `POST /api/campaigns` with a check question (section 8.3): the plan's
/// items made into a campaign, their panels frozen, and one job warming the
/// pictures and the layers.
pub(crate) fn create(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    doc: &Value,
) -> Result<Value, Reply> {
    make_grant(caller)?;
    let name = doc["name"]
        .as_str()
        .ok_or_else(|| bad("name: what the check is called"))?;
    let mut p = plan(doors, registry, ask, caller, doc)?;
    let jobs = queue_parts(registry.store(), caller, &mut p);
    if !p.pending.is_empty() {
        let mut r = Reply::error(
            409,
            format!(
                "{} result(s) are still being read into their parts; the check is made once they are (jobs {})",
                p.pending.len(),
                ids(&jobs)
            ),
        );
        r.body["parts_pending"] = json!(p.pending.len());
        r.body["parts_jobs"] = json!(jobs);
        return Err(r);
    }
    if p.items() > ITEMS_MAX {
        return Err(bad(format!(
            "source: {} items; a check holds at most {ITEMS_MAX}",
            p.items()
        )));
    }
    if p.chosen.is_empty() {
        let sealed = if p.left_out_sealed > 0 {
            format!("{} left out: sealed for certification, ", p.left_out_sealed)
        } else {
            String::new()
        };
        return Err(bad(format!(
            "the source leaves no result to check: {} named, {sealed}{} checked already, {} held by an open check, {} can't be drawn",
            p.named,
            p.checked_already,
            p.held,
            p.not_drawable.values().sum::<usize>()
        )));
    }
    // the items: a result each, or each part of a result in its order
    let mut items: Vec<CheckItem> = Vec::new();
    let mut shas: Vec<String> = Vec::new();
    if p.grain == "part" {
        let labels = part_labels(
            registry.store(),
            &p.chosen.iter().map(|(d, ..)| d.id).collect::<Vec<_>>(),
            &p.part_set,
        )?;
        for (d, panels, _) in &p.chosen {
            for l in labels.get(&d.id).into_iter().flatten() {
                items.push(CheckItem {
                    derivative_id: d.id,
                    part_set: p.part_set.clone(),
                    part_label: Some(*l),
                    panels: panels.clone(),
                });
                shas.push(d.sha256.clone());
            }
        }
        if items.is_empty() {
            return Err(bad(format!(
                "the results hold no part in their set {}; check them whole",
                p.part_set
            )));
        }
    } else {
        for (d, panels, _) in &p.chosen {
            items.push(CheckItem {
                derivative_id: d.id,
                part_set: p.part_set.clone(),
                part_label: None,
                panels: panels.clone(),
            });
            shas.push(d.sha256.clone());
        }
    }
    let hash = content_hash(
        &p.question,
        &shas.iter().cloned().zip(items.iter()).collect::<Vec<_>>(),
    );
    let mut source = p.source.clone();
    // what the check is made of, counted; never which role a panel shows,
    // which on a sealed scan is a system's answer withheld from its raters
    // (section 5.6, the review of E2, item 9): the item door says it panel
    // by panel where it may
    let mut kept = counts(registry.store(), &p, &[]);
    if let Some(m) = kept.as_object_mut() {
        for k in ["panels", "panels_missing", "panels_other_frame"] {
            m.remove(k);
        }
    }
    source["counts"] = kept;
    let pack_version = crate::campaigns::pack_version_of(doors);
    let adjudication = if doc["adjudication"].is_null() {
        json!({})
    } else {
        doc["adjudication"].clone()
    };
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    let stacks: Vec<i64> = items
        .iter()
        .flat_map(|i| i.panels.iter().map(|p| p.stack))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let made = campaign::create(
        registry,
        &New {
            name,
            owner: &caller.principal,
            question: &p.question,
            source,
            items: Items::Checks(items),
            handle_id: p.handle,
            content_hash: Some(&hash),
            pack_version: pack_version.as_deref(),
            raters_per_item: doc["raters_per_item"].as_i64().unwrap_or(1),
            raters: strings(&doc["raters"]),
            adjudicators: strings(&doc["adjudicators"]),
            adjudication: &adjudication,
            closes_into: doc["closes_into"].as_str().unwrap_or("check"),
            lease_seconds: doc["lease_seconds"].as_i64().unwrap_or(3600),
            inputs: BTreeMap::new(),
            hold_back: None,
            // a check shows the result and the scans; nothing is suggested
            suggest: Some(campaign::Suggest::None),
            hide_header: false,
        },
    )
    .map_err(campaign_err)?;
    // one job warms every panel's picture and the result's layer on it
    let warm = crate::derivatives::made_by_job(
        registry.store(),
        caller,
        vec![
            "derivative".into(),
            "layer".into(),
            "--campaign".into(),
            made.id.to_string(),
        ],
        &format!("check {}", made.name),
        &format!("the layers of check {}", made.name),
    )
    .ok()
    .and_then(|r| r.body["job"].as_i64());
    // the item door reads the warming job by the id kept with the check
    if let Some(j) = warm {
        let mut source = made.source.clone();
        source["warm_job"] = json!(j);
        registry
            .store()
            .update_by_id(
                nils_registry::schema::table("campaign"),
                &[("source", Param::from(source.to_string()))],
                "id",
                made.id,
            )
            .map_err(|e| Reply::error(500, e.to_string()))?;
    }
    let made = campaign::get(registry.store(), made.id)
        .map_err(campaign_err)?
        .ok_or_else(|| Reply::error(500, "the check was not written back"))?;
    let (pictures, pairs, layers, place) = to_build(registry.store(), &p);
    let mut v = crate::campaigns::shown(registry.store(), &made)?;
    v["pictures"] = json!({
        "stacks": stacks.len(), "missing": pictures, "have": stacks.len() - pictures,
        "place": place, "job": warm,
    });
    v["layers"] = json!({"pairs": pairs, "missing": layers, "have": pairs - layers, "job": warm});
    Ok(v)
}

/// The labels of each result's set, in the set's order.
fn part_labels(
    store: &mut Store,
    results: &[i64],
    set: &str,
) -> Result<BTreeMap<i64, Vec<i64>>, Reply> {
    let mut out: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let d = store.dialect();
    for chunk in results.chunks(500) {
        let sql = format!(
            "SELECT derivative_id, label FROM {} WHERE part_set = {} AND derivative_id IN ({}) \
             ORDER BY derivative_id, position, label",
            store.qualified("derivative_part"),
            d.param(1, Type::Text),
            ids(chunk)
        );
        for r in store.query(&sql, &[Param::from(set)])? {
            out.entry(r.int(0)?).or_default().push(r.int(1)?);
        }
    }
    Ok(out)
}

// ------------------------------------------------------------ the item

/// What a panel shows, in words: its role, or for the made-on scan the role
/// the visit's picks give it; on a scan of a sample sealed now, or where no
/// role is known, the series description, the file's own text (section
/// 5.6).
fn shows(store: &mut Store, role: &str, stack: i64, sealed: bool) -> Result<String, Reply> {
    if !sealed {
        if role != reg::MADE_ON {
            return Ok(role_word(role));
        }
        // the role the page and the chat would say: the main scans of no
        // scope, one place of truth (2026-10-11)
        let of = nils_registry::pick_truth::Of::own(store)?;
        let picked =
            crate::preflight::pick_roles(store, &[stack], &of).map_err(|e| Reply::error(500, e))?;
        if let Some(r) = picked.get(&stack).and_then(|p| p.roles.iter().next()) {
            return Ok(role_word(r));
        }
    }
    let sql = format!(
        "SELECT text_series_description FROM {} WHERE stack_id = {}",
        store.qualified("stack_fingerprint"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(stack)])?
        .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| format!("scan {stack}")))
}

/// Whether a result's layer on a scan is built, building or missing.
fn layer_state(
    store: &mut Store,
    d: &Derivative,
    set: &str,
    stack: i64,
    warming: bool,
) -> &'static str {
    let Ok(w) = crate::pyramid::working_place_cached(store) else {
        return "missing";
    };
    if crate::mask::layer_dir(Path::new(&w.path), stack, &d.sha256, set)
        .join("manifest.json")
        .exists()
    {
        "ready"
    } else if warming {
        "building"
    } else {
        "missing"
    }
}

/// The job warming a check's layers, where it is queued or running: the
/// one its making recorded, read by its id.
fn warming(store: &mut Store, c: &campaign::Campaign) -> Option<i64> {
    use nils_registry::job::{self, State};
    let id = c.source["warm_job"].as_i64()?;
    job::show(store, id)
        .ok()
        .flatten()
        .filter(|j| matches!(j.state, State::Queued | State::Running | State::Cancelling))
        .map(|j| j.id)
}

/// `GET /api/campaigns/{id}/items/{item}/check` (section 8.4): everything
/// the reader needs of one item in one read, for its rater or adjudicator.
/// No subject code, no day, no time; a sealed scan's panel says its series
/// description instead of its role, and an item whose result was made from
/// a sealed scan is refused ([`refuse_sealed`]).
pub(crate) fn item_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
    item: i64,
) -> Result<Reply, Reply> {
    if !reg::is_check(c) {
        return Err(Reply::error(
            409,
            format!("campaign {} is no check", c.name),
        ));
    }
    let principal = caller.principal.as_str();
    // section 8.4: a rater or an adjudicator of the check reads its item,
    // and nobody else, a reviewer included (the review of E2, item 17)
    let mine_a =
        campaign::assignments_of(registry.store(), item, principal).map_err(campaign_err)?;
    if mine_a.is_empty() && !crate::campaigns::rates_in(caller, c) {
        return Err(Reply::error(
            403,
            format!(
                "item {item} of check {} is read by its raters and adjudicators",
                c.name
            ),
        ));
    }
    // record 55, the rulings of 2026-10-12, item 6: an item whose result was
    // made from a stack of a sample sealed now is withheld; a holder of
    // sealed:see reads it, audited as sealed.read
    let unsealed_read = refuse_sealed(registry.store(), caller, c, item)?;
    let it = campaign::item(registry.store(), item)
        .map_err(campaign_err)?
        .ok_or_else(|| Reply::error(404, format!("no campaign item {item}")))?;
    let (Some(d_id), Some(set)) = (it.derivative_id, it.part_set.clone()) else {
        return Err(Reply::error(404, format!("item {item} names no result")));
    };
    let d = derivative::get(registry.store(), d_id)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .ok_or_else(|| Reply::error(404, format!("no derivative {d_id}")))?;
    let q = c.question().map_err(campaign_err)?;
    let (reasons, grain) = match &q {
        campaign::Question::Check { reasons, grain, .. } => {
            (reg::reasons_json(reasons), grain.clone())
        }
        _ => (json!([]), "result".into()),
    };
    let of = campaign::counts(registry.store(), c.id).map_err(campaign_err)?;
    let total: i64 = of["items"]
        .as_object()
        .map(|m| m.values().filter_map(Value::as_i64).sum())
        .unwrap_or(0);
    // the result: its pipeline, version, output and run, by its digest
    let (pipeline, version) = d
        .run_id
        .and_then(|r| {
            nils_registry::pipeline::run(registry.store(), r)
                .ok()
                .flatten()
        })
        .and_then(|r| {
            nils_registry::pipeline::get(registry.store(), r.pipeline_id)
                .ok()
                .flatten()
        })
        .map_or((None, None), |p| (Some(p.name), Some(p.version)));
    let output = crate::derivatives::output_of(registry.store(), &d).map(|(_, o)| o);
    let set_doc = d
        .part_sets
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["key"] == set.as_str())
        .cloned()
        .unwrap_or(Value::Null);
    let count = set_doc["count"].as_i64().unwrap_or(0);
    let (_, parts) = derivative::parts(
        registry.store(),
        d.id,
        &set,
        derivative::PartOrder::Position,
        0,
        PARTS_INLINE,
    )
    .map_err(|e| Reply::error(500, e.to_string()))?;
    let part = it
        .part_label
        .and_then(|l| parts.iter().find(|p| p.label == l).cloned())
        .map(|p| json!(p));
    // the panels, each with what it shows and whether its layer is there
    let panels = reg::panels_of(registry.store(), c.id, Some(item))?
        .remove(&item)
        .unwrap_or_default();
    let stacks: Vec<i64> = panels.iter().map(|p| p.stack).collect();
    let sealed = crate::campaigns::blind_among(registry.store(), caller, &stacks)?;
    let warm = warming(registry.store(), c);
    let mut shown_panels = Vec::new();
    for p in &panels {
        let s = sealed.contains(&p.stack);
        shown_panels.push(json!({
            "panel": p.panel, "stack": p.stack,
            "shows": shows(registry.store(), &p.role, p.stack, s)?,
            "role": if s { Value::Null } else { json!(p.role) },
            "made_on": p.role == reg::MADE_ON,
            // the made-on scan's tie is the result's own, which a layer's
            // build may have found voxel for voxel since it was frozen
            "link": match d.grid_link.as_deref() {
                Some(l @ ("same" | "world")) if p.role == reg::MADE_ON => l,
                _ => p.link.as_str(),
            },
            "layer": layer_state(registry.store(), &d, &set, p.stack, warm.is_some()),
        }));
    }
    // more on demand: the visit's other scans in the made-on scan's frame
    let more = frame_mates(registry.store(), &panels)?;
    let more_sealed = crate::campaigns::blind_among(registry.store(), caller, &more)?;
    let mut more_doc = Vec::new();
    for s in more {
        more_doc.push(json!({
            "stack": s,
            "shows": shows(registry.store(), reg::MADE_ON, s, more_sealed.contains(&s))?,
        }));
    }
    // the caller's own answer now, and an adjudicator's view of the raters'
    let answers = campaign::item_answers(registry.store(), item).map_err(campaign_err)?;
    let mine = answers
        .iter()
        .filter(|a| a.principal == principal)
        .max_by_key(|a| a.round)
        .map(|a| {
            json!({
                "answer": a.id, "value": a.value, "form": a.form, "why": a.why,
                "seconds": a.seconds, "round": a.round,
            })
        });
    let adjudicates = mine_a.iter().any(|a| a.role == "adjudicator");
    let voters: Vec<Value> = if adjudicates {
        answers
            .iter()
            .filter(|a| a.role == "rater")
            .enumerate()
            .map(|(i, a)| {
                json!({
                    "as": ((b'A' + i as u8) as char).to_string(),
                    "value": a.value, "form": a.form, "why": a.why,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut doc = json!({
        "item": item,
        "position": it.position + 1,
        "of": total,
        "state": it.state,
        "refused": it.refused_json(),
        "result": {
            "id": d.id, "pipeline": pipeline, "version": version, "output": output,
            "run": d.run_id, "sha256": d.sha256, "check": d.check_status,
        },
        "set": {
            "key": set, "words": set_doc["words"], "kind": set_doc["kind"], "grain": grain,
            "count": count, "note": set_doc["note"],
        },
        "part": part,
        "panels": shown_panels,
        "more": more_doc,
        "parts": parts,
        "reasons": reasons,
        "verdicts": reg::VERDICTS,
        "mine": mine,
        "warming": warm,
    });
    if count > PARTS_INLINE as i64 {
        doc["parts_door"] = json!(format!("/api/derivatives/{}/parts?set={set}", d.id));
    }
    if adjudicates {
        doc["answers"] = json!(voters);
    }
    if unsealed_read {
        audit_sealed_item(registry, caller, c, item, d.id)?;
    }
    Ok(Reply::ok(doc))
}

// ------------------------------------------------------------ sealed means sealed

/// What a door says of a check's item whose result was made from a stack of
/// a sample sealed now.
const SEALED_ITEM: &str = "its result was made from a stack of a sample sealed for certification, and a check never holds a sealed scan's result (record 55): what a system said of it is withheld until a certificate unseals it (record 48), and a holder of sealed:see reads it, audited as sealed.read";

/// Record 55, the rulings of 2026-10-12, item 6: a check made before the
/// ruling, or before its sample was sealed, may hold a result made from a
/// stack of a sample sealed now. Such an item is withheld from every rater
/// and adjudicator without sealed:see: each door that reads or answers it
/// refuses it (403, `sealed: true`, saying why and nothing of the result).
/// Answers whether the item is one, for a caller who reads sealed stacks,
/// whose read the door audits; false for an item of another campaign.
pub(crate) fn refuse_sealed(
    store: &mut Store,
    caller: &Caller,
    c: &campaign::Campaign,
    item: i64,
) -> Result<bool, Reply> {
    if !reg::is_check(c) {
        return Ok(false);
    }
    let Some(it) = campaign::item(store, item).map_err(campaign_err)? else {
        return Ok(false);
    };
    let Some(result) = it.derivative_id else {
        return Ok(false);
    };
    let found =
        derivative::of_ids(store, &[result]).map_err(|e| Reply::error(500, e.to_string()))?;
    let sealed = crate::derivatives::sealed_among(store, &found)
        .map_err(|e| Reply::error(500, e))?
        .contains(&result);
    if !sealed || crate::sealed::reads(&caller.access) {
        return Ok(sealed);
    }
    let mut r = Reply::error(
        403,
        format!(
            "item {} of check {}: {SEALED_ITEM}",
            it.position + 1,
            c.name
        ),
    );
    r.body["sealed"] = json!(true);
    Err(r)
}

/// Record 48, D1 of the move: the `sealed.read` row of a holder of
/// sealed:see whom the item door showed a check's item that it withholds
/// from everyone else.
fn audit_sealed_item(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
    item: i64,
    result: i64,
) -> Result<(), Reply> {
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &caller.principal,
            action: nils_registry::audit::Action::SealedRead,
            scope: json!({
                "door": "GET /api/campaigns/{id}/items/{item}/check",
                "campaign": c.id, "item": item, "derivative": result,
            }),
            policy: None,
            job_id: None,
            details: Some(json!({"grant": crate::grants::UNSEALED})),
        },
    )
    .map(|_| ())
    .map_err(|e| Reply::error(500, e.to_string()))
}

/// Record 55, the rulings of 2026-10-12, item 6: the items of a check that
/// a caller who does not read sealed stacks (`reads` false) is never
/// offered: those still asked whose result was made from a stack of a
/// sample sealed now. None for another campaign, for a reader of sealed
/// stacks, and while no sample is sealed.
pub(crate) fn withheld_items(
    store: &mut Store,
    reads: bool,
    c: &campaign::Campaign,
) -> Result<BTreeSet<i64>, Reply> {
    let mut out = BTreeSet::new();
    if reads || !reg::is_check(c) || !crate::sealed::any(store)? {
        return Ok(out);
    }
    let sql = format!(
        "SELECT id, derivative_id FROM {} WHERE campaign_id = {} AND derivative_id IS NOT NULL \
         AND state <> 'resolved'",
        store.qualified("campaign_item"),
        store.dialect().param(1, Type::Int)
    );
    let mut by_result: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(c.id)])? {
        by_result.entry(r.int(1)?).or_default().push(r.int(0)?);
    }
    let ids: Vec<i64> = by_result.keys().copied().collect();
    let results = derivative::of_ids(store, &ids).map_err(|e| Reply::error(500, e.to_string()))?;
    let sealed =
        crate::derivatives::sealed_among(store, &results).map_err(|e| Reply::error(500, e))?;
    for (result, items) in by_result {
        if sealed.contains(&result) {
            out.extend(items);
        }
    }
    Ok(out)
}

/// Record 55, the rulings of 2026-10-12, item 6: the engine's reading a
/// check's close takes of which of its results were made from a stack of a
/// sample sealed now, for any caller.
pub(crate) fn sealed_results(
    registry: &mut Registry,
    results: &[i64],
) -> Result<BTreeSet<i64>, String> {
    let store = registry.store();
    let found = derivative::of_ids(store, results).map_err(|e| e.to_string())?;
    crate::derivatives::sealed_among(store, &found)
}

/// The scans of the made-on scan's frame that no panel shows: what "+ scan"
/// offers (section 5.3).
fn frame_mates(store: &mut Store, panels: &[Panel]) -> Result<Vec<i64>, Reply> {
    let Some(made_on) = panels.first().map(|p| p.stack) else {
        return Ok(Vec::new());
    };
    let shown: BTreeSet<i64> = panels.iter().map(|p| p.stack).collect();
    let d = store.dialect();
    let sql = format!(
        "SELECT st.id FROM {} st JOIN {} se ON se.id = st.series_id \
         WHERE se.frame_of_reference_uid = (SELECT se2.frame_of_reference_uid FROM {} st2 \
              JOIN {} se2 ON se2.id = st2.series_id WHERE st2.id = {}) ORDER BY st.id",
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    Ok(store
        .query(&sql, &[Param::Int(made_on)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|s| !shown.contains(s))
        .take(32)
        .collect())
}

/// The open checks whose panels show a stack, or whose made-on scan is of
/// the stack's scanner frame (a scan a rater adds with "+ scan"), for the
/// pictures a rater reads through one.
pub(crate) fn campaigns_showing(store: &mut Store, stack: i64) -> Result<Vec<i64>, Reply> {
    let d = store.dialect();
    let sql = format!(
        "SELECT DISTINCT p.campaign_id FROM {} p JOIN {} c ON c.id = p.campaign_id \
         WHERE c.status = 'open' AND p.stack_id = {}",
        store.qualified("campaign_check_panel"),
        store.qualified("campaign"),
        d.param(1, Type::Int)
    );
    let mut out: Vec<i64> = store
        .query(&sql, &[Param::Int(stack)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    let sql = format!(
        "SELECT DISTINCT p.campaign_id FROM {} p JOIN {} c ON c.id = p.campaign_id \
         JOIN {} st ON st.id = p.stack_id JOIN {} se ON se.id = st.series_id \
         WHERE c.status = 'open' AND p.panel = 0 AND se.frame_of_reference_uid IS NOT NULL \
         AND se.frame_of_reference_uid = (SELECT se2.frame_of_reference_uid FROM {} st2 \
              JOIN {} se2 ON se2.id = st2.series_id WHERE st2.id = {})",
        store.qualified("campaign_check_panel"),
        store.qualified("campaign"),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(stack)])? {
        let id = r.int(0)?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// The open checks whose items hold a result.
pub(crate) fn campaigns_holding(store: &mut Store, derivative: i64) -> Result<Vec<i64>, Reply> {
    let sql = format!(
        "SELECT DISTINCT i.campaign_id FROM {} i JOIN {} c ON c.id = i.campaign_id \
         WHERE c.status = 'open' AND i.derivative_id = {}",
        store.qualified("campaign_item"),
        store.qualified("campaign"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query(&sql, &[Param::Int(derivative)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?)
}

/// What an item claimed next shows, by item, for the reader's warm-up: its
/// panels' stacks in order, and the result's layer on each, `{stack,
/// derivative, set}`, so the next items' masks warm with their pictures.
pub(crate) type Ahead = BTreeMap<i64, (Vec<i64>, Vec<Value>)>;

pub(crate) fn ahead_stacks(store: &mut Store, items: &str) -> Result<Ahead, Reply> {
    let mut out: Ahead = BTreeMap::new();
    let sql = format!(
        "SELECT p.item_id, p.stack_id, i.derivative_id, i.part_set FROM {} p \
         JOIN {} i ON i.id = p.item_id WHERE p.item_id IN ({items}) ORDER BY p.item_id, p.panel",
        store.qualified("campaign_check_panel"),
        store.qualified("campaign_item"),
    );
    for r in store.query(&sql, &[])? {
        let e = out.entry(r.int(0)?).or_default();
        let stack = r.int(1)?;
        e.0.push(stack);
        if let (Some(d), Some(set)) = (r.opt_int(2)?, r.opt_text(3)?) {
            e.1.push(json!({"stack": stack, "derivative": d, "set": set}));
        }
    }
    Ok(out)
}

// ------------------------------------------------------------ directly

/// `POST /api/derivatives/{id}/check` and `.../check/withdraw` (section
/// 8.5): one result checked by a person outside a campaign, and the check
/// taken back. A result an open check holds is refused (409).
pub(crate) fn direct_route(
    registry: &mut Registry,
    caller: &Caller,
    post: bool,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let (id, withdraw) = match segs {
        ["api", "derivatives", id, "check"] => (*id, false),
        ["api", "derivatives", id, "check", "withdraw"] => (*id, true),
        _ => return None,
    };
    if !post {
        return Some(Err(Reply::error(
            405,
            format!("/{} is a POST", segs.join("/")),
        )));
    }
    Some((|| -> Result<Reply, Reply> {
        let id: i64 = id
            .parse()
            .map_err(|_| Reply::error(404, "a derivative is named by its id"))?;
        let doc = if body.trim().is_empty() {
            json!({})
        } else {
            crate::serve::json_body(body)?
        };
        let kind = crate::serve::author_of(caller).0;
        if withdraw {
            let why = doc["why"].as_str();
            let ids =
                reg::withdraw(registry, id, &caller.principal, kind, why).map_err(campaign_err)?;
            let d = derivative::get(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            return Ok(Reply::ok(json!({
                "derivative": id, "withdrawn": ids, "check": d.and_then(|d| d.check_status),
            })));
        }
        let value = doc["value"]
            .as_str()
            .ok_or_else(|| bad("value: accepted, rejected or flagged"))?;
        let reasons: Vec<String> = match &doc["reasons"] {
            Value::Null => Vec::new(),
            Value::Array(l) => l
                .iter()
                .map(|r| {
                    r.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| bad("reasons: reason ids"))
                })
                .collect::<Result<_, _>>()?,
            _ => return Err(bad("reasons: a list of reason ids")),
        };
        let parts: Vec<(i64, String)> = match &doc["parts"] {
            Value::Null => Vec::new(),
            Value::Array(l) => l
                .iter()
                .map(|p| match (p["label"].as_i64(), p["reason"].as_str()) {
                    (Some(l), Some(r)) => Ok((l, r.to_string())),
                    _ => Err(bad("parts: each {label, reason}")),
                })
                .collect::<Result<_, _>>()?,
            _ => return Err(bad("parts: a list of {label, reason}")),
        };
        let done = reg::check_directly(
            registry,
            &reg::Direct {
                derivative: id,
                value,
                reasons: &reasons,
                parts: &parts,
                set: doc["set"].as_str(),
                why: doc["why"].as_str(),
                who: &caller.principal,
                kind,
            },
        )
        .map_err(campaign_err)?;
        let mut v = done.as_json();
        v["check"] = json!(value.trim());
        Ok(Reply::ok(v))
    })())
}

/// The (result, scan) pairs a check's panels draw, each once.
pub(crate) fn layer_pairs(
    store: &mut Store,
    campaign: i64,
) -> Result<Vec<(i64, i64)>, nils_registry::Error> {
    let sql = format!(
        "SELECT DISTINCT i.derivative_id, p.stack_id FROM {} p JOIN {} i ON i.id = p.item_id \
         WHERE p.campaign_id = {} AND i.derivative_id IS NOT NULL ORDER BY i.derivative_id, p.stack_id",
        store.qualified("campaign_check_panel"),
        store.qualified("campaign_item"),
        store.dialect().param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(campaign)])?
        .iter()
        .map(|r| Ok((r.int(0)?, r.int(1)?)))
        .collect()
}
