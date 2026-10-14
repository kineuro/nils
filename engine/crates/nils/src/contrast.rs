// SPDX-License-Identifier: AGPL-3.0-only

//! The post-contrast label at the engine's doors (record 55 F2 and F5, spec
//! §12.3): `nils contrast label` and `nils contrast show`, the label on a
//! stack's evidence line, its counts on the post-contrast step, and the
//! labelling again after a run that answers the axis.
//!
//! The rule is `nils_pack::contrast`'s and its registry side
//! `nils_classify::contrast`'s.

use std::path::PathBuf;

use clap::Subcommand;
use nils_registry::Registry;
use nils_registry::home::Home;
use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

use crate::Exit;

#[derive(Subcommand, Debug)]
pub(crate) enum ContrastCommand {
    /// Label the sessions again: the time rule's session pass and the
    /// subject step, over every subject with an MR stack, or those named
    Label {
        /// Only these subjects' sessions, by id
        #[arg(long = "subject", value_name = "ID")]
        subjects: Vec<i64>,
        /// Only the sessions of the subjects of these stacks, by id
        #[arg(long = "stack", value_name = "ID")]
        stacks: Vec<i64>,
        /// The pack's name, looked up in the pack directory
        #[arg(long, default_value = "mri")]
        pack: String,
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// One stack's label: the state, the minutes since injection, where it
    /// came from, the 72-hour flag and the checks that fired
    Show {
        /// The stack's id
        stack: i64,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn command(home: &Home, command: ContrastCommand) -> Result<(), Exit> {
    match command {
        ContrastCommand::Label {
            subjects,
            stacks,
            pack,
            pack_dir,
            json,
        } => {
            let dir = crate::pack_dir(home, pack_dir)?;
            let pack = crate::ask_cli::load_pack(&dir, &pack)?;
            let mut registry = crate::open(home)?;
            let store = registry.store();
            let mut named = subjects;
            if !stacks.is_empty() {
                named.extend(
                    nils_classify::contrast::subjects_of(store, &stacks)
                        .map_err(|e| crate::fail(e.to_string()))?,
                );
            }
            named.sort_unstable();
            named.dedup();
            let scope = (!named.is_empty()).then_some(named.as_slice());
            let done = nils_classify::contrast::label(store, &pack, scope)
                .map_err(|e| crate::fail(e.to_string()))?;
            if json {
                println!("{}", done.to_json());
            } else {
                println!(
                    "post-contrast label ({}): {} stacks of {} sessions of {} subjects",
                    nils_pack::contrast::RULE,
                    done.stacks,
                    done.sessions,
                    done.subjects
                );
                println!(
                    "  sessions with a T1 anchor {} ({} stacks)",
                    done.anchored_sessions, done.anchored_stacks
                );
                for (state, n) in &done.by_state {
                    println!("  {state:<14} {n:>8}");
                }
            }
            Ok(())
        }
        ContrastCommand::Show { stack, json } => {
            let mut registry = crate::open(home)?;
            let store = registry.store();
            let doc = of_stack(store, stack)
                .map_err(|e| crate::fail(e.to_string()))?
                .ok_or_else(|| {
                    crate::fail(format!(
                        "stack {stack} has no post-contrast label; nils contrast label writes them"
                    ))
                })?;
            if json {
                println!("{doc}");
            } else {
                println!("stack {stack}");
                for k in [
                    "contrast_state",
                    "minutes_since_injection",
                    "injection_ref",
                    "contrast_source",
                    "basis",
                    "contrast_prior_72h",
                    "asl_after_contrast",
                    "reason",
                    "family",
                    "use_level",
                    "checks",
                ] {
                    if !doc[k].is_null() {
                        println!("  {k:<24} {}", doc[k]);
                    }
                }
            }
            Ok(())
        }
    }
}

/// One stack's label, as the doors give it; none where it has none.
pub(crate) fn of_stack(store: &mut Store, stack: i64) -> Result<Option<Value>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT contrast_state, minutes_since_injection, injection_ref, minutes_basis, contrast_source, \
         basis, contrast_prior_72h, asl_after_contrast, reason, {}, weak_f, family, t1_family, use_level, \
         anchored, first_post_stack, last_pre_stack, rule, {} FROM {} WHERE stack_id = {}",
        crate::text_of(store, "contrast_label", "checks"),
        crate::text_of(store, "contrast_label", "labelled_at"),
        store.qualified("contrast_label"),
        d.param(1, Type::Int)
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(stack)])? else {
        return Ok(None);
    };
    let checks: Value = r
        .opt_text(9)?
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_else(|| json!([]));
    Ok(Some(json!({
        "stack": stack,
        "contrast_state": r.text(0)?,
        "minutes_since_injection": r.opt_double(1)?,
        "injection_ref": r.opt_text(2)?,
        "minutes_basis": r.opt_text(3)?,
        "contrast_source": r.text(4)?,
        "basis": r.text(5)?,
        "contrast_prior_72h": r.text(6)?,
        "asl_after_contrast": r.opt_text(7)?,
        "reason": r.opt_text(8)?,
        "checks": checks,
        "weak_f": r.int(10)? != 0,
        "family": r.text(11)?,
        "t1_family": r.int(12)? != 0,
        "use_level": r.text(13)?,
        "anchored": r.int(14)? != 0,
        "first_post_stack": r.opt_int(15)?,
        "last_pre_stack": r.opt_int(16)?,
        "rule": r.text(17)?,
        "labelled_at": r.opt_text(18)?,
    })))
}

/// The label's counts over the stacks `holds` names (a condition on a stack
/// aliased `x`): how many carry one, by state, and how many are in a
/// session with a T1 anchor.
pub(crate) fn counts(store: &mut Store, holds: &str) -> Result<Value, StoreError> {
    let sql = format!(
        "SELECT cl.contrast_state, cl.anchored, COUNT(*) FROM {} cl JOIN {} x ON x.id = cl.stack_id \
         WHERE {holds} GROUP BY cl.contrast_state, cl.anchored",
        store.qualified("contrast_label"),
        store.qualified("stack"),
    );
    let mut by_state = serde_json::Map::new();
    let (mut labelled, mut anchored) = (0i64, 0i64);
    for r in store.query(&sql, &[])? {
        let n = r.int(2)?;
        labelled += n;
        if r.int(1)? != 0 {
            anchored += n;
        }
        let e = by_state.entry(r.text(0)?.to_string()).or_insert(json!(0));
        *e = json!(e.as_i64().unwrap_or(0) + n);
    }
    Ok(json!({
        "rule": nils_pack::contrast::RULE,
        "labelled": labelled,
        "in_anchored_sessions": anchored,
        "by_state": by_state,
    }))
}

/// After a run that answers the post-contrast axis: label the sessions of
/// its stacks' subjects again, with the pack the engine serves. Says what
/// it did, or why it could not.
pub(crate) fn after_answers(home: &Home, registry: &mut Registry, stacks: &[i64]) -> Value {
    let pack = match crate::pack_dir(home, None)
        .ok()
        .and_then(|dir| crate::ask_cli::load_pack(&dir, "mri").ok())
    {
        Some(p) => p,
        None => return json!({"error": "no MRI pack is installed, so nothing was labelled"}),
    };
    let store = registry.store();
    let result = nils_classify::contrast::subjects_of(store, stacks)
        .and_then(|s| nils_classify::contrast::label(store, &pack, Some(&s)));
    match result {
        Ok(done) => done.to_json(),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// After a campaign closes or decisions are put in force: where any of the
/// decisions answers the post-contrast axis, a person's settled read is a
/// T1 anchor, so the sessions of its stacks' subjects are labelled again.
/// None where none of them does.
pub(crate) fn after_decisions(
    store: &mut Store,
    pack: Option<&nils_pack::Pack>,
    decisions: &[i64],
) -> Option<Value> {
    if decisions.is_empty() {
        return None;
    }
    let list = decisions
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT d.scope, d.ref FROM {} d WHERE d.id IN ({list}) AND d.axis = 'post_contrast'",
        store.qualified("decision"),
    );
    let rows = match store.query(&sql, &[]) {
        Ok(rows) => rows,
        Err(e) => return Some(json!({"error": e.to_string()})),
    };
    let mut stacks: Vec<i64> = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    for r in &rows {
        let (Ok(scope), Ok(reference)) = (r.text(0), r.text(1)) else {
            continue;
        };
        match scope {
            "stack" => stacks.extend(reference.parse::<i64>().ok()),
            "group" => groups.push(format!("'{}'", reference.replace('\'', "''"))),
            _ => {}
        }
    }
    if !groups.is_empty() {
        let sql = format!(
            "SELECT rm.stack_id FROM {} rm WHERE CAST(rm.item_id AS TEXT) IN ({})",
            store.qualified("review_member"),
            groups.join(", ")
        );
        if let Ok(rows) = store.query(&sql, &[]) {
            stacks.extend(rows.iter().filter_map(|r| r.int(0).ok()));
        }
    }
    if stacks.is_empty() {
        return None;
    }
    let Some(pack) = pack else {
        return Some(json!({"error": "no MRI pack is served, so nothing was labelled"}));
    };
    let result = nils_classify::contrast::subjects_of(store, &stacks)
        .and_then(|s| nils_classify::contrast::label(store, pack, Some(&s)));
    Some(match result {
        Ok(done) => done.to_json(),
        Err(e) => json!({"error": e.to_string()}),
    })
}

fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Record 55 E3, `x-nils.input.contrast`: the session facts the
/// post-contrast model reads of each stack, from its series and its
/// fingerprint: the contrast agent as written, whether any other contrast
/// field is filled, its clock (the fingerprint's earliest acquisition, else
/// the series' date and time), its series number and physics, whether its
/// image type is derived, its angiography flag and folded text, and the
/// post-contrast rules that fired on it.
pub(crate) fn facts(
    store: &mut Store,
    ids: &[i64],
) -> Result<std::collections::BTreeMap<i64, Value>, String> {
    let err = |e: StoreError| e.to_string();
    let d = store.dialect();
    let t = |alias: &str, table: &str, column: &str| {
        d.text_of_qualified(
            Some(alias),
            nils_registry::schema::table(table)
                .column(column)
                .unwrap_or_else(|| panic!("{table}.{column}")),
        )
    };
    let mut out = std::collections::BTreeMap::new();
    for chunk in ids.chunks(500) {
        let list = id_list(chunk);
        let sql = format!(
            "SELECT st.id, se.contrast_bolus_agent, se.contrast_bolus_route, se.contrast_bolus_volume, \
             se.contrast_bolus_total_dose, {}, se.contrast_flow_rate, COALESCE({}, {}), COALESCE({}, {}), \
             f.series_number, f.repetition_time, f.echo_time, f.inversion_time, f.flip_angle, \
             f.mr_acquisition_type, f.image_type, f.angio_flag, f.text_all_ci \
             FROM {} st JOIN {} se ON se.id = st.series_id LEFT JOIN {} f ON f.stack_id = st.id \
             WHERE st.id IN ({list})",
            t("se", "series", "contrast_bolus_start_time"),
            t("f", "stack_fingerprint", "earliest_acquisition_date"),
            t("se", "series", "series_date"),
            t("f", "stack_fingerprint", "earliest_acquisition_time"),
            t("se", "series", "series_time"),
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("stack_fingerprint"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let positive = |v: Option<f64>| v.is_some_and(|x| x > 0.0);
            let other_tag = r
                .opt_text(2)
                .map_err(err)?
                .is_some_and(|x| !x.trim().is_empty())
                || positive(r.opt_double(3).map_err(err)?)
                || positive(r.opt_double(4).map_err(err)?)
                || r.opt_text(5).map_err(err)?.is_some()
                || positive(r.opt_double(6).map_err(err)?);
            let image_type = r.opt_text(15).map_err(err)?;
            out.insert(
                r.int(0).map_err(err)?,
                json!({
                    "contrast_bolus_agent": r.opt_text(1).map_err(err)?,
                    "other_tag": other_tag,
                    "acquisition_date": r.opt_text(7).map_err(err)?,
                    "acquisition_time": r.opt_text(8).map_err(err)?,
                    "series_number": r.opt_int(9).map_err(err)?,
                    "repetition_time": r.opt_double(10).map_err(err)?,
                    "echo_time": r.opt_double(11).map_err(err)?,
                    "inversion_time": r.opt_double(12).map_err(err)?,
                    "flip_angle": r.opt_double(13).map_err(err)?,
                    "mr_acquisition_type": r.opt_text(14).map_err(err)?,
                    "derived": image_type.is_some_and(|x| x.to_ascii_uppercase().contains("DERIVED")),
                    "angio_flag": r.opt_text(16).map_err(err)?,
                    "text_all_ci": r.opt_text(17).map_err(err)?,
                    "post_contrast_rules": [],
                }),
            );
        }
        let sql = format!(
            "SELECT DISTINCT stack_id, rule FROM {} WHERE axis = 'post_contrast' AND stack_id IN ({list}) \
             ORDER BY stack_id, rule",
            store.qualified("classification_evidence"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            if let Some(v) = out.get_mut(&r.int(0).map_err(err)?)
                && let Some(a) = v["post_contrast_rules"].as_array_mut()
            {
                a.push(json!(r.text(1).map_err(err)?));
            }
        }
    }
    Ok(out)
}

/// The names of the datasets (the source places) that hold a file of each
/// stack, into each header as `datasets`.
pub(crate) fn with_datasets(
    store: &mut Store,
    ids: &[i64],
    headers: &mut std::collections::BTreeMap<i64, Value>,
) -> Result<(), String> {
    let err = |e: StoreError| e.to_string();
    let mut names_of: std::collections::BTreeMap<i64, Vec<String>> = Default::default();
    for p in nils_registry::place::list(store).map_err(err)? {
        if p.role != nils_registry::place::Role::Source || p.retired_at.is_some() {
            continue;
        }
        for s in crate::sources::source_ids(store, &p).map_err(err)? {
            names_of.entry(s).or_default().push(p.name.clone());
        }
    }
    let mut held: std::collections::BTreeMap<i64, std::collections::BTreeSet<String>> =
        Default::default();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT stack_id, source_id FROM {} WHERE stack_id IN ({})",
            store.qualified("source_stack"),
            id_list(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let names = names_of
                .get(&r.int(1).map_err(err)?)
                .cloned()
                .unwrap_or_default();
            held.entry(r.int(0).map_err(err)?)
                .or_default()
                .extend(names);
        }
    }
    for id in ids {
        if let Some(h) = headers.get_mut(id)
            && h.is_object()
        {
            h["datasets"] = json!(held.remove(id).unwrap_or_default());
        }
    }
    Ok(())
}

/// Each context entry with its series, its header (with its datasets) and
/// its session facts, as the run's own stacks carry them.
pub(crate) fn context(store: &mut Store, context: &mut [Value]) -> Result<(), String> {
    let err = |e: StoreError| e.to_string();
    let ids: Vec<i64> = context
        .iter()
        .filter_map(|c| c["stack_id"].as_i64())
        .collect();
    let mut series: std::collections::BTreeMap<i64, (i64, i64)> = Default::default();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT st.id, st.series_id, se.subject_id FROM {} st JOIN {} se ON se.id = st.series_id \
             WHERE st.id IN ({})",
            store.qualified("stack"),
            store.qualified("series"),
            id_list(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            series.insert(
                r.int(0).map_err(err)?,
                (r.int(1).map_err(err)?, r.int(2).map_err(err)?),
            );
        }
    }
    let subjects: std::collections::BTreeMap<i64, i64> =
        series.iter().map(|(k, (_, s))| (*k, *s)).collect();
    let mut headers = crate::pipelines::stack_headers(store, &subjects)?;
    with_datasets(store, &ids, &mut headers)?;
    let mut facts = facts(store, &ids)?;
    for c in context.iter_mut() {
        let Some(id) = c["stack_id"].as_i64() else {
            continue;
        };
        c["series_id"] = json!(series.get(&id).map(|(s, _)| *s));
        c["header"] = headers.remove(&id).unwrap_or(Value::Null);
        c["contrast"] = facts.remove(&id).unwrap_or(Value::Null);
    }
    Ok(())
}
