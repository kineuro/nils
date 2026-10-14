// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 H3, the picks door (`GET /api/cohorts/{name}/picks`): a page of
//! a cohort's open members, each with its sessions and, per session, each
//! role the served pack's picks declare: picked, doubted or missing, the
//! stack and who picked it. The desk draws the cohort's heatmap from it.
//!
//! The picks are read through the one resolver
//! ([`nils_registry::pick_truth`]): the cohort's own once its rules are
//! read, else the pack's, so the door agrees with the Main scans map. A
//! session is one the session cache holds (the visits door's meaning), and
//! a pick names it by the day it opened. A role is doubted where an open
//! `pick.border` review item names that subject, day and role; picked where
//! a live pick names it; else missing. Totals per role count every member's
//! sessions, not only the page's.
//!
//! Each session carries its studies, a label (`ses-` and the day it opened)
//! and its number in the member's sessions, oldest first. Record 55 K7:
//! below detail quasi the subject's code and the label come back as their
//! shapes. Record 48: a pick of a stack of a sample
//! sealed now keeps its state, and names no stack to a caller without
//! sealed:see (`withheld`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nils_registry::Registry;
use nils_registry::pick_truth::Of;
use nils_registry::store::Error as StoreError;
use serde_json::{Value, json};

use crate::grants::Detail;
use crate::scans::shape;
use crate::serve::{Caller, Reply};

/// A page of members when none is asked for, and at most.
const PAGE: usize = 24;
const MOST: usize = 200;

fn failed(e: StoreError) -> Reply {
    Reply::error(500, e.to_string())
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The roles the pack's picks declare, in its order.
pub(crate) fn roles_of(pack: Option<&nils_pack::Pack>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in pack.map(|p| p.picks.as_slice()).unwrap_or_default() {
        for r in &m.roles {
            if !out.contains(r) {
                out.push(r.clone());
            }
        }
    }
    out
}

/// A live pick of a role on a subject's day: who made it and its stacks.
struct Picked {
    person: bool,
    stacks: BTreeSet<i64>,
}

/// `GET /api/cohorts/{name}/picks`.
pub(crate) fn door(
    registry: &mut Registry,
    caller: &Caller,
    pack: Option<&nils_pack::Pack>,
    name: &str,
    query: &HashMap<String, String>,
) -> Result<Value, Reply> {
    let quasi = caller.access.detail >= Detail::Quasi;
    let limit = match query.get("limit").filter(|l| !l.is_empty()) {
        Some(l) => l
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MOST).contains(n))
            .ok_or_else(|| Reply::error(400, format!("limit is 1 to {MOST}")))?,
        None => PAGE,
    };
    let after = match query.get("after").filter(|a| !a.is_empty()) {
        Some(a) => Some(
            a.parse::<i64>()
                .map_err(|_| Reply::error(400, "after is a subject's id, as `next` gave it"))?,
        ),
        None => None,
    };
    let store = registry.store();
    let cohort = nils_registry::cohort::by_name(store, name)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(404, format!("no cohort named {name}")))?;
    let members: Vec<i64> = nils_registry::cohort::open_members(store, cohort.id)
        .map_err(failed)?
        .into_iter()
        .collect();
    let roles = roles_of(pack);
    let w = crate::viewer::window(store).map_err(failed)?;
    let cache_t = nils_registry::schema::table("session_cache");
    let d = store.dialect();
    let first = d.text_of(cache_t.column("first").expect("first"));
    // every member's sessions, by the day each opened
    let mut sessions: BTreeMap<i64, Vec<(i64, String)>> = BTreeMap::new();
    let mut codes: HashMap<i64, String> = HashMap::new();
    for chunk in members.chunks(500) {
        for r in store
            .query(
                &format!(
                    "SELECT id, subject_id, {first} FROM {} WHERE window_days = {w} \
                     AND subject_id IN ({}) ORDER BY subject_id, {first}",
                    store.qualified("session_cache"),
                    list(chunk)
                ),
                &[],
            )
            .map_err(failed)?
        {
            let day = r.text(2).map_err(failed)?;
            sessions
                .entry(r.int(1).map_err(failed)?)
                .or_default()
                .push((
                    r.int(0).map_err(failed)?,
                    day.get(..10).unwrap_or(day).to_string(),
                ));
        }
        for r in store
            .query(
                &format!(
                    "SELECT id, code FROM {} WHERE id IN ({})",
                    store.qualified("subject"),
                    list(chunk)
                ),
                &[],
            )
            .map_err(failed)?
        {
            codes.insert(
                r.int(0).map_err(failed)?,
                r.text(1).map_err(failed)?.to_string(),
            );
        }
    }
    // the live picks the cohort reads, by role, subject and day
    let of = Of::cohort(cohort.id, &cohort.name);
    let picks = of.picks(store);
    let pick_day = crate::text_of(store, "pick", "session_day");
    let mut picked: HashMap<(String, i64, String), Picked> = HashMap::new();
    for chunk in members.chunks(500) {
        for r in store
            .query(
                &format!(
                    "SELECT p.role, p.subject_id, {pick_day}, p.author_kind, ps.stack_id \
                     FROM {picks} p LEFT JOIN {} ps ON ps.pick_id = p.id \
                     WHERE p.subject_id IN ({})",
                    store.qualified("pick_stack"),
                    list(chunk)
                ),
                &[],
            )
            .map_err(failed)?
        {
            let day = r.text(2).map_err(failed)?;
            let key = (
                r.text(0).map_err(failed)?.to_string(),
                r.int(1).map_err(failed)?,
                day.get(..10).unwrap_or(day).to_string(),
            );
            let e = picked.entry(key).or_insert(Picked {
                person: false,
                stacks: BTreeSet::new(),
            });
            e.person |= r.text(3).map_err(failed)? == "person";
            if let Some(s) = r.opt_int(4).map_err(failed)? {
                e.stacks.insert(s);
            }
        }
    }
    // the open borders: a role doubted on a subject's day
    let mut doubted: BTreeSet<(String, i64, String)> = BTreeSet::new();
    let member_set: BTreeSet<i64> = members.iter().copied().collect();
    let sql = format!(
        "SELECT {} FROM {} WHERE kind = '{}' AND status IN ('open', 'staged')",
        crate::text_of(store, "review_item", "ref"),
        store.qualified("review_item"),
        nils_registry::review::PICK_BORDER_KIND
    );
    for r in store.query(&sql, &[]).map_err(failed)? {
        let reference: Value = r
            .opt_text(0)
            .map_err(failed)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        let (Some(subject), Some(role), Some(day)) = (
            reference["subject_id"].as_i64(),
            reference["role"].as_str(),
            reference["session_day"].as_str(),
        ) else {
            continue;
        };
        if member_set.contains(&subject) {
            doubted.insert((
                role.to_string(),
                subject,
                day.get(..10).unwrap_or(day).to_string(),
            ));
        }
    }
    // the stacks of a sample sealed now, named to no one without the grant
    let all_picked: Vec<i64> = picked
        .values()
        .flat_map(|p| p.stacks.iter().copied())
        .collect();
    let withheld = crate::sealed::withheld(store, &caller.access, &all_picked).map_err(failed)?;

    let state_of = |role: &str, subject: i64, day: &str| -> (&'static str, Option<&Picked>) {
        let key = (role.to_string(), subject, day.to_string());
        if doubted.contains(&key) {
            ("doubted", picked.get(&key))
        } else if let Some(p) = picked.get(&key) {
            ("picked", Some(p))
        } else {
            ("missing", None)
        }
    };
    let mut totals: BTreeMap<&str, [usize; 3]> =
        roles.iter().map(|r| (r.as_str(), [0; 3])).collect();
    for (subject, list) in &sessions {
        for (_, day) in list {
            for role in &roles {
                let i = match state_of(role, *subject, day).0 {
                    "picked" => 0,
                    "doubted" => 1,
                    _ => 2,
                };
                if let Some(t) = totals.get_mut(role.as_str()) {
                    t[i] += 1;
                }
            }
        }
    }
    let start = match after {
        None => 0,
        Some(a) => members
            .iter()
            .position(|m| *m == a)
            .map(|i| i + 1)
            .ok_or_else(|| Reply::error(400, format!("after names no member of {name}")))?,
    };
    let page: Vec<i64> = members.iter().skip(start).take(limit).copied().collect();
    let next = if start + page.len() < members.len() {
        page.last().copied()
    } else {
        None
    };
    // the studies of the page's sessions
    let page_sessions: Vec<i64> = page
        .iter()
        .flat_map(|s| sessions.get(s).into_iter().flatten().map(|(id, _)| *id))
        .collect();
    let mut studies: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for chunk in page_sessions.chunks(500) {
        for r in store
            .query(
                &format!(
                    "SELECT session_id, study_id FROM {} WHERE window_days = {w} \
                     AND session_id IN ({}) ORDER BY study_id",
                    store.qualified("session_cache_study"),
                    list(chunk)
                ),
                &[],
            )
            .map_err(failed)?
        {
            studies
                .entry(r.int(0).map_err(failed)?)
                .or_default()
                .push(r.int(1).map_err(failed)?);
        }
    }
    let subjects: Vec<Value> = page
        .iter()
        .map(|subject| {
            let code = codes.get(subject).cloned().unwrap_or_default();
            let list = sessions.get(subject).cloned().unwrap_or_default();
            let sessions: Vec<Value> = list
                .iter()
                .enumerate()
                .map(|(i, (id, day))| {
                    let mut by_role = serde_json::Map::new();
                    for role in &roles {
                        let (state, p) = state_of(role, *subject, day);
                        let hidden =
                            p.is_some_and(|p| p.stacks.iter().any(|s| withheld.contains(s)));
                        let stack = if hidden {
                            None
                        } else {
                            p.and_then(|p| p.stacks.iter().next().copied())
                        };
                        let mut v = json!({
                            "state": state,
                            "stack": stack,
                            "by": p.map(|p| if p.person { "person" } else { "rules" }),
                        });
                        if hidden {
                            v["withheld"] = json!(true);
                        }
                        by_role.insert(role.clone(), v);
                    }
                    let label = format!("ses-{}", day.replace('-', ""));
                    json!({
                        "session": id,
                        "studies": studies.get(id).cloned().unwrap_or_default(),
                        "label": if quasi { label } else { shape(&label) },
                        "number": i + 1,
                        "roles": by_role,
                    })
                })
                .collect();
            json!({
                "id": subject,
                "code": if quasi { code } else { shape(&code) },
                "sessions": sessions,
            })
        })
        .collect();
    let totals: serde_json::Map<String, Value> = totals
        .into_iter()
        .map(|(role, [p, d, m])| {
            (
                role.to_string(),
                json!({"picked": p, "doubted": d, "missing": m}),
            )
        })
        .collect();
    Ok(json!({
        "cohort": {"name": cohort.name, "id": cohort.id},
        "detail": caller.access.detail.name(),
        "roles": roles,
        "total": members.len(),
        "sessions": sessions.values().map(Vec::len).sum::<usize>(),
        "totals": totals,
        "count": subjects.len(),
        "subjects": subjects,
        "next": next,
    }))
}
