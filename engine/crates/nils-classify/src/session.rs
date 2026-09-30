// SPDX-License-Identifier: AGPL-3.0-only

//! The session pass over the registry (record 53, S2).
//!
//! The decision is `nils_pack::session`'s, the same code a replay over
//! header packets runs; what is here is the registry: which stacks the pass
//! targets, what their sessions hold, and what is written back.
//!
//! 1. Every stack of the pack's modality is read as the rules read it, with
//!    the axes in force (value, tier and confidence from
//!    `classification_axis`), and the pass's target is tried on it.
//! 2. For the stacks it holds on, every other stack of the same study is read
//!    with its series and frame of reference, and shown to the pass through
//!    the fields the pass names and nothing else.
//! 3. What the pass writes replaces the axis, at tier `session`, with an
//!    evidence row naming the pass, the rule and the siblings by stack id,
//!    and a review item, whatever the pass's `emit` says (record 53 R4). A
//!    rule that held back writes nothing and is counted.

use std::collections::{BTreeMap, HashMap, HashSet};

use nils_pack::Pack;
use nils_pack::pass::Pass;
use nils_pack::session::{InForce, Session, Sib, TIER};
use nils_pack::stack::{FIELDS, Stack, Value};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::now_iso;

use crate::classify::{axis_rows, select, select_stacks, to_stack};
use crate::job::Error;
use crate::passes::Ran;

/// A stack the target holds on, as it was read.
struct Target {
    id: i64,
    stack: Stack,
    private: Vec<String>,
    in_force: Vec<InForce>,
}

/// The axes in force of the stacks `filter` names, per axis of the pack. A
/// multi-valued axis arrives as one row per value.
fn in_force_of(
    store: &mut Store,
    pack: &Pack,
    filter: &str,
    params: &[Param],
) -> Result<HashMap<i64, Vec<InForce>>, Error> {
    let t = table("classification_axis");
    let d = store.dialect();
    let sql = format!(
        "SELECT stack_id, axis, value, {}, tier FROM {} WHERE {filter}",
        d.text_of(
            t.column("confidence")
                .expect("classification_axis.confidence")
        ),
        store.qualified("classification_axis"),
    );
    let mut out: HashMap<i64, Vec<InForce>> = HashMap::new();
    for r in store.query(&sql, params)? {
        let name = r.text(1)?;
        let Some(a) = pack.axes.iter().position(|x| x.name == name) else {
            continue;
        };
        let slot = &mut out
            .entry(r.int(0)?)
            .or_insert_with(|| vec![InForce::default(); pack.axes.len()])[a];
        if let Some(v) = r.opt_text(2)?
            && !v.is_empty()
        {
            slot.values.push(v.to_string());
        }
        slot.confidence = crate::classify::cell_text(r.get(3))
            .and_then(|c| c.parse().ok())
            .unwrap_or(slot.confidence);
        slot.tier = r.opt_text(4)?.unwrap_or("").to_string();
    }
    Ok(out)
}

/// The session a stack belongs to, and what its siblings are compared by.
#[derive(Clone)]
struct Place {
    series: i64,
    study: i64,
    frame: Option<String>,
}

fn places(store: &mut Store, filter: &str) -> Result<BTreeMap<i64, Place>, Error> {
    let sql = format!(
        "SELECT f.stack_id, f.series_id, f.study_id, s.frame_of_reference_uid FROM {} f \
         JOIN {} s ON s.id = f.series_id WHERE {filter}",
        store.qualified("stack_fingerprint"),
        store.qualified("series"),
    );
    let mut out = BTreeMap::new();
    for r in store.query(&sql, &[])? {
        out.insert(
            r.int(0)?,
            Place {
                series: r.int(1)?,
                study: r.int(2)?,
                frame: r.opt_text(3)?.filter(|f| !f.is_empty()).map(str::to_string),
            },
        );
    }
    Ok(out)
}

/// Try the pass's target on each stack of `rows`, with its axes in force.
fn consider(
    pack: &Pack,
    pass: &Pass,
    settings: &crate::job::Settings,
    rows: &[nils_registry::Row],
    forces: &mut HashMap<i64, Vec<InForce>>,
    targets: &mut Vec<Target>,
) -> Result<(), Error> {
    for r in rows {
        let (ids, stack, private) = to_stack(r, false, pack)?;
        let modality =
            stack.text(nils_pack::stack::field_index("modality").expect("modality is a field"));
        if modality != pack.modality || settings.modality.as_deref().is_some_and(|m| m != modality)
        {
            continue;
        }
        let in_force = forces
            .remove(&ids.stack)
            .unwrap_or_else(|| vec![InForce::default(); pack.axes.len()]);
        if nils_pack::session::targets(pack, pass.target.as_ref(), &stack, &private, &in_force) {
            targets.push(Target {
                id: ids.stack,
                stack,
                private,
                in_force,
            });
        }
    }
    Ok(())
}

fn list(ids: impl Iterator<Item = i64>) -> String {
    ids.map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
}

/// A sibling as the pass sees it: only the fields it names, and the private
/// elements among them.
fn seen(pack: &Pack, session: &Session, stack: &Stack, private: &[String]) -> (Stack, Vec<String>) {
    let reads = nils_pack::session::sibling_reads(session);
    let mut s = Stack::new();
    let first_private = FIELDS.len() + pack.derived.len();
    let mut p = vec![String::new(); pack.ingest.len()];
    for f in reads {
        if f < FIELDS.len() {
            let text = stack.as_text(f).into_owned();
            s.set(FIELDS[f], Value::Text(Some(&text)))
                .expect("a field of the fingerprint");
        } else if f >= first_private
            && let Some(v) = private.get(f - first_private)
        {
            p[f - first_private] = v.clone();
        }
    }
    (s, p)
}

#[allow(clippy::too_many_arguments)]
/// Run one session pass: find its targets, read their sessions, decide and
/// write. `filled` gains every (stack, axis) the pass wrote, so a later pass
/// in the same run does not fill it again from a corpus read before.
pub fn run(
    store: &mut Store,
    pack: &Pack,
    pass: &Pass,
    session: &Session,
    settings: &crate::job::Settings,
    cancel: &nils_digest::Cancel,
    job_id: i64,
    filled: &mut HashSet<(i64, String)>,
) -> Result<Ran, Error> {
    let mut ran = Ran {
        pass: pass.name.clone(),
        kind: pass.kind_name().to_string(),
        reference: pass.reference.scope.clone(),
        ..Ran::default()
    };

    // --- 1. the targets. Where the target requires axis values, only the
    // stacks that hold them all are read; otherwise every stack, window by
    // window.
    let mut targets: Vec<Target> = Vec::new();
    let required = pass
        .target
        .as_ref()
        .map(|t| t.required_axes())
        .unwrap_or_default();
    if required.is_empty() {
        let sql = select(store, settings.modality.as_deref(), false);
        let window = settings.window.max(1) as i64;
        let mut after = 0i64;
        loop {
            if cancel.stop() {
                return Ok(ran);
            }
            let rows = store.query(&sql, &[Param::Int(after), Param::Int(window)])?;
            let Some(last) = rows.last() else { break };
            let last = last.int(0)?;
            let d = store.dialect();
            let filter = format!(
                "stack_id > {} AND stack_id <= {}",
                d.param(1, Type::Int),
                d.param(2, Type::Int)
            );
            let mut forces =
                in_force_of(store, pack, &filter, &[Param::Int(after), Param::Int(last)])?;
            consider(pack, pass, settings, &rows, &mut forces, &mut targets)?;
            after = last;
            if (rows.len() as i64) < window {
                break;
            }
        }
    } else {
        let mut ids: Option<HashSet<i64>> = None;
        for (axis, value) in &required {
            let d = store.dialect();
            let sql = format!(
                "SELECT stack_id FROM {} WHERE axis = {} AND value = {}",
                store.qualified("classification_axis"),
                d.param(1, Type::Text),
                d.param(2, Type::Text),
            );
            let got: HashSet<i64> = store
                .query(
                    &sql,
                    &[
                        Param::from(pack.axes[*axis].name.as_str()),
                        Param::from(value.as_str()),
                    ],
                )?
                .iter()
                .map(|r| r.int(0))
                .collect::<Result<_, _>>()?;
            ids = Some(match ids {
                None => got,
                Some(had) => had.intersection(&got).copied().collect(),
            });
        }
        let mut ids: Vec<i64> = ids.unwrap_or_default().into_iter().collect();
        ids.sort_unstable();
        for chunk in ids.chunks(500) {
            if cancel.stop() {
                return Ok(ran);
            }
            let rows = store.query(&select_stacks(store, chunk), &[])?;
            let mut forces = in_force_of(
                store,
                pack,
                &format!("stack_id IN ({})", list(chunk.iter().copied())),
                &[],
            )?;
            consider(pack, pass, settings, &rows, &mut forces, &mut targets)?;
        }
    }
    ran.targets = targets.len() as i64;
    if targets.is_empty() {
        return Ok(ran);
    }

    // --- 2. their sessions
    let mut place: BTreeMap<i64, Place> = BTreeMap::new();
    for chunk in targets.chunks(500) {
        place.extend(places(
            store,
            &format!("f.stack_id IN ({})", list(chunk.iter().map(|t| t.id))),
        )?);
    }
    let studies: Vec<i64> = {
        let mut v: Vec<i64> = place.values().map(|p| p.study).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let mut members: BTreeMap<i64, Place> = BTreeMap::new();
    for chunk in studies.chunks(500) {
        members.extend(places(
            store,
            &format!("f.study_id IN ({})", list(chunk.iter().copied())),
        )?);
    }
    ran.pool = members.len();
    let mut read: HashMap<i64, (Stack, Vec<String>)> = HashMap::new();
    let ids: Vec<i64> = members.keys().copied().collect();
    for chunk in ids.chunks(500) {
        for r in store.query(&select_stacks(store, chunk), &[])? {
            let (i, stack, private) = to_stack(&r, false, pack)?;
            read.insert(i.stack, seen(pack, session, &stack, &private));
        }
    }

    // --- 3. decide
    let now = now_iso();
    let mut touched: Vec<(i64, String)> = Vec::new();
    let mut axis_rows_out: Vec<Vec<Param>> = Vec::new();
    let mut evidence: Vec<Vec<Param>> = Vec::new();
    let mut reviews: Vec<Vec<Param>> = Vec::new();
    for t in &targets {
        let Some(me) = place.get(&t.id) else { continue };
        let siblings: Vec<Sib> = members
            .iter()
            .filter(|(id, p)| **id != t.id && p.study == me.study)
            .filter_map(|(id, p)| {
                let (stack, private) = read.get(id)?.clone();
                Some(Sib {
                    id: *id,
                    stack,
                    private,
                    same_series: p.series == me.series,
                    same_frame_of_reference: match (&me.frame, &p.frame) {
                        (Some(a), Some(b)) => Some(a == b),
                        _ => None,
                    },
                })
            })
            .collect();
        let Some(answer) = nils_pack::session::decide(
            pack,
            pass.target.as_ref(),
            session,
            &t.stack,
            &t.private,
            &t.in_force,
            &siblings,
        ) else {
            *ran.by_method.entry("no rule".into()).or_insert(0) += 1;
            continue;
        };
        if answer.held.is_some() {
            *ran.by_method.entry("held".into()).or_insert(0) += 1;
            continue;
        }
        *ran.by_method.entry(answer.rule.clone()).or_insert(0) += 1;
        if answer.writes.is_empty() {
            continue;
        }
        ran.decided += 1;
        let cited = if answer.cited.is_empty() {
            "no sibling holds".to_string()
        } else {
            format!(
                "sibling stacks {}",
                answer
                    .cited
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        for (axis, values) in &answer.writes {
            let name = pack.axes[*axis].name.as_str();
            let stored = values.join(",");
            axis_rows_out.extend(axis_rows(t.id, name, &stored, answer.confidence, TIER));
            touched.push((t.id, name.to_string()));
            // Record 53 R4: every value a session pass writes names its
            // evidence and is asked about, whatever the pass's `emit` says;
            // no session answer is ever sure.
            evidence.push(vec![
                Param::Int(t.id),
                Param::from(name),
                Param::from(stored.as_str()),
                Param::from(TIER),
                Param::Double(answer.confidence),
                Param::from(pass.name.as_str()),
                Param::from(answer.rule.as_str()),
                Param::from("session"),
                Param::from(cited.as_str()),
                Param::from(pass.name.as_str()),
                Param::from(pass.reference.scope.as_str()),
            ]);
            if nils_pack::at_threshold(answer.confidence, pass.emit.review_below) {
                ran.at_threshold += 1;
            }
            reviews.push(vec![
                Param::from(format!("{name}:session")),
                Param::from("stack"),
                Param::from(serde_json::json!({"stack_id": t.id}).to_string()),
                Param::from(
                    serde_json::json!({
                        "axis": name,
                        "value": stored,
                        "confidence": answer.confidence,
                        "tier": TIER,
                        "pass": pass.name,
                        "rule": answer.rule,
                        "siblings": answer.cited,
                        "was": t.in_force[*axis].values,
                        "job": job_id,
                    })
                    .to_string(),
                ),
                Param::from("open"),
                Param::from(now.as_str()),
                Param::Int(job_id),
            ]);
            ran.review_items += 1;
        }
    }

    // --- 4. write, in one transaction
    if axis_rows_out.is_empty() {
        return Ok(ran);
    }
    store.begin()?;
    let write = (|| -> Result<(), nils_registry::store::Error> {
        let mut by_axis: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        for (stack, axis) in &touched {
            by_axis.entry(axis.clone()).or_default().push(*stack);
        }
        let axis_t = store.qualified("classification_axis");
        for (axis, stacks) in by_axis {
            for chunk in stacks.chunks(500) {
                store.execute(
                    &format!(
                        "DELETE FROM {axis_t} WHERE axis = '{}' AND stack_id IN ({})",
                        axis.replace('\'', "''"),
                        list(chunk.iter().copied())
                    ),
                    &[],
                )?;
            }
        }
        store.insert(
            &Insert::new(
                table("classification_axis"),
                &["stack_id", "axis", "value", "confidence", "tier"],
            ),
            &axis_rows_out,
        )?;
        if !evidence.is_empty() {
            store.insert(
                &Insert::new(
                    table("classification_evidence"),
                    &[
                        "stack_id",
                        "axis",
                        "value",
                        "tier",
                        "confidence",
                        "rule_set",
                        "rule",
                        "source",
                        "matched",
                        "pass",
                        "reference",
                    ],
                ),
                &evidence,
            )?;
        }
        // record 48, D1 of the move: a stack of a sample sealed now never
        // becomes a review item
        let withheld = nils_registry::labels::drop_sealed_items(store, &mut reviews, 2)
            .map_err(|e| nils_registry::store::Error::Message(e.to_string()))?;
        ran.review_items -= withheld as i64;
        if !reviews.is_empty() {
            store.insert(
                &Insert::new(
                    table("review_item"),
                    &[
                        "kind",
                        "scope",
                        "ref",
                        "evidence",
                        "status",
                        "created_at",
                        "job_id",
                    ],
                ),
                &reviews,
            )?;
        }
        Ok(())
    })();
    match write {
        Ok(()) => store.commit()?,
        Err(e) => {
            store.rollback().ok();
            return Err(Error::Store(e));
        }
    }
    filled.extend(touched);
    Ok(ran)
}
