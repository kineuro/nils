// SPDX-License-Identifier: AGPL-3.0-only

//! What models proposed, per model and axis (UX round 1, 2026-11-05).
//!
//! A model's run stages its answers at or above its threshold and leaves
//! the rest open (`nils_registry::proposals`). Review's Proposals page acts
//! on a model's proposals on one axis as two moves: commit the ones at or
//! above the threshold (the commit by filter, `POST /api/decisions/commit`
//! with the model and the axis, which re-sorts the scans it decides), and
//! make the rest a campaign for oneself.
//!
//! - **What waits** (`GET /api/review/proposals`, `nils review proposals`):
//!   per model and axis, the staged part a commit takes, what an open
//!   campaign holds, what was staged before the registry moved on, the
//!   scans below the threshold a campaign would ask and those a campaign
//!   asks already. Counts only, never a stack.
//! - **A campaign of the rest** (`POST /api/review/proposals/campaign`,
//!   `nils review proposals-campaign`): the model's open items on the axis
//!   no open campaign asks, a scan an item, asked of the axis alone; the
//!   caller its one rater, held back none (a campaign that trains or
//!   measures nothing, #569), the model's answer and probabilities
//!   suggested on each scan and authored by the model, closing into
//!   decisions in force. One transaction; the name, how many scans at most
//!   (`limit`, 500 by default) and which first (`order`: the least sure,
//!   the most sure, or a random draw by a `seed`) are the caller's (UX
//!   round 5, 2026-11-08: a campaign of 145,274 scans was one click away).
//!   The answer says how many scans were `left` out. The standing says the
//!   pace of the axis (`below.pace_s`) for the estimate.

use std::collections::BTreeMap;
use std::path::PathBuf;

use nils_registry::Registry;
use nils_registry::home::Home;
use nils_registry::proposals::{self, Order, RestCampaign, Standing};
use serde_json::{Value, json};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply, json_body};
use crate::{Exit, fail, usage};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 2] = [
    "GET /api/review/proposals",
    "POST /api/review/proposals/campaign",
];

/// Each door's policy row, as `serve::policy` reads them.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/review/proposals",
        false,
        false,
        "bounded",
        "one row per model and axis",
        "Reading what models proposed",
        "Read what models proposed",
    ),
    (
        "POST /api/review/proposals/campaign",
        true,
        false,
        "bounded",
        "one campaign of one model's proposals on one axis",
        "Making a campaign of a model's proposals",
        "Made a campaign of a model's proposals",
    ),
];

/// What each door needs, read by `serve::door` before its own table. The
/// campaign closes into decisions in force, so it is a reviewer's act as
/// well as a campaign's, as a close is.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    Some(match (method, segs) {
        ("GET", ["api", "review", "proposals"]) => (Need::One("review:see"), Detail::Plain),
        ("POST", ["api", "review", "proposals", "campaign"]) => {
            (Need::Both("campaigns:work", "review:work"), Detail::Plain)
        }
        _ => return None,
    })
}

/// The doors, or nothing when the path is not one of them. The grants were
/// checked by the table.
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    Some(match (method, segs) {
        ("GET", ["api", "review", "proposals"]) => standing(registry).map(Reply::ok),
        ("POST", ["api", "review", "proposals", "campaign"]) => make(doors, registry, caller, body),
        _ => return None,
    })
}

fn proposals_err(e: proposals::Error) -> Reply {
    match e {
        proposals::Error::Store(s) => Reply::error(500, s.to_string()),
        proposals::Error::Invalid(m) => Reply::error(400, m),
        proposals::Error::Refused(m) => Reply::error(409, m),
    }
}

fn standing(registry: &mut Registry) -> Result<Value, Reply> {
    let rows = proposals::standing(registry).map_err(proposals_err)?;
    Ok(json!({
        "count": rows.len(),
        "proposals": rows.iter().map(Standing::as_json).collect::<Vec<_>>(),
    }))
}

/// The question a campaign of the rest asks: the axis alone, as an axes
/// question held to the served pack's legal values, deriving nothing.
fn question_of(pack: Option<&nils_pack::Pack>, axis: &str) -> Result<Value, (u16, String)> {
    let mut q = json!({"kind": "axes", "axes": [axis], "derive": []});
    crate::campaigns::complete_axes(&mut q, pack)?;
    Ok(q)
}

/// Every name a value of the axis goes by, in lower case, to its identity,
/// as a suggestion is read (`campaigns::import_suggestions`).
fn names_of(
    question: &Value,
    pack: Option<&nils_pack::Pack>,
    axis: &str,
) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    for (k, v) in crate::campaigns::value_names(pack, Some(axis)) {
        names.insert(k.to_lowercase(), v);
    }
    if let Ok(q) = nils_registry::campaign::Question::parse(question) {
        for v in crate::reader::single_vocabulary(&q, axis, pack) {
            names.insert(v.to_lowercase(), v);
        }
    }
    names
}

/// How many scans a campaign of the rest takes when the caller does not say.
pub(crate) const DEFAULT_LIMIT: i64 = 500;

/// What the caller may say of a campaign of the rest: the model, the axis,
/// the name, the limit and the order. The rest is the act's own.
const NOT_THE_CALLERS: [&str; 7] = [
    "question",
    "raters",
    "hold_back",
    "suggest",
    "closes_into",
    "raters_per_item",
    "source",
];

fn make(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    for held in NOT_THE_CALLERS {
        if doc.get(held).is_some() {
            return Err(Reply::error(
                400,
                format!(
                    "{held} is not the caller's to say: a campaign of a model's proposals asks its axis alone, has the caller as its one rater, holds back none, suggests the model's answer and closes into decisions in force; it takes model, axis and name"
                ),
            ));
        }
    }
    let model = doc["model"]
        .as_str()
        .map(str::to_string)
        .or_else(|| doc["model"].as_i64().map(|i| i.to_string()))
        .ok_or_else(|| Reply::error(400, "model: the model's id, digest or name@version"))?;
    let axis = doc["axis"]
        .as_str()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| Reply::error(400, "axis: the axis the model proposed on"))?;
    let name = match &doc["name"] {
        Value::Null => None,
        Value::String(s) => Some(s.as_str()),
        _ => return Err(Reply::error(400, "name: what the campaign is called")),
    };
    let limit = match &doc["limit"] {
        Value::Null => Some(DEFAULT_LIMIT),
        Value::Number(n) if n.as_i64().is_some_and(|l| l >= 1) => n.as_i64(),
        _ => {
            return Err(Reply::error(
                400,
                format!(
                    "limit: how many scans at most, a whole number of one or more ({DEFAULT_LIMIT} when left out)"
                ),
            ));
        }
    };
    let order = match &doc["order"] {
        Value::Null => Order::LeastSure,
        Value::String(o) => Order::parse(o).ok_or_else(|| {
            Reply::error(
                400,
                format!("order: which scans first, {}", proposals::ORDERS),
            )
        })?,
        _ => {
            return Err(Reply::error(
                400,
                format!("order: which scans first, {}", proposals::ORDERS),
            ));
        }
    };
    let seed = match &doc["seed"] {
        Value::Null => None,
        Value::Number(n) if n.as_u64().is_some() => n.as_u64(),
        _ => {
            return Err(Reply::error(
                400,
                "seed: a whole number, for a random order",
            ));
        }
    };
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let question = question_of(pack.as_deref(), axis).map_err(|(s, m)| Reply::error(s, m))?;
    let names = names_of(&question, pack.as_deref(), axis);
    let pack_version = crate::campaigns::pack_version_of(doors);
    let made = proposals::campaign_of_rest(
        registry,
        &RestCampaign {
            model: &model,
            axis,
            name,
            owner: &caller.principal,
            question: &question,
            pack_version: pack_version.as_deref(),
            names: &names,
            limit,
            order,
            seed,
        },
    )
    .map_err(crate::campaigns::campaign_err)?;
    // record 58 P1 (spec §12.5): the maker's account name, for Results
    nils_registry::names::note(registry.store(), &caller.principal, caller.account_name())?;
    let mut v = made.as_json();
    v["counts"] = nils_registry::campaign::counts(registry.store(), made.campaign.id)
        .map_err(crate::campaigns::campaign_err)?;
    Ok(Reply::created(v))
}

// ------------------------------------------------------- the keyboard

/// `nils review proposals`: what models proposed that still waits.
pub(crate) fn list_verb(registry: &mut Registry, json_out: bool) -> Result<(), Exit> {
    let rows = proposals::standing(registry).map_err(|e| match e {
        proposals::Error::Store(s) => fail(s.to_string()),
        other => usage(other.to_string()),
    })?;
    if json_out {
        crate::campaigns::print(&json!({
            "count": rows.len(),
            "proposals": rows.iter().map(Standing::as_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if rows.is_empty() {
        println!("nothing a model proposed waits");
        return Ok(());
    }
    for s in &rows {
        let commit = s.commits();
        let at = s
            .thresholds
            .first()
            .map(|t| format!("{t}"))
            .unwrap_or_else(|| "none".into());
        println!(
            "{} on {}   threshold {at}   commit {} scan(s){}{}   below {} scan(s){}",
            s.model.label(),
            s.axis,
            commit.stacks,
            if s.held.stacks > 0 {
                format!(", {} held by an open campaign", s.held.stacks)
            } else {
                String::new()
            },
            if s.drifted > 0 {
                format!(", {} staged before the registry moved on", s.drifted)
            } else {
                String::new()
            },
            s.below.stacks,
            if s.asked.stacks > 0 {
                format!(
                    "; {} asked by {}",
                    s.asked.stacks,
                    s.campaigns
                        .iter()
                        .map(|(_, n)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                String::new()
            },
        );
    }
    Ok(())
}

/// `nils review proposals-campaign`: a campaign of what one model proposed
/// on one axis below its threshold, with the one at the keyboard its rater;
/// at most `limit` scans (0 for every one), in `order`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn campaign_verb(
    home: &Home,
    registry: &mut Registry,
    model: &str,
    axis: &str,
    name: Option<&str>,
    limit: i64,
    order: &str,
    seed: Option<u64>,
    pack_dir: Option<PathBuf>,
    pack_name: &str,
    json_out: bool,
) -> Result<(), Exit> {
    let order =
        Order::parse(order).ok_or_else(|| usage(format!("--order: {}", proposals::ORDERS)))?;
    if limit < 0 {
        return Err(usage("--limit: how many scans at most, or 0 for every one"));
    }
    let limit = (limit > 0).then_some(limit);
    let pack = crate::pack_dir(home, pack_dir)
        .ok()
        .and_then(|d| nils_pack::load(&d.join(pack_name), None).ok());
    let question = question_of(pack.as_ref(), axis).map_err(|(_, m)| usage(m))?;
    let names = names_of(&question, pack.as_ref(), axis);
    let pack_version = pack.as_ref().map(|p| format!("{}@{}", p.name, p.version));
    let principal = crate::campaigns::who();
    let made = proposals::campaign_of_rest(
        registry,
        &RestCampaign {
            model,
            axis,
            name,
            owner: &principal,
            question: &question,
            pack_version: pack_version.as_deref(),
            names: &names,
            limit,
            order,
            seed,
        },
    )
    .map_err(crate::campaigns::cerr)?;
    if json_out {
        let mut v = made.as_json();
        v["counts"] = nils_registry::campaign::counts(registry.store(), made.campaign.id)
            .map_err(crate::campaigns::cerr)?;
        crate::campaigns::print(&v);
        return Ok(());
    }
    let items = nils_registry::campaign::items(registry.store(), made.campaign.id)
        .map_err(crate::campaigns::cerr)?
        .len();
    println!(
        "campaign {} {}   {items} scan(s) from {} item(s), {} first, {} left out   rater {principal}   held back none   {} suggestion(s) by the model   closes into decision",
        made.campaign.id,
        made.campaign.name,
        made.adopted.len(),
        match made.order {
            Order::LeastSure => "the least sure".to_string(),
            Order::MostSure => "the most sure".to_string(),
            Order::Random => format!("a random draw (seed {})", made.seed.unwrap_or(0)),
        },
        made.left,
        made.suggestions.suggestions,
    );
    Ok(())
}
