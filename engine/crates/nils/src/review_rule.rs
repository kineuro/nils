// SPDX-License-Identifier: AGPL-3.0-only

//! System 1 in the review rule, at the doors (record 55 D3, D6 and D7 with
//! record 56; spec §11; slices A24 and A25).
//!
//! - **Both suggestions and both reasons** (§11.3,
//!   `GET /api/review/{id}/suggestions`): the rules' answer with the rule,
//!   the clause, the fields it read and the text they matched (also on an
//!   axis the rules resolved), and System 1's answer with its probability,
//!   the rules' reliability on the header, the fields that mattered and up
//!   to five similar settled stacks, drawn only from stacks outside every
//!   sealed sample and shown for display, never as votes. Until a recorded
//!   certificate names the System 1 model (D2) its opinion is shown marked
//!   as not confirmed (record 55, T20 as changed on 2026-10-10).
//! - **Blind first** (§11.4): a question about an axis carries a mode drawn
//!   once from the site's share (one in five by default); on a blind-first
//!   question the item and list doors hide NILS's own answers (never the
//!   header text) until the person gives a provisional answer
//!   (`POST /api/review/{id}/provisional`), and the suggestions door
//!   refuses until then. Every answer records whether it was given blind
//!   and the seconds it took ([`mark_applied`]).
//! - **The rounds and the gate** (§11.2, record 56 section 3): what each
//!   round accepted and sent to Review, by kind (`GET /api/review/rounds`),
//!   and the gate over the stacks of datasets flagged as having damaged
//!   headers (`POST /api/review/gate`, and after every System 1 run).
//! - **The damaged-header flag** (record 56 section 3,
//!   `GET`/`PUT /api/datasets/{name}/headers`): a person's flag, or else the
//!   signs (most of a dataset's scans with no series description and no
//!   protocol name).
//! - **System 1 over the archive** (§11.1, `POST /api/review/system1/run`):
//!   the chosen model's pipeline over every MR stack, on request.
//! - **The audit** (§11.5): `POST /api/audit/draws` draws 1 % of a round's
//!   accepted stacks by a seed keyed by DICOM identity and makes the audit
//!   campaign that reads them blind; `POST /api/audit/draws/{id}/grade`
//!   computes the accepted error and its exact bound from the settled
//!   answers. The raters are agents outside the engine, through the
//!   campaign doors (`docs/guides/review-rule-and-audit.md`).

use std::collections::{BTreeMap, BTreeSet};

use nils_registry::Registry;
use nils_registry::audit_draw::{self, Framed};
use nils_registry::blind;
use nils_registry::opinion;
use nils_registry::place::Place;
use nils_registry::round::{self, Round};
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply, json_body};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 13] = [
    "GET /api/review/{id}/suggestions",
    "POST /api/review/{id}/provisional",
    "GET /api/review/rounds",
    "GET /api/review/rounds/{id}",
    "POST /api/review/gate",
    "POST /api/review/system1/run",
    "GET /api/review/settings",
    "GET /api/datasets/{name}/headers",
    "PUT /api/datasets/{name}/headers",
    "POST /api/audit/draws",
    "GET /api/audit/draws",
    "GET /api/audit/draws/{id}",
    "POST /api/audit/draws/{id}/grade",
];

/// Each door's policy row, as `serve::policy` reads them.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/review/{id}/suggestions",
        false,
        false,
        "bounded",
        "one item's stacks",
        "Reading both suggestions",
        "Read both suggestions",
    ),
    (
        "POST /api/review/{id}/provisional",
        true,
        true,
        "free",
        "one answer",
        "Answering blind",
        "Answered blind",
    ),
    (
        "GET /api/review/rounds",
        false,
        false,
        "bounded",
        "every round",
        "Reading the review rounds",
        "Read the review rounds",
    ),
    (
        "GET /api/review/rounds/{id}",
        false,
        false,
        "bounded",
        "one round",
        "Reading a review round",
        "Read a review round",
    ),
    (
        "POST /api/review/gate",
        true,
        true,
        "bounded",
        "the flagged datasets' stacks",
        "Running the gate",
        "Ran the gate",
    ),
    (
        "POST /api/review/system1/run",
        true,
        false,
        "job",
        "one id",
        "Running System 1 over the archive",
        "Ran System 1 over the archive",
    ),
    (
        "GET /api/review/settings",
        false,
        false,
        "free",
        "one document",
        "Reading the review settings",
        "Read the review settings",
    ),
    (
        "GET /api/datasets/{name}/headers",
        false,
        false,
        "bounded",
        "one dataset's counts",
        "Reading a dataset's headers flag",
        "Read a dataset's headers flag",
    ),
    (
        "PUT /api/datasets/{name}/headers",
        true,
        true,
        "free",
        "one flag",
        "Flagging a dataset's headers",
        "Flagged a dataset's headers",
    ),
    (
        "POST /api/audit/draws",
        true,
        false,
        "bounded",
        "one draw",
        "Drawing an audit sample",
        "Drew an audit sample",
    ),
    (
        "GET /api/audit/draws",
        false,
        false,
        "bounded",
        "every draw",
        "Reading the audit draws",
        "Read the audit draws",
    ),
    (
        "GET /api/audit/draws/{id}",
        false,
        false,
        "bounded",
        "one draw",
        "Reading an audit draw",
        "Read an audit draw",
    ),
    (
        "POST /api/audit/draws/{id}/grade",
        true,
        true,
        "bounded",
        "one grade",
        "Grading an audit",
        "Graded an audit",
    ),
];

/// What each door needs, read by `serve::door` before its own table.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    use Detail::{Plain, Quasi};
    Some(match (method, segs) {
        ("GET", ["api", "review", _, "suggestions"])
        | ("GET", ["api", "review", "rounds" | "settings"])
        | ("GET", ["api", "review", "rounds", _]) => (Need::One("review:see"), Plain),
        ("POST", ["api", "review", _, "provisional"]) | ("POST", ["api", "review", "gate"]) => {
            (Need::One("review:work"), Plain)
        }
        // a run reads pixels and headers: Pipelines work at detail quasi,
        // as a step's run is
        ("POST", ["api", "review", "system1", "run"]) => (Need::One("pipelines:work"), Quasi),
        ("GET", ["api", "datasets", _, "headers"]) => {
            (Need::AnyOf(&["data:see", "review:see"]), Plain)
        }
        ("PUT", ["api", "datasets", _, "headers"]) => (Need::One("data:work"), Plain),
        // the audit is a review of what was accepted; its campaign is a
        // campaign, read by its raters through the campaign doors
        ("GET", ["api", "audit", "draws"]) | ("GET", ["api", "audit", "draws", _]) => {
            (Need::AnyOf(&["review:see", "campaigns:see"]), Plain)
        }
        ("POST", ["api", "audit", "draws"]) => (Need::Both("review:work", "campaigns:work"), Plain),
        ("POST", ["api", "audit", "draws", _, "grade"]) => (Need::One("review:work"), Plain),
        _ => return None,
    })
}

fn failed(e: StoreError) -> Reply {
    Reply::error(500, e.to_string())
}

fn id_of(s: &str) -> Result<i64, Reply> {
    s.parse::<i64>()
        .map_err(|_| Reply::error(404, format!("{s} is not an id")))
}

/// The share of questions drawn blind first, and the salt they are drawn
/// with, kept as registry settings.
pub(crate) fn blind_setting(registry: &mut Registry) -> Result<(f64, String), Reply> {
    let share = blind::share_of(
        registry
            .meta_value(blind::SHARE_KEY)
            .map_err(|e| Reply::error(500, e.to_string()))?
            .as_deref(),
    );
    // a site may set its own salt; the default is one word, so the same
    // question of the same registry always draws the same mode
    let salt = registry
        .meta_value(blind::SALT_KEY)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| blind::DEFAULT_SALT.to_string());
    Ok((share, salt))
}

/// A fresh salt: random bytes as hex.
fn uuid_like() -> String {
    let mut b = [0u8; 16];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    hex::encode(b)
}

/// Set the share (`PUT /api/settings` with `review.blind_share`).
pub(crate) fn set_share(registry: &mut Registry, share: f64, who: &str) -> Result<(), Reply> {
    if !(0.0..=1.0).contains(&share) {
        return Err(Reply::error(
            400,
            format!("review.blind_share is a share from 0 to 1, not {share}"),
        ));
    }
    registry
        .set_meta(blind::SHARE_KEY, &share.to_string())
        .map_err(|e| Reply::error(500, e.to_string()))?;
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: who,
            action: nils_registry::audit::Action::SettingsSet,
            scope: json!({"review": {"blind_share": share}}),
            policy: None,
            job_id: None,
            details: None,
        },
    )
    .map_err(|e| Reply::error(500, e.to_string()))?;
    Ok(())
}

/// The review settings as a door reads them.
pub(crate) fn settings_doc(registry: &mut Registry) -> Result<Value, Reply> {
    let (share, _) = blind_setting(registry)?;
    let chosen = opinion::chosen(registry.store()).map_err(failed)?;
    let confirmed = match &chosen {
        Some(c) => opinion::confirmed(registry.store(), c.model.id).map_err(failed)?,
        None => None,
    };
    Ok(json!({
        "blind_share": share, "default_blind_share": blind::DEFAULT_SHARE,
        "system1": {
            "task": opinion::TASK,
            "chosen": chosen.as_ref().map(nils_registry::model::Chosen::as_json),
            "confirmed": confirmed.is_some(), "certificate_id": confirmed,
        },
        "audit_share": audit_draw::DEFAULT_SHARE,
    }))
}

// ------------------------------------------------------- the blind view

/// The mode of each listed review question, drawn where it has none, and
/// for a blind-first one the caller has not answered provisionally, NILS's
/// own answers taken out of what the doors show: the evidence and the
/// members' evidence. The item's kind, scope, reference and members stay,
/// and the stack's header text is read through the reader as before.
pub(crate) fn blind_rows(
    registry: &mut Registry,
    caller: &Caller,
    rows: &mut [Value],
) -> Result<(), Reply> {
    let (share, salt) = blind_setting(registry)?;
    let store = registry.store();
    for row in rows.iter_mut() {
        let (Some(id), Some(kind)) = (row["id"].as_i64(), row["kind"].as_str()) else {
            continue;
        };
        let kind = kind.to_string();
        let mode = blind::mode_of(store, id, &kind, &salt, share).map_err(failed)?;
        row["mode"] = json!(mode);
        if mode == blind::BLIND_FIRST {
            let given = blind::provisional(store, id, &caller.principal).map_err(failed)?;
            row["provisional"] = given.as_ref().map_or(
                Value::Null,
                |p| json!({"answer": p.answer, "seconds": p.seconds}),
            );
            if given.is_none() {
                hide(row);
            }
        }
    }
    Ok(())
}

/// Take NILS's own answers out of an item as a door shows it.
fn hide(row: &mut Value) {
    let axes = axes_asked(&row["kind"], &row["evidence"]);
    row["evidence"] = json!({"blind_first": true, "axes": axes});
    if let Some(members) = row["member_stacks"].as_array_mut() {
        for m in members {
            m["evidence"] = Value::Null;
        }
    }
    if let Some(m) = row.as_object_mut() {
        m.remove("decision");
        m.remove("confidence");
    }
}

/// The axes a question asks: its evidence's list, or the axis its kind
/// names.
fn axes_asked(kind: &Value, evidence: &Value) -> Vec<String> {
    if let Some(list) = evidence["axes"].as_array() {
        return list
            .iter()
            .filter_map(|a| a.as_str().map(str::to_string))
            .collect();
    }
    kind.as_str()
        .and_then(|k| k.split_once(':'))
        .map(|(a, _)| vec![a.to_string()])
        .unwrap_or_default()
}

/// After an answer to a review question was applied: mark the decisions it
/// wrote with whether it was given blind, the seconds it took and the
/// question's mode. Blind where the question is blind first and the
/// answer is the provisional one the person gave before the suggestions
/// showed; a question nobody opened through the doors is timed by none.
pub(crate) fn mark_applied(
    registry: &mut Registry,
    caller: &Caller,
    item: i64,
    kind: &str,
    given: &Value,
    decisions: &[i64],
) -> Result<Value, Reply> {
    let (share, salt) = blind_setting(registry)?;
    let store = registry.store();
    let mode = blind::mode_of(store, item, kind, &salt, share).map_err(failed)?;
    let seconds = blind::seconds_since_read(store, item, &caller.principal).map_err(failed)?;
    let blind_given = if mode == blind::BLIND_FIRST {
        blind::provisional(store, item, &caller.principal)
            .map_err(failed)?
            .is_some_and(|p| blind::same_answer(&p.answer, given))
    } else {
        false
    };
    blind::mark_decision(store, decisions, blind_given, seconds, mode).map_err(failed)?;
    Ok(json!({"mode": mode, "blind": blind_given, "seconds": seconds}))
}

// --------------------------------------------- both suggestions and reasons

/// The newest decisions people settled on an axis, of stacks like this
/// one (the same scanner make and the same series description, else the
/// same protocol name), outside every sealed sample: up to five, for
/// display only.
fn similar(store: &mut Store, stack: i64, axis: &str) -> Result<Vec<Value>, StoreError> {
    let d = store.dialect();
    let me = store.query_opt(
        &format!(
            "SELECT manufacturer, text_series_description_ci, text_protocol_name_ci FROM {} WHERE stack_id = {}",
            store.qualified("stack_fingerprint"),
            d.param(1, nils_registry::schema::Type::Int)
        ),
        &[nils_registry::store::Param::Int(stack)],
    )?;
    let Some(me) = me else {
        return Ok(Vec::new());
    };
    let make = me.opt_text(0)?.map(str::to_string);
    let mut out = Vec::new();
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    for (column, value) in [
        (
            "text_series_description_ci",
            me.opt_text(1)?.map(str::to_string),
        ),
        ("text_protocol_name_ci", me.opt_text(2)?.map(str::to_string)),
    ] {
        let Some(value) = value.filter(|v| !v.trim().is_empty()) else {
            continue;
        };
        let by_make = if make.is_some() {
            format!(
                " AND f.manufacturer = {}",
                d.param(3, nils_registry::schema::Type::Text)
            )
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT de.ref, de.value, f.text_series_description FROM {} de JOIN {} f \
             ON de.scope = 'stack' AND f.stack_id = CAST(de.ref AS INTEGER) \
             WHERE de.axis = {} AND de.withdrawn_at IS NULL AND de.author_kind IN ('person', 'agent') \
             AND (de.staged_at IS NULL OR de.committed_at IS NOT NULL) AND f.{column} = {}{by_make} \
             ORDER BY de.id DESC LIMIT 40",
            store.qualified("decision"),
            store.qualified("stack_fingerprint"),
            d.param(1, nils_registry::schema::Type::Text),
            d.param(2, nils_registry::schema::Type::Text),
        );
        let mut params = vec![
            nils_registry::store::Param::from(axis),
            nils_registry::store::Param::from(value.as_str()),
        ];
        if let Some(m) = &make {
            params.push(nils_registry::store::Param::from(m.as_str()));
        }
        let rows = store.query(&sql, &params)?;
        let mut found: Vec<(i64, Option<String>, Option<String>)> = Vec::new();
        for r in &rows {
            let Ok(s) = r.text(0)?.parse::<i64>() else {
                continue;
            };
            if s == stack || !seen.insert(s) {
                continue;
            }
            found.push((
                s,
                r.opt_text(1)?.map(str::to_string),
                r.opt_text(2)?.map(str::to_string),
            ));
        }
        let ids: Vec<i64> = found.iter().map(|f| f.0).collect();
        let (sealed, _) = nils_registry::labels::sealed_now(store, &ids, &[])
            .map_err(|e| StoreError::Message(e.to_string()))?;
        for (s, value, description) in found {
            if sealed.contains(&s) {
                continue;
            }
            out.push(json!({
                "stack_id": s, "value": value, "series_description": description,
                "alike_by": column.trim_start_matches("text_").trim_end_matches("_ci"),
            }));
            if out.len() >= 5 {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// The rules' suggestion and reason on one axis of a stack, from the
/// reader's evidence line (record 48 R1): the value in force, who set it,
/// the rule, the clause, the fields it read and their values, the text it
/// matched at detail quasi, the other rules that voted and the line.
fn rules_reason(why: &Value, axis: &str) -> Value {
    let Some(entry) = why["axes"]
        .as_array()
        .and_then(|a| a.iter().find(|e| e["axis"] == axis))
    else {
        return json!({"axis": axis, "value": Value::Null, "line": format!("{axis}: no rule answered"), "decided": Value::Null});
    };
    json!({
        "axis": axis, "value": entry["value"], "set_by": entry["set_by"],
        "decided": entry["decided"], "voted": entry["voted"], "line": entry["line"],
        "overridden": entry.get("overridden").cloned().unwrap_or(Value::Null),
        "tier": entry["tier"], "basis": entry["basis"],
    })
}

/// `GET /api/review/{id}/suggestions`: both suggestions and both reasons
/// for the stacks a question asks about (one member with `member`, or the
/// first `limit` of a group), refused on a blind-first question until the
/// caller's provisional answer is in, and noted as shown.
fn suggestions(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    item: i64,
    query: &std::collections::HashMap<String, String>,
) -> Result<Reply, Reply> {
    let it = nils_registry::review::item(registry.store(), item)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .ok_or_else(|| Reply::error(404, format!("no review item {item}")))?;
    let (share, salt) = blind_setting(registry)?;
    let store = registry.store();
    let mode = blind::mode_of(store, item, &it.kind, &salt, share).map_err(failed)?;
    let given = blind::provisional(store, item, &caller.principal).map_err(failed)?;
    if mode == blind::BLIND_FIRST && given.is_none() {
        return Err(Reply::error(
            409,
            format!(
                "review item {item} is read blind first: give your provisional answer first (POST /api/review/{item}/provisional), and the suggestions show after it"
            ),
        ));
    }
    let mut stacks: Vec<i64> = match it.reference["stack_id"].as_i64() {
        Some(s) => vec![s],
        None => nils_registry::review::members(store, item)
            .map_err(|e| Reply::error(500, e.to_string()))?
            .iter()
            .map(|m| m.stack_id)
            .collect(),
    };
    if let Some(m) = query.get("member").and_then(|m| m.parse::<i64>().ok()) {
        if !stacks.contains(&m) {
            return Err(Reply::error(
                404,
                format!("stack {m} is not a member of review item {item}"),
            ));
        }
        stacks = vec![m];
    }
    let limit = query
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 200);
    let total = stacks.len();
    stacks.truncate(limit);
    // record 48: a stack of a sample sealed now is not read here at all
    let (sealed, _) = nils_registry::labels::sealed_now(store, &stacks, &[])
        .map_err(|e| Reply::error(500, e.to_string()))?;
    stacks.retain(|s| !sealed.contains(s));
    let axes = axes_asked(&json!(it.kind), &it.evidence);
    let chosen = opinion::chosen(store).map_err(failed)?;
    let model = chosen.as_ref().map(|c| c.model.clone());
    let certificate = match &model {
        Some(m) => opinion::confirmed(store, m.id).map_err(failed)?,
        None => None,
    };
    let said = opinion::newest(store, &stacks, model.as_ref().map(|m| m.id)).map_err(failed)?;
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let mut out = Vec::new();
    for &stack in &stacks {
        let why = crate::reader::why(store, stack, pack.as_deref(), quasi, false)
            .map_err(failed)?
            .unwrap_or(Value::Null);
        let mut per_axis = Vec::new();
        for axis in &axes {
            let o = said.get(&stack).and_then(|a| a.get(axis));
            let system1 = match o {
                None => Value::Null,
                Some(o) => {
                    let mut v = o.as_json();
                    v["model"] = model.as_ref().map_or(Value::Null, |m| m.named());
                    v["similar"] = json!(similar(store, stack, axis).map_err(failed)?);
                    v["agrees"] = json!(
                        rules_reason(&why, axis)["value"]
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| {
                                rules_reason(&why, axis)["value"].as_array().map(|l| {
                                    let mut w: Vec<String> = l
                                        .iter()
                                        .filter_map(|x| x.as_str().map(str::to_string))
                                        .collect();
                                    w.sort();
                                    w.join(",")
                                })
                            })
                            == o.value
                    );
                    v
                }
            };
            per_axis.push(json!({
                "axis": axis,
                "rules": rules_reason(&why, axis),
                "system1": system1,
            }));
        }
        out.push(json!({"stack_id": stack, "axes": per_axis, "header": why["header"]}));
    }
    blind::revealed(store, item, &caller.principal).map_err(failed)?;
    let status = match (&model, certificate) {
        (None, _) => "no_model",
        (Some(_), Some(_)) => "confirmed",
        (Some(_), None) => "not_confirmed",
    };
    Ok(Reply::ok(json!({
        "item": item, "kind": it.kind, "mode": mode,
        "provisional": given.map(|p| json!({"answer": p.answer, "seconds": p.seconds})),
        "system1": {
            "status": status, "model": model.as_ref().map(|m| m.named()),
            "certificate_id": certificate,
            "why": match status {
                "no_model" => "no System 1 model is chosen for its task",
                "not_confirmed" => "System 1 is not yet confirmed: no recorded certificate names it (D2); its opinion is a research model's",
                _ => "System 1 is confirmed by a recorded certificate",
            },
        },
        "stacks": out, "total_stacks": total, "sealed_left_out": sealed.len(),
        "similar_note": "similar settled stacks are shown for display, never as votes, and never from a sealed sample",
    })))
}

/// `POST /api/review/{id}/provisional`: a blind-first question's
/// provisional answer, `{value}`, `{nothing: true}` or `{values: {axis:
/// value}}`, given before any suggestion showed.
fn provisional(
    registry: &mut Registry,
    caller: &Caller,
    item: i64,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let it = nils_registry::review::item(registry.store(), item)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .ok_or_else(|| Reply::error(404, format!("no review item {item}")))?;
    let (share, salt) = blind_setting(registry)?;
    let store = registry.store();
    let mode = blind::mode_of(store, item, &it.kind, &salt, share).map_err(failed)?;
    if mode != blind::BLIND_FIRST {
        return Err(Reply::error(
            409,
            format!(
                "review item {item} is routine: its suggestions show at once, and it is answered with apply"
            ),
        ));
    }
    let answer = if doc["values"].is_object() {
        json!({"values": doc["values"]})
    } else if doc["nothing"].as_bool() == Some(true) {
        json!({"nothing": true})
    } else if let Some(v) = doc.get("value").filter(|v| v.is_string() || v.is_array()) {
        json!({"value": v})
    } else {
        return Err(Reply::error(
            400,
            "the provisional answer: value, nothing: true, or values: {axis: value}",
        ));
    };
    let given = blind::give_provisional(store, item, &caller.principal, &answer).map_err(failed)?;
    Ok(Reply::ok(json!({
        "item": item, "mode": mode, "answer": given.answer, "seconds": given.seconds,
        "answered_at": given.answered_at, "suggestions": format!("/api/review/{item}/suggestions"),
    })))
}

// ----------------------------------------------------- the flag and gate

/// A dataset's damaged-header signs: of its MR scans, how many carry no
/// series description and no protocol name, and whether that is most of
/// them (record 56 section 3: missing series descriptions are a sign;
/// until System 1's reliability model serves, the share decides).
fn signs(store: &mut Store, sources: &[i64]) -> Result<Value, StoreError> {
    if sources.is_empty() {
        return Ok(json!({"scans": 0, "no_names": 0, "share": 0.0, "damaged": false}));
    }
    let list = sources
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let holds = crate::operations::held_by(store, "x", &list);
    let sql = format!(
        "SELECT COUNT(*), SUM(CASE WHEN COALESCE(TRIM(f.text_series_description), '') = '' \
         AND COALESCE(TRIM(f.text_protocol_name), '') = '' THEN 1 ELSE 0 END) \
         FROM {} x JOIN {} f ON f.stack_id = x.id WHERE {holds} AND UPPER(COALESCE(f.modality, '')) = 'MR'",
        store.qualified("stack"),
        store.qualified("stack_fingerprint"),
    );
    let (n, bare) = match store.query_opt(&sql, &[])? {
        Some(r) => (r.int(0)?, r.opt_int(1)?.unwrap_or(0)),
        None => (0, 0),
    };
    let share = if n > 0 { bare as f64 / n as f64 } else { 0.0 };
    Ok(json!({
        "scans": n, "no_names": bare, "share": share,
        "damaged": n > 0 && share >= SIGNS_SHARE,
        "rule": format!("flagged by its signs where at least {:.0} % of its MR scans carry neither a series description nor a protocol name", SIGNS_SHARE * 100.0),
    }))
}

/// The share of nameless scans at which the signs flag a dataset.
const SIGNS_SHARE: f64 = 0.5;

/// A dataset's flag: a person's where one was set, else the signs'.
fn flag_of(store: &mut Store, place: &Place, sources: &[i64]) -> Result<Value, StoreError> {
    let set = round::headers_flag(store, place.id)?;
    let signs = signs(store, sources)?;
    let damaged = match &set {
        Some(f) => f["damaged"].as_bool().unwrap_or(false),
        None => signs["damaged"].as_bool().unwrap_or(false),
    };
    Ok(json!({
        "dataset": place.name, "damaged": damaged,
        "by": if set.is_some() { "person" } else { "signs" },
        "flag": set, "signs": signs,
        "gate": if damaged {
            "the full gate: System 1 must agree with the rules on each scan above its thresholds, once System 1 is confirmed"
        } else {
            "the rules decide; Review holds only the axes that matter they left empty"
        },
    }))
}

/// Every source dataset flagged as having damaged headers, with its
/// sources.
fn flagged(registry: &mut Registry) -> Result<Vec<(Place, Vec<i64>)>, StoreError> {
    let store = registry.store();
    let mut out = Vec::new();
    for p in nils_registry::place::active(store)? {
        if p.role != nils_registry::place::Role::Source {
            continue;
        }
        let sources = crate::sources::source_ids(store, &p)?;
        let f = flag_of(store, &p, &sources)?;
        if f["damaged"].as_bool() == Some(true) {
            out.push((p, sources));
        }
    }
    Ok(out)
}

/// The stacks of `stacks` (all, where none is given) held by a flagged
/// dataset, by the pack and version they were sorted with.
fn flagged_stacks(
    registry: &mut Registry,
    only: Option<&[i64]>,
    dataset: Option<&str>,
) -> Result<BTreeMap<(String, String), Vec<i64>>, StoreError> {
    let mut sources: Vec<i64> = Vec::new();
    for (p, s) in flagged(registry)? {
        if dataset.is_some_and(|d| d != p.name) {
            continue;
        }
        sources.extend(s);
    }
    let mut out: BTreeMap<(String, String), Vec<i64>> = BTreeMap::new();
    if sources.is_empty() {
        return Ok(out);
    }
    let store = registry.store();
    let list = sources
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let holds = crate::operations::held_by(store, "x", &list);
    let sql = format!(
        "SELECT x.id, c.pack, c.pack_version FROM {} x JOIN {} c ON c.stack_id = x.id WHERE {holds} ORDER BY x.id",
        store.qualified("stack"),
        store.qualified("classification"),
    );
    let keep: Option<BTreeSet<i64>> = only.map(|o| o.iter().copied().collect());
    for r in store.query(&sql, &[])? {
        let id = r.int(0)?;
        if keep.as_ref().is_some_and(|k| !k.contains(&id)) {
            continue;
        }
        out.entry((r.text(1)?.to_string(), r.text(2)?.to_string()))
            .or_default()
            .push(id);
    }
    Ok(out)
}

/// Run the gate over the flagged datasets' stacks (of `only`, where
/// given), round by round.
fn gate_over(
    registry: &mut Registry,
    only: Option<&[i64]>,
    dataset: Option<&str>,
    who: &str,
) -> Result<Value, StoreError> {
    let groups = flagged_stacks(registry, only, dataset)?;
    let mut rounds = Vec::new();
    for ((pack, version), stacks) in groups {
        let store = registry.store();
        let r = round::current(store, &pack, &version, who)?;
        let done = round::gate(store, &r, &stacks, who)?;
        rounds.push(json!({"round": r.as_json(), "gated": done.to_json()}));
    }
    Ok(json!({"rounds": rounds}))
}

/// After a System 1 run: the gate over its stacks of flagged datasets.
pub(crate) fn after_opinions(
    _home: &nils_registry::Home,
    registry: &mut Registry,
    stacks: &[i64],
    who: &str,
) -> Value {
    gate_over(registry, Some(stacks), None, who).unwrap_or_else(|e| json!({"error": e.to_string()}))
}

// --------------------------------------------------------- the rounds

/// The condition a stack aliased `x` meets when it was sorted with a
/// round's rules version.
fn of_round(store: &Store, r: &Round) -> String {
    format!(
        "x.id IN (SELECT c.stack_id FROM {} c WHERE c.pack = '{}' AND c.pack_version = '{}')",
        store.qualified("classification"),
        r.pack.replace('\'', "''"),
        r.pack_version.replace('\'', "''"),
    )
}

/// Whether a question's kind is of a class.
type KindTest = fn(&str) -> bool;

/// What a round accepted and sent to Review (record 55 T20 as changed on
/// 2026-10-10): its scans, the ones a sort question waits on by kind, the
/// gate's outcomes on flagged datasets, System 1's coverage, the shares,
/// the blind marks of the answers, and its audits.
fn report(registry: &mut Registry, r: &Round) -> Result<Value, StoreError> {
    let store = registry.store();
    let holds = of_round(store, r);
    let scans: i64 = store
        .query_opt(
            &format!(
                "SELECT COUNT(*) FROM {} x WHERE {holds}",
                store.qualified("stack")
            ),
            &[],
        )?
        .map(|row| row.int(0))
        .transpose()?
        .unwrap_or(0);
    let asked = crate::certainty::Asks::sort(store)?.count(store, &holds)?;
    // every waiting question about these scans, whoever asks it, to show
    // that none is a conflict, a low confidence, body part or post-contrast
    let all = crate::certainty::Asks::of(store, crate::certainty::WAITING, &|_| true)?
        .count(store, &holds)?;
    let kinds: [(&str, KindTest); 4] = [
        ("conflict", |k: &str| k.ends_with(":conflict")),
        ("low_confidence", |k: &str| k.ends_with(":low_confidence")),
        ("body_part", |k: &str| {
            k.starts_with("body_part:") || k.starts_with("body_region:")
        }),
        ("post_contrast", |k: &str| k.starts_with("post_contrast:")),
    ];
    let never: BTreeMap<&str, i64> = kinds
        .into_iter()
        .map(|(name, f)| {
            let n: i64 = asked
                .kinds
                .iter()
                .filter(|(k, _)| f(k))
                .map(|(_, n)| *n)
                .sum();
            (name, n)
        })
        .collect();
    let (outcomes, gate_accepted) = round::outcomes(store, r.id)?;
    let held = round::held_back(store, r.id)?;
    let with_opinion = opinion::stacks_with(store, &holds, r.model_id)?;
    let accepted = (scans - asked.scans - held.len() as i64).max(0);
    let share = |n: i64| {
        if scans > 0 {
            json!(n as f64 / scans as f64)
        } else {
            Value::Null
        }
    };
    let draws: Vec<Value> = audit_draw::list(store, Some(r.id))?
        .iter()
        .map(audit_draw::Draw::as_json)
        .collect();
    Ok(json!({
        "round": r.as_json(),
        "scans": scans,
        "accepted": accepted,
        "reviewed": asked.scans,
        "gate_held": held.len(),
        "shares": {"accepted": share(accepted), "review": share(asked.scans + held.len() as i64)},
        "review_by_kind": asked.kinds,
        "waiting_by_kind": all.kinds,
        "never_asked": never,
        "gate": {"outcomes": outcomes, "accepted_stacks": gate_accepted},
        "system1": {"model_id": r.model_id, "scans_with_opinion": with_opinion,
                    "coverage": share(with_opinion), "confirmed": r.confirmed},
        "answers": blind::decision_counts(store)?,
        "audits": draws,
    }))
}

/// The round of the served rules and the chosen model, as it would be;
/// none is begun by reading it.
fn current_doc(doors: &Doors, registry: &mut Registry) -> Result<Value, StoreError> {
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let chosen = opinion::chosen(registry.store())?;
    let confirmed = match &chosen {
        Some(c) => opinion::confirmed(registry.store(), c.model.id)?.is_some(),
        None => false,
    };
    Ok(json!({
        "pack": pack.as_ref().map(|p| p.name.clone()),
        "pack_version": pack.as_ref().map(|p| p.version.to_string()),
        "model": chosen.as_ref().map(|c| c.model.named()),
        "confirmed": confirmed,
    }))
}

// ------------------------------------------------------------ the audit

/// The axes an audit reads unless told: the header certificate's
/// (record 48), with the axes the pack derives from them.
pub(crate) const AUDIT_AXES: [&str; 5] =
    ["provenance", "technique", "modifier", "construct", "base"];

/// A round's frame: the stacks sorted under its rules version, MR, not of
/// a sample sealed now, that no sort question waits on and the gate did not
/// hold back, each with its DICOM identity and the rules' answer on the
/// audit's axes (a person's decision on an axis is left out of what was
/// accepted).
fn frame(store: &mut Store, r: &Round, axes: &[String]) -> Result<Vec<Framed>, StoreError> {
    let holds = of_round(store, r);
    let sql = format!(
        "SELECT x.id, st.study_instance_uid, se.series_instance_uid, x.stack_key FROM {} x \
         JOIN {} se ON se.id = x.series_id JOIN {} st ON st.id = se.study_id \
         LEFT JOIN {} f ON f.stack_id = x.id \
         WHERE {holds} AND UPPER(COALESCE(NULLIF(x.modality, ''), f.modality, '')) = 'MR' ORDER BY x.id",
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("study"),
        store.qualified("stack_fingerprint"),
    );
    let mut rows: Vec<(i64, String, String, String)> = Vec::new();
    for row in store.query(&sql, &[])? {
        rows.push((
            row.int(0)?,
            row.text(1)?.to_string(),
            row.text(2)?.to_string(),
            row.text(3)?.to_string(),
        ));
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let (sealed, _) = nils_registry::labels::sealed_now(store, &ids, &[])
        .map_err(|e| StoreError::Message(e.to_string()))?;
    let asked: BTreeSet<i64> = crate::certainty::Asks::sort(store)?
        .stacks(store, &holds)?
        .into_iter()
        .collect();
    let held = round::held_back(store, r.id)?;
    let keep: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|s| !sealed.contains(s) && !asked.contains(s) && !held.contains(s))
        .collect();
    let answers = round::rules_answers(store, &keep)?;
    let keep: BTreeSet<i64> = keep.into_iter().collect();
    let mut out = Vec::new();
    for (id, study, series, key) in rows {
        if !keep.contains(&id) {
            continue;
        }
        let a = answers.get(&id);
        let mut accepted = serde_json::Map::new();
        for axis in axes {
            match a.and_then(|m| m.get(axis)) {
                Some((_, true)) => {}
                Some((v, false)) => {
                    accepted.insert(axis.clone(), json!(v));
                }
                None => {
                    accepted.insert(axis.clone(), Value::Null);
                }
            }
        }
        out.push(Framed {
            stack_id: id,
            study_uid: study,
            series_uid: series,
            stack_key: key,
            accepted: Value::Object(accepted),
        });
    }
    Ok(out)
}

/// `POST /api/audit/draws`: a round's audit draw and the campaign that
/// reads it. Body: `round` (a round's id; the round of the served rules
/// and the chosen model where none is named), `seed` (a word; one is drawn
/// and kept where none is given), `share` (1 % by default), `axes` (the
/// header certificate's by default), `raters` and `adjudicators` (the
/// agent principals; anyone with `campaigns:work` where none are named),
/// `raters_per_item` (two by default) and `name`.
fn draw_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let r = match doc["round"].as_i64() {
        Some(id) => round::get(registry.store(), id)
            .map_err(failed)?
            .ok_or_else(|| Reply::error(404, format!("no review round {id}")))?,
        None => {
            let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack)
                .ok_or_else(|| {
                    Reply::error(409, "no pack is served here, so there is no round to audit")
                })?;
            round::current(
                registry.store(),
                &pack.name,
                &pack.version.to_string(),
                &caller.principal,
            )
            .map_err(failed)?
        }
    };
    let share = doc["share"].as_f64().unwrap_or(audit_draw::DEFAULT_SHARE);
    let seed = doc["seed"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("audit-round-{}-{}", r.id, uuid_like()));
    let axes: Vec<String> = match doc["axes"].as_array() {
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|a| {
                a.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| Reply::error(400, "axes: a list of the pack's axes"))
            })
            .collect::<Result<_, _>>()?,
        _ => AUDIT_AXES.iter().map(|a| a.to_string()).collect(),
    };
    let framed = frame(registry.store(), &r, &axes).map_err(failed)?;
    let (d, chosen) = audit_draw::draw(
        registry.store(),
        r.id,
        &seed,
        share,
        &framed,
        &caller.principal,
    )
    .map_err(|e| Reply::error(409, e))?;
    // the campaign: the drawn stacks frozen, an axes question as the
    // reference reads were asked, nothing suggested, closing into nothing
    let epoch = registry.meta().epoch;
    let rows: Vec<nils_registry::store::Row> = {
        let store = registry.store();
        let ids: Vec<i64> = chosen.iter().map(|f| f.stack_id).collect();
        let mut subject: BTreeMap<i64, i64> = BTreeMap::new();
        for chunk in ids.chunks(500) {
            let sql = format!(
                "SELECT x.id, se.subject_id FROM {} x JOIN {} se ON se.id = x.series_id WHERE x.id IN ({})",
                store.qualified("stack"),
                store.qualified("series"),
                chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for row in store.query(&sql, &[]).map_err(failed)? {
                subject.insert(row.int(0).map_err(failed)?, row.int(1).map_err(failed)?);
            }
        }
        ids.iter()
            .map(|s| {
                nils_registry::store::Row(vec![
                    nils_registry::store::Cell::Int(*s),
                    subject
                        .get(s)
                        .map_or(nils_registry::store::Cell::Null, |v| {
                            nils_registry::store::Cell::Int(*v)
                        }),
                ])
            })
            .collect()
    };
    let (handle, _) = crate::operations::save_stacks(
        registry.store(),
        rows,
        &caller.principal,
        epoch,
        &crate::operations::Pinned {
            pack_version: Some(&r.pack_version),
            ..Default::default()
        },
    )
    .map_err(|e| Reply::error(500, e))?;
    let mut question = json!({"kind": "axes", "axes": axes});
    let pack = doors
        .pack_dir
        .as_ref()
        .and_then(|dir| nils_pack::load(&dir.join(&doors.ask_pack), None).ok());
    crate::campaigns::complete_axes(&mut question, pack.as_ref())
        .map_err(|(s, m)| Reply::error(s, m))?;
    let keys =
        crate::campaigns::handle_keys(registry.store(), handle, nils_ask::ast::Grain::Stack)?;
    let items = crate::campaigns::items_of(registry.store(), nils_ask::ast::Grain::Stack, &keys)?;
    let hash = crate::campaigns::content_hash(registry.store(), handle)?;
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect()
    };
    let name = doc["name"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("audit-round-{}-draw-{}", r.id, d.id));
    let adjudication = json!({"when": "disagree", "metric": "exact"});
    let made = nils_registry::campaign::create(
        registry,
        &nils_registry::campaign::New {
            name: &name,
            owner: &caller.principal,
            question: &question,
            source: json!({"handle": handle, "audit_draw": d.id}),
            items,
            handle_id: Some(handle),
            content_hash: hash.as_deref(),
            pack_version: Some(&r.pack_version),
            raters_per_item: doc["raters_per_item"].as_i64().unwrap_or(2),
            raters: strings(&doc["raters"]),
            adjudicators: strings(&doc["adjudicators"]),
            adjudication: &adjudication,
            closes_into: "none",
            lease_seconds: doc["lease_seconds"].as_i64().unwrap_or(3600),
            inputs: BTreeMap::new(),
            hold_back: None,
            suggest: Some(nils_registry::campaign::Suggest::None),
            hide_header: false,
        },
    )
    .map_err(crate::campaigns::campaign_err)?;
    blind::set_purpose(registry.store(), made.id, "audit").map_err(|e| Reply::error(500, e))?;
    audit_draw::set_campaign(registry.store(), d.id, made.id).map_err(failed)?;
    let d = audit_draw::get(registry.store(), d.id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(500, "the draw was not kept"))?;
    Ok(Reply::ok(json!({
        "draw": d.as_json(), "round": r.as_json(),
        "campaign": {"id": made.id, "name": made.name, "purpose": "audit", "suggest": "none",
                      "closes_into": "none", "raters_per_item": made.raters_per_item,
                      "trains": false},
        "axes": axes,
        "raters": "agents outside the engine read each item blind through the campaign doors; their disagreements are settled by the adjudicator; no person reads (record 55, ruling 13)",
    })))
}

/// `POST /api/audit/draws/{id}/grade`: the accepted error of the draw's
/// round and its exact bound, from the audit campaign's settled answers.
fn grade_door(registry: &mut Registry, caller: &Caller, id: i64) -> Result<Reply, Reply> {
    let d = audit_draw::get(registry.store(), id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(404, format!("no audit draw {id}")))?;
    let campaign = d
        .campaign_id
        .ok_or_else(|| Reply::error(409, format!("audit draw {id} has no campaign to grade")))?;
    let c = nils_registry::campaign::get(registry.store(), campaign)
        .map_err(crate::campaigns::campaign_err)?
        .ok_or_else(|| Reply::error(404, format!("no campaign {campaign}")))?;
    let labels = nils_registry::labels::campaign_labels(
        registry.store(),
        campaign,
        nils_registry::labels::Of::Outcomes,
    )
    .map_err(|e| Reply::error(500, e.to_string()))?;
    let mut reads: BTreeMap<i64, BTreeMap<String, Value>> = BTreeMap::new();
    for l in labels {
        if let Some(s) = l.stack_id {
            reads
                .entry(s)
                .or_default()
                .insert(l.what.clone(), l.value.map_or(Value::Null, Value::String));
        }
    }
    let reads: Vec<audit_draw::Read> = reads
        .into_iter()
        .map(|(stack_id, settled)| audit_draw::Read { stack_id, settled })
        .collect();
    let closed = c.status == "closed";
    let g = audit_draw::grade(registry.store(), &d, &reads, closed, &caller.principal)
        .map_err(failed)?;
    Ok(Reply::ok(
        json!({"draw": id, "round_id": d.round_id, "grade": g}),
    ))
}

// ------------------------------------------------------------- routing

/// The doors of this module, before the engine's own table; none where the
/// path is not one of them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
    body: &str,
    quasi: bool,
) -> Option<Result<Reply, Reply>> {
    let get = method == "GET";
    let post = method == "POST";
    let put = method == "PUT";
    Some(match segs {
        ["api", "review", "rounds"] if get => (|| {
            let rounds = round::list(registry.store()).map_err(failed)?;
            let mut out = Vec::new();
            for r in &rounds {
                out.push(report(registry, r).map_err(failed)?);
            }
            Ok(Reply::ok(json!({
                "current": current_doc(doors, registry).map_err(failed)?,
                "rounds": out,
            })))
        })(),
        ["api", "review", "rounds", id] if get => (|| {
            let id = id_of(id)?;
            let r = round::get(registry.store(), id)
                .map_err(failed)?
                .ok_or_else(|| Reply::error(404, format!("no review round {id}")))?;
            Ok(Reply::ok(report(registry, &r).map_err(failed)?))
        })(),
        ["api", "review", "settings"] if get => settings_doc(registry).map(Reply::ok),
        ["api", "review", "gate"] if post => (|| {
            let doc = json_body(body)?;
            let done = gate_over(registry, None, doc["dataset"].as_str(), &caller.principal)
                .map_err(failed)?;
            Ok(Reply::ok(done))
        })(),
        ["api", "review", "system1", "run"] if post => archive_run(doors, registry, caller),
        ["api", "review", id, "suggestions"] if get => {
            id_of(id).and_then(|id| suggestions(doors, registry, caller, quasi, id, query))
        }
        ["api", "review", id, "provisional"] if post => {
            id_of(id).and_then(|id| provisional(registry, caller, id, body))
        }
        ["api", "datasets", name, "headers"] if get || put => (|| {
            let place = crate::scans::dataset_named(registry, name)?;
            let store = registry.store();
            let sources = crate::sources::source_ids(store, &place).map_err(failed)?;
            if put {
                let doc = json_body(body)?;
                let damaged = match &doc["damaged"] {
                    Value::Bool(b) => Some(*b),
                    Value::Null => None,
                    _ => {
                        return Err(Reply::error(
                            400,
                            "damaged: true, false, or null to let the signs decide",
                        ));
                    }
                };
                let why = doc["why"].as_str().map(str::trim).filter(|w| !w.is_empty());
                if damaged.is_some() && why.is_none() {
                    return Err(Reply::error(400, "why: what shows it, in a few words"));
                }
                round::set_headers_flag(store, place.id, damaged, why, &caller.principal)
                    .map_err(failed)?;
            }
            Ok(Reply::ok(flag_of(store, &place, &sources).map_err(failed)?))
        })(),
        ["api", "audit", "draws"] if post => draw_door(doors, registry, caller, body),
        ["api", "audit", "draws"] if get => (|| {
            let round = query.get("round").and_then(|r| r.parse::<i64>().ok());
            let draws: Vec<Value> = audit_draw::list(registry.store(), round)
                .map_err(failed)?
                .iter()
                .map(audit_draw::Draw::as_json)
                .collect();
            Ok(Reply::ok(json!({"draws": draws})))
        })(),
        ["api", "audit", "draws", id] if get => (|| {
            let id = id_of(id)?;
            let d = audit_draw::get(registry.store(), id)
                .map_err(failed)?
                .ok_or_else(|| Reply::error(404, format!("no audit draw {id}")))?;
            let mut v = d.as_json();
            // the drawn stacks by their DICOM identity and what was
            // accepted, for whoever reads the review; a rater reads the
            // items blind through the campaign doors instead
            if caller.access.holds("review:see") {
                let drawn: Vec<Value> = audit_draw::stacks(registry.store(), id)
                    .map_err(failed)?
                    .iter()
                    .map(|f| {
                        json!({"stack_id": f.stack_id, "study_uid": f.study_uid,
                               "series_uid": f.series_uid, "stack_key": f.stack_key,
                               "accepted": f.accepted})
                    })
                    .collect();
                v["stacks"] = json!(drawn);
            }
            Ok(Reply::ok(v))
        })(),
        ["api", "audit", "draws", id, "grade"] if post => {
            id_of(id).and_then(|id| grade_door(registry, caller, id))
        }
        _ => return None,
    })
}

/// `POST /api/review/system1/run`: System 1 over every MR stack of the
/// registry (the archive, on request), as one pipeline job.
fn archive_run(doors: &Doors, registry: &mut Registry, caller: &Caller) -> Result<Reply, Reply> {
    let planned = crate::operations::plan(registry, crate::system1::STEP).map_err(failed)?;
    let plan = planned.map_err(|r| r.reply(crate::system1::STEP))?;
    let epoch = registry.meta().epoch;
    let store = registry.store();
    let sql = format!(
        "SELECT x.id, se.subject_id FROM {} x JOIN {} se ON se.id = x.series_id \
         LEFT JOIN {} f ON f.stack_id = x.id \
         WHERE UPPER(COALESCE(NULLIF(x.modality, ''), f.modality, '')) = 'MR' ORDER BY x.id",
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("stack_fingerprint"),
    );
    let rows: Vec<nils_registry::store::Row> = store
        .query(&sql, &[])
        .map_err(failed)?
        .iter()
        .map(|r| {
            Ok(nils_registry::store::Row(vec![
                nils_registry::store::Cell::Int(r.int(0)?),
                r.opt_int(1)?.map_or(
                    nils_registry::store::Cell::Null,
                    nils_registry::store::Cell::Int,
                ),
            ]))
        })
        .collect::<Result<_, StoreError>>()
        .map_err(failed)?;
    if rows.is_empty() {
        return Err(Reply::error(
            409,
            "there are no MR scans to run System 1 over",
        ));
    }
    let (handle, frozen) = crate::operations::save_stacks(
        store,
        rows,
        &caller.principal,
        epoch,
        &crate::operations::Pinned::default(),
    )
    .map_err(|e| Reply::error(500, e))?;
    let mut command = vec![
        "run".to_string(),
        plan.pipeline,
        "--handle".to_string(),
        handle.to_string(),
    ];
    for m in &plan.models {
        command.extend(["--model".to_string(), m.to_string()]);
    }
    let command = crate::pipelines::located(doors.pack_dir.as_deref(), command)?;
    crate::requirements::at_the_door(
        &doors.home,
        doors.pack_dir.as_deref(),
        registry,
        &command,
        false,
    )?;
    let mut extra = crate::serve::queued_by(caller);
    extra["step"] = json!(crate::system1::STEP);
    extra["for"] = json!("archive");
    let id = nils_registry::job::enqueue_with(
        registry.store(),
        &command,
        Some("archive"),
        Some(&caller.principal),
        extra,
    )
    .map_err(crate::serve::job_err)?;
    Ok(Reply::accepted(json!({
        "job": id, "state": "queued", "step": crate::system1::STEP, "for": "archive",
        "scans": frozen, "handle": handle, "command": command,
    })))
}
