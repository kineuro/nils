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
    "GET /api/campaigns/{id}/items/{item}/candidates",
    "POST /api/campaigns/{id}/claim",
    "POST /api/campaigns/{id}/assignments/{assignment}/answer",
    "POST /api/campaigns/{id}/assignments/{assignment}/release",
    "POST /api/campaigns/{id}/assignments/{assignment}/renew",
    "POST /api/campaigns/{id}/items/{item}/metric",
    "POST /api/campaigns/{id}/close",
    "POST /api/campaigns/{id}/export",
    "GET /api/label-sets",
    "POST /api/label-sets",
    "GET /api/label-sets/{id}",
    "POST /api/decisions/commit",
    "GET /api/stacks/{stack}/why",
    "GET /api/campaigns/{id}/items/{item}/why",
    "GET /api/campaigns/{id}/items/{item}/header",
    "POST /api/campaigns/{id}/items/{item}/derive",
    "GET /api/campaigns/{id}/combinations",
    "GET /api/campaigns/{id}/batches",
    "POST /api/campaigns/{id}/batches/{batch}/accept",
    "GET /api/campaigns/{id}/stats",
    "GET /api/certificates",
    "POST /api/certificates",
    "POST /api/certificates/{id}/unseal",
];

/// What each door needs, read by `serve::door` like every other door's.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    use Detail::Plain;
    Some(match (method, segs) {
        ("GET", ["api", "campaigns"])
        | ("GET", ["api", "campaigns", _])
        | (
            "GET",
            [
                "api",
                "campaigns",
                _,
                "answers" | "batches" | "stats" | "combinations",
            ],
        )
        | (
            "GET",
            [
                "api",
                "campaigns",
                _,
                "items",
                _,
                "candidates" | "why" | "header",
            ],
        ) => (Need::One("campaigns:see"), Plain),
        // record 48: what the pack derives from a partial answer, a reading
        // that takes a body and writes nothing
        ("POST", ["api", "campaigns", _, "items", _, "derive"]) => {
            (Need::One("campaigns:see"), Plain)
        }
        // record 48: why one stack was judged so, one line per axis, is a
        // review reading, as the explanation is
        ("GET", ["api", "stacks", _, "why"]) => (Need::One("review:see"), Plain),
        ("POST", ["api", "campaigns", _, "batches", _, "accept"]) => {
            (Need::One("campaigns:work"), Plain)
        }
        // record 48 R2: a certificate and the unseal are the model
        // registry's acts
        ("GET", ["api", "certificates"]) => (Need::One("models:see"), Plain),
        ("POST", ["api", "certificates"]) | ("POST", ["api", "certificates", _, "unseal"]) => {
            (Need::One("models:work"), Plain)
        }
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
                "answer" | "release" | "renew",
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
        "GET /api/campaigns/{id}/items/{item}/candidates",
        false,
        false,
        "bounded",
        "one session's stacks",
        "Reading a session's candidates",
        "Read a session's candidates",
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
        "POST /api/campaigns/{id}/assignments/{assignment}/renew",
        true,
        false,
        "free",
        "one lease",
        "Renewing a lease",
        "Renewed a lease",
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
    (
        "GET /api/stacks/{stack}/why",
        false,
        false,
        "free",
        "one line per axis",
        "Reading why a stack was judged so",
        "Read why a stack was judged so",
    ),
    (
        "GET /api/campaigns/{id}/items/{item}/why",
        false,
        false,
        "free",
        "one line per axis",
        "Reading an item's evidence",
        "Read an item's evidence",
    ),
    (
        "GET /api/campaigns/{id}/items/{item}/header",
        false,
        false,
        "free",
        "one instance's stored header",
        "Reading an item's header",
        "Read an item's header",
    ),
    (
        "POST /api/campaigns/{id}/items/{item}/derive",
        false,
        true,
        "free",
        "one answer's derived axes",
        "Deriving axes from an answer",
        "Derived axes from an answer",
    ),
    (
        "GET /api/campaigns/{id}/combinations",
        false,
        false,
        "bounded",
        "the registry's classifications, counted once",
        "Counting the combinations of a question's axes",
        "Counted the combinations of a question's axes",
    ),
    (
        "GET /api/campaigns/{id}/batches",
        false,
        false,
        "bounded",
        "every batch of the items open to the caller",
        "Listing batches of like stacks",
        "Listed batches of like stacks",
    ),
    (
        "POST /api/campaigns/{id}/batches/{batch}/accept",
        true,
        false,
        "bounded",
        "one answer per item accepted",
        "Accepting a batch",
        "Accepted a batch",
    ),
    (
        "GET /api/campaigns/{id}/stats",
        false,
        false,
        "bounded",
        "counts and times per rater",
        "Reading how fast a campaign is read",
        "Read how fast a campaign is read",
    ),
    (
        "GET /api/certificates",
        false,
        false,
        "bounded",
        "every certificate",
        "Listing certificates",
        "Listed certificates",
    ),
    (
        "POST /api/certificates",
        true,
        false,
        "free",
        "one certificate",
        "Recording a certificate",
        "Recorded a certificate",
    ),
    (
        "POST /api/certificates/{id}/unseal",
        true,
        true,
        "bounded",
        "one sample unsealed",
        "Unsealing a certified sample",
        "Unsealed a certified sample",
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
    query: &HashMap<String, String>,
    body: &str,
) -> Option<Result<Reply, Reply>> {
    if !matches!(
        segs,
        ["api", "campaigns", ..]
            | ["api", "label-sets", ..]
            | ["api", "decisions", "commit"]
            | ["api", "stacks", _, "why"]
            | ["api", "certificates", ..]
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
        // record 48: a campaign's doors reach a campaign of the caller's own,
        // unless the caller reads every campaign; one that is not theirs is
        // answered as one that does not exist
        if let ["api", "campaigns", which, ..] = segs
            && !oversees(caller)
        {
            let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
            if !own(registry.store(), caller, &c)? {
                return Err(Reply::error(404, format!("no campaign {which}")));
            }
        }
        match segs {
            ["api", "campaigns"] if get => {
                let list = campaign::list(registry.store()).map_err(campaign_err)?;
                let mut out = Vec::new();
                for c in list {
                    if !oversees(caller) && !own(registry.store(), caller, &c)? {
                        continue;
                    }
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
                let mut v = shown(registry.store(), &c)?;
                // record 48, the reader's search: every name each asked value
                // goes by, from the served pack; generic, served blind alike
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                if let Some(vocabulary) = vocabulary_of(&c.question, pack.as_deref()) {
                    v["question"]["vocabulary"] = vocabulary;
                }
                if plain(caller) {
                    let free = c.question["kind"] == "free";
                    for it in v["items"].as_array_mut().into_iter().flatten() {
                        plain_item(it, &c.grain, free);
                    }
                }
                if !sees_all(registry.store(), caller, principal, &c)? {
                    blind_view(&mut v, principal);
                }
                Ok(Reply::ok(v))
            }
            ["api", "campaigns", which, "answers"] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let all = campaign::answers(registry.store(), c.id).map_err(campaign_err)?;
                let free = c.question["kind"] == "free";
                let sees_all = sees_all(registry.store(), caller, principal, &c)?;
                let list: Vec<Value> = all
                    .iter()
                    .filter(|a| sees_all || a.principal == principal)
                    .map(|a| {
                        let mut v = a.as_json();
                        axes_value(&c.question, &mut v["value"]);
                        if plain(caller) {
                            plain_answer(&mut v, free);
                        }
                        v
                    })
                    .collect();
                Ok(Reply::ok(json!({
                    "campaign": c.id, "count": list.len(), "answers": list,
                    "blind": !sees_all,
                })))
            }
            // record 45: the stacks a session item's pick is made among
            ["api", "campaigns", which, "items", item, "candidates"] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let item = id_of(item)?;
                belongs(registry.store(), c.id, "campaign_item", item)?;
                let it = campaign::item(registry.store(), item)
                    .map_err(campaign_err)?
                    .ok_or_else(|| Reply::error(404, format!("no campaign item {item}")))?;
                // the day as the day, whatever time a backend reads a date with
                let day = it
                    .session_day
                    .as_deref()
                    .map(|d| d.chars().take(10).collect::<String>());
                let (Some(subject), Some(day)) = (it.subject_id, day) else {
                    return Err(Reply::error(
                        400,
                        format!(
                            "item {item} is a stack's; only an item of sessions has candidates"
                        ),
                    ));
                };
                // a session is named by its subject and its day
                caller.allowed("a campaign of sessions", Need::Any, Detail::Quasi)?;
                let role = c.question["role"].as_str().map(str::to_string);
                let mut doc =
                    candidates(registry.store(), c.id, item, subject, &day, role.as_deref())?;
                // record 48 R2: what the classifier says of a stack read
                // blind is left out
                let stacks: Vec<i64> = doc["candidates"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["stack_id"].as_i64())
                    .collect();
                let hidden = blind_among(registry.store(), caller, &stacks)?;
                for cand in doc["candidates"].as_array_mut().into_iter().flatten() {
                    if cand["stack_id"]
                        .as_i64()
                        .is_some_and(|s| hidden.contains(&s))
                    {
                        cand["axes"] = json!({});
                        cand["blind"] = json!(true);
                    }
                }
                Ok(Reply::ok(doc))
            }
            ["api", "campaigns", which, "claim"] if post => {
                let doc = json_body(body)?;
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                // a session is named by its subject and its day, which is
                // quasi-identifying, and a rater of one must see it
                if c.grain == "session" {
                    caller.allowed("a campaign of sessions", Need::Any, Detail::Quasi)?;
                }
                let role =
                    Role::parse(doc["role"].as_str().unwrap_or("rater")).map_err(campaign_err)?;
                // record 48 R1: the most valuable item first, when asked
                let order = doc["order"]
                    .as_str()
                    .or_else(|| query.get("order").map(String::as_str))
                    .map(campaign::Order::parse)
                    .transpose()
                    .map_err(campaign_err)?
                    .unwrap_or_default();
                let claimed = campaign::claim_in(registry, c.id, principal, role, order, &now)
                    .map_err(campaign_err)?;
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
                // record 48: the rater's mark that the stack wants a second
                // look, a boolean when given
                let unsure = match &doc["unsure"] {
                    Value::Null => false,
                    Value::Bool(b) => *b,
                    _ => return Err(Reply::error(400, "unsure: true or false")),
                };
                let acting = crate::serve::acting_model(registry, caller)?.map(|m| m.id);
                // record 48 R1: what the engine suggested for the item, kept
                // beside the answer with the time it took; the engine's own,
                // never the caller's
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                let suggested = suggested_for(registry.store(), &c, a, pack.as_deref())?;
                // record 48: the axes derived from this answer through the
                // pack, the engine's own computation, kept beside it
                let derived = match value
                    .as_deref()
                    .and_then(|v| serde_json::from_str::<Value>(v).ok())
                    .filter(Value::is_object)
                {
                    Some(obj) => {
                        let q = c.question().map_err(campaign_err)?;
                        match assignment_stack(registry.store(), c.id, a)? {
                            Some(stack) => {
                                derived_for(registry.store(), pack.as_deref(), &q, stack, &obj)?
                            }
                            None => None,
                        }
                    }
                    None => None,
                };
                let answered = campaign::answer_with(
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
                        unsure,
                    },
                    &campaign::Timing {
                        suggested: suggested.as_deref(),
                        batch: false,
                        derived: derived.as_ref(),
                    },
                    &now,
                )
                .map_err(campaign_err)?;
                Ok(Reply::ok(json!({
                    "answer": answered.answer, "item": answered.item,
                    "state": answered.state, "adjudication": answered.adjudication,
                    "derived": derived,
                })))
            }
            // record 45: the rating workspace's heartbeat
            ["api", "campaigns", which, "assignments", a, "renew"] if post => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let a = id_of(a)?;
                belongs(registry.store(), c.id, "campaign_assignment", a)?;
                let done = campaign::renew(registry, a, principal, &now).map_err(campaign_err)?;
                Ok(Reply::ok(done.as_json()))
            }
            ["api", "campaigns", which, "assignments", a, "release"] if post => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let a = id_of(a)?;
                belongs(registry.store(), c.id, "campaign_assignment", a)?;
                // the campaign's owner and a holder of review:work give
                // back another's claim, audited with its holder
                let done = campaign::release_as(
                    registry,
                    a,
                    principal,
                    caller.access.holds("review:work"),
                    &now,
                )
                .map_err(campaign_err)?;
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
                no_sealed_flag(&doc)?;
                // blind through the export too: every answer of an open
                // campaign is for those who read them all at the answers door
                // record 48: and so are the outcomes of an open campaign, which
                // are its raters' answers, for a caller who reads only their own
                // campaigns
                if !sees_all(registry.store(), caller, principal, &c)? {
                    return Err(Reply::error(
                        403,
                        format!(
                            "rating in {} is blind until it closes: its {} are exported by an adjudicator or a holder of review:work",
                            c.name,
                            of.name()
                        ),
                    ));
                }
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                fill_derived(registry, pack.as_deref(), &c)?;
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
                    created_by: principal,
                };
                let dir = export_dir(registry.store(), doc["place"].as_str(), &name)?;
                let set = write_set(registry, &dir, true, &rows, &meta)
                    .map_err(|(status, e)| Reply::error(status, e))?;
                Ok(Reply::created(set_json(&set, None)))
            }
            // record 48 R1: the evidence line of a stack, one entry per axis
            ["api", "stacks", stack, "why"] if get => {
                let stack = id_of(stack)?;
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                let hidden = blind_to(registry.store(), caller, stack)?;
                let mut doc = crate::reader::why(
                    registry.store(),
                    stack,
                    pack.as_deref(),
                    !plain(caller),
                    hidden,
                )?
                .ok_or_else(|| {
                    Reply::error(404, format!("stack {stack} has not been classified"))
                })?;
                with_file(registry.store(), stack, !plain(caller), &mut doc)?;
                Ok(Reply::ok(doc))
            }
            ["api", "campaigns", which, "items", item, "why"] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let item = id_of(item)?;
                belongs(registry.store(), c.id, "campaign_item", item)?;
                let it = campaign::item(registry.store(), item)
                    .map_err(campaign_err)?
                    .ok_or_else(|| Reply::error(404, format!("no campaign item {item}")))?;
                let stack = it.stack_id.ok_or_else(|| {
                    Reply::error(
                        400,
                        format!("item {item} is a session's; the evidence line is a stack's"),
                    )
                })?;
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                let q = c.question().map_err(campaign_err)?;
                // record 48 R2: a stack of a sealed sample that an open
                // campaign asks is read blind by its raters
                let blind = blind_to(registry.store(), caller, stack)?;
                let mut doc = crate::reader::why(
                    registry.store(),
                    stack,
                    pack.as_deref(),
                    !plain(caller),
                    blind,
                )?
                .unwrap_or_else(|| json!({"stack": stack, "axes": [], "blind": false}));
                let suggested = if blind {
                    None
                } else {
                    crate::reader::suggestion(registry.store(), stack, &q, pack.as_deref())?
                };
                let mut s = json!(suggested);
                axes_value(&c.question, &mut s);
                doc["item"] = json!(item);
                doc["suggested"] = s;
                // record 48, after the first real read: blind hides the
                // systems' answers, never the file
                with_file(registry.store(), stack, !plain(caller), &mut doc)?;
                doc["header_door"] = json!(format!("/api/campaigns/{}/items/{item}/header", c.id));
                doc["worth"] = if blind {
                    Value::Null
                } else {
                    campaign::worth(registry.store(), &[stack], &campaign::axes_of(&q))
                        .map_err(campaign_err)?
                        .get(&stack)
                        .map(campaign::Worth::as_json)
                        .unwrap_or(Value::Null)
                };
                Ok(Reply::ok(doc))
            }
            // record 48, after the first real read: the whole stored header
            // of a representative instance, less what names a person, for an
            // item of the caller's own campaign
            ["api", "campaigns", which, "items", item, "header"] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let item = id_of(item)?;
                belongs(registry.store(), c.id, "campaign_item", item)?;
                let stack = item_stack(registry.store(), item)?;
                let mut doc = crate::file_header::whole(registry.store(), stack, !plain(caller))
                    .map_err(|e| Reply::error(500, e.to_string()))?
                    .ok_or_else(|| {
                        Reply::error(404, format!("stack {stack} is not in the registry"))
                    })?;
                doc["item"] = json!(item);
                doc["blind"] = json!(blind_to(registry.store(), caller, stack)?);
                Ok(Reply::ok(doc))
            }
            // record 48: the axes the pack derives from an answer, partial or
            // whole, so the reader shows them as the rater answers
            ["api", "campaigns", which, "items", item, "derive"] if post => {
                let doc = json_body(body)?;
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let item = id_of(item)?;
                belongs(registry.store(), c.id, "campaign_item", item)?;
                let stack = item_stack(registry.store(), item)?;
                let q = c.question().map_err(campaign_err)?;
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                let value = match &doc["value"] {
                    Value::Object(_) => doc["value"].clone(),
                    Value::String(t) => serde_json::from_str::<Value>(t)
                        .ok()
                        .filter(Value::is_object)
                        .ok_or_else(|| {
                            Reply::error(400, "value: the answer so far, {axis: value}")
                        })?,
                    _ => return Err(Reply::error(400, "value: the answer so far, {axis: value}")),
                };
                let derived = derived_for(registry.store(), pack.as_deref(), &q, stack, &value)?
                    .ok_or_else(|| {
                        Reply::error(409, format!("campaign {} derives no axes", c.name))
                    })?;
                Ok(Reply::ok(
                    json!({"item": item, "stack": stack, "derived": derived}),
                ))
            }
            // record 48, the reader's whole-combination search: how common
            // each combination of the answered axes is across the registry,
            // never counting this campaign's stacks or a sealed one
            ["api", "campaigns", which, "combinations"] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                let limit = query
                    .get("limit")
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(200)
                    .clamp(1, 1000);
                let doc = crate::reader::combinations(registry.store(), &c, pack.as_deref(), limit)
                    .map_err(|(st, m)| Reply::error(st, m))?;
                Ok(Reply::ok(doc))
            }
            ["api", "campaigns", which, "batches"] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                let found =
                    crate::reader::batches(registry.store(), &c, principal, pack.as_deref(), None)
                        .map_err(|(st, m)| Reply::error(st, m))?;
                crate::reader::remember(c.id, principal, &found);
                let sample = query
                    .get("sample")
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(5);
                Ok(Reply::ok(found.as_json(
                    &c,
                    c.question["kind"].as_str().unwrap_or_default(),
                    sample,
                )))
            }
            ["api", "campaigns", which, "batches", key, "accept"] if post => {
                let doc = json_body(body)?;
                // record 48 R1: what is held back is the campaign's, set by
                // its maker, and chosen by a seed the engine drew; never the
                // caller's to say
                for held in ["hold_back", "seed"] {
                    if doc.get(held).is_some() {
                        return Err(Reply::error(
                            400,
                            format!(
                                "{held} is not the caller's to say: the share a batch holds back is the campaign's (hold_back, set when it is made), and the engine draws the seed"
                            ),
                        ));
                    }
                }
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let named: Option<Vec<i64>> = match &doc["items"] {
                    Value::Null => None,
                    Value::Array(list) => {
                        let ids: Vec<i64> = list.iter().filter_map(Value::as_i64).collect();
                        if ids.len() != list.len() {
                            return Err(Reply::error(400, "items: a list of item ids"));
                        }
                        Some(ids)
                    }
                    _ => return Err(Reply::error(400, "items: a list of item ids")),
                };
                // an item of a sealed sample is never accepted in one move
                if let Some(ids) = &named {
                    let stacks = item_stacks(registry.store(), c.id, ids)?;
                    let (sealed, _) =
                        labels::sealed_now(registry.store(), &stacks, &[]).map_err(labels_err)?;
                    if !sealed.is_empty() {
                        return Err(Reply::error(
                            409,
                            format!(
                                "{} of these items are of a sealed certification sample, and each is read alone, never accepted in a batch",
                                sealed.len()
                            ),
                        ));
                    }
                }
                let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
                let batch =
                    crate::reader::batch_now(registry.store(), &c, principal, pack.as_deref(), key)
                        .map_err(|(st, m)| Reply::error(st, m))?
                        .ok_or_else(|| {
                            Reply::error(
                                409,
                                format!("campaign {} has no batch {key} open to {principal} now; list the batches again", c.name),
                            )
                        })?;
                if let Some(x) = named.iter().flatten().find(|i| !batch.items.contains(i)) {
                    return Err(Reply::error(
                        409,
                        format!("item {x} is not in batch {key} now"),
                    ));
                }
                // the share is held back over the whole batch, whatever the
                // caller names
                let seed =
                    campaign::hold_back_seed(registry.store(), c.id).map_err(campaign_err)?;
                let held = crate::reader::held_back(&batch.items, c.hold_back, &seed);
                let held_list: Vec<i64> = held.iter().copied().collect();
                campaign::hold_back(registry.store(), c.id, &held_list).map_err(campaign_err)?;
                let chosen: Vec<i64> = named
                    .unwrap_or_else(|| batch.items.clone())
                    .into_iter()
                    .filter(|i| !held.contains(i))
                    .collect();
                let acting = crate::serve::acting_model(registry, caller)?.map(|m| m.id);
                let given = Given {
                    assignment: 0,
                    principal,
                    author_kind: kind_of(caller),
                    model: acting,
                    value: Some(&batch.suggested),
                    form: None,
                    derivative_id: None,
                    why: None,
                    unsure: false,
                };
                let done = campaign::accept_many(
                    registry,
                    c.id,
                    &chosen,
                    &given,
                    Some(&batch.suggested),
                    &now,
                )
                .map_err(campaign_err)?;
                // record 48: each accepted answer keeps what it derives
                fill_derived(registry, pack.as_deref(), &c)?;
                let accepted: Vec<Value> = done
                    .accepted
                    .iter()
                    .map(|d| json!({"item": d.item, "answer": d.answer, "state": d.state}))
                    .collect();
                let refused: Vec<Value> = done
                    .refused
                    .iter()
                    .map(|(item, why)| json!({"item": item, "why": why}))
                    .collect();
                Ok(Reply::ok(json!({
                    "campaign": c.id, "batch": key, "hold_back": c.hold_back,
                    "accepted": accepted, "held_back": held_list, "refused": refused,
                })))
            }
            ["api", "campaigns", which, "stats"] if get => {
                let c = campaign::find(registry.store(), which).map_err(campaign_err)?;
                let mut doc = campaign::stats(registry.store(), c.id).map_err(campaign_err)?;
                // blind as the answers are: a rater reads their own row
                if !sees_all(registry.store(), caller, principal, &c)? {
                    if let Some(list) = doc["raters"].as_array_mut() {
                        list.retain(|r| r["principal"] == principal);
                    }
                    // the totals would give away the other raters' counts
                    if let Some(m) = doc.as_object_mut() {
                        m.remove("all");
                    }
                    doc["blind"] = json!(true);
                }
                Ok(Reply::ok(doc))
            }
            // record 48 R2: the certificates of sealed samples
            ["api", "certificates"] if get => {
                let list = labels::certificates(registry.store()).map_err(labels_err)?;
                Ok(Reply::ok(json!({
                    "count": list.len(),
                    "certificates": list.iter().map(labels::Certificate::as_json).collect::<Vec<_>>(),
                })))
            }
            ["api", "certificates"] if post => {
                person_only(doors, caller, "recording a certificate")?;
                let doc = json_body(body)?;
                let sample = doc["sample"].as_str().ok_or_else(|| {
                    Reply::error(
                        400,
                        "sample: the sealed sample it measured, as nils labels seal named it",
                    )
                })?;
                let mut ids = Vec::new();
                for m in doc["models"].as_array().into_iter().flatten() {
                    let reference = match m {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        _ => return Err(Reply::error(400, "models: ids, digests or name@version")),
                    };
                    let found = nils_registry::model::resolve(registry.store(), &reference)?
                        .ok_or_else(|| {
                            Reply::error(404, format!("no registered model answers to {reference}"))
                        })?;
                    ids.push(found.id);
                }
                let cert = labels::record_certificate(
                    registry,
                    sample,
                    &ids,
                    &doc["result"],
                    principal,
                    kind_of(caller),
                )
                .map_err(labels_err)?;
                Ok(Reply::created(cert.as_json()))
            }
            ["api", "certificates", id, "unseal"] if post => {
                person_only(doors, caller, "unsealing a certified sample")?;
                let id = id_of(id)?;
                let cert = labels::certificate(registry.store(), id)
                    .map_err(labels_err)?
                    .ok_or_else(|| Reply::error(404, format!("no certificate {id}")))?;
                let done = labels::unseal(registry, &cert.sample, id, principal, kind_of(caller))
                    .map_err(labels_err)?;
                Ok(Reply::ok(done.as_json()))
            }
            ["api", "label-sets"] if get => {
                let list = labels::list(registry.store()).map_err(labels_err)?;
                let mut out: Vec<Value> = Vec::new();
                for set in &list {
                    if !reads_set(registry.store(), caller, set)? {
                        continue;
                    }
                    let mut v = set.as_json();
                    training_now(registry.store(), set, &mut v);
                    out.push(v);
                }
                Ok(Reply::ok(json!({"count": out.len(), "label_sets": out})))
            }
            ["api", "label-sets"] if post => {
                // record 48: a set of decisions reads the review's decisions,
                // every stack's, which a rater of a campaign does not read
                if !caller.access.holds("review:see") {
                    return Err(Reply::error(
                        403,
                        format!(
                            "a label set of decisions reads the review's decisions and needs review:see; {principal} exports a campaign's labels through its export door"
                        ),
                    ));
                }
                let doc = json_body(body)?;
                no_sealed_flag(&doc)?;
                // record 48 R2: the development labels, everything usable
                // for training and nothing of a sample sealed now
                let for_training = doc["for_training"].as_bool().unwrap_or(false);
                if for_training && doc["staged"].as_bool().unwrap_or(false) {
                    return Err(Reply::error(
                        400,
                        "for_training reads the decisions in force; a staged one never trains",
                    ));
                }
                let axis = match doc["axis"].as_str() {
                    Some(a) => a.to_string(),
                    None if for_training => String::new(),
                    None => return Err(Reply::error(400, "axis: what the labels are of")),
                };
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
                let (rows, left_out) = if for_training {
                    labels::training_labels(
                        registry.store(),
                        (!axis.is_empty()).then_some(axis.as_str()),
                        stacks.as_deref(),
                        &authors,
                        campaign_id,
                    )
                    .map_err(labels_err)?
                } else {
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
                    (rows, 0)
                };
                let what = if axis.is_empty() {
                    "training".to_string()
                } else {
                    axis.clone()
                };
                let fallback = if for_training {
                    format!("{what}-training")
                } else {
                    what.clone()
                };
                let name = doc["name"].as_str().unwrap_or(&fallback).to_string();
                let meta = SetMeta {
                    name: &name,
                    kind: "decisions",
                    what: &what,
                    source: json!({
                        "axis": doc["axis"], "authors": authors, "campaign": campaign_id,
                        "selection": doc["selection"], "handle": handle_id,
                        "staged": doc["staged"].as_bool().unwrap_or(false),
                        "for_training": for_training, "left_out_sealed": left_out,
                    }),
                    campaign_id,
                    handle_id,
                    created_by: principal,
                };
                let dir = export_dir(registry.store(), doc["place"].as_str(), &name)?;
                let set = write_set(registry, &dir, true, &rows, &meta)
                    .map_err(|(status, e)| Reply::error(status, e))?;
                let mut v = set_json(&set, None);
                if for_training {
                    v["left_out_sealed"] = json!(left_out);
                }
                Ok(Reply::created(v))
            }
            ["api", "label-sets", id] if get => {
                let id = id_of(id)?;
                let set = labels::get(registry.store(), id)
                    .map_err(labels_err)?
                    .filter(|set| reads_set(registry.store(), caller, set).unwrap_or(false))
                    .ok_or_else(|| Reply::error(404, format!("no label set {id}")))?;
                // a session's day is quasi-identifying
                if set.what.starts_with("pick:") {
                    caller.allowed("a label set of sessions", Need::Any, Detail::Quasi)?;
                }
                // a set of forms or free text carries a person's words, which
                // are read at quasi as the answers are: at plain, the set's
                // metadata and counts only
                if plain(caller) && holds_words(&set.what) {
                    return Ok(Reply::ok(set_json(&set, None)));
                }
                // a set of a campaign's answers is as blind as its answers
                // door while the campaign is open: metadata and counts only
                if (set.kind == Of::Answers.name() || set.kind == Of::Outcomes.name())
                    && let Some(cid) = set.campaign_id
                {
                    let c =
                        campaign::find(registry.store(), &cid.to_string()).map_err(campaign_err)?;
                    if !sees_all(registry.store(), caller, principal, &c)? {
                        return Ok(Reply::ok(set_json(&set, None)));
                    }
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
                let mut v = set_json(&set, files);
                training_now(registry.store(), &set, &mut v);
                Ok(Reply::ok(v))
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
                // record 45 E3: a value of the axis, or several joined; an
                // empty text for "no value here", and null for not asked
                let value_of = |key: &str| -> Result<Option<String>, Reply> {
                    Ok(match doc.get(key) {
                        None | Some(Value::Null) => None,
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(Value::Array(list)) => Some(
                            list.iter()
                                .map(|v| {
                                    v.as_str().map(str::to_string).ok_or_else(|| {
                                        Reply::error(
                                            400,
                                            format!("{key}: a value or a list of values"),
                                        )
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()?
                                .join(","),
                        ),
                        Some(_) => {
                            return Err(Reply::error(
                                400,
                                format!("{key}: a value or a list of values"),
                            ));
                        }
                    })
                };
                let stacks = match &doc["stacks"] {
                    Value::Null => None,
                    Value::Array(list) => Some(
                        list.iter()
                            .map(|v| {
                                v.as_i64()
                                    .ok_or_else(|| Reply::error(400, "stacks: a list of stack ids"))
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    ),
                    _ => return Err(Reply::error(400, "stacks: a list of stack ids")),
                };
                let model = doc["model"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| doc["model"].as_i64().map(|i| i.to_string()));
                let axis = doc["axis"].as_str().map(str::to_string);
                let served = doors
                    .pack_dir
                    .as_ref()
                    .and_then(|dir| nils_pack::load(&dir.join(&doors.ask_pack), None).ok());
                let filter = nils_registry::review::CommitFilter {
                    min_confidence: doc["min_confidence"].as_f64(),
                    campaign: campaign_id,
                    model,
                    names: value_names(served.as_ref(), axis.as_deref()),
                    axis,
                    from: value_of("from")?,
                    to: value_of("to")?,
                    stacks,
                };
                let done = nils_registry::review::commit_where(
                    registry,
                    &filter,
                    doc["anyway"].as_bool().unwrap_or(false),
                    principal,
                    kind_of(caller),
                )
                .map_err(|e| match e {
                    nils_registry::review::Error::Refused(m) => Reply::error(409, m),
                    other => Reply::error(500, other.to_string()),
                })?;
                Ok(Reply::ok(json!({
                    "committed": done.decisions, "items": done.items, "left": done.left,
                    "split": done.split,
                })))
            }
            _ => Err(Reply::error(
                404,
                format!("{method} /{} is not a door", segs.join("/")),
            )),
        }
    })())
}

/// Record 48 R2: whether the caller reads a stack blind: it is of a sample
/// sealed now, an open campaign asks it, and the caller rates in that
/// campaign or is neither its adjudicator nor a holder of review:work.
pub(crate) fn blind_to(store: &mut Store, caller: &Caller, stack: i64) -> Result<bool, Reply> {
    Ok(crate::reader::hidden(
        store,
        stack,
        &caller.principal,
        caller.access.holds("review:work"),
    )?)
}

/// Record 48 R2: the stacks of these a caller reads blind.
pub(crate) fn blind_among(
    store: &mut Store,
    caller: &Caller,
    stacks: &[i64],
) -> Result<std::collections::BTreeSet<i64>, Reply> {
    Ok(crate::reader::hidden_stacks(
        store,
        stacks,
        &caller.principal,
        caller.access.holds("review:work"),
    )?)
}

/// Record 48 R2: a certificate and the unseal are a person's acts; an
/// agent's or a model's token is refused, whatever grants it holds.
fn person_only(doors: &Doors, caller: &Caller, act: &str) -> Result<(), Reply> {
    // two people are told apart by the identity the engine verified,
    // never by a local user name
    if !doors.identity_verified() {
        return Err(Reply::error(
            403,
            format!("{act} needs a verified identity; this engine runs with --auth off"),
        ));
    }
    let kind = kind_of(caller);
    if kind != "person" {
        return Err(Reply::error(
            403,
            format!("{act} is a person's act, and this caller acts as a {kind}"),
        ));
    }
    Ok(())
}

/// Record 48 R2: whether a set trains now, under the seals in force: a set
/// written while its sample was sealed trains once a certificate unseals
/// the sample. One reading for the list door and the set's own door.
fn training_now(store: &mut Store, set: &LabelSet, v: &mut Value) {
    if set.sealed && labels::usable_for_training(store, set.id).is_ok() {
        v["training"] = json!("allowed: its sample was unsealed by a certificate (record 48 R2)");
    }
}

/// Record 40 R3: whether a set is sealed is the registry's finding, from
/// the samples an operator sealed, and never the caller's to say.
fn no_sealed_flag(doc: &Value) -> Result<(), Reply> {
    if doc.get("sealed").is_some() {
        return Err(Reply::error(
            400,
            "sealed is not the caller's to say: a set is sealed when any of its items is of a sample an operator sealed (nils labels seal)",
        ));
    }
    Ok(())
}

/// Whether a set's labels may hold a person's words: a form's, a free
/// answer's, or the form beside a derivative.
fn holds_words(what: &str) -> bool {
    what == "form" || what == "free" || what.starts_with("derivative:")
}

/// Whether the caller reads at detail plain, where a campaign's doors
/// leave out what is quasi-identifying or free text.
/// Record 45: rating is blind until the campaign closes. A rater reads
/// their own answers; an adjudicator of the campaign and a holder of
/// review:work read every one, at the answers door, through an export of
/// the answers and in the label set it wrote.
fn sees_all(
    store: &mut Store,
    caller: &Caller,
    principal: &str,
    c: &campaign::Campaign,
) -> Result<bool, Reply> {
    Ok(c.status == "closed"
        || caller.access.holds("review:work")
        || c.adjudicators().iter().any(|p| p == principal)
        || campaign::assignments(store, c.id)
            .map_err(campaign_err)?
            .iter()
            .any(|a| a.role == "adjudicator" && a.principal.as_deref() == Some(principal)))
}

/// Record 48: whether the caller reads every campaign, as a holder of
/// review:work reads every answer. Anyone else reads the campaigns of their
/// own ([`own`]); `campaigns:see` alone does not open another's, since every
/// holder of `campaigns:work` holds it.
fn oversees(caller: &Caller) -> bool {
    caller.access.holds("review:work")
}

/// Record 48: whether a campaign is the caller's own: they made it, it
/// names them as a rater or an adjudicator, or they hold an assignment in
/// it. It fails closed: a campaign that names no raters is no one's but its
/// maker's, and is rated by those who read every campaign.
fn own(store: &mut Store, caller: &Caller, c: &campaign::Campaign) -> Result<bool, Reply> {
    let p = caller.principal.as_str();
    if c.owner == p || rates_in(caller, c) {
        return Ok(true);
    }
    Ok(campaign::assignments(store, c.id)
        .map_err(campaign_err)?
        .iter()
        .any(|a| a.principal.as_deref() == Some(p)))
}

/// Whether the caller is named in a campaign as a rater or an adjudicator.
/// A campaign that names no raters opens nothing through this.
fn rates_in(caller: &Caller, c: &campaign::Campaign) -> bool {
    let p = caller.principal.as_str();
    c.raters().iter().any(|r| r == p) || c.adjudicators().iter().any(|a| a == p)
}

/// Record 48: a campaign as a caller reads it who does not read every
/// answer: an item another person has been given keeps its state and loses
/// its outcome, agreement and metric, which are that person's; only the
/// caller's own assignments are listed; the open agreement is left out.
fn blind_view(v: &mut Value, principal: &str) {
    let mut others = std::collections::BTreeSet::new();
    if let Some(list) = v["assignments"].as_array_mut() {
        for a in list.iter() {
            if a["principal"].as_str() != Some(principal)
                && let Some(item) = a["item_id"].as_i64()
            {
                others.insert(item);
            }
        }
        list.retain(|a| a["principal"].as_str() == Some(principal));
    }
    for it in v["items"].as_array_mut().into_iter().flatten() {
        if it["id"].as_i64().is_some_and(|id| others.contains(&id))
            && let Some(m) = it.as_object_mut()
        {
            for key in ["outcome", "agreement", "metric"] {
                m.insert(key.into(), Value::Null);
            }
            m.insert("blind".into(), json!(true));
        }
    }
    if let Some(m) = v.as_object_mut() {
        m.remove("agreement");
    }
}

/// Record 48: whether the caller reads a label set: a reader of the review
/// reads every set; anyone else the sets they wrote and those of their own
/// campaigns.
fn reads_set(store: &mut Store, caller: &Caller, set: &LabelSet) -> Result<bool, Reply> {
    if caller.access.holds("review:see") || set.created_by == caller.principal {
        return Ok(true);
    }
    match set.campaign_id {
        Some(cid) => match campaign::get(store, cid).map_err(campaign_err)? {
            Some(c) => own(store, caller, &c),
            None => Ok(false),
        },
        None => Ok(false),
    }
}

/// Record 48: the open campaign through which a caller who does not hold
/// query:see may read a stack's pictures: one that names them as a rater or
/// an adjudicator and whose items hold the stack, or for a campaign of sessions, a session the
/// stack is of. None when there is no such campaign.
pub(crate) fn pictures_through(
    store: &mut Store,
    caller: &Caller,
    stack: i64,
) -> Result<Option<i64>, Reply> {
    let d = store.dialect();
    let mut asked: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    let sql = format!(
        "SELECT DISTINCT i.campaign_id FROM {} i JOIN {} c ON c.id = i.campaign_id \
         WHERE c.status = 'open' AND i.stack_id = {}",
        store.qualified("campaign_item"),
        store.qualified("campaign"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(stack)])? {
        asked.insert(r.int(0)?);
    }
    // the sessions the stack is of, each its subject and its day
    let sql = format!(
        "SELECT DISTINCT sc.subject_id, {} FROM {} k JOIN {} r ON r.id = k.series_id \
         JOIN {} cs ON cs.study_id = r.study_id JOIN {} sc ON sc.id = cs.session_id \
         WHERE k.id = {}",
        d.text_of_qualified(
            Some("sc"),
            nils_registry::schema::table("session_cache")
                .column("first")
                .expect("first"),
        ),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("session_cache_study"),
        store.qualified("session_cache"),
        d.param(1, Type::Int),
    );
    let day = |t: &str| t.chars().take(10).collect::<String>();
    let mut sessions: Vec<(i64, String)> = Vec::new();
    for r in store.query(&sql, &[Param::Int(stack)])? {
        if let Some(first) = r.opt_text(1)? {
            sessions.push((r.int(0)?, day(first)));
        }
    }
    for (subject, first) in sessions {
        let sql = format!(
            "SELECT i.campaign_id, {} FROM {} i JOIN {} c ON c.id = i.campaign_id \
             WHERE c.status = 'open' AND i.subject_id = {}",
            d.text_of_qualified(
                Some("i"),
                nils_registry::schema::table("campaign_item")
                    .column("session_day")
                    .expect("session_day"),
            ),
            store.qualified("campaign_item"),
            store.qualified("campaign"),
            d.param(1, Type::Int)
        );
        for r in store.query(&sql, &[Param::Int(subject)])? {
            if r.opt_text(1)?.is_some_and(|t| day(t) == first) {
                asked.insert(r.int(0)?);
            }
        }
    }
    for id in asked {
        if let Some(c) = campaign::get(store, id).map_err(campaign_err)?
            && c.status == "open"
            && rates_in(caller, &c)
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn plain(caller: &Caller) -> bool {
    caller.access.detail < Detail::Quasi
}

/// An item at detail plain: a session is named by its subject and the day
/// it opened, which are quasi-identifying, so a session's item keeps its id
/// and position and loses both, with the key that spells them; a form or a
/// free text the item came to is left out, as the answers' are.
fn plain_item(v: &mut Value, grain: &str, free: bool) {
    let Some(m) = v.as_object_mut() else {
        return;
    };
    if grain == "session" {
        m.remove("subject_id");
        m.remove("session_day");
        m.remove("key");
    }
    if let Some(Value::Object(o)) = m.get_mut("outcome") {
        o.remove("form");
        if free {
            o.remove("value");
        }
    }
}

/// An answer at detail plain: no why, no form, no actor detail, and no
/// value where the value is free text.
fn plain_answer(v: &mut Value, free: bool) {
    let Some(m) = v.as_object_mut() else {
        return;
    };
    for field in ["why", "form", "actor_detail"] {
        m.remove(field);
    }
    if free {
        m.remove("value");
    }
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

/// The stacks of some items of a campaign.
fn item_stacks(store: &mut Store, campaign: i64, items: &[i64]) -> Result<Vec<i64>, Reply> {
    let mut out = Vec::new();
    for chunk in items.chunks(500) {
        let ids = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT stack_id FROM {} WHERE campaign_id = {} AND id IN ({ids}) AND stack_id IS NOT NULL",
            store.qualified("campaign_item"),
            store.dialect().param(1, Type::Int)
        );
        for r in store.query(&sql, &[Param::Int(campaign)])? {
            out.push(r.int(0)?);
        }
    }
    Ok(out)
}

/// Record 48 R1: what the engine suggests for the item an assignment
/// leased, for the answer to keep beside it.
fn suggested_for(
    store: &mut Store,
    c: &campaign::Campaign,
    assignment: i64,
    pack: Option<&nils_pack::Pack>,
) -> Result<Option<String>, Reply> {
    let Ok(q) = c.question() else {
        return Ok(None);
    };
    if !matches!(
        q,
        campaign::Question::Axis { .. } | campaign::Question::Axes { .. }
    ) {
        return Ok(None);
    }
    let sql = format!(
        "SELECT i.stack_id FROM {} a JOIN {} i ON i.id = a.item_id WHERE a.id = {}",
        store.qualified("campaign_assignment"),
        store.qualified("campaign_item"),
        store.dialect().param(1, Type::Int)
    );
    let stack = store
        .query_opt(&sql, &[Param::Int(assignment)])?
        .and_then(|r| r.opt_int(0).ok().flatten());
    match stack {
        Some(s) => Ok(crate::reader::suggestion(store, s, &q, pack)?),
        None => Ok(None),
    }
}

/// Record 45: an axes answer is kept as one text, and read as the object
/// it is.
fn axes_value(question: &Value, value: &mut Value) {
    if question["kind"] == "axes"
        && let Some(text) = value.as_str()
        && let Ok(v) = serde_json::from_str::<Value>(text)
        && v.is_object()
    {
        *value = v;
    }
}

fn shown(store: &mut Store, c: &campaign::Campaign) -> Result<Value, Reply> {
    let mut v = c.as_json();
    v["counts"] = campaign::counts(store, c.id).map_err(campaign_err)?;
    v["items"] = json!(
        campaign::items(store, c.id)
            .map_err(campaign_err)?
            .iter()
            .map(|it| {
                let mut j = it.as_json();
                axes_value(&c.question, &mut j["outcome"]["value"]);
                j
            })
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
/// session opened, from the session cache the keys name, read five hundred
/// at a time, in the keys' order.
fn sessions_of(
    store: &mut Store,
    keys: &[(i64, Option<i64>)],
) -> Result<Vec<(i64, String)>, Reply> {
    let d = store.dialect();
    let t = nils_registry::schema::table("session_cache");
    let ids: Vec<i64> = keys.iter().map(|(k, _)| *k).collect();
    let mut found: HashMap<i64, (i64, String)> = HashMap::new();
    for chunk in ids.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, subject_id, {} FROM {} WHERE id IN ({list})",
            d.text_of(t.column("first").expect("first")),
            store.qualified("session_cache"),
        );
        for r in store.query(&sql, &[])? {
            found.insert(r.int(0)?, (r.int(1)?, r.text(2)?.to_string()));
        }
    }
    ids.iter()
        .map(|k| {
            found.get(k).cloned().ok_or_else(|| {
                Reply::error(
                    409,
                    format!("session {k} is not in the cache; rebuild the sessions"),
                )
            })
        })
        .collect()
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

/// Record 48 R1: the share of a batch a campaign holds back, as its maker
/// gives it; the engine's least when none is.
fn hold_back_of(v: &Value) -> Result<Option<f64>, Reply> {
    match v {
        Value::Null => Ok(None),
        v => v.as_f64().map(Some).ok_or_else(|| {
            Reply::error(
                400,
                format!(
                    "hold_back: the share of a batch read alone, from {} to 1",
                    campaign::HOLD_BACK_MIN
                ),
            )
        }),
    }
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
    let served = doors
        .pack_dir
        .as_ref()
        .and_then(|dir| nils_pack::load(&dir.join(&doors.ask_pack), None).ok());
    complete_axes(&mut question, served.as_ref()).map_err(|(s, m)| Reply::error(s, m))?;
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
        // record 48: a handle is the ask's result, read by those who read
        // the ask, so a rater cannot name stacks into a campaign of theirs
        caller.allowed(
            "a campaign over a handle",
            Need::One("query:see"),
            Detail::Plain,
        )?;
        let keys = handle_keys(registry.store(), h, want)?;
        (items_of(registry.store(), want, &keys)?, Some(h))
    } else if source["review"].is_object() {
        // and the open review items are the review's to read
        caller.allowed(
            "a campaign over review items",
            Need::One("review:see"),
            Detail::Plain,
        )?;
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
            hold_back: hold_back_of(&doc["hold_back"])?,
        },
    )
    .map_err(campaign_err)?;
    let mut v = shown(registry.store(), &made)?;
    v["pictures"] = pictures_of(registry.store(), &made)?;
    Ok(v)
}

/// Record 45 E3: every name a value of an axis goes by (its identity, its
/// label, the form the classifier stores) to its identity, so a commit by
/// filter compares values as the pack means them.
pub(crate) fn value_names(
    pack: Option<&nils_pack::Pack>,
    axis: Option<&str>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let (Some(pack), Some(axis)) = (pack, axis) else {
        return out;
    };
    if let Some(a) = pack.axes.iter().find(|a| a.name == axis) {
        for (i, v) in a.values.iter().enumerate() {
            out.insert(v.label.clone(), v.id.clone());
            out.insert(a.stored(i).to_string(), v.id.clone());
            out.insert(v.id.clone(), v.id.clone());
            // an identity the value had before a rename reads as it
            for a in &v.aliases {
                out.insert(a.clone(), v.id.clone());
            }
        }
    }
    out
}

/// Record 45 E4: an axes question as a campaign keeps it, the served pack's
/// legal combinations frozen into it (`nils_pack::legal`), so an answer is
/// held to the pack the campaign was made under. The constraints are never
/// the caller's to say, and without a pack no axes question is made.
fn complete_axes(
    question: &mut Value,
    pack: Option<&nils_pack::Pack>,
) -> Result<(), (u16, String)> {
    if question.get("vocabulary").is_some() {
        return Err((
            400,
            "vocabulary is not the caller's to say: the engine serves the pack's names for each value with the question".into(),
        ));
    }
    if question["kind"] != "axes" {
        return Ok(());
    }
    if question.get("constraints").is_some() {
        return Err((
            400,
            "constraints are not the caller's to say: the engine freezes the served pack's legal combinations into an axes question".into(),
        ));
    }
    let pack = pack.ok_or((
        409,
        "an axes question is held to the pack's legal combinations, and no pack is served here"
            .to_string(),
    ))?;
    let axes = strings(&question["axes"]);
    let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(m) = question["values"].as_object() {
        for (axis, list) in m {
            values.insert(axis.clone(), strings(list));
        }
    } else if !question["values"].is_null() {
        return Err((400, "values: {axis: [values]}".into()));
    }
    let c = nils_pack::legal::constraints(pack, &axes, &values).map_err(|e| (400, e))?;
    question["values"] = c["values"].clone();
    question["constraints"] = c;
    // record 48, after the first real read: the axes the pack computes from
    // the answer, named by the caller and checked, or found in the pack
    // when not named; `derive: []` derives nothing
    let derive = match &question["derive"] {
        Value::Null => nils_pack::derive::infer(pack, &axes).map_err(|e| (400, e))?,
        Value::Array(_) => strings(&question["derive"]),
        _ => return Err((400, "derive: a list of axis names".into())),
    };
    nils_pack::derive::check(pack, &axes, &derive).map_err(|e| (400, e))?;
    question["derive"] = json!(derive);
    Ok(())
}

/// Record 48, the reader's search: the names each value the question asks
/// goes by in the served pack (`nils_pack::legal::vocabulary`), for an axis
/// or an axes question; the axes it derives are never rows and get none.
/// Nothing without a served pack.
fn vocabulary_of(question: &Value, pack: Option<&nils_pack::Pack>) -> Option<Value> {
    let pack = pack?;
    let words = |v: &Value| strings(v);
    let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let all = |axis: &str| -> Vec<String> {
        pack.axes
            .iter()
            .find(|a| a.name == axis)
            .map(|a| a.values.iter().map(|v| v.id.clone()).collect())
            .unwrap_or_default()
    };
    match question["kind"].as_str() {
        Some("axis") => {
            let axis = question["axis"].as_str()?;
            let listed = words(&question["values"]);
            values.insert(
                axis.to_string(),
                if listed.is_empty() { all(axis) } else { listed },
            );
        }
        Some("axes") => {
            let derived = words(&question["derive"]);
            for axis in words(&question["axes"]) {
                if derived.contains(&axis) {
                    continue;
                }
                let listed = words(&question["values"][axis.as_str()]);
                let listed = if listed.is_empty() {
                    all(&axis)
                } else {
                    listed
                };
                values.insert(axis, listed);
            }
        }
        _ => return None,
    }
    Some(nils_pack::legal::vocabulary(pack, &values))
}

/// Record 48, after the first real read: the file's own text and physics
/// beside a reader's document, blind or not. These are what a radiologist
/// reads, never anything a system said of the stack.
fn with_file(store: &mut Store, stack: i64, quasi: bool, doc: &mut Value) -> Result<(), Reply> {
    let (texts, physics) = crate::file_header::texts_and_physics(store, stack, quasi)
        .map_err(|e| Reply::error(500, e.to_string()))?;
    doc["texts"] = Value::Object(texts);
    doc["physics"] = Value::Object(physics);
    Ok(())
}

/// The stack an item of stacks stands on.
fn item_stack(store: &mut Store, item: i64) -> Result<i64, Reply> {
    campaign::item(store, item)
        .map_err(campaign_err)?
        .ok_or_else(|| Reply::error(404, format!("no campaign item {item}")))?
        .stack_id
        .ok_or_else(|| {
            Reply::error(
                400,
                format!("item {item} is a session's; only a stack's axes are derived"),
            )
        })
}

/// The stack the item of an assignment stands on, where it is a stack's.
fn assignment_stack(
    store: &mut Store,
    campaign: i64,
    assignment: i64,
) -> Result<Option<i64>, Reply> {
    let d = store.dialect();
    let sql = format!(
        "SELECT i.stack_id FROM {} a JOIN {} i ON i.id = a.item_id WHERE a.id = {} AND a.campaign_id = {}",
        store.qualified("campaign_assignment"),
        store.qualified("campaign_item"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(assignment), Param::Int(campaign)])
        .map_err(|e| Reply::error(500, e.to_string()))?
        .and_then(|r| r.opt_int(0).ok().flatten()))
}

/// Record 48: the axes an axes question derives, computed for one stack
/// from an answer (whole or partial, `{axis: value | [values] | null |
/// "cant_tell"}`) through the served pack's own rules. None where the
/// question derives nothing or no pack is served; an asked axis the answer
/// does not name is taken as can't tell. Each derived axis comes back as
/// a value, a list for a multi-valued axis, null for none, or `cant_tell`
/// where an asked axis it reads was answered so.
fn derived_for(
    store: &mut Store,
    pack: Option<&nils_pack::Pack>,
    q: &campaign::Question,
    stack: i64,
    answer: &Value,
) -> Result<Option<Value>, Reply> {
    let campaign::Question::Axes { axes, derive, .. } = q else {
        return Ok(None);
    };
    let Some(pack) = pack else {
        return Ok(None);
    };
    if derive.is_empty() {
        return Ok(None);
    }
    let Some((s, private)) = nils_classify::classify::stack_of(store, pack, stack)
        .map_err(|e| Reply::error(500, e.to_string()))?
    else {
        return Err(Reply::error(
            409,
            format!("stack {stack} has no fingerprint, so nothing is derived for it"),
        ));
    };
    let mut given: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
    for axis in axes {
        let names = value_names(Some(pack), Some(axis));
        let one = |v: &str| names.get(v).cloned().unwrap_or_else(|| v.to_string());
        let read = match &answer[axis] {
            Value::Null if answer.get(axis).is_some() => Some(Vec::new()),
            Value::String(t) if t == campaign::CANT_TELL => None,
            Value::String(t) if t.trim().is_empty() => Some(Vec::new()),
            Value::String(t) => Some(vec![one(t.trim())]),
            Value::Array(list) => {
                let w: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
                if w.contains(&campaign::CANT_TELL) {
                    None
                } else {
                    Some(w.into_iter().map(one).collect())
                }
            }
            _ => None,
        };
        given.insert(axis.clone(), read);
    }
    let got = nils_pack::derive::derive(pack, &s, private, axes, derive, &given);
    let mut out = serde_json::Map::new();
    for d in derive {
        let multi = pack
            .axis_index(d)
            .map(|i| pack.axes[i].multi)
            .unwrap_or(false);
        let v = match got.get(d) {
            Some(None) => json!(campaign::CANT_TELL),
            Some(Some(vals)) if multi => json!(vals),
            Some(Some(vals)) => match vals.as_slice() {
                [] => Value::Null,
                [one] => json!(one),
                many => json!(many),
            },
            None => Value::Null,
        };
        out.insert(d.clone(), v);
    }
    Ok(Some(Value::Object(out)))
}

/// Record 48: every kept answer of an axes campaign that derives axes and
/// has none kept yet (given to a batch, or before a requestion) gets what
/// it derives, from its own value, through the served pack. What an answer
/// derived once is never changed.
fn fill_derived(
    registry: &mut Registry,
    pack: Option<&nils_pack::Pack>,
    c: &campaign::Campaign,
) -> Result<usize, Reply> {
    let q = c.question().map_err(campaign_err)?;
    let campaign::Question::Axes {
        axes,
        constraints,
        derive,
    } = &q
    else {
        return Ok(0);
    };
    if derive.is_empty() || pack.is_none() {
        return Ok(0);
    }
    let items: BTreeMap<i64, Option<i64>> = campaign::items(registry.store(), c.id)
        .map_err(campaign_err)?
        .into_iter()
        .map(|i| (i.id, i.stack_id))
        .collect();
    let mut n = 0;
    for a in campaign::answers(registry.store(), c.id).map_err(campaign_err)? {
        if a.derived.is_some() {
            continue;
        }
        let (Some(v), Some(Some(stack))) = (a.value.as_deref(), items.get(&a.item_id)) else {
            continue;
        };
        let Ok(joint) = campaign::stored_joint_of(axes, constraints, v) else {
            continue;
        };
        let obj: serde_json::Map<String, Value> = joint
            .iter()
            .map(|(k, vals)| (k.clone(), json!(vals)))
            .collect();
        if let Some(d) = derived_for(registry.store(), pack, &q, *stack, &Value::Object(obj))?
            && campaign::set_derived(registry.store(), a.id, &d).map_err(campaign_err)?
        {
            n += 1;
        }
    }
    Ok(n)
}

/// The stacks a campaign's items stand on: an item's stack, a session's
/// stacks, or the members of an adopted group.
fn campaign_stacks(store: &mut Store, c: &campaign::Campaign) -> Result<Vec<i64>, Reply> {
    let mut out = Vec::new();
    for it in campaign::items(store, c.id).map_err(campaign_err)? {
        if let Some(s) = it.stack_id {
            out.push(s);
        } else if let (Some(subject), Some(day)) = (it.subject_id, it.session_day.as_deref()) {
            out.extend(session_stacks(store, subject, day)?);
        } else {
            for m in nils_registry::review::members(store, it.review_item_id)
                .map_err(|e| Reply::error(500, e.to_string()))?
            {
                out.push(m.stack_id);
            }
        }
    }
    Ok(out)
}

/// The stacks of one session: its studies' series' stacks.
fn session_stacks(store: &mut Store, subject: i64, day: &str) -> Result<Vec<i64>, Reply> {
    let d = store.dialect();
    let sql = format!(
        "SELECT DISTINCT k.id FROM {} k JOIN {} r ON r.id = k.series_id \
         JOIN {} cs ON cs.study_id = r.study_id JOIN {} sc ON sc.id = cs.session_id \
         WHERE sc.subject_id = {} AND {} = {} ORDER BY k.id",
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("session_cache_study"),
        store.qualified("session_cache"),
        d.param(1, Type::Int),
        d.text_of(
            nils_registry::schema::table("session_cache")
                .column("first")
                .expect("first"),
        ),
        d.param(2, Type::Text),
    );
    Ok(store
        .query(&sql, &[Param::Int(subject), Param::from(day)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?)
}

/// Record 45: what a pick question is answered among, for one session
/// item: each of the session's stacks with what the classifier says of it,
/// and the picks standing for the role on the occasion (a run's, with its
/// scores, and a person's), each naming its stacks. Pack words and ids,
/// never a value of a person.
fn candidates(
    store: &mut Store,
    campaign: i64,
    item: i64,
    subject: i64,
    day: &str,
    role: Option<&str>,
) -> Result<Value, Reply> {
    let stacks = session_stacks(store, subject, day)?;
    let d = store.dialect();
    let mut axes: BTreeMap<i64, serde_json::Map<String, Value>> = BTreeMap::new();
    let mut series: BTreeMap<i64, i64> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({list}) ORDER BY stack_id, axis, value",
            store.qualified("classification_axis")
        );
        for r in store.query(&sql, &[])? {
            let entry = axes.entry(r.int(0)?).or_default();
            let values = entry
                .entry(r.text(1)?.to_string())
                .or_insert_with(|| json!([]));
            if let (Some(v), Some(list)) = (r.opt_text(2)?, values.as_array_mut()) {
                list.push(json!(v));
            }
        }
        let sql = format!(
            "SELECT id, series_id FROM {} WHERE id IN ({list})",
            store.qualified("stack")
        );
        for r in store.query(&sql, &[])? {
            series.insert(r.int(0)?, r.int(1)?);
        }
    }
    let mut picks = Vec::new();
    let mut picked: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    if let Some(role) = role {
        let t = nils_registry::schema::table("pick");
        let sql = format!(
            "SELECT id, author_kind, actor, score, margin, borders, {}, model FROM {} \
             WHERE role = {} AND subject_id = {} AND {} = {} AND withdrawn_at IS NULL ORDER BY id",
            d.text_of(t.column("considered").expect("considered")),
            store.qualified("pick"),
            d.param(1, Type::Text),
            d.param(2, Type::Int),
            d.text_of(t.column("session_day").expect("session_day")),
            d.param(3, Type::Text),
        );
        let rows = store.query(
            &sql,
            &[Param::from(role), Param::Int(subject), Param::from(day)],
        )?;
        for r in &rows {
            let id = r.int(0)?;
            let sql = format!(
                "SELECT stack_id FROM {} WHERE pick_id = {} ORDER BY stack_id",
                store.qualified("pick_stack"),
                d.param(1, Type::Int)
            );
            let its: Vec<i64> = store
                .query(&sql, &[Param::Int(id)])?
                .iter()
                .map(|x| x.int(0))
                .collect::<Result<_, _>>()?;
            for s in &its {
                picked.entry(*s).or_default().push(id);
            }
            picks.push(json!({
                "id": id, "author_kind": r.text(1)?, "actor": r.text(2)?,
                "score": r.opt_double(3)?, "margin": r.opt_double(4)?,
                "borders": r
                    .opt_text(5)?
                    .filter(|b| !b.is_empty())
                    .map(|b| b.split(',').map(str::to_string).collect::<Vec<_>>())
                    .unwrap_or_default(),
                "considered": r.opt_text(6)?.and_then(|t| serde_json::from_str::<Value>(t).ok()),
                "model": r.text(7)?, "stacks": its,
            }));
        }
    }
    let list: Vec<Value> = stacks
        .iter()
        .map(|s| {
            json!({
                "stack_id": s,
                "series_id": series.get(s),
                "axes": axes.remove(s).map(Value::Object).unwrap_or_else(|| json!({})),
                "picked_by": picked.get(s).cloned().unwrap_or_default(),
            })
        })
        .collect();
    Ok(json!({
        "campaign": campaign, "item": item, "subject_id": subject, "session_day": day,
        "role": role, "count": list.len(), "candidates": list, "picks": picks,
    }))
}

/// Record 45 E1 and R3: how many of a campaign's stacks have their picture,
/// and the job that builds the rest where the campaign pins a handle.
fn pictures_of(store: &mut Store, c: &campaign::Campaign) -> Result<Value, Reply> {
    let stacks = campaign_stacks(store, c)?;
    let mut v = crate::pyramid::pictures(store, &stacks);
    if v["missing"].as_u64().unwrap_or(0) > 0
        && let Some(h) = c.handle_id
    {
        v["build"] = json!(["pyramid", "build", "--handle", h.to_string()]);
    }
    Ok(v)
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
/// order makes the same for the same labels. The set is sealed when any of
/// its items is of a sealed sample (record 40 R3).
///
/// A set under a name taken is the name's next version, and never writes
/// over an earlier set's files: a door writes each version into `v<N>`
/// under the name's directory (`versioned`), and a directory that holds a
/// set already is refused. `labels.tsv` is written under a name of its own
/// and put in place only once the set is recorded, so the files a set's
/// row names are the ones its digest was taken of.
pub(crate) fn write_set(
    registry: &mut Registry,
    dir: &Path,
    versioned: bool,
    rows: &[Label],
    meta: &SetMeta<'_>,
) -> Result<LabelSet, (u16, String)> {
    let e500 = |e: String| (500, e);
    let text = labels::tsv(rows);
    let digest = sha256(text.as_bytes());
    let sealed = labels::sealed_among(registry.store(), rows).map_err(|e| e500(e.to_string()))?;
    let version =
        labels::next_version(registry.store(), meta.name).map_err(|e| e500(e.to_string()))?;
    let dir = if versioned {
        dir.join(format!("v{version}"))
    } else {
        dir.to_path_buf()
    };
    let dir = dir.as_path();
    for held in ["labels.tsv", "provenance.json"] {
        if dir.join(held).exists() {
            return Err((
                409,
                format!(
                    "{} holds a label set already, and a set never writes over another's files; write it into a directory of its own",
                    dir.display()
                ),
            ));
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| e500(format!("{}: {e}", dir.display())))?;
    let (handle_hash, scheme_digest, pack_version) = match meta.handle_id {
        Some(h) => {
            match nils_ask::handle::get(registry.store(), h).map_err(|e| e500(e.to_string()))? {
                Some(h) => (h.content_hash, h.scheme_digest, h.pack_version),
                None => (None, None, None),
            }
        }
        None => (None, None, None),
    };
    let place =
        nils_registry::place::holding(registry.store(), nils_registry::place::Role::Export, dir)
            .map_err(|e| e500(e.to_string()))?;
    let partial = dir.join(format!(
        ".labels.tsv.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&partial, &text).map_err(|e| e500(format!("{}: {e}", dir.display())))?;
    let recorded = labels::record(
        registry,
        &NewSet {
            name: meta.name,
            version,
            kind: meta.kind,
            what: meta.what,
            source: meta.source.clone(),
            campaign_id: meta.campaign_id,
            handle_id: meta.handle_id,
            pack_version: pack_version.as_deref(),
            scheme_digest: scheme_digest.as_deref(),
            sealed,
            rows: rows.len() as i64,
            digest: &digest,
            place_id: place.as_ref().map(|p| p.id),
            path: Some(&dir.display().to_string()),
            created_by: meta.created_by,
        },
    );
    let set = match recorded {
        Ok(set) => set,
        Err(e) => {
            std::fs::remove_file(&partial).ok();
            return Err(e500(e.to_string()));
        }
    };
    std::fs::rename(&partial, dir.join("labels.tsv"))
        .map_err(|e| e500(format!("{}: {e}", dir.display())))?;
    let provenance = json!({
        "label_set": set.id,
        "name": set.name,
        "version": set.version,
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
    .map_err(|e| e500(format!("{}: {e}", dir.display())))?;
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
        /// The order a rater's items come in: by position, or by value, the
        /// items where the systems or the rules disagree first, then the
        /// least confident (record 48)
        #[arg(long, default_value = "position", value_name = "position|value")]
        order: String,
        #[arg(long)]
        json: bool,
    },
    /// How fast the campaign is read: per rater, the answers, the median
    /// seconds from claim to answer, and the share of suggestions changed
    Stats {
        campaign: String,
        #[arg(long)]
        json: bool,
    },
    /// Answer a claimed item; an answer is never a decision
    Answer {
        /// The assignment a claim gave
        assignment: i64,
        /// The value: an axis value, a pick's stack ids (12,14), a text, or for an axes question the joint answer as JSON ({"base": "T1w", "modifier": ["FatSat"]}), where any axis may be "cant_tell" when the data give no clue
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
        /// Mark the stack unsure: answered, and a second look wanted
        #[arg(long)]
        unsure: bool,
    },
    /// Give a claimed item back unanswered: your own, or as the operator
    /// any rater's, which the audit records with its holder. A lease that
    /// ran out is ended as expired
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
    /// Move an open axes campaign to fewer asked axes, the rest derived
    /// from each answer through the pack's rules (record 48). Its items and
    /// every answer are kept; an answer given before is read on the axes
    /// asked now and derives the rest
    Requestion {
        /// The campaign, by name or id
        campaign: String,
        /// The axes the rater answers now, each asked before
        #[arg(long, value_name = "AXES", value_delimiter = ',', required = true)]
        axes: Vec<String>,
        /// The axes derived from the answer; the pack's own when not given
        #[arg(long, value_name = "AXES", value_delimiter = ',')]
        derive: Option<Vec<String>>,
        /// Where the pack is
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        /// The pack the campaign was made under
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
    /// other five kinds (axes, pick, form, derivative, free)
    #[arg(long, value_name = "JSON", conflicts_with_all = ["axis", "axes", "pick_role"])]
    question: Option<String>,
    /// An axis question: which value of this axis
    #[arg(long, value_name = "AXIS", conflicts_with = "axes")]
    axis: Option<String>,
    /// An axes question (record 45): these axes of each stack at once, as
    /// names joined by commas, held to the pack's legal combinations
    #[arg(long, value_name = "AXES", value_delimiter = ',')]
    axes: Option<Vec<String>>,
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
    /// The share of each batch accepted in one move that is held back to
    /// be read alone, from 0.1 to 1 (record 48)
    #[arg(long, value_name = "SHARE")]
    hold_back: Option<f64>,
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
    /// Seal a sample drawn for certification, before anyone looks: its
    /// stacks are kept as sealed, and a label set holding any of them is
    /// not training data (record 40 R3) until the certificate the sample
    /// was drawn for is recorded and the sample unsealed by it (record 48)
    Seal {
        /// The sample: a saved selection, frozen now into its stacks
        #[arg(long, value_name = "selection:NAME@V", conflicts_with = "handle")]
        select: Option<String>,
        /// The sample: the stacks of a handle
        #[arg(long, value_name = "ID")]
        handle: Option<i64>,
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        #[arg(long, default_value = "mri")]
        pack: String,
        #[arg(long)]
        json: bool,
    },
    /// Refused at the keyboard: a certificate is recorded at the engine's
    /// door, POST /api/certificates, by a person's token (record 48)
    Certificate {
        /// The sealed sample, as seal named it: selection:NAME@V or handle:ID
        #[arg(long, value_name = "SAMPLE")]
        sample: String,
        /// A model it certified, by id, digest or name@version
        #[arg(long = "model", value_name = "MODEL", required = true)]
        models: Vec<String>,
        /// The result, a JSON file
        #[arg(long, value_name = "FILE")]
        result: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Every certificate, newest first
    Certificates {
        #[arg(long)]
        json: bool,
    },
    /// Refused at the keyboard: a certified sample is unsealed at the
    /// engine's door, POST /api/certificates/{id}/unseal, by another
    /// person's token than the one who recorded the certificate (record 48)
    Unseal {
        /// The sample, or a label set's id
        which: String,
        #[arg(long, value_name = "ID")]
        certificate: i64,
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
    /// The axis; with --for-training and no axis, every axis with a decision
    #[arg(long, value_name = "AXIS", required_unless_present = "for_training")]
    axis: Option<String>,
    /// The development labels a training tool reads (record 48): the
    /// decisions in force by a person (unless --author says otherwise),
    /// leaving out every item of a sample sealed now
    #[arg(long)]
    for_training: bool,
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

/// Who answers or closes at the keyboard, read as the other verbs read it
/// (`review commit`, `review withdraw`): an agent or a model where a worker
/// says so in `NILS_ACTOR`, else a person. A model names the registered
/// model it is. Record 42 R6 then holds what they answer to staging, as it
/// does at the door (wave 43's proof: the keyboard recorded every answer
/// as a person's).
fn keyboard_author(registry: &mut Registry) -> Result<(&'static str, Option<i64>), Exit> {
    let kind = crate::actor_kind();
    if kind != "model" {
        return Ok((kind, None));
    }
    let actor = nils_registry::actor::current();
    let reference = match &actor["model"] {
        Value::String(s) if !s.trim().is_empty() => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => {
            return Err(usage(
                "a model acting names the registered model in NILS_ACTOR: {\"kind\": \"model\", \"model\": <id, sha256 digest or name@version>}",
            ));
        }
    };
    let m = nils_registry::model::resolve(registry.store(), &reference)?.ok_or_else(|| {
        usage(format!(
            "no registered model answers to {reference}; nils model list"
        ))
    })?;
    Ok(("model", Some(m.id)))
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

pub(crate) fn rerr(r: Reply) -> Exit {
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
        CampaignCommand::Stats {
            campaign: which,
            json,
        } => {
            let mut registry = crate::open(home)?;
            let c = campaign::find(registry.store(), &which).map_err(cerr)?;
            let doc = campaign::stats(registry.store(), c.id).map_err(cerr)?;
            if json {
                print(&doc);
                return Ok(());
            }
            let line = |who: &str, v: &Value| {
                let secs = |k: &str| {
                    v[k].as_f64()
                        .map(|s| format!("{s:.1} s"))
                        .unwrap_or_else(|| "-".into())
                };
                println!(
                    "  {:<24} {:>5} answer(s)  {:>5} read  {:>5} batched  median {}  p90 {}  changed {} of {}",
                    who,
                    v["answers"],
                    v["read"],
                    v["batched"],
                    secs("median_seconds"),
                    secs("p90_seconds"),
                    v["changed"],
                    v["suggested"]
                );
            };
            println!("campaign {} {}", c.id, c.name);
            for r in doc["raters"].as_array().into_iter().flatten() {
                line(r["principal"].as_str().unwrap_or_default(), r);
            }
            line("all", &doc["all"]);
            Ok(())
        }
        CampaignCommand::Claim {
            campaign: which,
            adjudicator,
            order,
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
            let order = campaign::Order::parse(&order).map_err(cerr)?;
            let claimed = campaign::claim_in(&mut registry, c.id, &principal, role, order, &now)
                .map_err(cerr)?;
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
            unsure,
        } => {
            let mut registry = crate::open(home)?;
            let form: Option<Value> = form
                .map(|f| serde_json::from_str(&f).map_err(|e| usage(format!("--form: {e}"))))
                .transpose()?;
            let principal = who();
            let (author_kind, model) = keyboard_author(&mut registry)?;
            // record 48 R1: the suggestion kept beside the answer, under the
            // pack the campaign was made with where it is found here
            let sql = format!(
                "SELECT campaign_id FROM {} WHERE id = {}",
                registry.store().qualified("campaign_assignment"),
                registry.store().dialect().param(1, Type::Int)
            );
            let campaign_of: Option<i64> = registry
                .store()
                .query_opt(&sql, &[Param::Int(assignment)])
                .map_err(|e| fail(e.to_string()))?
                .and_then(|r| r.int(0).ok());
            let suggested = match campaign_of {
                Some(cid) => {
                    let c = campaign::find(registry.store(), &cid.to_string()).map_err(cerr)?;
                    let name = c
                        .pack_version
                        .as_deref()
                        .and_then(|v| v.split('@').next())
                        .unwrap_or("mri")
                        .to_string();
                    let pack = crate::pack_dir(home, None)
                        .ok()
                        .and_then(|d| crate::reader::served_pack(Some(&d), &name));
                    let suggested =
                        suggested_for(registry.store(), &c, assignment, pack.as_deref())
                            .map_err(rerr)?;
                    // record 48: what the answer derives, under the pack the
                    // campaign was made under
                    let pack = pack.filter(|p| {
                        c.pack_version.as_deref()
                            == Some(format!("{}@{}", p.name, p.version).as_str())
                    });
                    let obj = value
                        .as_deref()
                        .and_then(|v| serde_json::from_str::<Value>(v).ok())
                        .filter(Value::is_object);
                    let derived = match (
                        obj,
                        assignment_stack(registry.store(), c.id, assignment).map_err(rerr)?,
                    ) {
                        (Some(obj), Some(stack)) => {
                            let q = c.question().map_err(cerr)?;
                            derived_for(registry.store(), pack.as_deref(), &q, stack, &obj)
                                .map_err(rerr)?
                        }
                        _ => None,
                    };
                    (suggested, derived)
                }
                None => (None, None),
            };
            let (suggested, derived) = suggested;
            let done = campaign::answer_with(
                &mut registry,
                &Given {
                    assignment,
                    principal: &principal,
                    author_kind,
                    model,
                    value: value.as_deref(),
                    form: form.as_ref(),
                    derivative_id: derivative,
                    why: why.as_deref(),
                    unsure,
                },
                &campaign::Timing {
                    suggested: suggested.as_deref(),
                    batch: false,
                    derived: derived.as_ref(),
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
            // the operator at the keyboard holds the registry itself, so
            // gives back any claim; the audit names the holder
            let a = campaign::release_as(&mut registry, assignment, &who(), true, &now)
                .map_err(cerr)?;
            if a.state == "expired" {
                println!("assignment {assignment}'s lease had run out; it is ended as expired");
            } else {
                println!("gave back assignment {assignment}");
            }
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
            let (author_kind, model) = keyboard_author(&mut registry)?;
            let closed = campaign::close(
                &mut registry,
                &Close {
                    campaign: c.id,
                    who: &principal,
                    author_kind,
                    model,
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
            for (item, why) in &closed.skipped {
                println!("  item {item} skipped: {why}");
            }
            println!("  agreement {}", closed.agreement);
            Ok(())
        }
        CampaignCommand::Requestion {
            campaign: which,
            axes,
            derive,
            pack_dir,
            pack,
            json,
        } => {
            let mut registry = crate::open(home)?;
            let served = crate::pack_dir(home, pack_dir)
                .ok()
                .and_then(|d| nils_pack::load(&d.join(&pack), None).ok())
                .ok_or_else(|| {
                    usage(format!(
                        "the {pack} pack is not found; name it with --pack-dir"
                    ))
                })?;
            let c = campaign::find(registry.store(), &which).map_err(cerr)?;
            if let Some(v) = c.pack_version.as_deref()
                && v != format!("{}@{}", served.name, served.version)
            {
                return Err(fail(format!(
                    "campaign {} was made under {v}, and the pack found is {}@{}; requestion it under the pack it was made under",
                    c.name, served.name, served.version
                )));
            }
            // the vocabulary the campaign's maker chose stays, on the axes
            // still asked
            let mut values = serde_json::Map::new();
            for a in &axes {
                if let Some(v) = c.question["values"].get(a) {
                    values.insert(a.clone(), v.clone());
                }
            }
            let mut question = json!({"kind": "axes", "axes": axes, "values": values});
            if let Some(d) = &derive {
                question["derive"] = json!(d);
            }
            complete_axes(&mut question, Some(&served)).map_err(|(_, m)| usage(m))?;
            let done = campaign::requestion(&mut registry, &which, &question, &who(), &now)
                .map_err(cerr)?;
            let c = campaign::find(registry.store(), &which).map_err(cerr)?;
            let filled = fill_derived(&mut registry, Some(&served), &c).map_err(rerr)?;
            let out = json!({
                "campaign": done.campaign, "name": c.name,
                "asked_before": done.asked_before, "asked": done.asked,
                "derive": done.derive, "answers_kept": done.answers,
                "answers_derived": filled,
            });
            if json {
                print(&out);
            } else {
                println!(
                    "campaign {} asks {} and derives {}; {} answer(s) kept, {} derived now",
                    c.name,
                    done.asked.join(", "),
                    if done.derive.is_empty() {
                        "nothing".to_string()
                    } else {
                        done.derive.join(", ")
                    },
                    done.answers,
                    filled
                );
            }
            Ok(())
        }
        CampaignCommand::Export {
            campaign: which,
            to,
            answers,
            name,
            json,
        } => {
            let mut registry = crate::open(home)?;
            require_export(&mut registry, &to)?;
            let c = campaign::find(registry.store(), &which).map_err(cerr)?;
            // record 48: answers without what they derive get it, under the
            // pack the campaign was made under only
            let pack_name = c
                .pack_version
                .as_deref()
                .and_then(|v| v.split('@').next())
                .unwrap_or("mri")
                .to_string();
            let served = crate::pack_dir(home, None)
                .ok()
                .and_then(|d| nils_pack::load(&d.join(&pack_name), None).ok())
                .filter(|p| {
                    c.pack_version.as_deref() == Some(format!("{}@{}", p.name, p.version).as_str())
                });
            fill_derived(&mut registry, served.as_ref(), &c).map_err(rerr)?;
            let of = if answers { Of::Answers } else { Of::Outcomes };
            let rows = labels::campaign_labels(registry.store(), c.id, of).map_err(lerr)?;
            let name = name.unwrap_or_else(|| c.name.clone());
            let what = c.question().map_err(cerr)?.what();
            let principal = who();
            let set = write_set(
                &mut registry,
                &to,
                false,
                &rows,
                &SetMeta {
                    name: &name,
                    kind: of.name(),
                    what: &what,
                    source: json!({"campaign": c.id, "name": c.name, "of": of.name(), "from": c.source}),
                    campaign_id: Some(c.id),
                    handle_id: c.handle_id,
                    created_by: &principal,
                },
            )
            .map_err(|(_, e)| fail(e))?;
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
        (None, None, None) if a.axes.is_some() => {
            json!({"kind": "axes", "axes": a.axes})
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
                "the question: --axis AXIS, --axes A,B, --pick-role ROLE or --question JSON",
            ));
        }
    };
    let mut question = question;
    let served = crate::pack_dir(home, a.pack_dir.clone())
        .ok()
        .and_then(|d| nils_pack::load(&d.join(&a.pack), None).ok());
    complete_axes(&mut question, served.as_ref()).map_err(|(_, m)| usage(m))?;
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
            hold_back: a.hold_back,
        },
    )
    .map_err(cerr)?;
    let pictures = pictures_of(registry.store(), &made).map_err(rerr)?;
    if a.json {
        let mut v = shown(registry.store(), &made).map_err(rerr)?;
        v["pictures"] = pictures;
        print(&v);
    } else {
        let n = campaign::items(registry.store(), made.id)
            .map_err(cerr)?
            .len();
        println!(
            "campaign {} {}   {} item(s)   {} question   closes into {}   pictures {} of {}",
            made.id,
            made.name,
            n,
            q.kind(),
            made.closes_into,
            pictures["have"],
            pictures["stacks"]
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
            let (rows, left_out) = if a.for_training {
                if a.staged {
                    return Err(usage(
                        "--for-training reads the decisions in force; a staged one never trains",
                    ));
                }
                labels::training_labels(
                    registry.store(),
                    a.axis.as_deref(),
                    stacks.as_deref(),
                    &a.authors,
                    campaign_id,
                )
                .map_err(lerr)?
            } else {
                let axis = a.axis.as_deref().unwrap_or_default();
                let rows = labels::decision_labels(
                    registry.store(),
                    &DecisionQuery {
                        axis,
                        stacks: stacks.as_deref(),
                        authors: &a.authors,
                        campaign: campaign_id,
                        staged_too: a.staged,
                    },
                )
                .map_err(lerr)?;
                (rows, 0)
            };
            let what = match (&a.axis, a.for_training) {
                (Some(axis), _) => axis.clone(),
                (None, _) => "training".to_string(),
            };
            let name = a.name.clone().unwrap_or_else(|| {
                if a.for_training {
                    format!("{what}-training")
                } else {
                    what.clone()
                }
            });
            let principal = who();
            let set = write_set(
                &mut registry,
                &a.to,
                false,
                &rows,
                &SetMeta {
                    name: &name,
                    kind: "decisions",
                    what: &what,
                    source: json!({
                        "axis": a.axis, "authors": a.authors, "campaign": campaign_id,
                        "selection": a.select, "handle": handle, "staged": a.staged,
                        "for_training": a.for_training, "left_out_sealed": left_out,
                    }),
                    campaign_id,
                    handle_id: handle,
                    created_by: &principal,
                },
            )
            .map_err(|(_, e)| fail(e))?;
            report_set(&set, a.json);
            if a.for_training && !a.json {
                println!("   left out {left_out} label(s) of a sample sealed now");
            }
            Ok(())
        }
        // record 48 R2: two people are told apart by the identity the
        // engine verified at its door, never by a local user name
        LabelsCommand::Certificate { .. } => Err(usage(
            "a certificate is recorded at the engine's door by a person's token, POST /api/certificates, so the two people of a certificate are told apart by a verified identity",
        )),
        LabelsCommand::Certificates { json } => {
            let mut registry = crate::open(home)?;
            let list = labels::certificates(registry.store()).map_err(lerr)?;
            if json {
                print(&json!(
                    list.iter()
                        .map(labels::Certificate::as_json)
                        .collect::<Vec<_>>()
                ));
                return Ok(());
            }
            if list.is_empty() {
                println!("no certificates");
            }
            for c in &list {
                let (sealed, unsealed) =
                    labels::sample_counts(registry.store(), &c.sample).map_err(lerr)?;
                println!(
                    "  {:>4}  {:<28} model(s) {:<10} {} sealed, {} unsealed  {}",
                    c.id,
                    c.sample,
                    c.model_ids
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                    sealed,
                    unsealed,
                    c.created_at
                );
            }
            Ok(())
        }
        LabelsCommand::Unseal { .. } => Err(usage(
            "a certified sample is unsealed at the engine's door by another person's token, POST /api/certificates/{id}/unseal, never at the keyboard",
        )),
        LabelsCommand::Seal {
            select,
            handle,
            pack_dir,
            pack,
            json,
        } => {
            let (handle, sample) = match (&select, handle) {
                (Some(spec), _) => {
                    let h = crate::ask_cli::freeze_selection(
                        home,
                        spec,
                        Grain::Stack,
                        pack_dir,
                        &pack,
                    )?;
                    let (name, version) = selection_spec(spec);
                    let mut registry = crate::open(home)?;
                    let version = match version {
                        Some(v) => v,
                        None => nils_ask::selection::get(registry.store(), name, None)
                            .map_err(|e| fail(e.to_string()))?
                            .map(|v| v.version)
                            .ok_or_else(|| usage(format!("no selection {spec}")))?,
                    };
                    (h, format!("selection:{name}@{version}"))
                }
                (None, Some(h)) => (h, format!("handle:{h}")),
                (None, None) => {
                    return Err(usage(
                        "the sample: --select selection:NAME@V or --handle ID",
                    ));
                }
            };
            let mut registry = crate::open(home)?;
            let stacks: Vec<i64> = handle_keys(registry.store(), handle, Grain::Stack)
                .map_err(rerr)?
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            let done = labels::seal(&mut registry, &sample, Some(handle), &stacks, &who())
                .map_err(lerr)?;
            if json {
                print(&done.as_json());
            } else {
                println!(
                    "sealed {}: {} stack(s), {} sealed already; a label set holding any of them is never training data",
                    done.sample, done.stacks, done.already
                );
            }
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
                    false,
                    &rows,
                    &SetMeta {
                        name: &format!("v0-{axis}"),
                        kind: "imported",
                        what: &axis,
                        source: json!({"imported": "v0", "file_sha256": sha256(text.as_bytes()), "counts": done.as_json()}),
                        campaign_id: None,
                        handle_id: None,
                        created_by: &principal,
                    },
                )
                .map_err(|(_, e)| fail(e))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Record 42 at detail plain: a session's item loses its subject, its
    /// day and the key that spells them; an answer its why, form and actor
    /// detail, and a free answer its text.
    #[test]
    fn plain_detail_leaves_out_the_session_and_the_free_text() {
        let mut item = json!({
            "id": 3, "position": 0, "subject_id": 7, "session_day": "2024-05-06",
            "key": "session:7:2024-05-06", "outcome": {"value": "12,14", "form": {"a": 1}},
        });
        plain_item(&mut item, "session", false);
        assert_eq!(
            item,
            json!({"id": 3, "position": 0, "outcome": {"value": "12,14"}})
        );
        let mut stack =
            json!({"id": 4, "stack_id": 9, "key": "stack:9", "outcome": {"value": "a note"}});
        plain_item(&mut stack, "stack", true);
        assert_eq!(
            stack,
            json!({"id": 4, "stack_id": 9, "key": "stack:9", "outcome": {}})
        );
        let mut answer = json!({
            "id": 1, "value": "a note", "why": "because", "form": {"a": 1},
            "actor_detail": {"kind": "agent"}, "principal": "anna@lab",
        });
        plain_answer(&mut answer, true);
        assert_eq!(answer, json!({"id": 1, "principal": "anna@lab"}));
    }
}
