// SPDX-License-Identifier: AGPL-3.0-only

//! The Main scans doors (record 55, decision 6; the main-scans study,
//! section 5): a dataset's or a cohort's main-scan rules, read, as text,
//! saved as a new version that queues the scope's pick run; and the map of
//! what the rules, saved or drafted, take for a role, read from the stored
//! axes and written nowhere. A scope is `?cohort=NAME` or `?dataset=NAME`
//! (or ID), or the same under `scope` or at the top of a body.

use std::collections::{BTreeSet, HashMap};

use nils_classify::main_scans::{self, Ask, Columns, Compare, Group};
use nils_pack::pick::Model;
use nils_pack::pick_rules::Rules;
use nils_registry::Registry;
use nils_registry::pick_rules::{self, SaveError, Scope};
use serde_json::{Value, json};

use crate::serve::{Doors, Reply};

/// The longest reason a version of the rules is saved with.
const REASON_MOST: usize = 500;

/// The largest body the rules and map doors read.
pub(crate) const BODY_MOST: usize = 256 * 1024;

/// A body over [`BODY_MOST`] is refused before it is read.
pub(crate) fn sized(body: &str) -> Result<(), Reply> {
    if body.len() > BODY_MOST {
        return Err(Reply::error(
            413,
            format!("a body of at most {BODY_MOST} bytes"),
        ));
    }
    Ok(())
}

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

/// The scope a door names: in the body's `scope`, at the top of the body,
/// or in the query.
pub(crate) fn scope_of(
    registry: &mut Registry,
    query: &HashMap<String, String>,
    body: Option<&Value>,
) -> Result<Scope, Reply> {
    named_scope(registry, query, body)?
        .ok_or_else(|| Reply::error(400, "cohort names the cohort, or dataset the dataset"))
}

/// [`scope_of`], none where the door names no scope.
pub(crate) fn named_scope(
    registry: &mut Registry,
    query: &HashMap<String, String>,
    body: Option<&Value>,
) -> Result<Option<Scope>, Reply> {
    let text = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };
    let mut cohort = None;
    let mut dataset = None;
    if let Some(b) = body {
        for at in [&b["scope"], b] {
            cohort = cohort.or_else(|| text(&at["cohort"]));
            dataset = dataset.or_else(|| text(&at["dataset"]));
        }
    }
    cohort = cohort.or_else(|| query.get("cohort").filter(|c| !c.is_empty()).cloned());
    dataset = dataset.or_else(|| query.get("dataset").filter(|d| !d.is_empty()).cloned());
    let store = registry.store();
    match (cohort, dataset) {
        (Some(_), Some(_)) => Err(Reply::error(
            400,
            "a scope is a cohort or a dataset, not both",
        )),
        (Some(c), None) => Scope::cohort(store, &c)
            .map_err(failed)?
            .map(Some)
            .ok_or_else(|| Reply::error(404, format!("no cohort named {c}"))),
        (None, Some(d)) => Scope::dataset(store, &d)
            .map_err(failed)?
            .map(Some)
            .ok_or_else(|| Reply::error(404, format!("no dataset named {d}"))),
        (None, None) => Ok(None),
    }
}

/// The scope a person's pick made for a dataset or a cohort belongs to: its
/// own once it has rules saved; else the pack's, marked as made for it, so
/// that its first save carries the pick in.
pub(crate) fn person_scope(
    registry: &mut Registry,
    named: Option<&Scope>,
) -> Result<(Option<Scope>, Option<String>), Reply> {
    let Some(scope) = named else {
        return Ok((None, None));
    };
    if pick_rules::latest(registry.store(), scope)
        .map_err(failed)?
        .is_some()
    {
        Ok((Some(scope.clone()), None))
    } else {
        Ok((None, Some(pick_rules::made_in(scope))))
    }
}

/// The occasion a person's pick names, when it names one: `session_id`
/// (the map's), or `subject_id` with `session` (its day). It must be one of
/// the subject's own under the scheme, and the subject one of the named
/// scope's.
pub(crate) fn occasion_of(
    registry: &mut Registry,
    scheme: &nils_registry::session::Scheme,
    named: Option<&Scope>,
    doc: &Value,
) -> Result<Option<(i64, String)>, Reply> {
    let session_id = doc["session_id"].as_i64();
    let day = doc["session"].as_str().or(doc["session_day"].as_str());
    let subject = doc["subject_id"].as_i64();
    if session_id.is_none() && (subject.is_none() || day.is_none()) {
        return Ok(None);
    }
    let labels = nils_session::labels_by_study(registry.store(), scheme).map_err(failed)?;
    let found = labels.values().find(|l| match session_id {
        Some(id) => l.session_id == id && subject.is_none_or(|s| s == l.subject_id),
        None => {
            Some(l.subject_id) == subject
                && day.is_some_and(|d| d.get(..10).unwrap_or(d) == l.first.to_string())
        }
    });
    let Some(l) = found else {
        return Err(Reply::error(
            404,
            "no such visit of the subject under the scheme",
        ));
    };
    if let Some(scope) = named {
        let subjects = crate::pick_scopes::subjects_of(registry.store(), scope).map_err(failed)?;
        if !subjects.contains(&l.subject_id) {
            return Err(Reply::error(
                409,
                format!("the subject is not one of {}'s", scope.label()),
            ));
        }
    }
    Ok(Some((l.subject_id, l.first.to_string())))
}

/// The served pack a door names (`pack`, else the served one), with picks.
fn pack_of(
    doors: &Doors,
    query: &HashMap<String, String>,
    body: Option<&Value>,
) -> Result<std::sync::Arc<nils_pack::Pack>, Reply> {
    let name = body
        .and_then(|b| b["pack"].as_str())
        .or_else(|| query.get("pack").map(String::as_str))
        .filter(|n| !n.is_empty())
        .unwrap_or(&doors.ask_pack)
        .to_string();
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &name)
        .ok_or_else(|| Reply::error(409, format!("no pack named {name} is served")))?;
    if pack.picks.is_empty() {
        return Err(Reply::error(
            409,
            format!(
                "{} declares no picks, so it has no main-scan rules",
                pack.id()
            ),
        ));
    }
    Ok(pack)
}

/// The pick that declares a role, or the first for the rules as a whole.
fn model_of<'a>(pack: &'a nils_pack::Pack, role: Option<&str>) -> Result<&'a Model, Reply> {
    match role {
        None => Ok(&pack.picks[0]),
        Some(r) => pack
            .picks
            .iter()
            .find(|m| m.roles.iter().any(|x| x == r))
            .ok_or_else(|| {
                Reply::error(400, format!("{r} is not a role of {}'s picks", pack.id()))
            }),
    }
}

/// A scope's current rules: its latest saved version read against the
/// pick's defaults, or the defaults themselves (version 1).
fn current(
    registry: &mut Registry,
    model: &Model,
    scope: &Scope,
) -> Result<(Rules, Option<pick_rules::Saved>), Reply> {
    let defaults = Rules::defaults(model);
    match pick_rules::latest(registry.store(), scope).map_err(failed)? {
        None => Ok((defaults, None)),
        Some(saved) => Rules::from_json(&saved.rules, &defaults)
            .map(|r| (r, Some(saved)))
            .map_err(|e| Reply::error(500, format!("the saved rules do not read: {e}"))),
    }
}

/// A refusal of a rules document: 400, naming the field.
fn refused(e: nils_pack::pick_rules::Refusal) -> Reply {
    let mut r = Reply::error(400, e.to_string());
    r.body["field"] = Value::from(e.field);
    r
}

/// `GET /api/picks/rules`: the scope, the pack, the current rules and the
/// versions, the newest first, version 1 (the pack's defaults) last.
pub(crate) fn rules(
    doors: &Doors,
    registry: &mut Registry,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let scope = scope_of(registry, query, None)?;
    let pack = pack_of(doors, query, None)?;
    let model = model_of(&pack, None)?;
    let (rules, saved) = current(registry, model, &scope)?;
    let doc = rules.to_json();
    let current = match &saved {
        Some(s) => json!({
            "version": s.version, "saved": true, "reason": s.reason, "author": s.author,
            "at": s.created_at, "digest": s.digest, "rules": doc,
        }),
        None => json!({
            "version": 1, "saved": false, "reason": "the pack's defaults", "author": null,
            "at": null, "digest": pick_rules::digest(&doc), "rules": doc,
        }),
    };
    let mut versions: Vec<Value> = pick_rules::versions(registry.store(), &scope)
        .map_err(failed)?
        .iter()
        .map(|s| json!({"version": s.version, "reason": s.reason, "author": s.author, "at": s.created_at}))
        .collect();
    versions
        .push(json!({"version": 1, "reason": "the pack's defaults", "author": null, "at": null}));
    Ok(Reply::ok(json!({
        "scope": scope.to_json(),
        "pack": {"name": pack.name, "version": pack.version.to_string()},
        "current": current,
        "versions": versions,
        // what the scope's picks are read from now, and a version whose
        // run has not finished yet
        "picks": pick_rules::state(registry.store(), &scope).map_err(failed)?,
    })))
}

/// `GET /api/picks/rules/text`: a version's rules as YAML, the comments
/// beside the fields; the current one without `version`.
pub(crate) fn text(
    doors: &Doors,
    registry: &mut Registry,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let scope = scope_of(registry, query, None)?;
    let pack = pack_of(doors, query, None)?;
    let model = model_of(&pack, None)?;
    let defaults = Rules::defaults(model);
    let rules = match query.get("version").filter(|v| !v.is_empty()) {
        None => current(registry, model, &scope)?.0,
        Some(v) => {
            let n: i64 = v
                .parse()
                .map_err(|_| Reply::error(400, "version: a number"))?;
            if n == 1 {
                defaults
            } else {
                let saved = pick_rules::version(registry.store(), &scope, n)
                    .map_err(failed)?
                    .ok_or_else(|| {
                        Reply::error(404, format!("{} has no version {n}", scope.label()))
                    })?;
                Rules::from_json(&saved.rules, &defaults)
                    .map_err(|e| Reply::error(500, format!("the saved rules do not read: {e}")))?
            }
        }
    };
    Ok(Reply::raw(
        "text/yaml; charset=utf-8",
        rules.to_yaml().into_bytes(),
        Vec::new(),
    ))
}

/// `POST /api/picks/rules`: a new version, `based_on + 1`. Answers the
/// version and the scope, which the door queues a run for.
pub(crate) fn save(
    doors: &Doors,
    registry: &mut Registry,
    query: &HashMap<String, String>,
    doc: &Value,
    author: &str,
) -> Result<(i64, Scope, String), Reply> {
    let scope = scope_of(registry, query, Some(doc))?;
    let pack = pack_of(doors, query, Some(doc))?;
    let model = model_of(&pack, None)?;
    let reason = doc["reason"].as_str().unwrap_or("").trim();
    if reason.chars().count() > REASON_MOST {
        let mut r = Reply::error(400, format!("reason: at most {REASON_MOST} characters"));
        r.body["field"] = Value::from("reason");
        return Err(r);
    }
    if reason.is_empty() {
        let mut r = Reply::error(422, "a version of the rules says why it was made");
        r.body["field"] = Value::from("reason");
        return Err(r);
    }
    let rules = Rules::from_json(&doc["rules"], &Rules::defaults(model)).map_err(refused)?;
    let based_on = doc["based_on"].as_i64().ok_or_else(|| {
        let mut r = Reply::error(400, "based_on: the version the rules were changed from");
        r.body["field"] = Value::from("based_on");
        r
    })?;
    match pick_rules::save(
        registry.store(),
        &scope,
        &rules.to_json(),
        reason,
        author,
        based_on,
    ) {
        Ok(version) => Ok((version, scope, pack.name.clone())),
        Err(SaveError::Behind { latest }) => {
            let mut r = Reply::error(409, SaveError::Behind { latest }.to_string());
            r.body["latest"] = Value::from(latest);
            Err(r)
        }
        Err(SaveError::NoReason) => Err(Reply::error(422, SaveError::NoReason.to_string())),
        Err(SaveError::Store(e)) => Err(failed(e)),
    }
}

/// Which map door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Door {
    Map,
    Subjects,
    Strips,
}

/// The dataset each of the subjects' stacks came through, by the source
/// places in force.
fn datasets(
    registry: &mut Registry,
    subjects: &BTreeSet<i64>,
) -> Result<HashMap<i64, String>, Reply> {
    let store = registry.store();
    let mut by_source: HashMap<i64, String> = HashMap::new();
    for p in nils_registry::place::active(store).map_err(failed)? {
        if p.role != nils_registry::place::Role::Source {
            continue;
        }
        for id in crate::sources::source_ids(store, &p).map_err(failed)? {
            by_source.entry(id).or_insert_with(|| p.name.clone());
        }
    }
    let ids: Vec<i64> = subjects.iter().copied().collect();
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT ss.stack_id, ss.source_id FROM {} ss JOIN {} f ON f.stack_id = ss.stack_id \
             WHERE f.subject_id IN ({})",
            store.qualified("source_stack"),
            store.qualified("stack_fingerprint"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &[]).map_err(failed)? {
            if let Some(name) = by_source.get(&r.int(1).map_err(failed)?) {
                out.entry(r.int(0).map_err(failed)?)
                    .or_insert_with(|| name.clone());
            }
        }
    }
    Ok(out)
}

/// `POST /api/picks/map`, `/map/subjects` and `/map/strips`.
pub(crate) fn map(
    doors: &Doors,
    registry: &mut Registry,
    caller: &crate::serve::Caller,
    doc: &Value,
    door: Door,
) -> Result<Reply, Reply> {
    let none = HashMap::new();
    let scope = scope_of(registry, &none, Some(doc))?;
    let pack = pack_of(doors, &none, Some(doc))?;
    let role = doc["role"]
        .as_str()
        .filter(|r| !r.is_empty())
        .ok_or_else(|| Reply::error(400, "role names the role the map is of"))?;
    let model = model_of(&pack, Some(role))?;
    let by = match doc["columns"].as_str() {
        None => Columns::Scanner,
        Some(w) => {
            Columns::parse(w).ok_or_else(|| Reply::error(400, "columns: scanner or dataset"))?
        }
    };
    let scheme = match doc["scheme"].as_str() {
        None | Some("default" | "day") => nils_registry::session::Scheme::default(),
        Some(n) => crate::stored_scheme(registry, n).map_err(Reply::from)?,
    };
    let (saved, stored) = current(registry, model, &scope)?;
    let version = stored.as_ref().map_or(1, |s| s.version);
    let draft = match &doc["rules"] {
        Value::Null => None,
        d => Some(Rules::from_json(d, &Rules::defaults(model)).map_err(refused)?),
    };
    let subjects = crate::pick_scopes::subjects_of(registry.store(), &scope).map_err(failed)?;
    let reading = pick_rules::reading(registry.store(), &scope).map_err(failed)?;
    // and what the rules door says of them
    let datasets = match by {
        Columns::Dataset => datasets(registry, &subjects)?,
        Columns::Scanner => HashMap::new(),
    };
    // a stack of a sample sealed now is no part of the map for a caller
    // who may not read it (record 48)
    let withheld: BTreeSet<i64> = if crate::sealed::reads(&caller.access) {
        BTreeSet::new()
    } else {
        let store = registry.store();
        let sql = format!(
            "SELECT stack_id FROM {} WHERE unsealed_at IS NULL",
            store.qualified("sealed_stack")
        );
        store
            .query(&sql, &[])
            .map_err(failed)?
            .iter()
            .filter_map(|r| r.int(0).ok())
            .collect()
    };
    let read = main_scans::read(
        registry,
        &Ask {
            model,
            role,
            scheme: &scheme,
            subjects: &subjects,
            reading: reading.as_ref(),
            datasets: &datasets,
            withheld: &withheld,
        },
    )
    .map_err(failed)?;
    // the scan within a kind is scored against the registry's population,
    // which a map needs where it names scans
    let reference = match (door, draft.is_some()) {
        (Door::Subjects, _) | (Door::Map, true) => Some(
            main_scans::reference(registry.store(), model, role, &doors.pick_references)
                .map_err(failed)?,
        ),
        _ => None,
    };
    let cmp = Compare::new(&read, model, role, saved, draft, reference.as_ref());
    let out = match door {
        Door::Map => main_scans::map(&read, &cmp, model, role, by, version),
        Door::Strips => main_scans::strips(&read, &cmp, model, role, by),
        Door::Subjects => {
            let group = Group::parse(&doc["group"]).map_err(|m| Reply::error(400, m))?;
            let by_changes = match doc["order"].as_str() {
                None | Some("changes") => true,
                Some("visits") => false,
                Some(_) => return Err(Reply::error(400, "order: changes or visits")),
            };
            let page = doc["page"].as_u64().unwrap_or(0) as usize;
            let per_page = doc["per_page"].as_u64().unwrap_or(24).clamp(1, 48) as usize;
            let mut page =
                main_scans::subjects(&read, &cmp, by, &group, by_changes, page, per_page);
            // below detail quasi a subject's code and a person's words are
            // shown by their shape, as the viewer shows them (record 55 K7)
            if caller.access.detail < crate::grants::Detail::Quasi {
                for s in page["subjects"].as_array_mut().into_iter().flatten() {
                    if let Some(code) = s["subject"].as_str() {
                        s["subject"] = Value::from(crate::scans::shape(code));
                    }
                    for v in s["visits"].as_array_mut().into_iter().flatten() {
                        if let Some(why) = v["why"].as_str() {
                            v["why"] = Value::from(crate::scans::shape(why));
                        }
                    }
                }
            }
            page
        }
    };
    Ok(Reply::ok(out))
}
