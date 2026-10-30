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
//!   sealed sample and shown for display, never as votes. A System 1 model
//!   admitted and chosen for its task stands (record 55, ruling 27): its
//!   opinion is shown as confirmed, with the recorded certificate that
//!   names it where one exists; a certificate is no longer required.
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
//!   signs, set at the end of every sort ([`crate::headers`]: nameless scans,
//!   words the pack does not know, a maker never seen, each held to the
//!   pack's threshold).
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
pub(crate) const DOORS: [&str; 15] = [
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
    "POST /api/audit/draws/{id}/settle",
    "GET /api/audit/draws/pooled",
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
    (
        "POST /api/audit/draws/{id}/settle",
        true,
        false,
        "bounded",
        "one settle campaign",
        "Settling an audit",
        "Settled an audit",
    ),
    (
        "GET /api/audit/draws/pooled",
        false,
        false,
        "bounded",
        "the draws named",
        "Pooling audit rounds",
        "Pooled audit rounds",
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
        ("POST", ["api", "audit", "draws", _, "settle"]) => {
            (Need::Both("review:work", "campaigns:work"), Plain)
        }
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
    let (confirmed, certified) = match &chosen {
        Some(c) => (
            opinion::confirmed(registry.store(), c.model.id).map_err(failed)?,
            opinion::certificate(registry.store(), c.model.id).map_err(failed)?,
        ),
        None => (false, None),
    };
    // record 55 ruling 27: the model stands on its admission and choice; a
    // certificate, where one names it, is shown (ruling 22 b: on a sealed
    // sample or citing its graded run) and no longer required
    let certificate = match certified {
        Some(id) => nils_registry::labels::certificate(registry.store(), id)
            .map_err(|e| Reply::error(500, e.to_string()))?
            .map(|c| json!({"id": c.id, "cites": if c.cites_evidence() { "evidence" } else { "sample" }, "words": c.words()})),
        None => None,
    };
    Ok(json!({
        "blind_share": share, "default_blind_share": blind::DEFAULT_SHARE,
        "system1": {
            "task": opinion::TASK,
            "chosen": chosen.as_ref().map(nils_registry::model::Chosen::as_json),
            "confirmed": confirmed, "certificate_id": certified,
            "certificate": certificate,
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
pub(crate) fn rules_reason(why: &Value, axis: &str) -> Value {
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
    let (confirmed, certificate) = match &model {
        Some(m) => (
            opinion::confirmed(store, m.id).map_err(failed)?,
            opinion::certificate(store, m.id).map_err(failed)?,
        ),
        None => (false, None),
    };
    let said = opinion::newest(store, &stacks, model.as_ref().map(|m| m.id)).map_err(failed)?;
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let stored = round::rules_answers(store, &stacks).map_err(failed)?;
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
                    // the model's answer against the rules' as it read them,
                    // in its own words, else against the stored value: the
                    // gate's own comparison
                    let stored = stored
                        .get(&stack)
                        .and_then(|a| a.get(axis))
                        .and_then(|(v, _)| v.clone());
                    v["agrees"] = json!(o.agrees(stored.as_deref()));
                    v["rules_compared"] = json!(o.rules_compared(stored.as_deref()));
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
    let status = match (&model, confirmed) {
        (None, _) => "no_model",
        (Some(_), true) => "confirmed",
        (Some(_), false) => "not_confirmed",
    };
    Ok(Reply::ok(json!({
        "item": item, "kind": it.kind, "mode": mode,
        "provisional": given.map(|p| json!({"answer": p.answer, "seconds": p.seconds})),
        "system1": {
            "status": status, "model": model.as_ref().map(|m| m.named()),
            "certificate_id": certificate,
            "why": match status {
                "no_model" => "no System 1 model is chosen for its task",
                "not_confirmed" => "System 1 does not stand: its model is not admitted and chosen for its task; its opinion is a research model's",
                _ if certificate.is_some() => "System 1 stands on its admission and is chosen for its task; a recorded certificate also names it",
                _ => "System 1 stands on its admission and is chosen for its task (record 55, ruling 27)",
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

/// Whether one dataset is held to the full gate, as its flag says (record
/// 56 §6.3: an answer's re-sort runs the gate again only there): a person's
/// word, or its signs, read with `pack` where no sort kept them.
pub(crate) fn damaged(
    store: &mut Store,
    place: &Place,
    sources: &[i64],
    pack: Option<&nils_pack::Pack>,
) -> Result<bool, StoreError> {
    Ok(crate::headers::flag_of(store, place, sources, pack)?["damaged"].as_bool() == Some(true))
}

/// Every source dataset flagged as having damaged headers, with its
/// sources: by a person's word, or by its signs, read with `pack` where no
/// sort kept them.
fn flagged(
    registry: &mut Registry,
    pack: Option<&nils_pack::Pack>,
) -> Result<Vec<(Place, Vec<i64>)>, StoreError> {
    let store = registry.store();
    let mut out = Vec::new();
    for p in nils_registry::place::active(store)? {
        if p.role != nils_registry::place::Role::Source {
            continue;
        }
        let sources = crate::sources::source_ids(store, &p)?;
        let f = crate::headers::flag_of(store, &p, &sources, pack)?;
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
    pack: Option<&nils_pack::Pack>,
) -> Result<BTreeMap<(String, String), Vec<i64>>, StoreError> {
    let mut sources: Vec<i64> = Vec::new();
    for (p, s) in flagged(registry, pack)? {
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

/// The pack's words for staging System 1's answers (record 55, ruling
/// 20): what naming nothing is on an axis, and how a value is stored.
struct PackWords(std::sync::Arc<nils_pack::Pack>);

impl PackWords {
    fn axis(&self, name: &str) -> Option<&nils_pack::rules::Axis> {
        self.0.axes.iter().find(|a| a.name == name)
    }
}

impl round::Words for PackWords {
    fn names_nothing(&self, axis: &str, rule: Option<&str>) -> bool {
        let Some(a) = self.axis(axis) else {
            return false;
        };
        if a.multi {
            return false;
        }
        match rule {
            None => true,
            // the fallback word, technique's `Unknown`, is none of its values
            Some(v) => {
                a.default.as_deref() == Some(v)
                    && a.value_index(v).is_none()
                    && a.id_of_stored(v).is_none()
            }
        }
    }

    fn stored(&self, axis: &str, value: &str) -> Result<String, String> {
        let a = self
            .axis(axis)
            .ok_or_else(|| format!("the pack {} has no axis {axis}", self.0.id()))?;
        let mut words = Vec::new();
        for w in value.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            // System 1 answers in the vocabulary's identities (3D-TSE,
            // T2starw); the axis may store the label (SPACE, T2*w)
            let i = a
                .value_index(w)
                .or_else(|| a.id_of_stored(w).and_then(|id| a.value_index(id)))
                .ok_or_else(|| {
                    format!(
                        "{w} is none of the {axis} values of the pack {}",
                        self.0.id()
                    )
                })?;
            // the pack's own marker for an axis nobody could decide
            if a.values[i].id == "Unknown" {
                return Err(format!("System 1 names no {axis}: it says Unknown"));
            }
            words.push(a.stored(i).to_string());
        }
        words.sort();
        words.dedup();
        match words.len() {
            0 => Err(format!("System 1 names no {axis} value")),
            1 => Ok(words.remove(0)),
            _ => Err(format!(
                "{axis} takes one value, and System 1 names {value}"
            )),
        }
    }
}

/// Run the gate over the flagged datasets' stacks (of `only`, where
/// given), round by round, and stage System 1's answers where the rules
/// name nothing (record 55, ruling 20) with the words of the round's pack
/// in `pack_dir`.
fn gate_over(
    registry: &mut Registry,
    only: Option<&[i64]>,
    dataset: Option<&str>,
    who: &str,
    pack_dir: Option<&std::path::Path>,
) -> Result<Value, StoreError> {
    // a dataset no sort has read the signs of is read with the engine's own
    // pack, as the doors read it
    let signs_pack = crate::headers::served(pack_dir, crate::headers::DEFAULT_PACK);
    let groups = flagged_stacks(registry, only, dataset, signs_pack.as_deref())?;
    let mut rounds = Vec::new();
    for ((pack, version), stacks) in groups {
        let store = registry.store();
        let r = round::current(store, &pack, &version, who)?;
        let mut done = round::gate(store, &r, &stacks, who)?;
        // what the gate no longer asks: System 1's staged answers withdrawn
        // as the model, audited
        round::retire(registry, &mut done)?;
        if !done.questions.is_empty() {
            // the words of the round's own rules version, never another's
            let served = crate::reader::served_pack(pack_dir, &pack);
            let why_not = match &served {
                None => Some(format!(
                    "the pack {pack} is not served here, so System 1's words are not read as its values"
                )),
                Some(p) if p.version.to_string() != version => Some(format!(
                    "the served pack {pack} is {} and the round's rules are {version}, so System 1's words are not read as the round's values",
                    p.version
                )),
                Some(_) => None,
            };
            // the round's own words only: another version's are never read
            let words = match (served, why_not) {
                (Some(p), None) => Ok(PackWords(p)),
                (_, Some(why)) => Err(why),
                (None, None) => Err(format!("the pack {pack} is not served here")),
            };
            let st = round::stage(
                registry,
                &r,
                &done.questions,
                match &words {
                    Ok(w) => Ok(w as &dyn round::Words),
                    Err(why) => Err(why.as_str()),
                },
                who,
            )?;
            done.staged.staged = st.staged;
            done.staged.kept = st.kept;
            done.staged.withdrawn += st.withdrawn;
            done.staged.waiting = st.waiting;
            for (why, n) in st.not_staged {
                *done.staged.not_staged.entry(why).or_default() += n;
            }
        }
        rounds.push(json!({"round": r.as_json(), "gated": done.to_json()}));
    }
    Ok(json!({"rounds": rounds}))
}

/// After a System 1 run: the gate over its stacks of flagged datasets.
pub(crate) fn after_opinions(
    home: &nils_registry::Home,
    registry: &mut Registry,
    stacks: &[i64],
    who: &str,
    pack_dir: Option<&std::path::Path>,
) -> Value {
    // the server's pack folder where the run was given one, else the
    // registry's
    let packs = match pack_dir {
        Some(d) => Some(d.to_path_buf()),
        None => crate::pack_dir(home, None).ok(),
    };
    gate_over(registry, Some(stacks), None, who, packs.as_deref())
        .unwrap_or_else(|e| json!({"error": e.to_string()}))
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
        Some(c) => opinion::confirmed(registry.store(), c.model.id)?,
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
/// accepted). Record 58 P2 (spec §13.1): with `since`, only the stacks
/// accepted after it, by the time the sort that judged them ended; each
/// with its vendor and acquisition year (the study's date where the
/// fingerprint has no acquisition date).
/// Where a draw's frame starts: nowhere, a given time, or just after a
/// draw (its stored time, compared in the database).
#[derive(Clone, Copy)]
enum Cutoff<'a> {
    None,
    Time(&'a str),
    Draw(i64),
}

/// One stored answer of an axis as the pack names it (7b, the rehearsal of
/// the audit): the registry's rows keep v0's stored form (`HASTE`, `TIRM`,
/// `FatSat,MoCo`), while a campaign's question, its answers and its labels
/// speak the pack's identities (`SS-TSE`, `IR-TSE`, `["FatSat", "MoCo"]`).
/// Each word is read as the value that stores it first, then by its
/// identity, a former identity or its label; a word the axis's fallback
/// names (technique's `Unknown`, which is none of its values) or `None`
/// on a multi-valued axis is no value. A single-valued axis gives one
/// identity or null, a multi-valued one a list or null; a word the pack
/// does not know is kept as it is, and the question refuses it.
pub(crate) fn named_by_pack(pack: &nils_pack::Pack, axis: &str, stored: &Value) -> Value {
    let Some(a) = pack.axes.iter().find(|a| a.name == axis) else {
        return stored.clone();
    };
    let words: Vec<String> = match stored {
        Value::Null => return Value::Null,
        Value::String(s) if a.multi => s.split(',').map(|w| w.trim().to_string()).collect(),
        Value::String(s) => vec![s.trim().to_string()],
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .map(|w| w.trim().to_string())
            .collect(),
        other => return other.clone(),
    };
    let mut ids: Vec<String> = Vec::new();
    for w in words {
        if w.is_empty() {
            continue;
        }
        let known = a
            .id_of_stored(&w)
            .map(str::to_string)
            .or_else(|| a.value_named(&w).map(|i| a.values[i].id.clone()));
        let id = match known {
            Some(id) => id,
            None if a.default.as_deref() == Some(w.as_str()) => continue,
            None if a.multi && w.eq_ignore_ascii_case("none") => continue,
            None => w,
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    match (a.multi, ids.len()) {
        (_, 0) => Value::Null,
        (true, _) => json!(ids),
        (false, _) => json!(ids[0]),
    }
}

fn frame(
    store: &mut Store,
    r: &Round,
    axes: &[String],
    since: Cutoff<'_>,
    pack: Option<&nils_pack::Pack>,
    exclude_drawn: bool,
) -> Result<Vec<Framed>, StoreError> {
    let holds = of_round(store, r);
    let d = store.dialect();
    let fp = nils_registry::schema::table("stack_fingerprint");
    let study = nils_registry::schema::table("study");
    let (after, params) = match since {
        Cutoff::Time(t) => (
            format!(
                " AND x.id IN (SELECT c.stack_id FROM {} c JOIN {} j ON j.id = c.job_id WHERE COALESCE(j.finished_at, j.started_at) >= {})",
                store.qualified("classification"),
                store.qualified("job"),
                d.param(1, nils_registry::schema::Type::Timestamp)
            ),
            vec![nils_registry::store::Param::from(t)],
        ),
        // the last draw's own stored time, compared in the database and
        // never read back as text
        Cutoff::Draw(id) => (
            format!(
                " AND x.id IN (SELECT c.stack_id FROM {} c JOIN {} j ON j.id = c.job_id WHERE COALESCE(j.finished_at, j.started_at) > (SELECT created_at FROM {} WHERE id = {}))",
                store.qualified("classification"),
                store.qualified("job"),
                store.qualified("audit_draw"),
                d.param(1, nils_registry::schema::Type::Int)
            ),
            vec![nils_registry::store::Param::Int(id)],
        ),
        Cutoff::None => (String::new(), Vec::new()),
    };
    // 7b: a draw that continues the round's chain of draws by count never
    // frames a stack an earlier draw by count of the round holds, so an
    // inclusive `since` at the last draw's time draws nothing twice
    let (after, params) = if exclude_drawn {
        let n = params.len() + 1;
        let mut params = params;
        params.push(nils_registry::store::Param::Int(r.id));
        (
            format!(
                "{after} AND x.id NOT IN (SELECT s.stack_id FROM {} s JOIN {} a ON a.id = s.draw_id WHERE a.round_id = {} AND a.count IS NOT NULL)",
                store.qualified("audit_draw_stack"),
                store.qualified("audit_draw"),
                d.param(n, nils_registry::schema::Type::Int)
            ),
            params,
        )
    } else {
        (after, params)
    };
    let sql = format!(
        "SELECT x.id, st.study_instance_uid, se.series_instance_uid, x.stack_key, f.manufacturer, {}, {} FROM {} x \
         JOIN {} se ON se.id = x.series_id JOIN {} st ON st.id = se.study_id \
         LEFT JOIN {} f ON f.stack_id = x.id \
         WHERE {holds}{after} AND UPPER(COALESCE(NULLIF(x.modality, ''), f.modality, '')) = 'MR' ORDER BY x.id",
        d.text_of_qualified(
            Some("f"),
            fp.column("earliest_acquisition_date")
                .expect("earliest_acquisition_date"),
        ),
        d.text_of_qualified(Some("st"), study.column("study_date").expect("study_date")),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("study"),
        store.qualified("stack_fingerprint"),
    );
    type Row = (i64, String, String, String, Option<String>, Option<i64>);
    let year_of = |t: Option<&str>| -> Option<i64> {
        t.and_then(|t| t.get(0..4))
            .and_then(|y| y.parse::<i64>().ok())
    };
    let mut rows: Vec<Row> = Vec::new();
    for row in store.query(&sql, &params)? {
        let vendor = row
            .opt_text(4)?
            .filter(|m| !m.trim().is_empty())
            .map(|m| nils_pipeline::pack::facts::vendor_of(m).to_string());
        let year = year_of(row.opt_text(5)?).or(year_of(row.opt_text(6)?));
        rows.push((
            row.int(0)?,
            row.text(1)?.to_string(),
            row.text(2)?.to_string(),
            row.text(3)?.to_string(),
            vendor,
            year,
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
    for (id, study, series, key, vendor, year) in rows {
        if !keep.contains(&id) {
            continue;
        }
        let a = answers.get(&id);
        let mut accepted = serde_json::Map::new();
        for axis in axes {
            match a.and_then(|m| m.get(axis)) {
                Some((_, true)) => {}
                Some((v, false)) => {
                    let v = json!(v);
                    // read once here, in the pack's names (7b)
                    let named = pack.map_or_else(|| v.clone(), |p| named_by_pack(p, axis, &v));
                    accepted.insert(axis.clone(), named);
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
            vendor,
            year,
        });
    }
    Ok(out)
}

/// What a campaign the audit makes over stacks is made with.
struct Over<'a> {
    name: String,
    question: Value,
    source: Value,
    pack_version: &'a str,
    raters_per_item: i64,
    raters: Vec<String>,
    adjudicators: Vec<String>,
    adjudication: Value,
    lease_seconds: i64,
    suggest: nils_registry::campaign::Suggest,
}

/// A campaign over stacks, frozen as a saved selection, closing into
/// nothing: the raters' campaign of a draw, its settle and its rule review.
fn campaign_over(
    registry: &mut Registry,
    caller: &Caller,
    stacks: &[i64],
    o: Over<'_>,
) -> Result<nils_registry::campaign::Campaign, Reply> {
    let epoch = registry.meta().epoch;
    let rows: Vec<nils_registry::store::Row> = {
        let store = registry.store();
        let mut subject: BTreeMap<i64, i64> = BTreeMap::new();
        for chunk in stacks.chunks(500) {
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
        stacks
            .iter()
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
            pack_version: Some(o.pack_version),
            ..Default::default()
        },
    )
    .map_err(|e| Reply::error(500, e))?;
    let keys =
        crate::campaigns::handle_keys(registry.store(), handle, nils_ask::ast::Grain::Stack)?;
    let items = crate::campaigns::items_of(registry.store(), nils_ask::ast::Grain::Stack, &keys)?;
    let hash = crate::campaigns::content_hash(registry.store(), handle)?;
    let mut source = json!({"handle": handle});
    if let Some(m) = o.source.as_object() {
        for (k, v) in m {
            source[k] = v.clone();
        }
    }
    nils_registry::campaign::create(
        registry,
        &nils_registry::campaign::New {
            name: &o.name,
            owner: &caller.principal,
            question: &o.question,
            source,
            items,
            handle_id: Some(handle),
            content_hash: hash.as_deref(),
            pack_version: Some(o.pack_version),
            raters_per_item: o.raters_per_item,
            raters: o.raters,
            adjudicators: o.adjudicators,
            adjudication: &o.adjudication,
            closes_into: "none",
            lease_seconds: o.lease_seconds,
            inputs: BTreeMap::new(),
            hold_back: None,
            suggest: Some(o.suggest),
            hide_header: false,
        },
    )
    .map_err(crate::campaigns::campaign_err)
}

/// `POST /api/audit/draws`: a round's audit draw and the campaign that
/// reads it. Body: `round` (a round's id; the round of the served rules
/// and the chosen model where none is named), `seed` (a word; one is drawn
/// and kept where none is given), `share` (1 % by default) or, record 58
/// P2 (spec §13.1), `count` (a fixed number of stacks, 150 for the monthly
/// audit) with `since` (the stacks accepted at this time or after it;
/// after the round's last draw by count where none is given; a draw that
/// continues the chain never frames a stack an earlier one holds), `axes`
/// (the header certificate's by default), `raters` and `adjudicators` (the agent principals; anyone
/// with `campaigns:work` where none are named), `raters_per_item` (two by
/// default) and `name`. A draw by count is settled by a person
/// (`/settle`), so its raters' campaign is never adjudicated: a
/// disagreement between the raters alone decides nothing.
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
    let by_count = !doc["count"].is_null();
    if by_count && !doc["share"].is_null() {
        return Err(Reply::error(
            400,
            "count or share: a draw takes a fixed number of stacks or a share of the round's, not both",
        ));
    }
    if !by_count && !doc["since"].is_null() {
        return Err(Reply::error(
            400,
            "since: a draw by count reads the stacks accepted since a time; give count with it",
        ));
    }
    let (size, since) = if by_count {
        let n = doc["count"]
            .as_u64()
            .filter(|n| *n >= 1)
            .ok_or_else(|| Reply::error(400, "count: a whole number of stacks, at least one"))?;
        let since = match &doc["since"] {
            Value::Null => {
                let last = audit_draw::last_count_draw(registry.store(), r.id).map_err(failed)?;
                last.map(|(id, at)| (Some(id), at))
            }
            Value::String(t) => Some((
                None,
                nils_registry::time::secs_of(t)
                    .or_else(|| nils_registry::time::secs_of(&format!("{t}T00:00:00Z")))
                    .map(nils_registry::time::iso_of)
                    .ok_or_else(|| {
                        Reply::error(400, "since: a time such as 2026-11-01T00:00:00Z")
                    })?,
            )),
            _ => {
                return Err(Reply::error(
                    400,
                    "since: a time such as 2026-11-01T00:00:00Z",
                ));
            }
        };
        (audit_draw::Size::Count(n as usize), since)
    } else {
        (
            audit_draw::Size::Share(doc["share"].as_f64().unwrap_or(audit_draw::DEFAULT_SHARE)),
            None,
        )
    };
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
    let cutoff = match &since {
        Some((Some(last), _)) => Cutoff::Draw(*last),
        Some((None, t)) => Cutoff::Time(t),
        None => Cutoff::None,
    };
    let pack = doors
        .pack_dir
        .as_ref()
        .and_then(|dir| nils_pack::load(&dir.join(&doors.ask_pack), None).ok());
    // 7b: a draw by count continues the round's chain (the default since,
    // or one at or after the last draw's time) and then frames no stack an
    // earlier draw by count holds; a since before the last draw reads an
    // earlier window again, as it was, so a seed draws the same stacks
    // (how a draw is checked)
    let exclude_drawn = by_count
        && match &since {
            Some((Some(_), _)) => true,
            Some((None, t)) => {
                match audit_draw::last_count_draw(registry.store(), r.id).map_err(failed)? {
                    Some((_, last)) => {
                        nils_registry::time::secs_of(t) >= nils_registry::time::secs_of(&last)
                    }
                    None => false,
                }
            }
            None => false,
        };
    let framed = frame(
        registry.store(),
        &r,
        &axes,
        cutoff,
        pack.as_ref(),
        exclude_drawn,
    )
    .map_err(failed)?;
    // 7b: a default `since` that leaves nothing to draw says the stamp it
    // used and how to draw from earlier
    if framed.is_empty()
        && let Some((Some(last), t)) = &since
    {
        return Err(Reply::error(
            409,
            format!(
                "no stack was accepted since {t}, the time of draw {last}, the round's last draw by count, which is where a draw by count starts when no since is given; to draw the stacks accepted at that second, give since {t} (the stacks accepted at it or after, less any an earlier draw by count holds); to read an earlier window again, give an earlier since"
            ),
        ));
    }
    let since: Option<String> = since.map(|(_, t)| t);
    let (d, chosen) = audit_draw::draw(
        registry.store(),
        r.id,
        &seed,
        size,
        since.as_deref(),
        &framed,
        &caller.principal,
    )
    .map_err(|e| Reply::error(409, e))?;
    // the campaign: the drawn stacks frozen, an axes question as the
    // reference reads were asked, nothing suggested, closing into nothing
    let mut question = json!({"kind": "axes", "axes": axes});
    crate::campaigns::complete_axes(&mut question, pack.as_ref())
        .map_err(|(s, m)| Reply::error(s, m))?;
    let name = doc["name"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("audit-round-{}-draw-{}", r.id, d.id));
    // record 58 P2: a draw by count is settled by a person against the
    // accepted answer, never by an adjudicator between the raters
    let adjudication = if by_count {
        json!({"when": "never", "metric": "exact"})
    } else {
        json!({"when": "disagree", "metric": "exact"})
    };
    let ids: Vec<i64> = chosen.iter().map(|f| f.stack_id).collect();
    let made = campaign_over(
        registry,
        caller,
        &ids,
        Over {
            name,
            question,
            source: json!({"audit_draw": d.id}),
            pack_version: &r.pack_version,
            raters_per_item: doc["raters_per_item"].as_i64().unwrap_or(2),
            raters: crate::campaigns::strings_of(&doc["raters"]),
            adjudicators: crate::campaigns::strings_of(&doc["adjudicators"]),
            adjudication,
            lease_seconds: doc["lease_seconds"].as_i64().unwrap_or(3600),
            suggest: nils_registry::campaign::Suggest::None,
        },
    )?;
    blind::set_purpose(registry.store(), made.id, "audit").map_err(|e| Reply::error(500, e))?;
    audit_draw::set_campaign(registry.store(), d.id, made.id).map_err(failed)?;
    let d = audit_draw::get(registry.store(), d.id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(500, "the draw was not kept"))?;
    let raters = if by_count {
        "two raters, agents outside the engine, read each item blind through the campaign doors; a person settles in verify mode every stack where a rater differs from the accepted answer and a seeded share of the agreements (POST /api/audit/draws/{id}/settle); a disagreement between the raters alone decides nothing (record 58 P2)"
    } else {
        "agents outside the engine read each item blind through the campaign doors; their disagreements are settled by the adjudicator; no person reads (record 55, ruling 13)"
    };
    Ok(Reply::ok(json!({
        "draw": d.as_json(), "round": r.as_json(),
        "campaign": {"id": made.id, "name": made.name, "purpose": "audit", "suggest": "none",
                      "closes_into": "none", "raters_per_item": made.raters_per_item,
                      "adjudication": made.adjudication().map(|a| a.to_json()).unwrap_or(Value::Null),
                      "trains": false},
        "axes": axes,
        "raters": raters,
    })))
}

/// The neutral names the sources of a settle item are imported under,
/// rotated per item by the seed, so that no name says which is the
/// accepted answer.
fn source_names(seed: &str, item: i64) -> [&'static str; 3] {
    const NAMES: [&str; 3] = ["source-a", "source-b", "source-c"];
    let h = nils_registry::verify::sha256_hex(format!("{seed}:sources:{item}").as_bytes());
    let k = (u8::from_str_radix(&h[..2], 16).expect("two hex digits") % 3) as usize;
    [NAMES[k], NAMES[(k + 1) % 3], NAMES[(k + 2) % 3]]
}

/// An accepted answer as an axes answer's text: every audited axis named,
/// null where the rules gave it no value.
fn accepted_text(accepted: &Value) -> String {
    match accepted {
        Value::Object(_) => accepted.to_string(),
        _ => "{}".into(),
    }
}

/// A settle asked for again (7b): the settle the draw has, with its counts
/// as the first call gave them, and nothing made. `again` says so. The
/// counts are worked out again from the raters' answers and the settle's
/// own seed and share, so they are the first call's while those answers
/// stand; `proposals` is null, as nothing was imported now.
fn settled_again(registry: &mut Registry, d: &audit_draw::Draw) -> Result<Reply, Reply> {
    let seed = d.settle_seed.clone().unwrap_or_else(|| d.seed.clone());
    let share = d.settle_share.unwrap_or(audit_draw::SETTLE_SHARE);
    let drawn = audit_draw::stacks(registry.store(), d.id).map_err(failed)?;
    let (reads, raters) = match d.campaign_id {
        Some(rc) => {
            let answers = audit_draw::raters_answers(registry.store(), rc)
                .map_err(crate::campaigns::campaign_err)?;
            let reads: BTreeMap<i64, Vec<audit_draw::Answered>> = answers
                .iter()
                .map(|(s, l)| (*s, l.iter().map(|(_, _, a)| a.clone()).collect()))
                .collect();
            let raters = nils_registry::campaign::get(registry.store(), rc)
                .map_err(crate::campaigns::campaign_err)?
                .map_or(1, |c| c.raters_per_item.max(1) as usize);
            (reads, raters)
        }
        None => (BTreeMap::new(), 1),
    };
    let ts = audit_draw::to_settle(&seed, share, &drawn, &reads, raters);
    let campaign = match d.settle_campaign_id {
        Some(c) => {
            let made = nils_registry::campaign::get(registry.store(), c)
                .map_err(crate::campaigns::campaign_err)?
                .ok_or_else(|| Reply::error(404, format!("no campaign {c}")))?;
            let items = nils_registry::campaign::items(registry.store(), c)
                .map_err(crate::campaigns::campaign_err)?
                .len();
            Some((made, items))
        }
        None => None,
    };
    let settle = match campaign {
        Some((made, items)) => json!({
            "campaign": {"id": made.id, "name": made.name, "purpose": "verify",
                         "suggest": "imported", "closes_into": "none",
                         "raters_per_item": made.raters_per_item, "trains": false,
                         "status": made.status},
            "items": items, "differences": ts.differ.len(), "agreements": ts.agree,
            "agreements_settled": ts.agreements_drawn.len(), "seed": seed, "share": share,
            "planted": nils_registry::verify::summary(registry.store(), made.id).map_err(failed)?,
            "proposals": Value::Null,
            "again": true,
        }),
        None => json!({
            "campaign": Value::Null,
            "items": 0, "differences": 0, "agreements": ts.agree,
            "agreements_settled": 0, "seed": seed, "share": share,
            "planted": Value::Null, "proposals": Value::Null,
            "again": true,
            "note": "nothing to settle: no rater differs from the accepted answer and the share of the agreements is none; the draw is graded with every stack standing as the raters' agreement",
        }),
    };
    Ok(Reply::ok(json!({"draw": d.as_json(), "settle": settle})))
}

/// `POST /api/audit/draws/{id}/settle` (record 58 P2, spec §13.3): once
/// both raters have answered every drawn stack, the verify campaign a
/// person settles the round in: every stack where a rater's answer differs
/// from the accepted answer, and a seeded share of the stacks where both
/// agree with it. Its proposal is the accepted answer; where a rater
/// differs, the accepted answer and the differing answer are shown as A
/// and B, unlabelled. Body: `seed` (the draw's where none is given),
/// `share` (0.10), `planted` (as a verify campaign takes it; planted from
/// the settle's seed where none is given), `settlers` and `adjudicators`
/// (the people; anyone with `campaigns:work` where none are named),
/// `raters_per_item` (one person by default), `lease_seconds` and `name`.
fn settle_door(
    registry: &mut Registry,
    caller: &Caller,
    id: i64,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let d = audit_draw::get(registry.store(), id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(404, format!("no audit draw {id}")))?;
    if d.settled() {
        return settled_again(registry, &d);
    }
    let raters_campaign = d
        .campaign_id
        .ok_or_else(|| Reply::error(409, format!("audit draw {id} has no raters' campaign")))?;
    let c = nils_registry::campaign::get(registry.store(), raters_campaign)
        .map_err(crate::campaigns::campaign_err)?
        .ok_or_else(|| Reply::error(404, format!("no campaign {raters_campaign}")))?;
    let seed = match &doc["seed"] {
        Value::Null => d.seed.clone(),
        Value::String(s) if !s.trim().is_empty() => s.clone(),
        _ => {
            return Err(Reply::error(
                400,
                "seed: a word the settle's share is drawn by",
            ));
        }
    };
    let share = match &doc["share"] {
        Value::Null => audit_draw::SETTLE_SHARE,
        v => v
            .as_f64()
            .filter(|s| (0.0..=1.0).contains(s))
            .ok_or_else(|| Reply::error(400, "share: of the agreements, from 0 to 1"))?,
    };
    let drawn = audit_draw::stacks(registry.store(), id).map_err(failed)?;
    let answers = audit_draw::raters_answers(registry.store(), raters_campaign)
        .map_err(crate::campaigns::campaign_err)?;
    let reads: BTreeMap<i64, Vec<audit_draw::Answered>> = answers
        .iter()
        .map(|(s, l)| (*s, l.iter().map(|(_, _, a)| a.clone()).collect()))
        .collect();
    let raters = c.raters_per_item.max(1) as usize;
    let ts = audit_draw::to_settle(&seed, share, &drawn, &reads, raters);
    if !ts.waiting.is_empty() {
        return Err(Reply::error(
            409,
            format!(
                "{} of the draw's {} stacks wait on a rater's answer; the settle is made once {raters} raters have answered every stack",
                ts.waiting.len(),
                drawn.len()
            ),
        ));
    }
    let held = ts.stacks(&drawn);
    if held.is_empty() {
        // every rater agrees with the accepted answer and the seeded share
        // of the agreements is none: the draw is settled with no campaign,
        // and its grade takes every stack as the raters' agreement
        audit_draw::set_settle(registry.store(), id, None, &seed, share).map_err(failed)?;
        let d = audit_draw::get(registry.store(), id)
            .map_err(failed)?
            .ok_or_else(|| Reply::error(500, "the draw was not kept"))?;
        return Ok(Reply::ok(json!({
            "draw": d.as_json(),
            "settle": {
                "campaign": Value::Null,
                "items": 0, "differences": 0, "agreements": ts.agree,
                "agreements_settled": 0, "seed": seed, "share": share,
                "planted": Value::Null, "proposals": [],
                "note": "nothing to settle: no rater differs from the accepted answer and the share of the agreements is none; the draw is graded with every stack standing as the raters' agreement",
            },
        })));
    }
    let name = doc["name"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("audit-draw-{id}-settle"));
    let made = campaign_over(
        registry,
        caller,
        &held,
        Over {
            name,
            question: c.question.clone(),
            source: json!({"audit_draw": id, "settles": raters_campaign}),
            pack_version: c.pack_version.as_deref().unwrap_or_default(),
            raters_per_item: doc["raters_per_item"].as_i64().unwrap_or(1),
            raters: crate::campaigns::strings_of(&doc["settlers"]),
            adjudicators: crate::campaigns::strings_of(&doc["adjudicators"]),
            adjudication: json!({"when": "disagree", "metric": "exact"}),
            lease_seconds: doc["lease_seconds"].as_i64().unwrap_or(3600),
            suggest: nils_registry::campaign::Suggest::Imported,
        },
    )?;
    blind::set_purpose(registry.store(), made.id, nils_registry::verify::PURPOSE)
        .map_err(|e| Reply::error(500, e))?;
    let planted = if doc["planted"].is_null() {
        json!({"seed": seed})
    } else {
        doc["planted"].clone()
    };
    let planting =
        nils_registry::verify::Planting::parse(&planted).map_err(crate::campaigns::campaign_err)?;
    nils_registry::verify::plant(registry.store(), made.id, &planting)
        .map_err(crate::campaigns::campaign_err)?;
    // the proposals: the differing answers first and the accepted answer
    // last, so that the accepted answer is the proposal shown and, where a
    // rater differs, A and B are it and a differing answer
    let item_of: BTreeMap<i64, i64> = nils_registry::campaign::items(registry.store(), made.id)
        .map_err(crate::campaigns::campaign_err)?
        .into_iter()
        .filter_map(|i| i.stack_id.map(|s| (s, i.id)))
        .collect();
    let accepted: BTreeMap<i64, &Value> = drawn.iter().map(|f| (f.stack_id, &f.accepted)).collect();
    // a question of one axis takes the axis's value, as its answers do; a
    // question of several, the joint's text
    let single = c
        .question()
        .ok()
        .and_then(|q| nils_registry::campaign::single_axis(&q));
    // the accepted answer as a proposal: none where its asked axis has no
    // value (the item still goes to the settle, with nothing filled in)
    let proposal_of = |read: &Value| -> Option<String> {
        match &single {
            Some(axis) => match &read[axis.as_str()] {
                Value::Null => None,
                Value::String(v) if v.trim().is_empty() => None,
                Value::String(v) => Some(v.clone()),
                Value::Array(l) => {
                    let words = l
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",");
                    (!words.is_empty()).then_some(words)
                }
                other => Some(other.to_string()),
            },
            None => Some(accepted_text(read)),
        }
    };
    // a rater's differing answer as an alternative (A or B): the same rule
    // for one axis and for several. A can't tell, whole or on one axis, is
    // a difference but never an alternative: on a joint answer the axis
    // keeps the accepted value, and an answer that then equals the accepted
    // answer offers nothing. A missing value on a single axis offers nothing.
    let alternative_of = |accepted: &Value, read: &audit_draw::Answered| -> Option<String> {
        let cant = |v: &Value| {
            audit_draw::comparable(v).is_some_and(|w| audit_draw::CANT_TELL.contains(&w.as_str()))
        };
        match &single {
            Some(axis) => {
                let v = read.get(axis.as_str())?;
                if cant(v) {
                    return None;
                }
                let joint = json!({ axis.as_str(): v });
                proposal_of(&joint)
            }
            None => {
                let mut kept = read.clone();
                for (axis, v) in read {
                    if cant(v) {
                        kept.insert(axis.clone(), accepted[axis.as_str()].clone());
                    }
                }
                if audit_draw::agrees(accepted, &kept) {
                    return None;
                }
                Some(Value::Object(kept.into_iter().collect()).to_string())
            }
        }
    };
    let (mut differing, mut proposals) = (Vec::new(), Vec::new());
    for (stack, item) in &item_of {
        let names = source_names(&seed, *item);
        let a = accepted.get(stack).copied().unwrap_or(&Value::Null);
        if let Some(value) = proposal_of(a) {
            proposals.push(nils_registry::suggestion::Given {
                item: Some(*item),
                stack: None,
                series: None,
                value: Some(value),
                confidences: BTreeMap::new(),
                author: Some(names[0].to_string()),
            });
        }
        let mut seen: Vec<audit_draw::Answered> = Vec::new();
        for (_, _, read) in answers.get(stack).into_iter().flatten() {
            if audit_draw::agrees(a, read) || seen.contains(read) || seen.len() >= 2 {
                continue;
            }
            seen.push(read.clone());
            let Some(value) = alternative_of(a, read) else {
                continue;
            };
            differing.push(nils_registry::suggestion::Given {
                item: Some(*item),
                stack: None,
                series: None,
                value: Some(value),
                confidences: BTreeMap::new(),
                author: Some(names[seen.len()].to_string()),
            });
        }
    }
    let settle_campaign = nils_registry::campaign::get(registry.store(), made.id)
        .map_err(crate::campaigns::campaign_err)?
        .ok_or_else(|| Reply::error(500, "the settle campaign was not kept"))?;
    let now = nils_registry::time::now_iso();
    let names = BTreeMap::new();
    let mut imported = Vec::new();
    let mut refused = 0i64;
    for (rows, source) in [
        (&differing, audit_draw::RATER_SOURCE),
        (&proposals, "audit settle"),
    ] {
        if rows.is_empty() {
            continue;
        }
        let done = nils_registry::suggestion::import(
            registry,
            &nils_registry::suggestion::Import {
                campaign: settle_campaign.id,
                rows,
                author: None,
                source: Some(source),
                who: &caller.principal,
                names: &names,
                dry_run: false,
            },
            &now,
        )
        .map_err(crate::campaigns::campaign_err)?;
        imported.push(json!({"rows": done.rows, "suggestions": done.suggestions,
                             "refused_values": done.refused_values}));
        refused += done.refused_values;
    }
    // the draw is tied to its settle campaign before anything is reported,
    // so a second call cannot make a second one
    audit_draw::set_settle(registry.store(), id, Some(made.id), &seed, share).map_err(failed)?;
    if refused > 0 {
        return Err(Reply::error(
            409,
            format!(
                "{refused} proposals of settle campaign {} were refused by its question (an accepted or differing value the question does not take); the campaign is kept, with those items left without a proposal or an alternative",
                made.id
            ),
        ));
    }
    let d = audit_draw::get(registry.store(), id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(500, "the draw was not kept"))?;
    Ok(Reply::ok(json!({
        "draw": d.as_json(),
        "settle": {
            "campaign": {"id": made.id, "name": made.name, "purpose": "verify",
                         "suggest": "imported", "closes_into": "none",
                         "raters_per_item": made.raters_per_item, "trains": false},
            "items": held.len(), "differences": ts.differ.len(), "agreements": ts.agree,
            "agreements_settled": ts.agreements_drawn.len(), "seed": seed, "share": share,
            "planted": nils_registry::verify::summary(registry.store(), made.id).map_err(failed)?,
            "proposals": imported,
        },
    })))
}

/// `POST /api/audit/draws/{id}/grade`: the accepted error of the draw's
/// round and its exact bound. A settled draw (record 58 P2) is graded from
/// the settle's answers, a stack not settled standing as the raters'
/// agreement with the accepted answer; a draw from before, from its
/// campaign's settled answers. A round over the registered rule opens a
/// rule review (record 56's loop), never a certificate: a campaign over its
/// wrong stacks showing the rules' answer and evidence, closing into
/// nothing, recorded on the draw. Body: `reviewers` (anyone with
/// `campaigns:work` where none are named).
fn grade_door(
    registry: &mut Registry,
    caller: &Caller,
    id: i64,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = if body.trim().is_empty() {
        json!({})
    } else {
        json_body(body)?
    };
    let d = audit_draw::get(registry.store(), id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(404, format!("no audit draw {id}")))?;
    let campaign = d
        .campaign_id
        .ok_or_else(|| Reply::error(409, format!("audit draw {id} has no campaign to grade")))?;
    let c = nils_registry::campaign::get(registry.store(), campaign)
        .map_err(crate::campaigns::campaign_err)?
        .ok_or_else(|| Reply::error(404, format!("no campaign {campaign}")))?;
    // record 58 P2: a draw by count is graded from its settle alone, never
    // from the raters' agreement with each other
    if d.count.is_some() && !d.settled() {
        return Err(Reply::error(
            409,
            format!(
                "audit draw {id} is graded from its settle: once both raters have answered, POST /api/audit/draws/{id}/settle"
            ),
        ));
    }
    let (reads, closed, settle) = if d.settled() {
        let (reads, settle) =
            audit_draw::settled_reads(registry.store(), &d).map_err(|e| Reply::error(500, e))?;
        let closed = settle["closed"] == true;
        (reads, closed, settle)
    } else {
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
            .map(|(stack_id, settled)| audit_draw::Read {
                stack_id,
                settled,
                stood: false,
            })
            .collect();
        (reads, c.status == "closed", Value::Null)
    };
    let g = audit_draw::grade(
        registry.store(),
        &d,
        &reads,
        closed,
        settle,
        &caller.principal,
    )
    .map_err(failed)?;
    // record 58 P2: a month over the rule opens a rule review, once
    let mut review = json!({"campaign_id": d.review_campaign_id, "opened": false});
    if g["rule"]["passed"] == false && d.review_campaign_id.is_none() {
        let stacks: Vec<i64> = g["wrong_stacks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| w["stack_id"].as_i64())
            .collect();
        let r = round::get(registry.store(), d.round_id)
            .map_err(failed)?
            .ok_or_else(|| Reply::error(404, format!("no review round {}", d.round_id)))?;
        let made = campaign_over(
            registry,
            caller,
            &stacks,
            Over {
                name: format!("audit-draw-{id}-rule-review"),
                question: c.question.clone(),
                source: json!({"audit_draw": id, "rule_review": true}),
                pack_version: &r.pack_version,
                raters_per_item: 1,
                raters: crate::campaigns::strings_of(&doc["reviewers"]),
                adjudicators: Vec::new(),
                adjudication: json!({"when": "disagree", "metric": "exact"}),
                lease_seconds: 3600,
                suggest: nils_registry::campaign::Suggest::Rules,
            },
        )?;
        audit_draw::set_review(registry.store(), id, made.id).map_err(failed)?;
        review = json!({"campaign_id": made.id, "name": made.name, "opened": true,
                        "stacks": stacks.len(), "suggest": "rules", "closes_into": "none",
                        "loop": "record 56: the round's wrong stacks with the rules' answer and evidence, for proposals at /api/packs/{name}/proposals; never a certificate"});
    }
    Ok(Reply::ok(
        json!({"draw": id, "round_id": d.round_id, "grade": g, "rule_review": review}),
    ))
}

/// `GET /api/audit/draws/pooled?ids=1,2,3` (record 58 P2, spec §13.4):
/// several graded rounds pooled, with the same fields as a round's grade.
fn pooled_door(
    registry: &mut Registry,
    query: &std::collections::HashMap<String, String>,
) -> Result<Reply, Reply> {
    let ids: Vec<i64> = query
        .get("ids")
        .map(|t| {
            t.split(',')
                .filter(|x| !x.trim().is_empty())
                .map(|x| {
                    x.trim()
                        .parse::<i64>()
                        .map_err(|_| Reply::error(400, "ids: draw ids, comma separated"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    if ids.is_empty() {
        return Err(Reply::error(
            400,
            "ids: the graded draws to pool, comma separated",
        ));
    }
    let mut draws = Vec::new();
    for id in ids {
        draws.push(
            audit_draw::get(registry.store(), id)
                .map_err(failed)?
                .ok_or_else(|| Reply::error(404, format!("no audit draw {id}")))?,
        );
    }
    let p = audit_draw::pool(&draws).map_err(|e| Reply::error(409, e))?;
    Ok(Reply::ok(json!({"pooled": p})))
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
            let done = gate_over(
                registry,
                None,
                doc["dataset"].as_str(),
                &caller.principal,
                doors.pack_dir.as_deref(),
            )
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
            // the deployment's pack, for a dataset no sort has read the
            // signs of since they are kept
            let pack = crate::headers::served(doors.pack_dir.as_deref(), &doors.ask_pack);
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
            Ok(Reply::ok(
                crate::headers::flag_of(store, &place, &sources, pack.as_deref())
                    .map_err(failed)?,
            ))
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
        ["api", "audit", "draws", "pooled"] if get => pooled_door(registry, query),
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
                               "accepted": f.accepted, "vendor": f.vendor, "year": f.year})
                    })
                    .collect();
                v["stacks"] = json!(drawn);
            }
            Ok(Reply::ok(v))
        })(),
        ["api", "audit", "draws", id, "grade"] if post => {
            id_of(id).and_then(|id| grade_door(registry, caller, id, body))
        }
        ["api", "audit", "draws", id, "settle"] if post => {
            id_of(id).and_then(|id| settle_door(registry, caller, id, body))
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
    // 2026-10-17: the archive is more scans than one container holds; it
    // is run in parts, each a run of its own, in this one job
    let command = crate::pipelines::in_parts(registry.store(), command, frozen);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mri() -> nils_pack::Pack {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
        nils_pack::load(&dir, None).expect("the MRI pack loads")
    }

    /// 7b: the registry's stored form read in the pack's value names, once,
    /// where a draw reads the accepted answer.
    #[test]
    fn a_stored_answer_reads_in_the_packs_names() {
        let pack = mri();
        let n = |axis: &str, v: Value| named_by_pack(&pack, axis, &v);
        // v0's display names, stored, are the values that store them
        assert_eq!(n("technique", json!("HASTE")), json!("SS-TSE"));
        assert_eq!(n("technique", json!("TIRM")), json!("IR-TSE"));
        assert_eq!(n("technique", json!("SPACE")), json!("3D-TSE"));
        // an identity is kept, so reading twice changes nothing
        assert_eq!(n("technique", json!("SS-TSE")), json!("SS-TSE"));
        assert_eq!(n("base", json!("T2*w")), json!("T2starw"));
        assert_eq!(n("base", json!("T2starw")), json!("T2starw"));
        // a comma list on a multi-valued axis is a list, None is no value
        assert_eq!(
            n("modifier", json!("FatSat,MoCo")),
            json!(["FatSat", "MoCo"])
        );
        assert_eq!(n("modifier", json!("None")), Value::Null);
        assert_eq!(n("modifier", Value::Null), Value::Null);
        // base's Unknown is one of its values; technique's fallback word is none
        assert_eq!(n("base", json!("Unknown")), json!("Unknown"));
        assert_eq!(n("technique", json!("Unknown")), Value::Null);
        // a word the pack does not know is kept, for the question to refuse
        assert_eq!(n("technique", json!("NoSuch")), json!("NoSuch"));
        assert_eq!(n("colour", json!("red")), json!("red"));
    }
}
