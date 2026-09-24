// SPDX-License-Identifier: AGPL-3.0-only

//! Campaigns and label sets at the door and the keyboard (record 42 S5, S6
//! and S7). The mechanism is the registry's (`nils_registry::campaign`,
//! `nils_registry::labels`); what lives here is what a door and a verb add
//! to it: freezing a selection into the item list, the verified author of
//! an answer and a close, the files of a label set with their digest, and
//! the grants of R7, `campaigns:see` and `campaigns:work`, since a rater is
//! not a reviewer of the whole queue.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use nils_ask::ast::Grain;
use nils_registry::campaign::{self, Close, Given, Items, New, ReviewQuery, Role};
use nils_registry::home::Home;
use nils_registry::labels::{self, DecisionQuery, Label, LabelSet, NewSet, Of};
use nils_registry::schema::Type;
use nils_registry::{Param, Registry, Store};
use serde_json::{Value, json};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply, json_body};
use crate::{Exit, fail, usage};

// ------------------------------------------------------------ the doors

/// The doors this module answers, for the capabilities.
pub(crate) const DOORS: &[&str] = &[
    "GET /api/campaigns",
    "POST /api/campaigns",
    "GET /api/campaigns/{id}",
    "GET /api/campaigns/{id}/answers",
    "POST /api/campaigns/{id}/claim",
    "POST /api/campaigns/{id}/assignments/{assignment}/answer",
    "POST /api/campaigns/{id}/assignments/{assignment}/release",
    "POST /api/campaigns/{id}/items/{item}/metric",
    "POST /api/campaigns/{id}/close",
    "POST /api/campaigns/{id}/export",
    "GET /api/label-sets",
    "POST /api/label-sets",
    "GET /api/label-sets/{id}",
    "POST /api/decisions/commit",
];

/// What each door needs, read by `serve::door` like every other door's.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    use Detail::Plain;
    Some(match (method, segs) {
        ("GET", ["api", "campaigns"])
        | ("GET", ["api", "campaigns", _])
        | ("GET", ["api", "campaigns", _, "answers"]) => (Need::One("campaigns:see"), Plain),
        ("POST", ["api", "campaigns"])
        | ("POST", ["api", "campaigns", _, "claim" | "export"])
        | (
            "POST",
            [
                "api",
                "campaigns",
                _,
                "assignments",
                _,
                "answer" | "release",
            ],
        )
        | ("POST", ["api", "campaigns", _, "items", _, "metric"]) => {
            (Need::One("campaigns:work"), Plain)
        }
        // closing writes decisions into the registry, which is a
        // reviewer's act as well as the campaign's
        ("POST", ["api", "campaigns", _, "close"]) => {
            (Need::Both("campaigns:work", "review:work"), Plain)
        }
        ("GET", ["api", "label-sets"]) | ("GET", ["api", "label-sets", _]) => {
            (Need::AnyOf(&["campaigns:see", "review:see"]), Plain)
        }
        ("POST", ["api", "label-sets"]) => (Need::AnyOf(&["campaigns:work", "review:work"]), Plain),
        ("POST", ["api", "decisions", "commit"]) => (Need::One("review:work"), Plain),
        _ => return None,
    })
}

/// The policy rows of these doors: door, writes, idempotent, cost, cap, and
/// the two labels.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/campaigns",
        false,
        false,
        "bounded",
        "every campaign",
        "Listing campaigns",
        "Listed campaigns",
    ),
    (
        "POST /api/campaigns",
        true,
        false,
        "bounded",
        "one campaign",
        "Making a campaign",
        "Made a campaign",
    ),
    (
        "GET /api/campaigns/{id}",
        false,
        false,
        "bounded",
        "one campaign and its items",
        "Reading a campaign",
        "Read a campaign",
    ),
    (
        "GET /api/campaigns/{id}/answers",
        false,
        false,
        "bounded",
        "every answer",
        "Reading a campaign's answers",
        "Read a campaign's answers",
    ),
    (
        "POST /api/campaigns/{id}/claim",
        true,
        false,
        "free",
        "one item",
        "Claiming an item",
        "Claimed an item",
    ),
    (
        "POST /api/campaigns/{id}/assignments/{assignment}/answer",
        true,
        false,
        "free",
        "one answer",
        "Answering",
        "Answered",
    ),
    (
        "POST /api/campaigns/{id}/assignments/{assignment}/release",
        true,
        true,
        "free",
        "one assignment",
        "Giving an item back",
        "Gave an item back",
    ),
    (
        "POST /api/campaigns/{id}/items/{item}/metric",
        true,
        false,
        "free",
        "one item",
        "Posting a metric",
        "Posted a metric",
    ),
    (
        "POST /api/campaigns/{id}/close",
        true,
        false,
        "bounded",
        "one report",
        "Closing a campaign",
        "Closed a campaign",
    ),
    (
        "POST /api/campaigns/{id}/export",
        true,
        false,
        "bounded",
        "one label set",
        "Exporting a campaign's labels",
        "Exported a campaign's labels",
    ),
    (
        "GET /api/label-sets",
        false,
        false,
        "bounded",
        "every label set",
        "Listing label sets",
        "Listed label sets",
    ),
    (
        "POST /api/label-sets",
        true,
        false,
        "bounded",
        "one label set",
        "Exporting labels",
        "Exported labels",
    ),
    (
        "GET /api/label-sets/{id}",
        false,
        false,
        "bounded",
        "one label set with its files",
        "Reading a label set",
        "Read a label set",
    ),
    (
        "POST /api/decisions/commit",
        true,
        false,
        "bounded",
        "the decisions committed",
        "Committing staged decisions",
        "Committed staged decisions",
    ),
];

fn campaign_err(e: campaign::Error) -> Reply {
    match e {
        campaign::Error::Store(s) => Reply::error(500, s.to_string()),
        campaign::Error::Invalid(m) => Reply::error(400, m),
        campaign::Error::NotFound(m) => Reply::error(404, m),
        campaign::Error::Refused(m) => Reply::error(409, m),
        campaign::Error::Forbidden(m) => Reply::error(403, m),
    }
}

fn labels_err(e: labels::Error) -> Reply {
    match e {
        labels::Error::Store(s) => Reply::error(500, s.to_string()),
        labels::Error::Invalid(m) => Reply::error(400, m),
        labels::Error::NotFound(m) => Reply::error(404, m),
        labels::Error::Refused(m) => Reply::error(409, m),
    }
}

/// The author kind of the caller's verified actor, as `apply` takes it.
fn kind_of(caller: &Caller) -> &str {
    crate::serve::author_of(caller).0
}

/// The doors under `/api/campaigns` and `/api/label-sets`, or nothing when
/// the path is not one of them. The grants were checked by the table.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    if !matches!(
        segs,
        ["api", "campaigns", ..] | ["api", "label-sets", ..] | ["api", "decisions", "commit"]
    ) {
        return None;
    }
    let get = method == "GET";
    let post = method == "POST";
    let principal = caller.principal.as_str();
    let now = nils_registry::time::now_iso();
    let id_of = |s: &str| -> Result<i64, Reply> {
        s.parse::<i64>()
            .map_err(|_| Reply::error(404, format!("{s} is not an id")))
    };
    Some((|| -> Result<Reply, Reply> {
        match segs {
            ["api", "campaigns"] if get => {
                let list = campaign::list(registry.store()).map_err(campaign_err)?;
                let mut out = Vec::new();
                for c in list {
                    let mut v = c.as_json();
                    v["counts"] = campaign::counts(registry.store(), c.id).map_err(campaign_err)?;
                    out.push(v);
                }
                Ok(Reply::ok(json!({"count": out.len(), "campaigns": out})))
            }
            ["api", "campaigns"] if post => {
                let doc = json_body(body)?;
                let made = create_at_door(doors, registry, ask, caller, &doc)?;
                Ok(Reply::created(made))
            }
            ["api", "campaigns", which] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                Ok(Reply::ok(shown(registry.store(), &c)?))
            }
            ["api", "campaigns", which, "answers"] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let all = campaign::answers(registry.store(), c.id).map_err(campaign_err)?;
                let list: Vec<Value> = all.iter().map(campaign::Answer::as_json).collect();
                Ok(Reply::ok(
                    json!({"campaign": c.id, "count": list.len(), "answers": list}),
                ))
            }
            ["api", "campaigns", which, "claim"] if post => {
                let doc = json_body(body)?;
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let role =
                    Role::parse(doc["role"].as_str().unwrap_or("rater")).map_err(campaign_err)?;
                let claimed =
                    campaign::claim(registry, c.id, principal, role, &now).map_err(campaign_err)?;
                Ok(Reply::ok(match claimed {
                    Some(cl) => cl.as_json(),
                    None => json!({"assignment": null, "item": null, "why": format!(
                        "nothing in campaign {} is left for {principal} as a {}", c.name, role.name()
                    )}),
                }))
            }
            ["api", "campaigns", which, "assignments", a, "answer"] if post => {
                let doc = json_body(body)?;
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let a = id_of(a)?;
                belongs(registry.store(), c.id, "campaign_assignment", a)?;
                let form = (!doc["form"].is_null()).then(|| doc["form"].clone());
                let value = match &doc["value"] {
                    Value::Null => None,
                    Value::String(s) => Some(s.clone()),
                    // a pick's stacks may come as a list
                    Value::Array(list) => Some(
                        list.iter()
                            .map(|v| v.to_string().trim_matches('"').to_string())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                    other => Some(other.to_string()),
                };
                let acting = crate::serve::acting_model(registry, caller)?.map(|m| m.id);
                let answered = campaign::answer(
                    registry,
                    &Given {
                        assignment: a,
                        principal,
                        author_kind: kind_of(caller),
                        model: acting,
                        value: value.as_deref(),
                        form: form.as_ref(),
                        derivative_id: doc["derivative_id"].as_i64(),
                        why: doc["why"].as_str(),
                    },
                    &now,
                )
                .map_err(campaign_err)?;
                Ok(Reply::ok(json!({
                    "answer": answered.answer, "item": answered.item,
                    "state": answered.state, "adjudication": answered.adjudication,
                })))
            }
            ["api", "campaigns", which, "assignments", a, "release"] if post => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let a = id_of(a)?;
                belongs(registry.store(), c.id, "campaign_assignment", a)?;
                let done = campaign::release(registry, a, principal, &now).map_err(campaign_err)?;
                Ok(Reply::ok(done.as_json()))
            }
            ["api", "campaigns", which, "items", item, "metric"] if post => {
                let doc = json_body(body)?;
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let item = id_of(item)?;
                belongs(registry.store(), c.id, "campaign_item", item)?;
                let value = doc["value"]
                    .as_f64()
                    .ok_or_else(|| Reply::error(400, "value: the metric, a number"))?;
                let name = doc["name"].as_str().unwrap_or("external");
                let done = campaign::post_metric(registry, item, principal, name, value, &now)
                    .map_err(campaign_err)?;
                Ok(Reply::ok(json!({
                    "item": done.item, "state": done.state, "adjudication": done.adjudication,
                })))
            }
            ["api", "campaigns", which, "close"] if post => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let pack = doors
                    .pack_dir
                    .as_ref()
                    .and_then(|dir| nils_pack::load(&dir.join(&doors.ask_pack), None).ok());
                let picks = pick_writer(pack);
                let acting = crate::serve::acting_model(registry, caller)?.map(|m| m.id);
                let closed = campaign::close(
                    registry,
                    &Close {
                        campaign: c.id,
                        who: principal,
                        author_kind: kind_of(caller),
                        model: acting,
                        picks: Some(&picks),
                    },
                    &now,
                )
                .map_err(campaign_err)?;
                Ok(Reply::ok(closed.as_json()))
            }
            ["api", "campaigns", which, "export"] if post => {
                let doc = json_body(body)?;
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let of = match doc["of"].as_str().unwrap_or("outcomes") {
                    "outcomes" => Of::Outcomes,
                    "answers" => Of::Answers,
                    other => {
                        return Err(Reply::error(
                            400,
                            format!("of: outcomes or answers, not {other}"),
                        ));
                    }
                };
                let rows =
                    labels::campaign_labels(registry.store(), c.id, of).map_err(labels_err)?;
                let name = doc["name"].as_str().unwrap_or(&c.name).to_string();
                let meta = SetMeta {
                    name: &name,
                    kind: of.name(),
                    what: &c.question().map_err(campaign_err)?.what(),
                    source: json!({"campaign": c.id, "name": c.name, "of": of.name(), "from": c.source}),
                    campaign_id: Some(c.id),
                    handle_id: c.handle_id,
                    sealed: doc["sealed"].as_bool().unwrap_or(false),
                    created_by: principal,
                };
                let dir = export_dir(registry.store(), doc["place"].as_str(), &name)?;
                let set =
                    write_set(registry, &dir, &rows, &meta).map_err(|e| Reply::error(500, e))?;
                Ok(Reply::created(set_json(&set, None)))
            }
            ["api", "label-sets"] if get => {
                let list = labels::list(registry.store()).map_err(labels_err)?;
                let out: Vec<Value> = list.iter().map(LabelSet::as_json).collect();
                Ok(Reply::ok(json!({"count": out.len(), "label_sets": out})))
            }
            ["api", "label-sets"] if post => {
                let doc = json_body(body)?;
                let axis = doc["axis"]
                    .as_str()
                    .ok_or_else(|| Reply::error(400, "axis: what the labels are of"))?
                    .to_string();
                let authors: Vec<String> = doc["authors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                let campaign_id = match doc["campaign"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| doc["campaign"].as_i64().map(|i| i.to_string()))
                {
                    Some(w) => Some(
                        campaign::find(registry.store(), &w)
                            .map_err(campaign_err)?
                            .id,
                    ),
                    None => None,
                };
                let (handle_id, stacks) = match (&doc["selection"], doc["handle"].as_i64()) {
                    (Value::String(spec), _) => {
                        let (h, keys) =
                            freeze_at_door(doors, registry, ask, caller, spec, Grain::Stack)?;
                        (
                            Some(h),
                            Some(keys.into_iter().map(|(k, _)| k).collect::<Vec<_>>()),
                        )
                    }
                    (_, Some(h)) => {
                        let keys = handle_keys(registry.store(), h, Grain::Stack)?;
                        (Some(h), Some(keys.into_iter().map(|(k, _)| k).collect()))
                    }
                    _ => (None, None),
                };
                let rows = labels::decision_labels(
                    registry.store(),
                    &DecisionQuery {
                        axis: &axis,
                        stacks: stacks.as_deref(),
                        authors: &authors,
                        campaign: campaign_id,
                        staged_too: doc["staged"].as_bool().unwrap_or(false),
                    },
                )
                .map_err(labels_err)?;
                let name = doc["name"].as_str().unwrap_or(&axis).to_string();
                let meta = SetMeta {
                    name: &name,
                    kind: "decisions",
                    what: &axis,
                    source: json!({
                        "axis": axis, "authors": authors, "campaign": campaign_id,
                        "selection": doc["selection"], "handle": handle_id,
                        "staged": doc["staged"].as_bool().unwrap_or(false),
                    }),
                    campaign_id,
                    handle_id,
                    sealed: doc["sealed"].as_bool().unwrap_or(false),
                    created_by: principal,
                };
                let dir = export_dir(registry.store(), doc["place"].as_str(), &name)?;
                let set =
                    write_set(registry, &dir, &rows, &meta).map_err(|e| Reply::error(500, e))?;
                Ok(Reply::created(set_json(&set, None)))
            }
            ["api", "label-sets", id] if get => {
                let id = id_of(id)?;
                let set = labels::get(registry.store(), id)
                    .map_err(labels_err)?
                    .ok_or_else(|| Reply::error(404, format!("no label set {id}")))?;
                // a session's day is quasi-identifying
                if set.what.starts_with("pick:") {
                    caller.allowed("a label set of sessions", Need::Any, Detail::Quasi)?;
                }
                let files = set.path.as_deref().map(|p| {
                    let dir = Path::new(p);
                    json!({
                        "labels.tsv": std::fs::read_to_string(dir.join("labels.tsv")).ok(),
                        "provenance.json": std::fs::read_to_string(dir.join("provenance.json"))
                            .ok()
                            .and_then(|t| serde_json::from_str::<Value>(&t).ok()),
                    })
                });
                Ok(Reply::ok(set_json(&set, files)))
            }
            ["api", "decisions", "commit"] if post => {
                let doc = json_body(body)?;
                let campaign_id = match doc["campaign"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| doc["campaign"].as_i64().map(|i| i.to_string()))
                {
                    Some(w) => Some(
                        campaign::find(registry.store(), &w)
                            .map_err(campaign_err)?
                            .id,
                    ),
                    None => None,
                };
                let filter = nils_registry::review::CommitFilter {
                    min_confidence: doc["min_confidence"].as_f64(),
                    campaign: campaign_id,
                };
                let done = nils_registry::review::commit_where(
                    registry,
                    &filter,
                    doc["anyway"].as_bool().unwrap_or(false),
                    principal,
                )
                .map_err(|e| match e {
                    nils_registry::review::Error::Refused(m) => Reply::error(409, m),
                    other => Reply::error(500, other.to_string()),
                })?;
                Ok(Reply::ok(json!({
                    "committed": done.decisions, "items": done.items, "left": done.left,
                })))
            }
            _ => Err(Reply::error(
                404,
                format!("{method} /{} is not a door", segs.join("/")),
            )),
        }
    })())
}

/// Whether a row of a campaign's table belongs to the campaign the path
/// names, so that one campaign's path never reaches another's rows.
fn belongs(store: &mut Store, campaign: i64, t: &str, id: i64) -> Result<(), Reply> {
    let d = store.dialect();
    let sql = format!(
        "SELECT campaign_id FROM {} WHERE id = {}",
        store.qualified(t),
        d.param(1, Type::Int)
    );
    match store.query_opt(&sql, &[Param::Int(id)])? {
        Some(r) if r.int(0)? == campaign => Ok(()),
        _ => Err(Reply::error(
            404,
            format!("campaign {campaign} has no {id} of that kind"),
        )),
    }
}

fn shown(store: &mut Store, c: &campaign::Campaign) -> Result<Value, Reply> {
    let mut v = c.as_json();
    v["counts"] = campaign::counts(store, c.id).map_err(campaign_err)?;
    v["items"] = json!(
        campaign::items(store, c.id)
            .map_err(campaign_err)?
            .iter()
            .map(campaign::Item::as_json)
            .collect::<Vec<_>>()
    );
    v["assignments"] = json!(
        campaign::assignments(store, c.id)
            .map_err(campaign_err)?
            .iter()
            .map(campaign::Assignment::as_json)
            .collect::<Vec<_>>()
    );
    if c.status == "open" {
        v["agreement"] = campaign::agreement(store, c.id).map_err(campaign_err)?;
    }
    Ok(v)
}

fn set_json(set: &LabelSet, files: Option<Value>) -> Value {
    let mut v = set.as_json();
    if let Some(f) = files {
        v["files"] = f;
    }
    v
}

// ------------------------------------------------ freezing a selection

/// The keys a handle named, each with its subject.
type Keys = Vec<(i64, Option<i64>)>;

/// The document that freezes a saved selection into a list of stacks or
/// sessions: the selection as its opening set and, when its grain is wider,
/// the stacks or sessions under it, at record level so that the handle
/// keeps the keys.
pub(crate) fn freeze_document(
    name: &str,
    version: u64,
    grain: Grain,
    want: Grain,
) -> Result<Value, String> {
    let from = format!("selection:{name}@{version}");
    let mut sets = serde_json::Map::new();
    sets.insert("start".into(), json!({"grain": grain.name(), "from": from}));
    let out = if grain == want {
        "start"
    } else {
        let under = matches!(
            (grain, want),
            (Grain::Subject | Grain::Session, Grain::Stack) | (Grain::Subject, Grain::Session)
        );
        if !under {
            return Err(format!(
                "selection {name}@{version} is a {} set, and a campaign of {}s cannot be made from it",
                grain.name(),
                want.name()
            ));
        }
        sets.insert("items".into(), json!({"grain": want.name(), "of": "start"}));
        "items"
    };
    Ok(json!({"ast_version": 1, "sets": sets, "out": {"set": out, "level": "record"}}))
}

/// A selection as `name@version`, or its current version.
fn selection_spec(spec: &str) -> (&str, Option<u64>) {
    let spec = spec.strip_prefix("selection:").unwrap_or(spec);
    match spec.rsplit_once('@') {
        Some((n, v)) => (n, v.parse::<u64>().ok()),
        None => (spec, None),
    }
}

/// The keys of a handle, each with its subject; refused unless the handle
/// is of the grain wanted and still keeps its rows.
pub(crate) fn handle_keys(
    store: &mut Store,
    handle: i64,
    want: Grain,
) -> Result<Vec<(i64, Option<i64>)>, Reply> {
    let h = nils_ask::handle::get(store, handle)?
        .ok_or_else(|| Reply::error(404, format!("no handle {handle}")))?;
    if h.grain != want {
        return Err(Reply::error(
            400,
            format!(
                "handle {handle} is a {} set; this wants {}s",
                h.grain.name(),
                want.name()
            ),
        ));
    }
    if !h.has_rows() || h.withdrawn_at.is_some() || h.truncated {
        return Err(Reply::error(
            409,
            format!("handle {handle} keeps no complete list of keys; run its question again"),
        ));
    }
    Ok(nils_ask::handle::keys(store, handle)?)
}

/// Run the freezing document of a selection through the ask door itself,
/// under the caller's own grants, and read the keys of the handle it left.
fn freeze_at_door(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    spec: &str,
    want: Grain,
) -> Result<(i64, Keys), Reply> {
    let (name, version) = selection_spec(spec);
    let saved = nils_ask::selection::get(registry.store(), name, version)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .ok_or_else(|| Reply::error(404, format!("no selection {spec}")))?;
    let grain = saved
        .ask
        .sets
        .get(&saved.ask.out.set)
        .map(|s| s.grain)
        .unwrap_or(Grain::Subject);
    let doc =
        freeze_document(name, saved.version, grain, want).map_err(|m| Reply::error(400, m))?;
    let reply = crate::serve::ask_call(
        doors,
        registry,
        ask,
        caller,
        "POST",
        "/api/ask/run",
        &HashMap::new(),
        &json!({"document": doc, "fresh": true}).to_string(),
    );
    if reply.status != 200 {
        return Err(reply);
    }
    let handle = reply.body["handle"]
        .as_i64()
        .ok_or_else(|| Reply::error(500, "the ask door answered no handle"))?;
    let keys = handle_keys(registry.store(), handle, want)?;
    Ok((handle, keys))
}

/// Sessions as the pick question names them: the subject and the day the
/// session opened, from the session cache the keys name.
fn sessions_of(
    store: &mut Store,
    keys: &[(i64, Option<i64>)],
) -> Result<Vec<(i64, String)>, Reply> {
    let d = store.dialect();
    let t = nils_registry::schema::table("session_cache");
    let sql = format!(
        "SELECT subject_id, {} FROM {} WHERE id = {}",
        d.text_of(t.column("first").expect("first")),
        store.qualified("session_cache"),
        d.param(1, Type::Int)
    );
    let mut out = Vec::new();
    for (k, _) in keys {
        let r = store.query_opt(&sql, &[Param::Int(*k)])?.ok_or_else(|| {
            Reply::error(
                409,
                format!("session {k} is not in the cache; rebuild the sessions"),
            )
        })?;
        out.push((r.int(0)?, r.text(1)?.to_string()));
    }
    Ok(out)
}

/// The content hash of a handle.
fn content_hash(store: &mut Store, handle: i64) -> Result<Option<String>, Reply> {
    Ok(nils_ask::handle::get(store, handle)?.and_then(|h| h.content_hash))
}

fn pack_version_of(doors: &Doors) -> Option<String> {
    let dir = doors.pack_dir.as_ref()?;
    let pack = nils_pack::load(&dir.join(&doors.ask_pack), None).ok()?;
    Some(format!("{}@{}", pack.name, pack.version))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn inputs_of(v: &Value) -> BTreeMap<String, Vec<i64>> {
    v.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, ids)| {
                    (
                        k.clone(),
                        ids.as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_i64)
                            .collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn create_at_door(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    doc: &Value,
) -> Result<Value, Reply> {
    let name = doc["name"]
        .as_str()
        .ok_or_else(|| Reply::error(400, "name: what the campaign is called"))?;
    // an axis question that lists no values takes the served pack's
    let mut question = doc["question"].clone();
    if question["kind"] == "axis"
        && question["values"].is_null()
        && let (Some(dir), Some(axis)) = (&doors.pack_dir, question["axis"].as_str())
        && let Ok(pack) = nils_pack::load(&dir.join(&doors.ask_pack), None)
        && let Some(a) = pack.axes.iter().find(|a| a.name == axis)
    {
        question["values"] = json!(a.values.iter().map(|v| v.id.clone()).collect::<Vec<_>>());
    }
    let question = &question;
    let q = campaign::Question::parse(question).map_err(campaign_err)?;
    let want = if matches!(q, campaign::Question::Pick { .. }) {
        Grain::Session
    } else {
        Grain::Stack
    };
    let source = &doc["source"];
    let (items, handle_id) = if let Some(spec) = source["selection"].as_str() {
        let (h, keys) = freeze_at_door(doors, registry, ask, caller, spec, want)?;
        (items_of(registry.store(), want, &keys)?, Some(h))
    } else if let Some(h) = source["handle"].as_i64() {
        let keys = handle_keys(registry.store(), h, want)?;
        (items_of(registry.store(), want, &keys)?, Some(h))
    } else if source["review"].is_object() {
        let r = &source["review"];
        let found = campaign::review_items(
            registry.store(),
            &ReviewQuery {
                kind: r["kind"].as_str().map(str::to_string),
                kind_prefix: r["kind_prefix"].as_str().map(str::to_string),
                job_id: r["job_id"].as_i64(),
                limit: r["limit"].as_u64().map(|n| n as usize),
            },
        )
        .map_err(campaign_err)?;
        (Items::Review(found), None)
    } else {
        return Err(Reply::error(
            400,
            "source: {selection: name@v}, {handle: id} or {review: {kind | kind_prefix, job_id?, limit?}}",
        ));
    };
    let hash = match handle_id {
        Some(h) => content_hash(registry.store(), h)?,
        None => None,
    };
    let pack_version = pack_version_of(doors);
    let adjudication = if doc["adjudication"].is_null() {
        json!({})
    } else {
        doc["adjudication"].clone()
    };
    let made = campaign::create(
        registry,
        &New {
            name,
            owner: &caller.principal,
            question,
            source: source.clone(),
            items,
            handle_id,
            content_hash: hash.as_deref(),
            pack_version: pack_version.as_deref(),
            raters_per_item: doc["raters_per_item"].as_i64().unwrap_or(1),
            raters: strings(&doc["raters"]),
            adjudicators: strings(&doc["adjudicators"]),
            adjudication: &adjudication,
            closes_into: doc["closes_into"].as_str().unwrap_or("none"),
            lease_seconds: doc["lease_seconds"].as_i64().unwrap_or(3600),
            inputs: inputs_of(&doc["inputs"]),
        },
    )
    .map_err(campaign_err)?;
    shown(registry.store(), &made)
}

fn items_of(store: &mut Store, want: Grain, keys: &[(i64, Option<i64>)]) -> Result<Items, Reply> {
    Ok(match want {
        Grain::Session => Items::Sessions(sessions_of(store, keys)?),
        _ => Items::Stacks(keys.iter().map(|(k, _)| *k).collect()),
    })
}

// ------------------------------------------------------------ the files

/// What a set is, beside its rows.
pub(crate) struct SetMeta<'a> {
    pub(crate) name: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) what: &'a str,
    pub(crate) source: Value,
    pub(crate) campaign_id: Option<i64>,
    pub(crate) handle_id: Option<i64>,
    pub(crate) sealed: bool,
    pub(crate) created_by: &'a str,
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// Where a door writes a set: under `labels/<name>` in the export place it
/// names, or in the one export place there is.
fn export_dir(store: &mut Store, place: Option<&str>, name: &str) -> Result<PathBuf, Reply> {
    use nils_registry::place::{self, Role as PlaceRole};
    let chosen = match place {
        Some(n) => {
            let p = place::by_name(store, n)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .filter(|p| p.retired_at.is_none())
                .ok_or_else(|| Reply::error(404, format!("no place in force is named {n}")))?;
            if p.role != PlaceRole::Export {
                return Err(Reply::error(400, format!("{n} is not an export place")));
            }
            p
        }
        None => {
            let mut export: Vec<_> = place::active(store)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .into_iter()
                .filter(|p| p.role == PlaceRole::Export)
                .collect();
            match export.len() {
                1 => export.remove(0),
                0 => {
                    return Err(Reply::error(
                        409,
                        "no export place is declared, so labels have nowhere to go; `nils place add` declares one",
                    ));
                }
                _ => {
                    return Err(Reply::error(
                        400,
                        "place: which export place, since there are several",
                    ));
                }
            }
        }
    };
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    Ok(PathBuf::from(chosen.path).join("labels").join(safe))
}

/// Write `labels.tsv` and `provenance.json` into a directory and record
/// the set. The digest is the sha256 of `labels.tsv`, which the canonical
/// order makes the same for the same labels.
pub(crate) fn write_set(
    registry: &mut Registry,
    dir: &Path,
    rows: &[Label],
    meta: &SetMeta<'_>,
) -> Result<LabelSet, String> {
    let text = labels::tsv(rows);
    let digest = sha256(text.as_bytes());
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (handle_hash, scheme_digest, pack_version) = match meta.handle_id {
        Some(h) => match nils_ask::handle::get(registry.store(), h).map_err(|e| e.to_string())? {
            Some(h) => (h.content_hash, h.scheme_digest, h.pack_version),
            None => (None, None, None),
        },
        None => (None, None, None),
    };
    let place =
        nils_registry::place::holding(registry.store(), nils_registry::place::Role::Export, dir)
            .map_err(|e| e.to_string())?;
    std::fs::write(dir.join("labels.tsv"), &text).map_err(|e| format!("{}: {e}", dir.display()))?;
    let set = labels::record(
        registry,
        &NewSet {
            name: meta.name,
            kind: meta.kind,
            what: meta.what,
            source: meta.source.clone(),
            campaign_id: meta.campaign_id,
            handle_id: meta.handle_id,
            pack_version: pack_version.as_deref(),
            scheme_digest: scheme_digest.as_deref(),
            sealed: meta.sealed,
            rows: rows.len() as i64,
            digest: &digest,
            place_id: place.as_ref().map(|p| p.id),
            path: Some(&dir.display().to_string()),
            created_by: meta.created_by,
        },
    )
    .map_err(|e| e.to_string())?;
    let provenance = json!({
        "label_set": set.id,
        "name": set.name,
        "kind": set.kind,
        "what": set.what,
        "source": set.source,
        "campaign": set.campaign_id,
        "handle": {"id": set.handle_id, "content_hash": handle_hash},
        "epoch": set.epoch,
        "pack_version": set.pack_version,
        "scheme_digest": set.scheme_digest,
        "sealed": set.sealed,
        "training": set.as_json()["training"],
        "rows": set.rows,
        "columns": labels::COLUMNS,
        "digest": {"sha256": set.digest, "of": "labels.tsv"},
        "created_by": set.created_by,
        "created_at": set.created_at,
        "engine": env!("CARGO_PKG_VERSION"),
    });
    std::fs::write(
        dir.join("provenance.json"),
        serde_json::to_string_pretty(&provenance).unwrap_or_default() + "\n",
    )
    .map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(set)
}

// ------------------------------------------------------------ the verbs

/// `nils campaign`: one question asked of a frozen list of items, answered
/// by raters under leases and closed through the one write path (record 42).
#[derive(Debug, Subcommand)]
pub(crate) enum CampaignCommand {
    /// Make a campaign from a frozen selection, a handle, or open review items
    Create(Box<CreateArgs>),
    /// Every campaign, newest first
    List {
        #[arg(long)]
        json: bool,
    },
    /// One campaign: its question, its items and where each stands
    Show {
        /// The campaign, by name or id
        campaign: String,
        /// Every answer as well
        #[arg(long)]
        answers: bool,
        #[arg(long)]
        json: bool,
    },
    /// Claim the next item under a lease: as a rater, or with
    /// --adjudicator the next item waiting for an adjudicator
    Claim {
        campaign: String,
        #[arg(long)]
        adjudicator: bool,
        #[arg(long)]
        json: bool,
    },
    /// Answer a claimed item; an answer is never a decision
    Answer {
        /// The assignment a claim gave
        assignment: i64,
        /// The value: an axis value, a pick's stack ids (12,14), a text
        #[arg(long, value_name = "VALUE")]
        value: Option<String>,
        /// The form, as JSON
        #[arg(long, value_name = "JSON")]
        form: Option<String>,
        /// The derivative the answer's file was registered as
        #[arg(long, value_name = "ID")]
        derivative: Option<i64>,
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
    },
    /// Give a claimed item back unanswered
    Release { assignment: i64 },
    /// Post an external metric for an item whose raters all answered (a
    /// Dice over their masks); below the campaign's threshold the item goes
    /// to an adjudicator
    Metric {
        item: i64,
        #[arg(long, default_value = "dice")]
        name: String,
        #[arg(long)]
        value: f64,
    },
    /// Close the campaign: every settled item into one decision, a pick, or
    /// nothing, as the campaign says; every answer stays
    Close {
        campaign: String,
        /// Where the pack a pick campaign's picks are written under is
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        /// The pack a pick campaign's picks are written under
        #[arg(long, default_value = "mri")]
        pack: String,
        #[arg(long)]
        json: bool,
    },
    /// Write the campaign's labels: labels.tsv and provenance.json
    Export {
        campaign: String,
        /// The directory, under an export place
        #[arg(long, value_name = "DIR")]
        to: PathBuf,
        /// One row per answer rather than per settled item
        #[arg(long)]
        answers: bool,
        /// The items were drawn from a sealed certification sample, so the
        /// set is never training data (record 40 R3)
        #[arg(long)]
        sealed: bool,
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct CreateArgs {
    /// The campaign's name
    name: String,
    /// The question as JSON: {"kind": "axis", "axis": "body_part"} and the
    /// other four kinds (pick, form, derivative, free)
    #[arg(long, value_name = "JSON", conflicts_with_all = ["axis", "pick_role"])]
    question: Option<String>,
    /// An axis question: which value of this axis
    #[arg(long, value_name = "AXIS")]
    axis: Option<String>,
    /// The values an axis answer may take; the pack's when not given
    #[arg(long = "value", value_name = "VALUE")]
    values: Vec<String>,
    /// A pick question: which stacks stand for this role in each session
    #[arg(long, value_name = "ROLE")]
    pick_role: Option<String>,
    /// The items: a saved selection, frozen now
    #[arg(long, value_name = "selection:NAME@V")]
    select: Option<String>,
    /// The items: the keys of a handle
    #[arg(long, value_name = "ID")]
    handle: Option<i64>,
    /// The items: the open review items of this kind
    #[arg(long, value_name = "KIND")]
    review_kind: Option<String>,
    /// The items: the open review items whose kind starts so (body_part:)
    #[arg(long, value_name = "PREFIX")]
    review_prefix: Option<String>,
    /// At most this many review items
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
    #[arg(long, default_value_t = 1, value_name = "N")]
    raters_per_item: i64,
    /// Who may rate; anyone with campaigns:work when none is named
    #[arg(long = "rater", value_name = "PRINCIPAL")]
    raters: Vec<String>,
    /// Who may adjudicate; anyone with campaigns:work when none is named
    #[arg(long = "adjudicator", value_name = "PRINCIPAL")]
    adjudicators: Vec<String>,
    #[arg(long, default_value = "disagree", value_name = "disagree|always|never")]
    adjudicate: String,
    #[arg(long, default_value = "exact", value_name = "exact|kappa|external")]
    metric: String,
    /// The external metric's threshold
    #[arg(long, value_name = "T")]
    threshold: Option<f64>,
    #[arg(long, default_value = "none", value_name = "decision|stage|pick|none")]
    closes_into: String,
    #[arg(long, default_value_t = 3600, value_name = "SECONDS")]
    lease_seconds: i64,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    #[arg(long, default_value = "mri")]
    pack: String,
    #[arg(long)]
    json: bool,
}

/// `nils labels`: decisions as labelled data with their provenance.
#[derive(Debug, Subcommand)]
pub(crate) enum LabelsCommand {
    /// Write the decisions in force on one axis as labels.tsv and
    /// provenance.json, with a digest over the labels
    Export(Box<ExportArgs>),
    /// Every label set, newest first
    List {
        #[arg(long)]
        json: bool,
    },
    /// One label set
    Show {
        id: i64,
        #[arg(long)]
        json: bool,
    },
    /// Import v0's human labels of one axis from a TSV of
    /// SeriesInstanceUID, value and date, as person decisions marked
    /// imported:v0 with their date (record 42 R5)
    ImportV0 {
        /// The TSV
        #[arg(long, value_name = "FILE")]
        tsv: PathBuf,
        #[arg(long, default_value = "body_part", value_name = "AXIS")]
        axis: String,
        /// The values an imported label may take; the pack's when not given
        #[arg(long = "value", value_name = "VALUE")]
        values: Vec<String>,
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        #[arg(long, default_value = "mri")]
        pack: String,
        /// Count what it would do, and write nothing
        #[arg(long)]
        dry_run: bool,
        /// Also write the imported labels as a set into this directory
        #[arg(long, value_name = "DIR")]
        to: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    #[arg(long, value_name = "AXIS")]
    axis: String,
    /// Only the stacks of a saved selection, frozen now and pinned
    #[arg(long, value_name = "selection:NAME@V", conflicts_with = "handle")]
    select: Option<String>,
    /// Only the stacks of a handle, which the set pins
    #[arg(long, value_name = "ID")]
    handle: Option<i64>,
    /// Only decisions in force by these kinds of author
    #[arg(long = "author", value_name = "person|agent|model")]
    authors: Vec<String>,
    /// Only decisions a campaign's close wrote
    #[arg(long, value_name = "CAMPAIGN")]
    campaign: Option<String>,
    /// Staged decisions too, which are not in force
    #[arg(long)]
    staged: bool,
    /// The directory, under an export place
    #[arg(long, value_name = "DIR")]
    to: PathBuf,
    /// The stacks were drawn from a sealed certification sample, so the set
    /// is never training data (record 40 R3)
    #[arg(long)]
    sealed: bool,
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    #[arg(long, default_value = "mri")]
    pack: String,
    #[arg(long)]
    json: bool,
}

fn who() -> String {
    std::env::var("NILS_PRINCIPAL")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(crate::actor)
}

fn cerr(e: campaign::Error) -> Exit {
    match e {
        campaign::Error::Store(s) => fail(s.to_string()),
        other => usage(other.to_string()),
    }
}

fn lerr(e: labels::Error) -> Exit {
    match e {
        labels::Error::Store(s) => fail(s.to_string()),
        other => usage(other.to_string()),
    }
}

fn rerr(r: Reply) -> Exit {
    let m = r.body["error"].as_str().unwrap_or("refused").to_string();
    if r.status >= 500 { fail(m) } else { usage(m) }
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn require_export(registry: &mut Registry, dir: &Path) -> Result<(), Exit> {
    crate::places::require(registry.store(), nils_registry::place::Role::Export, dir)
        .map(|_| ())
        .map_err(|r| fail(r.message))
}

/// Record 42 S3's person-pick writer, through which a pick campaign closes:
/// the pick the served pack declares for the role, on the occasion the item
/// names under the campaign's scheme, left standing by a pick run, naming
/// its campaign. Without a pack a pick cannot be written, and each item
/// says so.
fn pick_writer(
    pack: Option<nils_pack::Pack>,
) -> impl Fn(&mut Registry, &campaign::PickAsk<'_>) -> Result<i64, String> {
    move |registry, ask| {
        let Some(pack) = &pack else {
            return Err("no pack is served, and a pick is the pack's".into());
        };
        let scheme = match ask.scheme {
            "default" | "day" => nils_registry::session::Scheme::default(),
            name => crate::stored_scheme(registry, name).map_err(|e| e.message)?,
        };
        nils_classify::picking::set_person(
            registry,
            pack,
            &scheme,
            &nils_classify::picking::PersonPick {
                role: ask.role,
                stacks: ask.stacks,
                model: None,
                why: ask.why,
                actor: ask.who,
                campaign: Some(ask.campaign),
                occasion: Some((ask.subject_id, ask.session_day)),
            },
        )
        .map(|picked| picked.id)
        .map_err(|e| e.to_string())
    }
}

/// The values an axis takes in the pack, for an axis question or an import
/// that names none.
fn pack_values(home: &Home, dir: Option<PathBuf>, pack: &str, axis: &str) -> Vec<String> {
    let Ok(dir) = crate::pack_dir(home, dir) else {
        return Vec::new();
    };
    let Ok(p) = nils_pack::load(&dir.join(pack), None) else {
        return Vec::new();
    };
    p.axes
        .iter()
        .find(|a| a.name == axis)
        .map(|a| a.values.iter().map(|v| v.id.clone()).collect())
        .unwrap_or_default()
}

pub(crate) fn campaign_command(home: &Home, cmd: CampaignCommand) -> Result<(), Exit> {
    let now = nils_registry::time::now_iso();
    match cmd {
        CampaignCommand::Create(args) => create_verb(home, *args),
        CampaignCommand::List { json } => {
            let mut registry = crate::open(home)?;
            let list = campaign::list(registry.store()).map_err(cerr)?;
            if json {
                let mut out = Vec::new();
                for c in &list {
                    let mut v = c.as_json();
                    v["counts"] = campaign::counts(registry.store(), c.id).map_err(cerr)?;
                    out.push(v);
                }
                print(&json!(out));
                return Ok(());
            }
            if list.is_empty() {
                println!("no campaigns");
            }
            for c in &list {
                let counts = campaign::counts(registry.store(), c.id).map_err(cerr)?;
                let total: i64 = counts["items"]
                    .as_object()
                    .map(|m| m.values().filter_map(Value::as_i64).sum())
                    .unwrap_or(0);
                println!(
                    "  {:>4}  {:<24} {:<7} {:<10} {:>5} item(s)  {} answer(s)  closes into {}",
                    c.id,
                    c.name,
                    c.status,
                    c.question["kind"].as_str().unwrap_or(""),
                    total,
                    counts["answers"],
                    c.closes_into
                );
            }
            Ok(())
        }
        CampaignCommand::Show {
            campaign: which,
            answers,
            json,
        } => {
            let mut registry = crate::open(home)?;
            let c = campaign::find(registry.store(), &which).map_err(cerr)?;
            let mut doc = shown(registry.store(), &c).map_err(rerr)?;
            if answers {
                doc["answers"] = json!(
                    campaign::answers(registry.store(), c.id)
                        .map_err(cerr)?
                        .iter()
                        .map(campaign::Answer::as_json)
                        .collect::<Vec<_>>()
                );
            }
            if json {
                print(&doc);
                return Ok(());
            }
            println!(
                "campaign {} {}   {}   {} rater(s) per item   closes into {}",
                c.id, c.name, c.status, c.raters_per_item, c.closes_into
            );
            println!("  question   {}", c.question);
            println!("  source     {}", c.source);
            println!("  adjudicate {}", c.adjudication);
            println!("  items      {}", doc["counts"]["items"]);
            println!("  leases     {}", doc["counts"]["assignments"]);
            println!("  answers    {}", doc["counts"]["answers"]);
            println!("  agreement  {}", doc["agreement"]);
            Ok(())
        }
        CampaignCommand::Claim {
            campaign: which,
            adjudicator,
            json,
        } => {
            let mut registry = crate::open(home)?;
            let c = campaign::find(registry.store(), &which).map_err(cerr)?;
            let role = if adjudicator {
                Role::Adjudicator
            } else {
                Role::Rater
            };
            let principal = who();
            let claimed =
                campaign::claim(&mut registry, c.id, &principal, role, &now).map_err(cerr)?;
            match claimed {
                Some(cl) if json => print(&cl.as_json()),
                Some(cl) => println!(
                    "assignment {}   item {} ({})   {} round {}   until {}",
                    cl.assignment.id,
                    cl.item.id,
                    cl.item.key,
                    cl.assignment.role,
                    cl.assignment.round,
                    cl.assignment.lease_until.as_deref().unwrap_or("-")
                ),
                None => println!("nothing in campaign {} is left for {principal}", c.name),
            }
            Ok(())
        }
        CampaignCommand::Answer {
            assignment,
            value,
            form,
            derivative,
            why,
        } => {
            let mut registry = crate::open(home)?;
            let form: Option<Value> = form
                .map(|f| serde_json::from_str(&f).map_err(|e| usage(format!("--form: {e}"))))
                .transpose()?;
            let principal = who();
            let done = campaign::answer(
                &mut registry,
                &Given {
                    assignment,
                    principal: &principal,
                    author_kind: "person",
                    model: None,
                    value: value.as_deref(),
                    form: form.as_ref(),
                    derivative_id: derivative,
                    why: why.as_deref(),
                },
                &now,
            )
            .map_err(cerr)?;
            println!(
                "answer {}   item {} is {}{}",
                done.answer,
                done.item,
                done.state,
                done.adjudication
                    .map(|a| format!("; adjudication offered as assignment {a}"))
                    .unwrap_or_default()
            );
            Ok(())
        }
        CampaignCommand::Release { assignment } => {
            let mut registry = crate::open(home)?;
            campaign::release(&mut registry, assignment, &who(), &now).map_err(cerr)?;
            println!("gave back assignment {assignment}");
            Ok(())
        }
        CampaignCommand::Metric { item, name, value } => {
            let mut registry = crate::open(home)?;
            let done = campaign::post_metric(&mut registry, item, &who(), &name, value, &now)
                .map_err(cerr)?;
            println!("item {} is {}", done.item, done.state);
            Ok(())
        }
        CampaignCommand::Close {
            campaign: which,
            pack_dir,
            pack,
            json,
        } => {
            let mut registry = crate::open(home)?;
            let c = campaign::find(registry.store(), &which).map_err(cerr)?;
            let principal = who();
            let pack = crate::pack_dir(home, pack_dir)
                .ok()
                .and_then(|d| nils_pack::load(&d.join(&pack), None).ok());
            let picks = pick_writer(pack);
            let closed = campaign::close(
                &mut registry,
                &Close {
                    campaign: c.id,
                    who: &principal,
                    author_kind: "person",
                    model: None,
                    picks: Some(&picks),
                },
                &now,
            )
            .map_err(cerr)?;
            if json {
                print(&closed.as_json());
                return Ok(());
            }
            println!(
                "closed campaign {}: {} item(s) resolved, {} left; {} decision(s){}, {} pick(s)",
                c.name,
                closed.resolved,
                closed.unresolved,
                closed.decisions.len(),
                if closed.staged { " staged" } else { "" },
                closed.picks.len()
            );
            for (item, why) in &closed.refused {
                println!("  item {item}: {why}");
            }
            println!("  agreement {}", closed.agreement);
            Ok(())
        }
        CampaignCommand::Export {
            campaign: which,
            to,
            answers,
            sealed,
            name,
            json,
        } => {
            let mut registry = crate::open(home)?;
            require_export(&mut registry, &to)?;
            let c = campaign::find(registry.store(), &which).map_err(cerr)?;
            let of = if answers { Of::Answers } else { Of::Outcomes };
            let rows = labels::campaign_labels(registry.store(), c.id, of).map_err(lerr)?;
            let name = name.unwrap_or_else(|| c.name.clone());
            let what = c.question().map_err(cerr)?.what();
            let principal = who();
            let set = write_set(
                &mut registry,
                &to,
                &rows,
                &SetMeta {
                    name: &name,
                    kind: of.name(),
                    what: &what,
                    source: json!({"campaign": c.id, "name": c.name, "of": of.name(), "from": c.source}),
                    campaign_id: Some(c.id),
                    handle_id: c.handle_id,
                    sealed,
                    created_by: &principal,
                },
            )
            .map_err(fail)?;
            report_set(&set, json);
            Ok(())
        }
    }
}

fn report_set(set: &LabelSet, json: bool) {
    if json {
        print(&set.as_json());
    } else {
        println!(
            "label set {} {}   {} row(s)   sha256 {}{}",
            set.id,
            set.name,
            set.rows,
            set.digest,
            if set.sealed {
                "   sealed: never training data"
            } else {
                ""
            }
        );
        if let Some(p) = &set.path {
            println!("  into {p}");
        }
    }
}

fn create_verb(home: &Home, a: CreateArgs) -> Result<(), Exit> {
    let question: Value = match (&a.question, &a.axis, &a.pick_role) {
        (Some(q), _, _) => {
            serde_json::from_str(q).map_err(|e| usage(format!("--question: {e}")))?
        }
        (None, Some(axis), _) => {
            let values = if a.values.is_empty() {
                pack_values(home, a.pack_dir.clone(), &a.pack, axis)
            } else {
                a.values.clone()
            };
            json!({"kind": "axis", "axis": axis, "values": values})
        }
        (None, None, Some(role)) => json!({"kind": "pick", "role": role}),
        _ => {
            return Err(usage(
                "the question: --axis AXIS, --pick-role ROLE or --question JSON",
            ));
        }
    };
    let q = campaign::Question::parse(&question).map_err(cerr)?;
    let want = if matches!(q, campaign::Question::Pick { .. }) {
        Grain::Session
    } else {
        Grain::Stack
    };
    let mut adjudication = json!({"when": a.adjudicate, "metric": a.metric});
    if let Some(t) = a.threshold {
        adjudication["threshold"] = json!(t);
    }
    let (handle, source) = match (&a.select, a.handle) {
        (Some(spec), _) => {
            let h =
                crate::ask_cli::freeze_selection(home, spec, want, a.pack_dir.clone(), &a.pack)?;
            (Some(h), json!({"selection": spec, "handle": h}))
        }
        (None, Some(h)) => (Some(h), json!({"handle": h})),
        (None, None) => (None, Value::Null),
    };
    let mut registry = crate::open(home)?;
    let (items, source) = match handle {
        Some(h) => {
            let keys = handle_keys(registry.store(), h, want).map_err(rerr)?;
            (
                items_of(registry.store(), want, &keys).map_err(rerr)?,
                source,
            )
        }
        None => {
            let query = ReviewQuery {
                kind: a.review_kind.clone(),
                kind_prefix: a.review_prefix.clone(),
                job_id: None,
                limit: a.limit,
            };
            let found = campaign::review_items(registry.store(), &query).map_err(cerr)?;
            (Items::Review(found), json!({"review": query.to_json()}))
        }
    };
    let hash = match handle {
        Some(h) => content_hash(registry.store(), h).map_err(rerr)?,
        None => None,
    };
    let pack_version = crate::pack_dir(home, a.pack_dir.clone())
        .ok()
        .and_then(|d| nils_pack::load(&d.join(&a.pack), None).ok())
        .map(|p| format!("{}@{}", p.name, p.version));
    let principal = who();
    let made = campaign::create(
        &mut registry,
        &New {
            name: &a.name,
            owner: &principal,
            question: &question,
            source,
            items,
            handle_id: handle,
            content_hash: hash.as_deref(),
            pack_version: pack_version.as_deref(),
            raters_per_item: a.raters_per_item,
            raters: a.raters.clone(),
            adjudicators: a.adjudicators.clone(),
            adjudication: &adjudication,
            closes_into: &a.closes_into,
            lease_seconds: a.lease_seconds,
            inputs: BTreeMap::new(),
        },
    )
    .map_err(cerr)?;
    if a.json {
        print(&shown(registry.store(), &made).map_err(rerr)?);
    } else {
        let n = campaign::items(registry.store(), made.id)
            .map_err(cerr)?
            .len();
        println!(
            "campaign {} {}   {} item(s)   {} question   closes into {}",
            made.id,
            made.name,
            n,
            q.kind(),
            made.closes_into
        );
    }
    Ok(())
}

pub(crate) fn labels_command(home: &Home, cmd: LabelsCommand) -> Result<(), Exit> {
    match cmd {
        LabelsCommand::Export(a) => {
            let a = *a;
            for k in &a.authors {
                if !["person", "agent", "model"].contains(&k.as_str()) {
                    return Err(usage(format!("--author {k}: person, agent or model")));
                }
            }
            let handle = match (&a.select, a.handle) {
                (Some(spec), _) => Some(crate::ask_cli::freeze_selection(
                    home,
                    spec,
                    Grain::Stack,
                    a.pack_dir.clone(),
                    &a.pack,
                )?),
                (None, h) => h,
            };
            let mut registry = crate::open(home)?;
            require_export(&mut registry, &a.to)?;
            let stacks: Option<Vec<i64>> = match handle {
                Some(h) => Some(
                    handle_keys(registry.store(), h, Grain::Stack)
                        .map_err(rerr)?
                        .into_iter()
                        .map(|(k, _)| k)
                        .collect(),
                ),
                None => None,
            };
            let campaign_id = match &a.campaign {
                Some(w) => Some(campaign::find(registry.store(), w).map_err(cerr)?.id),
                None => None,
            };
            let rows = labels::decision_labels(
                registry.store(),
                &DecisionQuery {
                    axis: &a.axis,
                    stacks: stacks.as_deref(),
                    authors: &a.authors,
                    campaign: campaign_id,
                    staged_too: a.staged,
                },
            )
            .map_err(lerr)?;
            let name = a.name.clone().unwrap_or_else(|| a.axis.clone());
            let principal = who();
            let set = write_set(
                &mut registry,
                &a.to,
                &rows,
                &SetMeta {
                    name: &name,
                    kind: "decisions",
                    what: &a.axis,
                    source: json!({
                        "axis": a.axis, "authors": a.authors, "campaign": campaign_id,
                        "selection": a.select, "handle": handle, "staged": a.staged,
                    }),
                    campaign_id,
                    handle_id: handle,
                    sealed: a.sealed,
                    created_by: &principal,
                },
            )
            .map_err(fail)?;
            report_set(&set, a.json);
            Ok(())
        }
        LabelsCommand::List { json } => {
            let mut registry = crate::open(home)?;
            let list = labels::list(registry.store()).map_err(lerr)?;
            if json {
                print(&json!(
                    list.iter().map(LabelSet::as_json).collect::<Vec<_>>()
                ));
                return Ok(());
            }
            if list.is_empty() {
                println!("no label sets");
            }
            for s in &list {
                println!(
                    "  {:>4}  {:<24} {:<10} {:<16} {:>7} row(s)  {}{}",
                    s.id,
                    s.name,
                    s.kind,
                    s.what,
                    s.rows,
                    &s.digest[..s.digest.len().min(16)],
                    if s.sealed { "  sealed" } else { "" }
                );
            }
            Ok(())
        }
        LabelsCommand::Show { id, json } => {
            let mut registry = crate::open(home)?;
            let set = labels::get(registry.store(), id)
                .map_err(lerr)?
                .ok_or_else(|| usage(format!("no label set {id}")))?;
            if json {
                print(&set.as_json());
            } else {
                report_set(&set, false);
                println!("  source   {}", set.source);
                println!(
                    "  training {}",
                    set.as_json()["training"].as_str().unwrap_or("")
                );
            }
            Ok(())
        }
        LabelsCommand::ImportV0 {
            tsv,
            axis,
            values,
            pack_dir,
            pack,
            dry_run,
            to,
            json,
        } => {
            let text = std::fs::read_to_string(&tsv)
                .map_err(|e| usage(format!("{}: {e}", tsv.display())))?;
            let (parsed, bad) = labels::parse_v0(&text);
            let allowed = if values.is_empty() {
                pack_values(home, pack_dir, &pack, &axis)
            } else {
                values
            };
            let mut registry = crate::open(home)?;
            if let Some(dir) = &to {
                require_export(&mut registry, dir)?;
            }
            let principal = who();
            let done =
                labels::import_v0(&mut registry, &parsed, &axis, &allowed, &principal, dry_run)
                    .map_err(lerr)?;
            let mut doc = done.as_json();
            doc["unreadable_lines"] = json!(bad);
            doc["dry_run"] = json!(dry_run);
            if let (Some(dir), false) = (&to, dry_run) {
                let rows = labels::imported_labels(registry.store(), &axis).map_err(lerr)?;
                let set = write_set(
                    &mut registry,
                    dir,
                    &rows,
                    &SetMeta {
                        name: &format!("v0-{axis}"),
                        kind: "imported",
                        what: &axis,
                        source: json!({"imported": "v0", "file_sha256": sha256(text.as_bytes()), "counts": done.as_json()}),
                        campaign_id: None,
                        handle_id: None,
                        sealed: false,
                        created_by: &principal,
                    },
                )
                .map_err(fail)?;
                doc["label_set"] = set.as_json();
            }
            if json {
                print(&doc);
                return Ok(());
            }
            // counts only: never a UID
            println!(
                "nils labels import-v0   {} label(s){}   {} series matched, {} not in this registry   {} stack(s)",
                done.labels,
                if dry_run { " (dry run)" } else { "" },
                done.series_matched,
                done.series_unmatched,
                done.stacks
            );
            println!(
                "  {} decision(s) written, {} already imported, {} held by a person's decision, {} value(s) refused, {} date(s) unreadable, {} line(s) unreadable",
                done.decisions.len(),
                done.already,
                done.held,
                done.refused_values,
                done.bad_dates,
                bad
            );
            if let Some(s) = doc.get("label_set") {
                println!(
                    "  label set {} sha256 {}",
                    s["id"],
                    s["digest"].as_str().unwrap_or("")
                );
            }
            Ok(())
        }
    }
}
