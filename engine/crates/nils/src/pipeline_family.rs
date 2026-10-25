// SPDX-License-Identifier: AGPL-3.0-only
//! Pipeline families in the engine (record 57 P6; Wave 7b spec section 7):
//! one catalog row for the person, several member pipelines behind it,
//! the family document's rules choosing the member per unit.
//!
//! The registry keeps a family in `registry_meta` under
//! `pipeline_family:<name>`: the document as loaded, its digest and the
//! catalog id of each member, so the catalog doors list it as one row that
//! opens to its members. `nils pipeline add <folder>` of a folder with a
//! `nils-family.yaml` adds every member's pair (each a pipeline of its own)
//! and then the family; `nils run <family>` routes each unit of the frozen
//! selection with the family's rules, makes one handle and one ordinary run
//! per member over the units it was sent, and every unit records the
//! member and the rule that chose (`variant`) and the family and its
//! digest in its pack record; `--preflight` answers the routing; `nils
//! pipeline replay <folder>` counts the sessions per member, against the
//! family the registry runs today.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nils_pipeline::pack::PipelinePack;
use nils_pipeline::pack::facts::Facts;
use nils_pipeline::pack::family::{self, Family, Routing};
use nils_pipeline::pack::rules::{Person, Status};
use nils_registry::Registry;
use nils_registry::pipeline as rows;
use nils_registry::store::{Param, Store};
use serde_json::{Value, json};

use crate::pipeline_pack::{self, PersonAsk, UnitOf};
use crate::{Exit, fail, usage};

/// Where the registry keeps a family, under its name.
pub(crate) const META_PREFIX: &str = "pipeline_family:";

/// A family as the registry keeps it.
#[derive(Debug, Clone)]
pub(crate) struct Stored {
    pub name: String,
    pub digest: String,
    pub document: Value,
    /// Member key to the catalog id of the member's version added with it.
    pub members: BTreeMap<String, i64>,
    pub added_by: String,
    pub added_at: String,
    /// The document parsed once, when the row is read.
    parsed: Result<Family, String>,
}

impl Stored {
    pub(crate) fn family(&self) -> Result<&Family, String> {
        self.parsed.as_ref().map_err(Clone::clone)
    }

    /// Refuse a run or a replay whose stored member ids no longer exist in
    /// the catalog, with the words to add the family again.
    pub(crate) fn members_exist(&self, store: &mut Store) -> Result<(), String> {
        let f = self.family()?;
        let mut gone = Vec::new();
        for k in f.members.keys() {
            let there = match self.members.get(k) {
                Some(id) => rows::get(store, *id).ok().flatten().is_some(),
                None => false,
            };
            if !there {
                gone.push(k.clone());
            }
        }
        if gone.is_empty() {
            return Ok(());
        }
        Err(format!(
            "family {}: its member{} {} no longer in the catalog; add the family's folder again with `nils pipeline add <folder>`",
            self.name,
            if gone.len() == 1 { "" } else { "s" },
            if gone.len() == 1 {
                format!("{} is", gone[0])
            } else {
                format!("{} are", gone.join(", "))
            }
        ))
    }

    fn of_json(name: &str, v: &Value) -> Option<Stored> {
        Some(Stored {
            name: name.to_string(),
            digest: v["digest"].as_str()?.to_string(),
            document: v["document"].clone(),
            parsed: family::from_value(v["document"].clone()),
            members: v["members"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, id)| id.as_i64().map(|id| (k.clone(), id)))
                        .collect()
                })
                .unwrap_or_default(),
            added_by: v["added_by"].as_str().unwrap_or_default().to_string(),
            added_at: v["added_at"].as_str().unwrap_or_default().to_string(),
        })
    }
}

/// The family of this name, where the registry holds one.
pub(crate) fn get(store: &mut Store, name: &str) -> Result<Option<Stored>, String> {
    let sql = format!(
        "SELECT value FROM {} WHERE key = {}",
        store.qualified("registry_meta"),
        store.dialect().param(1, nils_registry::schema::Type::Text)
    );
    let key = format!("{META_PREFIX}{name}");
    let row = store
        .query_opt(&sql, &[Param::from(key.as_str())])
        .map_err(|e| e.to_string())?;
    Ok(row
        .and_then(|r| r.text(0).ok().map(str::to_string))
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| Stored::of_json(name, &v)))
}

/// Every family the registry holds, by name.
pub(crate) fn list(store: &mut Store) -> Result<Vec<Stored>, String> {
    let sql = format!(
        "SELECT key, value FROM {} WHERE key LIKE {} ORDER BY key",
        store.qualified("registry_meta"),
        store.dialect().param(1, nils_registry::schema::Type::Text)
    );
    let like = format!("{META_PREFIX}%");
    let mut out = Vec::new();
    for r in store
        .query(&sql, &[Param::from(like.as_str())])
        .map_err(|e| e.to_string())?
    {
        let (Ok(key), Ok(text)) = (r.text(0), r.text(1)) else {
            continue;
        };
        let name = key.trim_start_matches(META_PREFIX);
        if let Some(v) = serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|v| Stored::of_json(name, &v))
        {
            out.push(v);
        }
    }
    Ok(out)
}

/// The catalog's family decoration, for the door and the command line
/// alike: each member's row names its family, and the families come back
/// as their one row each, which opens to its members. The families are
/// read, and each parsed, once.
pub(crate) fn decorate(store: &mut Store, docs: &mut [Value]) -> Result<Vec<Value>, String> {
    let stored = list(store)?;
    let mut of: BTreeMap<String, String> = BTreeMap::new();
    for s in &stored {
        if let Ok(f) = s.family() {
            for m in f.members.values() {
                of.insert(m.pipeline.clone(), s.name.clone());
            }
        }
    }
    for d in docs.iter_mut() {
        if let Some(f) = d["name"].as_str().and_then(|n| of.get(n)) {
            d["family"] = json!(f);
        }
    }
    Ok(stored.iter().map(|s| doc(store, s)).collect())
}

/// A family as the doors and `--json` answer it: the one row, which opens
/// to its members, each with its catalog version and standing.
pub(crate) fn doc(store: &mut Store, s: &Stored) -> Value {
    let f = match s.family() {
        Ok(f) => f,
        Err(e) => {
            return json!({"name": s.name, "digest": s.digest, "error": e, "family": true});
        }
    };
    let members: Vec<Value> = f
        .members
        .iter()
        .map(|(k, m)| {
            let row = s
                .members
                .get(k)
                .and_then(|id| rows::get(store, *id).ok().flatten());
            let level = row.as_ref().map(|p| p.level.clone());
            let (label, id, standing) = match &row {
                Some(p) => {
                    let d = crate::pipelines::pipeline_doc_of(store, p);
                    (
                        Some(p.label()),
                        Some(p.id),
                        d["standing"].clone(),
                    )
                }
                None => (None, None, Value::Null),
            };
            json!({
                "member": k, "pipeline": m.pipeline, "method": m.method, "words": m.words,
                "pipeline_id": id, "label": label, "level": level, "standing": standing,
                "tasks": f.tasks.iter().filter(|(_, t)| t.members.contains(k)).map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "family": true,
        "name": s.name, "version": f.version, "words": f.words, "digest": s.digest,
        "roles": f.roles,
        "goals": {"goal": f.goals.goal, "switches": f.goals.switches},
        "tasks": f.tasks.iter().map(|(k, t)| (k.clone(), json!({"level": t.level, "words": t.words, "members": t.members}))).collect::<serde_json::Map<_, _>>(),
        "members": members,
        "needs": f.needs.keys().cloned().collect::<Vec<_>>(),
        "rules": f.rules.iter().map(|r| json!({"id": r.id, "when": r.text, "route": r.route, "why": r.why, "source": r.source})).collect::<Vec<_>>(),
        "added_by": s.added_by, "added_at": s.added_at,
        "opens": format!("/api/pipelines/family/{}", s.name),
        "document": s.document,
    })
}

/// Validate a family's folder offline: the family and every member's pair,
/// with the loader's words.
pub(crate) fn validate(dir: &Path) -> Result<Value, Exit> {
    let (f, members) = family::load_dir(dir, None).map_err(usage)?;
    Ok(json!({
        "folder": dir.display().to_string(),
        "family": f.name,
        "version": f.version,
        "digest": f.digest(),
        "tasks": f.tasks.keys().cloned().collect::<Vec<_>>(),
        "rules": f.rules.len(),
        "members": members.iter().map(|(k, (d, p))| json!({
            "member": k, "pipeline": d.name, "level": d.level.name(),
            "descriptor_digest": d.digest(),
            "pack": p.as_ref().map_or(json!("none"), |p| json!({"digest": p.digest(), "version": p.version, "rules": p.rules.len(), "checks": p.checks.len()})),
            "version_digest": nils_pipeline::pack::version_digest(&d.digest(), p.as_ref().map(PipelinePack::digest).as_deref()),
        })).collect::<Vec<_>>(),
        "ok": true,
    }))
}

/// Add a family's folder: every member's pair, then the family itself.
pub(crate) fn add_folder(registry: &mut Registry, dir: &Path) -> Result<Value, Exit> {
    let (f, members) = family::load_dir(dir, None).map_err(usage)?;
    // the member texts, read once, before anything is written
    let mut texts: Vec<(&String, String, Option<String>)> = Vec::new();
    for k in members.keys() {
        let folder = dir.join(k);
        let d_text = std::fs::read_to_string(folder.join(nils_pipeline::pack::DESCRIPTOR_FILE))
            .map_err(|e| usage(format!("{}: {e}", folder.display())))?;
        let p_text = std::fs::read_to_string(folder.join(nils_pipeline::pack::FILE)).ok();
        texts.push((k, d_text, p_text));
    }
    // the members and the family are added together or not at all
    registry.store().begin().map_err(|e| fail(e.to_string()))?;
    let done = add_members_and_family(registry, &f, &texts);
    match done {
        Ok(v) => {
            registry.store().commit().map_err(|e| fail(e.to_string()))?;
            Ok(v)
        }
        Err(e) => {
            let _ = registry.store().rollback();
            Err(e)
        }
    }
}

type MemberTexts<'a> = [(&'a String, String, Option<String>)];

fn add_members_and_family(
    registry: &mut Registry,
    f: &Family,
    texts: &MemberTexts<'_>,
) -> Result<Value, Exit> {
    let mut ids = BTreeMap::new();
    let mut added = Vec::new();
    for (k, d_text, p_text) in texts {
        let v = crate::pipelines::add_descriptor(registry, d_text, p_text.as_deref())?;
        let id = v["id"].as_i64().unwrap_or_default();
        ids.insert((*k).clone(), id);
        added.push(json!({"member": k, "pipeline": v["name"], "id": id, "label": v["label"], "added": v["added"]}));
    }
    let digest = f.digest();
    let before = get(registry.store(), &f.name).map_err(fail)?;
    let same = before
        .as_ref()
        .is_some_and(|b| b.digest == digest && b.members == ids);
    let who = crate::actor();
    let now = nils_registry::time::now_iso();
    if !same {
        let value = json!({
            "digest": digest, "document": f.document, "members": ids,
            "added_by": who, "added_at": now,
        });
        registry
            .set_meta(&format!("{META_PREFIX}{}", f.name), &value.to_string())
            .map_err(|e| fail(e.to_string()))?;
        crate::audit(
            registry,
            nils_registry::audit::Action::PipelineAdd,
            json!({"family": f.name, "members": ids}),
            Some(json!({"digest": digest, "replaced": before.as_ref().map(|b| b.digest.clone())})),
        )?;
    }
    let stored = get(registry.store(), &f.name)
        .map_err(fail)?
        .ok_or_else(|| fail("the family was not kept"))?;
    let mut v = doc(registry.store(), &stored);
    v["added"] = json!(!same);
    v["members_added"] = json!(added);
    Ok(v)
}

/// One unit of a family run: its name as a run names it, what it holds,
/// and where the family sent it.
#[derive(Debug, Clone)]
pub(crate) struct RoutedUnit {
    pub unit: String,
    pub subject_id: i64,
    pub session_day: Option<String>,
    pub stacks: Vec<i64>,
    pub routing: Routing,
}

/// The key a unit's family record is found by in the runner: the
/// subject's id and, for a session unit, its day.
pub(crate) fn unit_key(subject_id: Option<i64>, day: Option<&str>) -> String {
    match (subject_id, day) {
        (Some(s), Some(d)) => format!("{s}:{d}"),
        (Some(s), None) => s.to_string(),
        _ => String::new(),
    }
}

/// What the runner records on every unit of a member's run: the family,
/// the member, and per unit the rule that chose. Carried in `RunArgs`
/// from the family run to the member's run, never from the command line.
#[derive(Debug, Clone, Default)]
pub(crate) struct Route {
    pub family: String,
    pub digest: String,
    pub member: String,
    pub pipeline: String,
    /// [`unit_key`] to `(rule, why)`.
    pub by_unit: BTreeMap<String, (String, Option<String>)>,
}

impl Route {
    /// The `variant` and `family` a unit's pack record carries.
    pub(crate) fn record(&self, key: &str) -> Option<(Value, Value)> {
        let (rule, why) = self.by_unit.get(key)?;
        Some((
            json!({"variant": self.member, "pipeline": self.pipeline, "rule": rule, "why": why}),
            json!({"name": self.family, "digest": self.digest}),
        ))
    }
}

/// The person's choices checked against the family's goals.
pub(crate) fn person_of(f: &Family, ask: &PersonAsk, user: &str) -> Result<Person, String> {
    let mut choices = BTreeMap::new();
    if let Some(g) = &ask.goal {
        choices.insert("goal".to_string(), f.choice("goal", &json!(g))?);
    }
    for (k, v) in &ask.switches {
        choices.insert(k.clone(), f.choice(k, v)?);
    }
    Ok(Person {
        user: user.to_string(),
        choices,
        given: BTreeSet::new(),
        proposed: BTreeSet::new(),
        let_run: ask.let_run,
    })
}

/// One unit of a set of stacks at a task's level, before it is routed.
struct UnitStacks {
    unit: String,
    subject: i64,
    day: Option<String>,
    /// The session days it holds (one for a session unit).
    days: BTreeSet<String>,
    of: UnitOf,
    stacks: Vec<i64>,
}

/// The units of a set of stacks at a task's level: a session unit per
/// (subject, day), a participant unit per subject, each with its stacks by
/// role and the days it holds.
fn units_of(store: &mut Store, stacks: &[i64], level: &str) -> Result<Vec<UnitStacks>, String> {
    let roles = crate::preflight::roles_of_stacks(store, stacks)?;
    let sessions = crate::pipelines::stack_sessions(store, stacks)?;
    let mut units: BTreeMap<(i64, Option<String>), UnitStacks> = BTreeMap::new();
    for s in stacks {
        let Some((subject, day)) = sessions.get(s) else {
            continue;
        };
        let key = (*subject, (level == "session").then(|| day.clone()));
        let e = units.entry(key.clone()).or_insert_with(|| UnitStacks {
            unit: match &key.1 {
                Some(d) => format!("sub-{subject}_ses-{d}"),
                None => format!("sub-{subject}"),
            },
            subject: *subject,
            day: key.1.clone(),
            days: BTreeSet::new(),
            of: UnitOf {
                by_role: BTreeMap::new(),
                subject_id: Some(*subject),
                session_day: key.1.clone(),
            },
            stacks: Vec::new(),
        });
        e.days.insert(day.clone());
        e.stacks.push(*s);
        for r in roles.get(s).into_iter().flatten() {
            e.of.by_role.entry(r.clone()).or_default().push(*s);
        }
    }
    Ok(units.into_values().collect())
}

/// Route every unit of these stacks under the family and the person's
/// choices: the registry's facts, the unit facts, then the rules.
pub(crate) fn route_stacks(
    registry: &mut Registry,
    f: &Family,
    stacks: &[i64],
    person: &Person,
) -> Result<Vec<RoutedUnit>, String> {
    let task = f.task_of(&person.choices);
    let level = f
        .tasks
        .get(&task)
        .map(|t| t.level.clone())
        .unwrap_or_else(|| "session".into());
    let host = pipeline_pack::host_facts(registry);
    let store = registry.store();
    let units = units_of(store, stacks, &level)?;
    let of: Vec<UnitOf> = units.iter().map(|u| u.of.clone()).collect();
    let mut facts = pipeline_pack::facts_for(store, &of, &host);
    for (u, f_) in units.iter().zip(facts.iter_mut()) {
        let t1w: Vec<i64> = u.of.by_role.get("t1w").cloned().unwrap_or_default();
        let same = pipeline_pack::same_protocol(store, &t1w);
        family::set_unit_facts(f_, u.days.len(), same);
    }
    Ok(units
        .into_iter()
        .zip(facts.iter())
        .map(|(u, facts)| RoutedUnit {
            unit: u.unit,
            subject_id: u.subject,
            session_day: u.day,
            stacks: u.stacks,
            routing: family::route(f, facts, person),
        })
        .collect())
}

/// The counts of a routing, as the pre-flight and the replay answer them.
pub(crate) fn counts(routed: &[&Routing]) -> Value {
    let mut by_member: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_words: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
    for r in routed {
        if let Some(m) = &r.member
            && r.status != Status::Skipped
        {
            *by_member.entry(m.clone()).or_default() += 1;
            if let Some(rule) = &r.rule {
                *by_rule.entry(rule.clone()).or_default() += 1;
            }
        }
        if r.status == Status::Skipped {
            *by_words
                .entry(r.words.clone().unwrap_or_default())
                .or_default() += 1;
        }
    }
    let mut skipped_by_words: Vec<(String, usize)> = by_words.into_iter().collect();
    skipped_by_words.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    json!({
        "units": routed.len(),
        "routed": routed.iter().filter(|r| r.status == Status::Routed).count(),
        "skipped": routed.iter().filter(|r| r.status == Status::Skipped).count(),
        "cannot_tell": routed.iter().filter(|r| r.status == Status::Skipped && r.cannot_tell.is_some()).count(),
        "with_caveats": routed.iter().filter(|r| !r.caveats.is_empty()).count(),
        "by_member": by_member,
        "by_rule": by_rule,
        "skipped_by_words": skipped_by_words.iter().map(|(w, n)| json!({"words": w, "units": n})).collect::<Vec<_>>(),
    })
}

/// The pre-flight of a family run: every unit with where it goes.
pub(crate) fn preflight_doc(
    s: &Stored,
    f: &Family,
    person: &Person,
    ask: &PersonAsk,
    units: &[RoutedUnit],
) -> Value {
    let task = f.task_of(&person.choices);
    let routings: Vec<&Routing> = units.iter().map(|u| &u.routing).collect();
    json!({
        "family": s.name,
        "digest": s.digest,
        "task": task,
        "level": f.tasks.get(&task).map(|t| t.level.clone()),
        "person": ask.doc(),
        "units": units.iter().map(|u| {
            let mut d = u.routing.doc();
            d["unit"] = json!(u.unit);
            d["subject_id"] = json!(u.subject_id);
            d["session_day"] = json!(u.session_day);
            d["stacks"] = json!(u.stacks);
            d
        }).collect::<Vec<_>>(),
        "counts": counts(&routings),
    })
}

/// The pre-flight in words.
pub(crate) fn preflight_words(doc: &Value) -> String {
    let c = &doc["counts"];
    let n = |k: &str| c[k].as_u64().unwrap_or_default();
    let mut out = vec![format!(
        "family {} under the task {}: {} units, {} routed, {} skipped ({} cannot tell), {} with caveats",
        doc["family"].as_str().unwrap_or_default(),
        doc["task"].as_str().unwrap_or_default(),
        n("units"),
        n("routed"),
        n("skipped"),
        n("cannot_tell"),
        n("with_caveats")
    )];
    for (m, k) in c["by_member"].as_object().into_iter().flatten() {
        out.push(format!("  {m}: {}", k.as_u64().unwrap_or_default()));
    }
    for w in c["skipped_by_words"].as_array().into_iter().flatten() {
        out.push(format!(
            "  skipped {}: {}",
            w["units"].as_u64().unwrap_or_default(),
            w["words"].as_str().unwrap_or_default()
        ));
    }
    for u in doc["units"].as_array().into_iter().flatten() {
        let words = || u["words"].as_str().unwrap_or_default().to_string();
        let where_ = match u["member"].as_str() {
            _ if u["status"] == "skipped" => words(),
            Some(m) => format!("{m} ({})", u["rule"].as_str().unwrap_or_default()),
            None => words(),
        };
        out.push(format!(
            "  {} {}: {where_}",
            u["unit"].as_str().unwrap_or_default(),
            u["status"].as_str().unwrap_or_default()
        ));
    }
    out.join("\n")
}

/// A handle of these stacks, pinned as the parent handle was, with the
/// family and the member it was made for, so the member's run says where
/// its units came from.
fn member_handle(
    store: &mut Store,
    parent: i64,
    stacks: &[i64],
    s: &Stored,
    member: &str,
) -> Result<i64, Exit> {
    let h = nils_ask::handle::get(store, parent)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| usage(format!("no handle {parent}")))?;
    let subjects: BTreeMap<i64, Option<i64>> = nils_ask::handle::keys(store, parent)
        .map_err(|e| fail(e.to_string()))?
        .into_iter()
        .collect();
    let rows_: Vec<nils_registry::store::Row> = stacks
        .iter()
        .map(|st| {
            use nils_registry::store::{Cell, Row};
            Row(vec![
                Cell::Int(*st),
                subjects
                    .get(st)
                    .copied()
                    .flatten()
                    .map_or(Cell::Null, Cell::Int),
            ])
        })
        .collect();
    let mut pins = h.selection_versions.as_array().cloned().unwrap_or_default();
    pins.push(json!({"handle": parent, "family": s.name, "digest": s.digest, "member": member}));
    let (id, _) = crate::operations::save_stacks(
        store,
        rows_,
        &h.principal,
        h.epoch,
        &crate::operations::Pinned {
            versions: Value::Array(pins),
            pack_version: h.pack_version.as_deref(),
            scheme_digest: h.scheme_digest.as_deref(),
        },
    )
    .map_err(fail)?;
    Ok(id)
}

/// `nils run <family>`: route, then one run per member over the units it
/// was sent; `--preflight` answers the routing and runs nothing.
pub(crate) fn run(
    home: &nils_registry::Home,
    args: &crate::pipelines::RunArgs,
    s: &Stored,
) -> Result<(), Exit> {
    let f = s.family().map_err(fail)?;
    {
        let mut registry = crate::open(home)?;
        s.members_exist(registry.store()).map_err(usage)?;
    }
    let ask =
        PersonAsk::of_args(args.goal.as_deref(), &args.switches, args.let_run).map_err(usage)?;
    let who = crate::actor();
    let person = person_of(f, &ask, &who).map_err(usage)?;
    let (handle, _main) = crate::preflight::frozen_over(
        home,
        args.select.as_deref(),
        args.handle,
        args.pack_dir.clone(),
        &args.pack,
        &f.roles,
    )?
    .ok_or_else(|| {
        usage(format!(
            "nils run {}: a family runs over a frozen selection: --select selection:<name>@<v>, --select main@dataset:<name>, or --handle <id>",
            s.name
        ))
    })?;
    let mut registry = crate::open(home)?;
    let stacks = crate::pipelines::handle_stacks(registry.store(), handle)?;
    let units = route_stacks(&mut registry, f, &stacks, &person).map_err(fail)?;
    let doc = preflight_doc(s, f, &person, &ask, &units);
    if args.preflight {
        if args.json {
            return crate::pipelines::print_json(&doc);
        }
        println!("{}", preflight_words(&doc));
        return Ok(());
    }
    if units.iter().all(|u| u.routing.status == Status::Skipped) {
        return Err(usage(format!("no unit can run: {}", preflight_words(&doc))));
    }
    // one handle and one run per member, in the family's member order
    let mut per_member: BTreeMap<String, Vec<&RoutedUnit>> = BTreeMap::new();
    for u in &units {
        if u.routing.status == Status::Skipped {
            eprintln!(
                "{}: skipped: {}",
                u.unit,
                u.routing.words.clone().unwrap_or_default()
            );
            continue;
        }
        if let Some(m) = &u.routing.member {
            per_member.entry(m.clone()).or_default().push(u);
        }
    }
    // Wave 7b A7: a workflow step is one run, so under a chain the units
    // must route to one member; several members are refused in words
    if args.chain.is_some() && per_member.len() > 1 {
        return Err(usage(format!(
            "family {}: a workflow step is one run, and this scope routes its units to {} members ({}); plan the workflow over a selection whose units route to one member, or run the family on its own",
            s.name,
            per_member.len(),
            per_member.keys().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    for (member, list) in per_member {
        let Some(m) = f.members.get(&member) else {
            continue;
        };
        let member_stacks: Vec<i64> = list.iter().flat_map(|u| u.stacks.iter().copied()).collect();
        let h = member_handle(registry.store(), handle, &member_stacks, s, &member)?;
        let route = Route {
            family: s.name.clone(),
            digest: s.digest.clone(),
            member: member.clone(),
            pipeline: m.pipeline.clone(),
            by_unit: list
                .iter()
                .map(|u| {
                    (
                        unit_key(Some(u.subject_id), u.session_day.as_deref()),
                        (
                            u.routing.rule.clone().unwrap_or_default(),
                            u.routing.why.clone(),
                        ),
                    )
                })
                .collect(),
        };
        // the member's pack takes the switches it offers; the rest are the
        // family's own
        let offered: BTreeSet<String> = s
            .members
            .get(&member)
            .and_then(|id| rows::get(registry.store(), *id).ok().flatten())
            .and_then(|p| pipeline_pack::pack_of(&p))
            .map(|p| p.goals.switches.keys().cloned().collect())
            .unwrap_or_default();
        let switches: Vec<String> = args
            .switches
            .iter()
            .filter(|w| {
                w.split_once('=')
                    .is_some_and(|(k, _)| offered.contains(k.trim()))
            })
            .cloned()
            .collect();
        eprintln!(
            "family {}: {} unit(s) to {} ({}), handle {h}",
            s.name,
            list.len(),
            member,
            m.pipeline
        );
        let member_args = crate::pipelines::RunArgs {
            // the version the family record pins, by its id
            pipeline: Some(s.members[&member].to_string()),
            select: None,
            handle: Some(h),
            switches,
            family: Some(route),
            // the chain, where a workflow step runs the family, is the
            // member run's: it records itself on the step and binds the
            // links
            ..args.clone()
        };
        crate::pipelines::run_command(home, member_args)?;
    }
    Ok(())
}

/// `nils pipeline replay <family folder>`: every session the registry's
/// main-scan picks make (grouped per person where the task runs over
/// participants), routed by the folder's family and by the family the
/// registry holds, read-only; the counts per member and what changes.
pub(crate) fn replay(registry: &mut Registry, dir: &Path, ask: &PersonAsk) -> Result<Value, Exit> {
    let (f, _members) = family::load_dir(dir, None).map_err(usage)?;
    let who = crate::actor();
    let person = person_of(&f, ask, &who).map_err(usage)?;
    let task = f.task_of(&person.choices);
    let level = f
        .tasks
        .get(&task)
        .map(|t| t.level.clone())
        .unwrap_or_else(|| "session".into());
    let before = get(registry.store(), &f.name).map_err(fail)?;
    if let Some(b) = &before {
        b.members_exist(registry.store()).map_err(usage)?;
    }
    let host = pipeline_pack::host_facts(registry);
    let store = registry.store();
    let sessions = crate::pipeline_replay::sessions(store).map_err(fail)?;
    // the population: sessions, or persons with every session of theirs
    let mut units: Vec<(UnitOf, BTreeSet<String>)> = Vec::new();
    if level == "participant" {
        let mut by_subject: BTreeMap<i64, (UnitOf, BTreeSet<String>)> = BTreeMap::new();
        for u in sessions {
            let Some(s) = u.subject_id else { continue };
            let e = by_subject.entry(s).or_insert_with(|| {
                (
                    UnitOf {
                        by_role: BTreeMap::new(),
                        subject_id: Some(s),
                        session_day: None,
                    },
                    BTreeSet::new(),
                )
            });
            if let Some(d) = &u.session_day {
                e.1.insert(d.clone());
            }
            for (r, st) in u.by_role {
                e.0.by_role.entry(r).or_default().extend(st);
            }
        }
        units.extend(by_subject.into_values());
    } else {
        units.extend(sessions.into_iter().map(|u| {
            let days: BTreeSet<String> = u.session_day.iter().cloned().collect();
            (u, days)
        }));
    }
    let (with, without): (Vec<_>, Vec<_>) = units
        .into_iter()
        .partition(|(u, _)| u.by_role.contains_key("t1w"));
    let of: Vec<UnitOf> = with.iter().map(|(u, _)| u.clone()).collect();
    let mut facts: Vec<Facts> = pipeline_pack::facts_for(store, &of, &host);
    for ((u, days), f_) in with.iter().zip(facts.iter_mut()) {
        let t1w: Vec<i64> = u.by_role.get("t1w").cloned().unwrap_or_default();
        let same = pipeline_pack::same_protocol(store, &t1w);
        family::set_unit_facts(f_, days.len().max(1), same);
    }
    let now: Vec<Routing> = facts
        .iter()
        .map(|x| family::route(&f, x, &person))
        .collect();
    let now_refs: Vec<&Routing> = now.iter().collect();
    let mut doc = json!({
        "family": f.name,
        "version": f.version,
        "digest": f.digest(),
        "task": task,
        "level": level,
        "units": with.len(),
        "without_t1w": without.len(),
        "person": ask.doc(),
        "now": counts(&now_refs),
        "against": Value::Null,
        "before": Value::Null,
        "changes": Value::Null,
        "read_only": true,
    });
    if let Some(b) = before {
        let bf = b.family().map_err(fail)?;
        let bperson = person_of(bf, ask, &who).unwrap_or_else(|_| person.clone());
        let then: Vec<Routing> = facts
            .iter()
            .map(|x| family::route(bf, x, &bperson))
            .collect();
        let then_refs: Vec<&Routing> = then.iter().collect();
        let mut moved: BTreeMap<(Option<String>, Option<String>), usize> = BTreeMap::new();
        for (a, z) in then.iter().zip(&now) {
            let from = (a.status != Status::Skipped)
                .then(|| a.member.clone())
                .flatten();
            let to = (z.status != Status::Skipped)
                .then(|| z.member.clone())
                .flatten();
            if from != to {
                *moved.entry((from, to)).or_default() += 1;
            }
        }
        doc["against"] =
            json!({"digest": b.digest, "same": b.digest == f.digest(), "added_at": b.added_at});
        doc["before"] = counts(&then_refs);
        doc["changes"] = json!({
            "member": moved.into_iter().map(|((from, to), n)| json!({"from": from, "to": to, "units": n})).collect::<Vec<_>>(),
            "run_to_skipped": then.iter().zip(&now).filter(|(a, z)| a.status != Status::Skipped && z.status == Status::Skipped).count(),
            "skipped_to_run": then.iter().zip(&now).filter(|(a, z)| a.status == Status::Skipped && z.status != Status::Skipped).count(),
        });
    }
    Ok(doc)
}

/// The replay in words.
pub(crate) fn replay_words(doc: &Value) -> String {
    let n = |v: &Value| v.as_u64().unwrap_or_default();
    let c = &doc["now"];
    let mut out = vec![format!(
        "family {} ({}) under the task {}: {} units ({} without a T1w left out)",
        doc["family"].as_str().unwrap_or_default(),
        doc["digest"]
            .as_str()
            .map(|d| &d[..12.min(d.len())])
            .unwrap_or_default(),
        doc["task"].as_str().unwrap_or_default(),
        n(&doc["units"]),
        n(&doc["without_t1w"])
    )];
    out.push(format!(
        "now: {} routed, {} skipped ({} cannot tell), {} with caveats",
        n(&c["routed"]),
        n(&c["skipped"]),
        n(&c["cannot_tell"]),
        n(&c["with_caveats"])
    ));
    for (m, k) in c["by_member"].as_object().into_iter().flatten() {
        out.push(format!("  {m}: {}", n(k)));
    }
    for w in c["skipped_by_words"].as_array().into_iter().flatten() {
        out.push(format!(
            "  skipped {}: {}",
            n(&w["units"]),
            w["words"].as_str().unwrap_or_default()
        ));
    }
    match &doc["against"] {
        Value::Null => out.push("against: the registry holds no family of this name yet".into()),
        a => {
            out.push(format!(
                "against the registry's family ({}{}):",
                a["digest"]
                    .as_str()
                    .map(|d| &d[..12.min(d.len())])
                    .unwrap_or_default(),
                if a["same"] == json!(true) {
                    ", the same document"
                } else {
                    ""
                }
            ));
            let ch = &doc["changes"];
            out.push(format!(
                "  {} run to skipped, {} skipped to run",
                n(&ch["run_to_skipped"]),
                n(&ch["skipped_to_run"])
            ));
            for m in ch["member"].as_array().into_iter().flatten() {
                out.push(format!(
                    "  member {}: {} to {}",
                    n(&m["units"]),
                    m["from"].as_str().unwrap_or("none"),
                    m["to"].as_str().unwrap_or("none")
                ));
            }
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use nils_pipeline::pack::facts::{Facts, Val};
    use nils_pipeline::pack::family::{self, Family};
    use nils_pipeline::pack::rules::{Person, Status};
    use serde_json::json;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../pipelines/registration")
    }

    fn shipped() -> (Family, family::Members) {
        family::load_dir(&repo(), None).unwrap_or_else(|e| panic!("{e}"))
    }

    fn person(choices: &[(&str, serde_json::Value)]) -> Person {
        Person {
            user: "tester".into(),
            choices: choices
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            given: Default::default(),
            proposed: Default::default(),
            let_run: false,
        }
    }

    /// A session with a T1w (and the roles given), each stack of the brain
    /// with the dimension and voxel asked, acquired alike.
    fn session(stacks: &[(&str, &str, f64)]) -> Facts {
        let mut f = Facts::default();
        for (role, dim, voxel) in stacks {
            f.roles.insert(role.to_string());
            f.set(&format!("{role}.dimension"), Val::Text(dim.to_string()));
            f.set(&format!("{role}.voxel_max_mm"), Val::Number(*voxel));
            f.set(&format!("{role}.voxel_min_mm"), Val::Number(*voxel));
            f.set(&format!("{role}.slice_thickness_mm"), Val::Number(*voxel));
            f.set(&format!("{role}.body_part"), Val::Text("brain".into()));
            f.set(&format!("{role}.base"), Val::Text("T1".into()));
            f.set(
                &format!("{role}.post_contrast"),
                Val::Text("not_given".into()),
            );
            f.set(&format!("{role}.field_strength_t"), Val::Number(3.0));
            f.set(&format!("{role}.vendor"), Val::Text("siemens".into()));
        }
        family::set_unit_facts(&mut f, 1, Some(true));
        f
    }

    fn routed(f: &Family, facts: &Facts, choices: &[(&str, serde_json::Value)]) -> family::Routing {
        family::route(f, facts, &person(choices))
    }

    /// G11, the first half, offline: each task's rule picks its method on
    /// fixture sessions, and the refusals skip in the family's words.
    #[test]
    fn g11_each_task_s_rule_picks_its_method_on_fixture_sessions() {
        let (f, _) = shipped();
        let within = [("task", json!("within_session"))];
        // both 3D at 1.5 mm or finer: ANTs rigid
        let r = routed(
            &f,
            &session(&[("t1w", "3D", 1.0), ("flair", "3D", 1.0)]),
            &within,
        );
        assert_eq!(r.status, Status::Routed, "{r:?}");
        assert_eq!(r.member.as_deref(), Some("ants"));
        assert_eq!(r.rule.as_deref(), Some("within-session-fine-3d"));
        assert_eq!(r.pipeline.as_deref(), Some("registration-ants"));
        // a T2w alone beside the T1w, both fine: ANTs rigid too
        let r = routed(
            &f,
            &session(&[("t1w", "3D", 1.0), ("t2w", "3D", 1.2)]),
            &within,
        );
        assert_eq!(r.member.as_deref(), Some("ants"));
        assert_eq!(r.rule.as_deref(), Some("within-session-fine-3d-t2w"));
        // a 2D FLAIR: SynthMorph
        let r = routed(
            &f,
            &session(&[("t1w", "3D", 1.0), ("flair", "2D", 5.0)]),
            &within,
        );
        assert_eq!(r.member.as_deref(), Some("synthmorph"));
        assert_eq!(r.rule.as_deref(), Some("within-session-thick-or-2d"));
        // a 3D FLAIR coarser than 1.5 mm: SynthMorph
        let r = routed(
            &f,
            &session(&[("t1w", "3D", 1.0), ("flair", "3D", 2.0)]),
            &within,
        );
        assert_eq!(r.member.as_deref(), Some("synthmorph"));
        // nothing beside the T1w: skipped in words
        let r = routed(&f, &session(&[("t1w", "3D", 1.0)]), &within);
        assert_eq!(r.status, Status::Skipped);
        assert!(
            r.words.as_deref().unwrap().contains("nothing to align"),
            "{r:?}"
        );
        // not a brain: refused
        let mut spine = session(&[("t1w", "3D", 1.0), ("flair", "3D", 1.0)]);
        spine.set("t1w.body_part", Val::Text("spine".into()));
        let r = routed(&f, &spine, &within);
        assert_eq!(r.status, Status::Skipped);
        assert!(
            r.words.as_deref().unwrap().contains("not of the brain"),
            "{r:?}"
        );
        // a body part the registry cannot tell: cannot tell
        spine.set("t1w.body_part", Val::Unknown);
        let r = routed(&f, &spine, &within);
        assert_eq!(r.words.as_deref(), Some("cannot tell: t1w.body_part"));
        assert_eq!(r.cannot_tell.as_deref(), Some("t1w.body_part"));

        // across visits: two 3D visits acquired alike go to robust
        let visits = [("task", json!("across_visits"))];
        let mut two = session(&[("t1w", "3D", 1.0)]);
        family::set_unit_facts(&mut two, 2, Some(true));
        let r = routed(&f, &two, &visits);
        assert_eq!(r.member.as_deref(), Some("robust"), "{r:?}");
        assert_eq!(r.rule.as_deref(), Some("across-visits-rigid"));
        assert_eq!(r.pipeline.as_deref(), Some("registration-robust"));
        assert!(r.caveats.is_empty(), "{:?}", r.caveats);
        // three visits too; at research grade the halfway SyN caveat
        family::set_unit_facts(&mut two, 3, Some(true));
        let r = routed(
            &f,
            &two,
            &[
                ("task", json!("across_visits")),
                ("goal", json!("research")),
            ],
        );
        assert_eq!(r.member.as_deref(), Some("robust"));
        assert!(
            r.caveats.iter().any(|c| c.words.contains("halfway")),
            "{:?}",
            r.caveats
        );
        // one visit: nothing to align
        let mut one = session(&[("t1w", "3D", 1.0)]);
        family::set_unit_facts(&mut one, 1, Some(true));
        let r = routed(&f, &one, &visits);
        assert_eq!(r.status, Status::Skipped);
        assert!(r.words.as_deref().unwrap().contains("one visit"), "{r:?}");
        // a changed protocol: the SynthMorph visits member, not shipped
        let mut changed = session(&[("t1w", "3D", 1.0)]);
        family::set_unit_facts(&mut changed, 2, Some(false));
        let r = routed(&f, &changed, &visits);
        assert_eq!(r.status, Status::Skipped);
        assert!(
            r.words.as_deref().unwrap().contains("not acquired alike"),
            "{r:?}"
        );
        // 2D visits: the same
        let mut flat = session(&[("t1w", "2D", 5.0)]);
        family::set_unit_facts(&mut flat, 2, Some(true));
        let r = routed(&f, &flat, &visits);
        assert_eq!(r.status, Status::Skipped);

        // to the template: a fine 3D T1w at the standard goal is SyN
        let template = [("task", json!("to_template"))];
        let r = routed(&f, &session(&[("t1w", "3D", 1.0)]), &template);
        assert_eq!(r.member.as_deref(), Some("ants"));
        assert_eq!(r.rule.as_deref(), Some("to-template-syn"));
        // the fast goal: SynthMorph joint
        let r = routed(
            &f,
            &session(&[("t1w", "3D", 1.0)]),
            &[("task", json!("to_template")), ("goal", json!("fast"))],
        );
        assert_eq!(r.member.as_deref(), Some("synthmorph"));
        assert_eq!(r.rule.as_deref(), Some("to-template-synthmorph"));
        // a coarse T1w: SynthMorph joint
        let r = routed(&f, &session(&[("t1w", "3D", 1.5)]), &template);
        assert_eq!(r.member.as_deref(), Some("synthmorph"));
        // a post-contrast T1w to the template: a caveat
        let mut post = session(&[("t1w", "3D", 1.0)]);
        post.set("t1w.post_contrast", Val::Text("given".into()));
        let r = routed(&f, &post, &template);
        assert_eq!(r.member.as_deref(), Some("ants"));
        assert!(
            r.caveats.iter().any(|c| c.words.contains("post-contrast")),
            "{:?}",
            r.caveats
        );
        // the record a unit keeps
        let r = routed(
            &f,
            &session(&[("t1w", "3D", 1.0), ("flair", "3D", 1.0)]),
            &within,
        );
        let v = r.variant_doc();
        assert_eq!(v["variant"], "ants");
        assert_eq!(v["pipeline"], "registration-ants");
        assert_eq!(v["rule"], "within-session-fine-3d");
        assert_eq!(r.facts.get("task"), Val::Text("within_session".into()));
    }

    /// Each member's pack judges the member's own task switch into its knob
    /// and refuses what the family does not send it.
    #[test]
    fn the_members_packs_take_the_family_s_task() {
        let (_, members) = shipped();
        for (k, (d, p)) in &members {
            let p = p.as_ref().unwrap();
            if k == "robust" {
                assert_eq!(p.goals.switches["task"], vec![json!("across_visits")]);
                continue;
            }
            let resolved = d.resolve(&[]).unwrap();
            for task in ["within_session", "to_template"] {
                let mut facts = session(&[("t1w", "3D", 1.0), ("flair", "3D", 1.0)]);
                facts.set("task", Val::Text(task.into()));
                let person = Person {
                    user: "tester".into(),
                    choices: [("task".to_string(), json!(task))].into_iter().collect(),
                    given: Default::default(),
                    proposed: Default::default(),
                    let_run: false,
                };
                let j = nils_pipeline::pack::rules::judge(p, d, &resolved, &facts, &person);
                assert_eq!(
                    j.knobs["task"].value,
                    json!(task),
                    "{k} {task}: {:?}",
                    j.knobs["task"]
                );
                assert!(
                    j.knobs["task"].by.starts_with("rule:"),
                    "{k}: {}",
                    j.knobs["task"].by
                );
            }
        }
    }

    /// The arithmetic of every check of section 7, per member pack: a good
    /// value holds, a bad one does not, and the action is the spec's.
    #[test]
    fn the_registration_checks_hold_good_values_and_refuse_bad_ones() {
        let (_, members) = shipped();
        // metric, a good value, a bad value, the action
        let cases: &[(&str, f64, f64, &str)] = &[
            ("rigid_flip", 0.0, 1.0, "fail"),
            ("rigid_translation_mm", 12.0, 51.0, "flag"),
            ("rigid_rotation_deg", 4.0, 31.0, "flag"),
            ("folding_pct", 0.001, 0.02, "flag"),
            ("inverse_consistency_mm", 0.05, 0.2, "flag"),
        ];
        for (k, (_, p)) in &members {
            let p = p.as_ref().unwrap();
            for (metric, good, bad, action) in cases {
                if *metric == "folding_pct" && k == "robust" {
                    continue;
                }
                let checks: Vec<_> = p.checks.iter().filter(|c| c.metric == *metric).collect();
                assert!(!checks.is_empty(), "{k} has no check on {metric}");
                for c in &checks {
                    assert_eq!(c.action.name(), *action, "{k} {metric}");
                    assert!(c.holds(*good), "{k} {metric}: {good} should hold");
                    assert!(!c.holds(*bad), "{k} {metric}: {bad} should not hold");
                    assert_eq!(
                        c.source.as_deref(),
                        Some("policy"),
                        "{k} {metric}: policy, to be calibrated"
                    );
                }
            }
            // the determinant near 1: both bounds
            let det: Vec<_> = p
                .checks
                .iter()
                .filter(|c| c.metric == "rigid_determinant")
                .collect();
            assert_eq!(det.len(), 2, "{k}");
            assert!(det.iter().all(|c| c.holds(1.0) && c.holds(0.99)), "{k}");
            assert!(det.iter().any(|c| !c.holds(0.9)), "{k}");
            assert!(det.iter().any(|c| !c.holds(1.1)), "{k}");
            // the label Dice is a cohort check within the stratum
            let dice = p
                .checks
                .iter()
                .find(|c| c.metric == "alignment_dice")
                .unwrap();
            assert!(dice.cohort, "{k}");
            assert_eq!(dice.min_sessions, 30, "{k}");
            // SynthSeg's quality at 0.65 fails the FreeSurfer members
            if k != "ants" {
                let qc = p
                    .checks
                    .iter()
                    .find(|c| c.metric == "qc_fixed" || c.metric == "qc_min")
                    .unwrap();
                assert_eq!(qc.action.name(), "fail");
                assert!(qc.holds(0.65) && qc.holds(0.9) && !qc.holds(0.64), "{k}");
            }
        }
    }

    #[test]
    fn the_unit_key_names_a_session_or_a_person() {
        assert_eq!(super::unit_key(Some(7), Some("2024-01-02")), "7:2024-01-02");
        assert_eq!(super::unit_key(Some(7), None), "7");
        let route = super::Route {
            family: "registration".into(),
            digest: "d".into(),
            member: "ants".into(),
            pipeline: "registration-ants".into(),
            by_unit: [("7:2024-01-02".to_string(), ("fine".to_string(), None))]
                .into_iter()
                .collect(),
        };
        let (variant, fam) = route.record("7:2024-01-02").unwrap();
        assert_eq!(variant["variant"], "ants");
        assert_eq!(variant["rule"], "fine");
        assert_eq!(fam["name"], "registration");
        assert!(route.record("8").is_none());
    }

    /// A skipped unit prints its skip words, not the member a rule would
    /// have chosen.
    #[test]
    fn a_skipped_unit_is_listed_with_its_words() {
        let doc = json!({
            "family": "registration", "task": "within_session", "level": "session",
            "counts": {"units": 2, "ready": 0, "routed": 1, "skipped": 1, "cannot_tell": 0,
                       "with_caveats": 0, "by_member": {"ants": 1}, "skipped_by_words": []},
            "units": [
                {"unit": "1:2026-01-01", "status": "skipped", "member": "ants", "rule": "r1",
                 "words": "the scans are not brain scans"},
                {"unit": "2:2026-01-02", "status": "routed", "member": "ants", "rule": "r1", "words": null},
            ],
        });
        let words = super::preflight_words(&doc);
        assert!(
            words.contains("1:2026-01-01 skipped: the scans are not brain scans"),
            "{words}"
        );
        assert!(words.contains("2:2026-01-02 routed: ants (r1)"), "{words}");
    }

    /// A family whose stored member ids are gone from the catalog is
    /// refused with the words to add it again; one whose ids are there is
    /// not.
    #[test]
    fn a_family_with_a_member_gone_from_the_catalog_is_refused() {
        use nils_registry::{Backend, Home, InitOptions, Scheme};
        let dir = std::env::temp_dir().join(format!("nils-family-members-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let home = Home::new(&dir);
        home.keys(None).add("nils", b"a test passphrase").unwrap();
        let mut registry = home
            .init(&InitOptions {
                backend: Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: Scheme::DEFAULT,
                key: "nils".into(),
                display_length: 12,
                session_scheme: None,
            })
            .unwrap();
        super::add_folder(&mut registry, &repo()).unwrap_or_else(|_| panic!("add_folder"));
        let s = super::get(registry.store(), "registration")
            .unwrap()
            .expect("the family is kept");
        s.members_exist(registry.store()).unwrap();
        let mut gone = s.clone();
        gone.members.insert("ants".into(), 9999);
        let e = gone.members_exist(registry.store()).unwrap_err();
        assert!(e.contains("ants is no longer in the catalog"), "{e}");
        assert!(e.contains("nils pipeline add"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
