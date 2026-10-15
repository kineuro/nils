// SPDX-License-Identifier: AGPL-3.0-only

//! The Main scans doors (record 55, decision 6; the main-scans study,
//! section 5): a dataset's or a cohort's main-scan rules, read, as text,
//! saved as a new version that queues the scope's pick run; and the map of
//! what the rules, saved or drafted, take for a role, read from the stored
//! axes and written nowhere. A scope is `?cohort=NAME` or `?dataset=NAME`
//! (or ID), or the same under `scope` or at the top of a body. A map door
//! asked for a `layer` (body region, body part or post-contrast) says what
//! each visit's scans are by that operation's answers, and whether its step
//! has run over the scope (record 55, decision 6: the data map's layers).

use std::collections::{BTreeSet, HashMap};

use nils_classify::main_scans::{self, Ask, Columns, Compare, Group, Layer};
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

/// The doors of this module, as the capabilities list them (2026-10-11:
/// they were routed and granted but not listed, so the desk hid the page).
pub(crate) const DOORS: [&str; 7] = [
    "GET /api/picks/layer-step",
    "GET /api/picks/rules",
    "GET /api/picks/rules/text",
    "POST /api/picks/rules",
    "POST /api/picks/map",
    "POST /api/picks/map/subjects",
    "POST /api/picks/map/strips",
];

/// Their policy rows: whether a door writes, takes an idempotency key, its
/// cost, its result cap and its label; the grant and the detail are read
/// from the door table.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/picks/layer-step",
        false,
        false,
        "bounded",
        "one document",
        "Reading a layer's step",
        "Read a layer's step",
    ),
    (
        "GET /api/picks/rules",
        false,
        false,
        "free",
        "one document",
        "Reading main-scan rules",
        "Read main-scan rules",
    ),
    (
        "GET /api/picks/rules/text",
        false,
        false,
        "free",
        "one document",
        "Reading main-scan rules as text",
        "Read main-scan rules as text",
    ),
    (
        "POST /api/picks/rules",
        true,
        false,
        "job",
        "one id",
        "Saving main-scan rules",
        "Saved main-scan rules",
    ),
    (
        "POST /api/picks/map",
        false,
        false,
        "bounded",
        "one document",
        "Mapping main scans",
        "Mapped main scans",
    ),
    (
        "POST /api/picks/map/subjects",
        false,
        false,
        "bounded",
        "per_page subjects",
        "Listing a group's subjects",
        "Listed a group's subjects",
    ),
    (
        "POST /api/picks/map/strips",
        false,
        false,
        "bounded",
        "every subject of the scope",
        "Reading every subject's kinds",
        "Read every subject's kinds",
    ),
];

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

/// The visits of these subjects in the session cache under the scheme
/// before a door reads them (2026-10-12): a dataset read and sorted with no
/// pick run since, its picks off or its run not yet over, had no visits on
/// its map, since nothing had built the cache for its subjects. Only the
/// timelines that changed are rebuilt.
pub(crate) fn visits_cached(
    registry: &mut Registry,
    scheme: &nils_registry::session::Scheme,
    subjects: &BTreeSet<i64>,
) -> Result<(), Reply> {
    if subjects.is_empty() {
        return Ok(());
    }
    let anchors =
        nils_session::Anchors::resolve(registry, scheme, std::collections::BTreeMap::new())
            .map_err(failed)?;
    nils_session::ensure_for(registry, scheme, &anchors, subjects).map_err(failed)?;
    Ok(())
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
        (None, Some(d)) => {
            let scope = Scope::dataset(store, &d)
                .map_err(failed)?
                .ok_or_else(|| Reply::error(404, format!("no dataset named {d}")))?;
            // a root holds datasets and is none itself (Wave 7a): its
            // folders are each a dataset of their own
            let root = nils_registry::place::show(store, scope.id)
                .map_err(failed)?
                .is_some_and(|p| nils_registry::place::is_root(&p.dataset));
            if root {
                let mut r = Reply::error(
                    400,
                    format!(
                        "{} is a root, not a dataset: each folder under it is a dataset of its own",
                        scope.name
                    ),
                );
                r.body["field"] = Value::from("dataset");
                return Err(r);
            }
            Ok(Some(scope))
        }
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
    // a visit named by its subject and day is read from the cache brought
    // up to date for that subject; one named by the map's id was there when
    // the map was read
    if let Some(s) = subject {
        visits_cached(registry, scheme, &BTreeSet::from([s]))?;
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

/// The datasets whose folders hold a file of each of the subjects' stacks,
/// by name, as the one place that says which folders hold a stack reads
/// them (`dataset_subjects::holders`): of the active datasets, each
/// `source` row the one's that holds it most closely, never a root
/// (2026-10-11: the root, read first, claimed its datasets' stacks, so the
/// columns were named after it). Nima, 2026-10-12: a dataset is a folder
/// that holds stacks, and a stack copied into two datasets is one stack
/// with a path in each (the duplicate policy of 2026-10-10), so it is in
/// both; the map named the first that read it alone.
fn datasets(
    registry: &mut Registry,
    subjects: &BTreeSet<i64>,
) -> Result<HashMap<i64, Vec<String>>, Reply> {
    let store = registry.store();
    let mut by_source: HashMap<i64, String> = HashMap::new();
    for d in nils_registry::place::dataset_sources(store).map_err(failed)? {
        for id in d.sources {
            by_source.insert(id, d.name.clone());
        }
    }
    let ids: Vec<i64> = subjects.iter().copied().collect();
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT hst.id, hss.source_id FROM {} hst {} JOIN {} f ON f.stack_id = hst.id \
             WHERE f.subject_id IN ({})",
            store.qualified("stack"),
            nils_registry::dataset_subjects::holders(store.schema(), "hst", "hss"),
            store.qualified("stack_fingerprint"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &[]).map_err(failed)? {
            if let Some(name) = by_source.get(&r.int(1).map_err(failed)?) {
                let names = out.entry(r.int(0).map_err(failed)?).or_default();
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
    }
    for names in out.values_mut() {
        names.sort();
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
    let layer = layer_of(doc)?;
    let (saved, stored) = current(registry, model, &scope)?;
    let version = stored.as_ref().map_or(1, |s| s.version);
    let draft = match &doc["rules"] {
        Value::Null => None,
        d => Some(Rules::from_json(d, &Rules::defaults(model)).map_err(refused)?),
    };
    let subjects = crate::pick_scopes::subjects_of(registry.store(), &scope).map_err(failed)?;
    // one place of truth (2026-10-11): the scope's own picks once read,
    // else the pack's, as every other reader of this scope reads them
    let picks = nils_registry::pick_truth::Of::scope(registry.store(), &scope).map_err(failed)?;
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
    // a dataset's own stacks, those its folder holds a file of: the map's
    // visits, candidates, scans taken and layer are read from these alone
    // (Nima, 2026-10-12: a dataset holds stacks, never a subject's
    // membership); a cohort holds every stack of its open members. Its
    // `source` rows are read once, for these and for the layer's step.
    let ids = crate::pick_scopes::sources_of(registry.store(), &scope).map_err(failed)?;
    let held = match &ids {
        Some(ids) => Some(
            nils_registry::pick_rules::stacks_of_sources(registry.store(), ids).map_err(failed)?,
        ),
        None => None,
    };
    let sources = ids.as_deref().map(comma);
    // the scope's visits, in the cache before the map reads them
    visits_cached(registry, &scheme, &subjects)?;
    let mut read = main_scans::read(
        registry,
        &Ask {
            model,
            role,
            scheme: &scheme,
            subjects: &subjects,
            picks: &picks,
            datasets: &datasets,
            withheld: &withheld,
            held: held.as_ref(),
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
    // the subjects door's group, worked out once: its page is what a layer
    // is read for alone there
    let paging = if door == Door::Subjects {
        let group = Group::parse(&doc["group"]).map_err(|m| Reply::error(400, m))?;
        let by_changes = match doc["order"].as_str() {
            None | Some("changes") => true,
            Some("visits") => false,
            Some(_) => return Err(Reply::error(400, "order: changes or visits")),
        };
        let page = doc["page"].as_u64().unwrap_or(0) as usize;
        let per_page = doc["per_page"].as_u64().unwrap_or(24).clamp(1, 48) as usize;
        let members = main_scans::Members::of(&read, &cmp, by, &group, by_changes);
        Some((members, page, per_page))
    } else {
        None
    };
    if let Some(layer) = layer {
        let only = paging
            .as_ref()
            .map(|(members, page, per_page)| members.page(&read, *page, *per_page));
        read.add_layer(
            registry.store(),
            layer,
            &withheld,
            held.as_ref(),
            only.as_ref(),
        )
        .map_err(failed)?;
    }
    let mut out = match door {
        Door::Map => main_scans::map(&read, &cmp, model, role, by, version),
        Door::Strips => main_scans::strips(&read, &cmp, model, role, by),
        Door::Subjects => {
            let Some((members, page, per_page)) = paging else {
                return Err(Reply::error(500, "the subjects door's page was not read"));
            };
            let mut page = main_scans::subjects(&read, &cmp, &members, page, per_page);
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
    if let Some(layer) = layer
        && door == Door::Map
    {
        // whether the step that answers the layer has run over the scope,
        // as the scope's Data page says it, with why a run of it would be
        // refused now: on the map door alone, the scope's summary, since
        // the subjects and strips doors are asked beside it and its cost
        // was measured on each (2026-10-15)
        out["layer"]["step"] = layer_step(registry, &scope, sources.as_deref(), caller, layer)?;
    }
    Ok(Reply::ok(out))
}

/// `GET /api/picks/layer-step?cohort=NAME&layer=L` (or `dataset=`): the
/// step that answers a layer over a scope alone, as the map door gives it,
/// for a page to ask while the step waits or runs instead of the whole map
/// (the second review of 2026-10-15). Read by whoever may read the map or
/// the cohort's heatmap.
pub(crate) fn layer_step_door(
    registry: &mut Registry,
    caller: &crate::serve::Caller,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let scope = scope_of(registry, query, None)?;
    // a dataset's step is read as its Data page is, with data:see or
    // pipelines:see; review:see reads a cohort's, as its heatmap is read
    if scope.kind == nils_registry::pick_rules::ScopeKind::Dataset
        && !caller.access.holds("data:see")
        && !caller.access.holds("pipelines:see")
    {
        return Err(Reply::error(
            403,
            "a dataset's step is read with data:see or pipelines:see",
        ));
    }
    let layer = layer_named(query.get("layer").map(String::as_str))?.ok_or_else(|| {
        let mut r = Reply::error(400, "layer: body_region, body_part or post_contrast");
        r.body["field"] = Value::from("layer");
        r
    })?;
    let sources = scope_sources(registry, &scope)?;
    let step = layer_step(registry, &scope, sources.as_deref(), caller, layer)?;
    Ok(Reply::ok(json!({
        "scope": scope.to_json(), "layer": layer.name(), "operation": layer.step(), "step": step,
    })))
}

/// The layer a map door asks for: `body_region`, `body_part` or
/// `post_contrast`, none where `layer` is missing or null.
fn layer_of(doc: &Value) -> Result<Option<Layer>, Reply> {
    match &doc["layer"] {
        Value::Null => Ok(None),
        Value::String(w) => layer_named(Some(w)),
        _ => layer_named(Some("?")),
    }
}

/// A layer as a door names it, the same words for every door that takes
/// one: 400 naming the field where it is no layer.
pub(crate) fn layer_named(word: Option<&str>) -> Result<Option<Layer>, Reply> {
    Layer::from_word(word).map_err(|words| {
        let mut r = Reply::error(400, words);
        r.body["field"] = Value::from("layer");
        r
    })
}

/// A dataset's `source` rows, resolved once a request as its Data page
/// resolves them, as a comma list; none for a cohort.
fn scope_sources(registry: &mut Registry, scope: &Scope) -> Result<Option<String>, Reply> {
    Ok(crate::pick_scopes::sources_of(registry.store(), scope)
        .map_err(failed)?
        .as_deref()
        .map(comma))
}

fn comma(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The step of the operation that answers a layer, over the scope's scans,
/// as the dataset's or the cohort's own page carries it
/// (`crate::operations::steps`): its state (done once a run of it finished
/// over the scans or its model answered one of them), its run and job, what
/// it answered, and why a run of it would be refused now. A dataset's counts
/// keep a sealed sample as its Data page's do; a cohort's leave it out for
/// a caller who may not read it, as the cohort's door does.
fn layer_step(
    registry: &mut Registry,
    scope: &Scope,
    sources: Option<&str>,
    caller: &crate::serve::Caller,
    layer: Layer,
) -> Result<Value, Reply> {
    use nils_registry::pick_rules::ScopeKind;
    let ids = sources.unwrap_or_default().to_string();
    let (held, hide_sealed) = match scope.kind {
        ScopeKind::Dataset => (crate::operations::Scope::Sources(&ids), false),
        ScopeKind::Cohort => (
            crate::operations::Scope::Cohort(scope.id),
            !crate::sealed::reads(&caller.access),
        ),
    };
    let target = scope.label();
    crate::operations::step_of(registry, held, &target, layer.step(), hide_sealed)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(500, format!("{} is not an operation", layer.step())))
}
