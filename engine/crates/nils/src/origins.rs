// SPDX-License-Identifier: AGPL-3.0-only

//! Where each axis label of a scan came from (UX round 2, 2026-11-06).
//!
//! A person reading a visit's scans saw a spine label on an axial brain
//! scan. The body part had been decided by their own campaign answer, a
//! decision in force, but no sort had run since, so the axis still held the
//! rules' older value, and nothing on the page said where a label came
//! from, who or what set it, or when.
//!
//! - **The origins** (`GET /api/origins?stack=ID | session=ID |
//!   studies=A,B`, `nils origins`): the axes of one scan or of one visit's
//!   scans (a session the cache holds, or the studies of a visit it does
//!   not), each axis with the value the sort holds and where that value came
//!   from: the rules (the pack and its version, the rule set and rule that
//!   decided where the evidence names one, and when the sort ran), a model
//!   (its name and version, the run and its probability, where its answer
//!   was committed as a decision) or a decision (a person's or an agent's:
//!   who, when, and the campaign item or review item it closed). Beside it,
//!   the decision in force on the axis by the sort's own precedence (a
//!   person over an agent over a model, then the narrowest scope); where it
//!   and the held value differ the axis is `unsorted` ("decided neck;
//!   sorted before the decision"), and where the held value was a decision
//!   no longer in force it is `withdrawn`. A fixed number of queries per
//!   visit, never one per scan.
//! - **The re-sort** (`POST /api/origins/resort`, `nils origins --resort`):
//!   the target's scans sorted again, by the pack, version and overlay each
//!   was sorted with, as a commit's re-sort is
//!   ([`crate::review_questions::resort_stacks`]); the answer carries the
//!   origins as they are after it.
//!
//! What a caller is shown: a stack of a sample sealed now is withheld,
//! counted only (record 48); the words a rule matched are text a stack
//! carried and are shown at detail quasi only, as the explain door does; a
//! decision a campaign not yet closed wrote names neither its value, nor who,
//! nor its item, to a caller who does not read every answer of that
//! campaign (record 45 and 48: rating is blind until the close). The
//! campaign answers behind a decision are never in this answer.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nils_registry::Registry;
use nils_registry::home::Home;
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply, json_body};
use crate::{Exit, fail, usage};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 2] = ["GET /api/origins", "POST /api/origins/resort"];

/// Each door's policy row, as `serve::policy` reads them.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/origins",
        false,
        false,
        "bounded",
        "one scan or one visit's scans",
        "Reading where a scan's labels came from",
        "Read where a scan's labels came from",
    ),
    (
        "POST /api/origins/resort",
        true,
        false,
        "bounded",
        "one scan or one visit's scans",
        "Sorting a scan or a visit again",
        "Sorted a scan or a visit again",
    ),
];

/// The most scans one answer covers: a visit the viewer reads whole.
pub(crate) const MOST: usize = 5_000;

/// What each door needs, read by `serve::door` before its own table. The
/// origins are read wherever the scans are, and by a reviewer; a re-sort is
/// a sort, the classify verb's grant.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    Some(match (method, segs) {
        ("GET", ["api", "origins"]) => (Need::AnyOf(&["data:see", "review:see"]), Detail::Plain),
        ("POST", ["api", "origins", "resort"]) => (Need::One("pipelines:work"), Detail::Plain),
        _ => return None,
    })
}

/// What an answer is of: one scan, a session the cache holds, or the
/// studies of a visit it does not hold, as the scans door names a visit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Target {
    Stack(i64),
    Session(i64),
    Studies(Vec<i64>),
}

impl Target {
    /// From the door's query or a body's fields: exactly one of `stack`,
    /// `session` and `studies` (a comma list, or an array in a body).
    fn named(
        stack: Option<String>,
        session: Option<String>,
        studies: Option<String>,
    ) -> Result<Target, String> {
        let id = |what: &str, v: &str| {
            v.trim()
                .parse::<i64>()
                .map_err(|_| format!("{what} is an id, as the scans door gave it"))
        };
        match (stack, session, studies) {
            (Some(s), None, None) => Ok(Target::Stack(id("stack", &s)?)),
            (None, Some(s), None) => Ok(Target::Session(id("session", &s)?)),
            (None, None, Some(list)) => {
                let ids = list
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| id("studies", s))
                    .collect::<Result<Vec<_>, _>>()?;
                if ids.is_empty() || ids.len() > crate::viewer::VISIT_STUDIES_MOST {
                    return Err(format!(
                        "studies names 1 to {} studies",
                        crate::viewer::VISIT_STUDIES_MOST
                    ));
                }
                Ok(Target::Studies(ids))
            }
            (None, None, None) => {
                Err("name one scan (stack), one session or the studies of a visit".to_string())
            }
            _ => Err("stack, session or studies: one, named one way".to_string()),
        }
    }

    pub(crate) fn of_query(query: &HashMap<String, String>) -> Result<Target, String> {
        let get = |k: &str| query.get(k).filter(|v| !v.trim().is_empty()).cloned();
        Target::named(get("stack"), get("session"), get("studies"))
    }

    fn of_body(body: &Value) -> Result<Target, String> {
        let get = |k: &str| match &body[k] {
            Value::Number(n) => Some(n.to_string()),
            Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
            Value::Array(a) => Some(
                a.iter()
                    .map(|v| v.as_i64().map_or_else(|| v.to_string(), |n| n.to_string()))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            _ => None,
        };
        Target::named(get("stack"), get("session"), get("studies"))
    }

    fn doc(&self) -> Value {
        match self {
            Target::Stack(s) => json!({"kind": "stack", "id": s}),
            Target::Session(s) => json!({"kind": "session", "id": s}),
            Target::Studies(ids) => json!({"kind": "studies", "ids": ids}),
        }
    }
}

/// What the caller is shown, from its grants and who it is.
pub(crate) struct Shown<'a> {
    /// Detail quasi or above: the words a rule matched.
    pub(crate) quasi: bool,
    /// The caller, for the sealed samples and the campaigns it reads every
    /// answer of; none at the keyboard, which reads everything.
    pub(crate) caller: Option<&'a Caller>,
}

fn store_err(e: StoreError) -> String {
    e.to_string()
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn quoted(ids: &[i64]) -> String {
    ids.iter()
        .map(|i| format!("'{i}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One stack of the target: its series, its subject and the machine that
/// made it, which a decision may name instead of the stack.
#[derive(Debug, Clone)]
struct Of {
    id: i64,
    series: i64,
    subject: i64,
    /// `manufacturer=`, `model=` and `station=` with the value in lower
    /// case, as the sort reads an origin decision's reference.
    origin: Vec<String>,
}

/// The target's stacks, each with what a decision may name it by.
fn stacks_of(store: &mut Store, t: &Target) -> Result<Vec<Of>, String> {
    let filter = match t {
        Target::Stack(s) => format!("st.id = {s}"),
        Target::Session(id) => format!(
            "se.study_id IN (SELECT scs.study_id FROM {} scs WHERE scs.session_id = {id}) \
             AND se.subject_id = (SELECT sc.subject_id FROM {} sc WHERE sc.id = {id})",
            store.qualified("session_cache_study"),
            store.qualified("session_cache")
        ),
        Target::Studies(ids) => format!(
            "se.study_id IN ({}) \
             AND se.subject_id = (SELECT vs.subject_id FROM {} vs WHERE vs.id = se.study_id)",
            list(ids),
            store.qualified("study")
        ),
    };
    let sql = format!(
        "SELECT st.id, st.series_id, se.subject_id, f.manufacturer, f.manufacturer_model_name, f.station_name \
         FROM {} st JOIN {} se ON se.id = st.series_id \
         LEFT JOIN {} f ON f.stack_id = st.id \
         WHERE {filter} ORDER BY st.id",
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("stack_fingerprint"),
    );
    let mut out: Vec<Of> = Vec::new();
    for r in store.query(&sql, &[]).map_err(store_err)? {
        let id = r.int(0).map_err(store_err)?;
        if out.last().is_some_and(|o| o.id == id) {
            continue;
        }
        let mut origin = Vec::new();
        for (i, name) in [(3, "manufacturer"), (4, "model"), (5, "station")] {
            if let Some(v) = r.opt_text(i).map_err(store_err)?
                && !v.trim().is_empty()
            {
                origin.push(format!("{name}={}", v.to_lowercase()));
            }
        }
        out.push(Of {
            id,
            series: r.int(1).map_err(store_err)?,
            subject: r.int(2).map_err(store_err)?,
            origin,
        });
        if out.len() > MOST {
            return Err(format!(
                "the target holds more than {MOST} scans; ask for one visit or one scan"
            ));
        }
    }
    Ok(out)
}

/// A decision in force, as the answer reads it.
#[derive(Debug, Clone)]
struct Decision {
    id: i64,
    scope: String,
    reference: String,
    axis: String,
    value: Option<String>,
    actor: String,
    kind: String,
    version: Option<String>,
    model: Option<i64>,
    campaign: Option<i64>,
    committed_by: Option<String>,
    at: Option<String>,
}

/// The sort's rank of an author: a person over an agent over a model.
fn rank(kind: &str) -> u8 {
    nils_registry::review::rank(kind)
}

/// A stack's sort: the pack, its version, the overlay, the job and when it
/// ran.
type SortedBy = (String, String, Option<String>, Option<i64>, Option<String>);

/// An axis as the sort holds it: its values, the confidence and the tier.
type Held = (Vec<String>, f64, String);

/// Everything the answer reads of the target's stacks, a fixed number of
/// queries for any number of stacks (in chunks of 500).
#[derive(Default)]
struct Read {
    /// stack -> its sort
    sorted: BTreeMap<i64, SortedBy>,
    /// stack -> axis -> what the sort holds
    axes: BTreeMap<i64, BTreeMap<String, Held>>,
    /// stack -> axis -> evidence rows
    evidence: BTreeMap<i64, BTreeMap<String, Vec<Value>>>,
    /// (group item, stack) -> the member's evidence
    members: BTreeMap<(i64, i64), Value>,
    decisions: Vec<Decision>,
    /// decision -> (campaign item, position)
    items: BTreeMap<i64, (i64, i64)>,
    /// campaign -> (name, status)
    campaigns: BTreeMap<i64, (String, String)>,
    /// decision -> the review item it closed (id, kind)
    reviews: BTreeMap<i64, (i64, String)>,
    /// model -> (name, version)
    models: BTreeMap<i64, (String, String)>,
    /// (pack, version) -> the axes its rules write, so a decision on an
    /// axis the sort never writes is not read as one it has not applied
    pack_axes: BTreeMap<(String, String), BTreeSet<String>>,
}

impl Read {
    fn of(store: &mut Store, stacks: &[Of]) -> Result<Read, String> {
        let mut read = Read::default();
        let ids: Vec<i64> = stacks.iter().map(|s| s.id).collect();
        let d = store.dialect();
        let member_t = nils_registry::schema::table("review_member");
        let member_evidence = d.text_of_qualified(
            Some("m"),
            member_t.column("evidence").expect("a review member column"),
        );
        let in_force = "withdrawn_at IS NULL AND (staged_at IS NULL OR committed_at IS NOT NULL)";
        let mut groups: BTreeSet<i64> = BTreeSet::new();
        for chunk in ids.chunks(500) {
            let these = list(chunk);
            let sql = format!(
                "SELECT c.stack_id, c.pack, c.pack_version, c.overlay, c.job_id, j.finished_at, j.started_at \
                 FROM {} c LEFT JOIN {} j ON j.id = c.job_id WHERE c.stack_id IN ({these})",
                store.qualified("classification"),
                store.qualified("job"),
            );
            for r in store.query(&sql, &[]).map_err(store_err)? {
                let at = match r.opt_text(5).map_err(store_err)? {
                    Some(t) => Some(t.to_string()),
                    None => r.opt_text(6).map_err(store_err)?.map(str::to_string),
                };
                read.sorted.insert(
                    r.int(0).map_err(store_err)?,
                    (
                        r.text(1).map_err(store_err)?.to_string(),
                        r.text(2).map_err(store_err)?.to_string(),
                        r.opt_text(3).map_err(store_err)?.map(str::to_string),
                        r.opt_int(4).map_err(store_err)?,
                        at,
                    ),
                );
            }
            let sql = format!(
                "SELECT stack_id, axis, value, confidence, tier FROM {} WHERE stack_id IN ({these}) \
                 ORDER BY stack_id, axis, value",
                store.qualified("classification_axis")
            );
            for r in store.query(&sql, &[]).map_err(store_err)? {
                let entry = read
                    .axes
                    .entry(r.int(0).map_err(store_err)?)
                    .or_default()
                    .entry(r.text(1).map_err(store_err)?.to_string())
                    .or_insert_with(|| (Vec::new(), 0.0, String::new()));
                if let Some(v) = r.opt_text(2).map_err(store_err)?
                    && !v.is_empty()
                {
                    entry.0.push(v.to_string());
                }
                entry.1 = r.double(3).map_err(store_err)?;
                entry.2 = r.text(4).map_err(store_err)?.to_string();
            }
            let sql = format!(
                "SELECT stack_id, axis, value, tier, confidence, rule_set, rule, source, matched, \
                        pass, reference, author, author_kind, model_id, campaign_id \
                 FROM {} WHERE stack_id IN ({these}) ORDER BY stack_id, axis, id",
                store.qualified("classification_evidence")
            );
            for r in store.query(&sql, &[]).map_err(store_err)? {
                let tier = r.text(3).map_err(store_err)?.to_string();
                let row = json!({
                    "value": r.text(2).map_err(store_err)?,
                    "tier": tier,
                    "basis": nils_pack::basis_of(&tier),
                    "confidence": r.double(4).map_err(store_err)?,
                    "rule_set": r.text(5).map_err(store_err)?,
                    "rule": r.text(6).map_err(store_err)?,
                    "source": r.text(7).map_err(store_err)?,
                    "matched": r.opt_text(8).map_err(store_err)?,
                    "pass": r.opt_text(9).map_err(store_err)?,
                    "reference": r.opt_text(10).map_err(store_err)?,
                    "author": r.opt_text(11).map_err(store_err)?,
                    "author_kind": r.opt_text(12).map_err(store_err)?,
                    "model_id": r.opt_int(13).map_err(store_err)?,
                    "campaign_id": r.opt_int(14).map_err(store_err)?,
                });
                if let Some(m) = row["model_id"].as_i64() {
                    read.models.insert(m, (String::new(), String::new()));
                }
                read.evidence
                    .entry(r.int(0).map_err(store_err)?)
                    .or_default()
                    .entry(r.text(1).map_err(store_err)?.to_string())
                    .or_default()
                    .push(row);
            }
            // the grouped questions that hold these stacks and a decision
            // in force, with each member's own evidence (a model's
            // probabilities and its run)
            let sql = format!(
                "SELECT m.item_id, m.stack_id, {member_evidence} FROM {} m \
                 WHERE m.stack_id IN ({these}) AND EXISTS (SELECT 1 FROM {} dd \
                 WHERE dd.scope = 'group' AND dd.ref = CAST(m.item_id AS TEXT) \
                 AND dd.withdrawn_at IS NULL AND (dd.staged_at IS NULL OR dd.committed_at IS NOT NULL))",
                store.qualified("review_member"),
                store.qualified("decision"),
            );
            for r in store.query(&sql, &[]).map_err(store_err)? {
                let item = r.int(0).map_err(store_err)?;
                groups.insert(item);
                let evidence: Value = r
                    .opt_text(2)
                    .map_err(store_err)?
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or(Value::Null);
                read.members
                    .insert((item, r.int(1).map_err(store_err)?), evidence);
            }
        }
        // the axes each pack the target was sorted with writes
        let packs: BTreeSet<(String, String)> = read
            .sorted
            .values()
            .map(|(p, v, ..)| (p.clone(), v.clone()))
            .collect();
        for (pack, version) in packs {
            let sql = format!(
                "SELECT DISTINCT axis FROM {} WHERE pack = {} AND pack_version = {}",
                store.qualified("classification_voter"),
                d.param(1, nils_registry::schema::Type::Text),
                d.param(2, nils_registry::schema::Type::Text),
            );
            let rows = store
                .query(
                    &sql,
                    &[
                        nils_registry::Param::from(pack.as_str()),
                        nils_registry::Param::from(version.as_str()),
                    ],
                )
                .map_err(store_err)?;
            let mut axes = BTreeSet::new();
            for r in rows {
                axes.insert(r.text(0).map_err(store_err)?.to_string());
            }
            read.pack_axes.insert((pack, version), axes);
        }
        // the decisions in force that name a stack, its series, its subject,
        // a grouped question it is a member of, or a machine
        let series: Vec<i64> = stacks
            .iter()
            .map(|s| s.series)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let subjects: Vec<i64> = stacks
            .iter()
            .map(|s| s.subject)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let origins: BTreeSet<&str> = stacks
            .iter()
            .flat_map(|s| s.origin.iter().map(String::as_str))
            .collect();
        let groups: Vec<i64> = groups.into_iter().collect();
        let mut wheres: Vec<String> = vec!["scope = 'origin'".to_string()];
        for chunk in ids.chunks(500) {
            wheres.push(format!("(scope = 'stack' AND ref IN ({}))", quoted(chunk)));
        }
        for chunk in series.chunks(500) {
            wheres.push(format!("(scope = 'series' AND ref IN ({}))", quoted(chunk)));
        }
        for chunk in subjects.chunks(500) {
            wheres.push(format!(
                "(scope = 'subject' AND ref IN ({}))",
                quoted(chunk)
            ));
        }
        for chunk in groups.chunks(500) {
            wheres.push(format!("(scope = 'group' AND ref IN ({}))", quoted(chunk)));
        }
        let sql = format!(
            "SELECT id, scope, ref, axis, value, actor, author_kind, author_version, model_id, campaign_id, \
                    committed_by, committed_at, decided_at \
             FROM {} WHERE {in_force} AND ({}) ORDER BY id",
            store.qualified("decision"),
            wheres.join(" OR ")
        );
        for r in store.query(&sql, &[]).map_err(store_err)? {
            let scope = r.text(1).map_err(store_err)?.to_string();
            let reference = r.text(2).map_err(store_err)?.to_string();
            // a machine's decision reaches only the machines the target holds
            if scope == "origin" && !origins.contains(reference.to_lowercase().as_str()) {
                continue;
            }
            let at = match r.opt_text(11).map_err(store_err)? {
                Some(t) => Some(t.to_string()),
                None => r.opt_text(12).map_err(store_err)?.map(str::to_string),
            };
            let decision = Decision {
                id: r.int(0).map_err(store_err)?,
                scope,
                reference,
                axis: r.text(3).map_err(store_err)?.to_string(),
                value: r.opt_text(4).map_err(store_err)?.map(str::to_string),
                actor: r.text(5).map_err(store_err)?.to_string(),
                kind: r
                    .opt_text(6)
                    .map_err(store_err)?
                    .unwrap_or("person")
                    .to_string(),
                version: r.opt_text(7).map_err(store_err)?.map(str::to_string),
                model: r.opt_int(8).map_err(store_err)?,
                campaign: r.opt_int(9).map_err(store_err)?,
                committed_by: r.opt_text(10).map_err(store_err)?.map(str::to_string),
                at,
            };
            if let Some(m) = decision.model {
                read.models.insert(m, (String::new(), String::new()));
            }
            read.decisions.push(decision);
        }
        // what each decision closed: a campaign's item, a review item
        let decided: Vec<i64> = read.decisions.iter().map(|d| d.id).collect();
        for chunk in decided.chunks(500) {
            let sql = format!(
                "SELECT decision_id, id, position, campaign_id FROM {} WHERE decision_id IN ({}) ORDER BY id",
                store.qualified("campaign_item"),
                list(chunk)
            );
            for r in store.query(&sql, &[]).map_err(store_err)? {
                read.items.insert(
                    r.int(0).map_err(store_err)?,
                    (r.int(1).map_err(store_err)?, r.int(2).map_err(store_err)?),
                );
                read.campaigns
                    .insert(r.int(3).map_err(store_err)?, (String::new(), String::new()));
            }
            let sql = format!(
                "SELECT decision_id, id, kind FROM {} WHERE decision_id IN ({}) ORDER BY id",
                store.qualified("review_item"),
                list(chunk)
            );
            for r in store.query(&sql, &[]).map_err(store_err)? {
                read.reviews.entry(r.int(0).map_err(store_err)?).or_insert((
                    r.int(1).map_err(store_err)?,
                    r.text(2).map_err(store_err)?.to_string(),
                ));
            }
        }
        for d in &read.decisions {
            if let Some(c) = d.campaign {
                read.campaigns.insert(c, (String::new(), String::new()));
            }
        }
        // a grouped decision's own question is its review item
        for d in &read.decisions {
            if d.scope == "group"
                && let Ok(item) = d.reference.parse::<i64>()
            {
                read.reviews
                    .entry(d.id)
                    .or_insert((item, format!("{}:model", d.axis)));
            }
        }
        let campaigns: Vec<i64> = read.campaigns.keys().copied().collect();
        if !campaigns.is_empty() {
            let sql = format!(
                "SELECT id, name, status FROM {} WHERE id IN ({})",
                store.qualified("campaign"),
                list(&campaigns)
            );
            for r in store.query(&sql, &[]).map_err(store_err)? {
                read.campaigns.insert(
                    r.int(0).map_err(store_err)?,
                    (
                        r.text(1).map_err(store_err)?.to_string(),
                        r.text(2).map_err(store_err)?.to_string(),
                    ),
                );
            }
        }
        let models: Vec<i64> = read.models.keys().copied().collect();
        if !models.is_empty() {
            let sql = format!(
                "SELECT id, name, version FROM {} WHERE id IN ({})",
                store.qualified("model"),
                list(&models)
            );
            for r in store.query(&sql, &[]).map_err(store_err)? {
                read.models.insert(
                    r.int(0).map_err(store_err)?,
                    (
                        r.text(1).map_err(store_err)?.to_string(),
                        r.text(2).map_err(store_err)?.to_string(),
                    ),
                );
            }
        }
        Ok(read)
    }

    /// The decision in force on one axis of one stack, by the sort's own
    /// precedence (`nils_classify`'s `Decisions::load` and `for_stack`):
    /// within a scope the highest rank, then the later; across scopes the
    /// highest rank, then the narrowest (the stack, a grouped question it
    /// is a member of, its series, its subject, its machine). A machine is
    /// read by the first of its manufacturer, model and station that a
    /// decision names, the later of those on the same name.
    fn in_force(&self, of: &Of, axis: &str) -> Option<&Decision> {
        let on = |scope: &'static str| {
            self.decisions
                .iter()
                .filter(move |d| d.axis == axis && d.scope == scope)
        };
        let stack = of.id.to_string();
        let series = of.series.to_string();
        let subject = of.subject.to_string();
        let mut candidates: Vec<(u8, &Decision)> = Vec::new();
        if let Some(d) = best(on("stack").filter(|d| d.reference == stack)) {
            candidates.push((4, d));
        }
        if let Some(d) = best(on("group").filter(|d| {
            d.reference
                .parse::<i64>()
                .is_ok_and(|item| self.members.contains_key(&(item, of.id)))
        })) {
            candidates.push((3, d));
        }
        if let Some(d) = best(on("series").filter(|d| d.reference == series)) {
            candidates.push((2, d));
        }
        if let Some(d) = best(on("subject").filter(|d| d.reference == subject)) {
            candidates.push((1, d));
        }
        if let Some(d) = of.origin.iter().find_map(|o| {
            on("origin")
                .filter(|d| d.reference.to_lowercase() == *o)
                .max_by_key(|d| d.id)
        }) {
            candidates.push((0, d));
        }
        candidates
            .into_iter()
            .max_by_key(|(narrow, d)| (rank(&d.kind), *narrow))
            .map(|(_, d)| d)
    }
}

/// Of a scope's decisions on one key, the one the sort keeps: the highest
/// rank, then the later.
fn best<'a>(it: impl Iterator<Item = &'a Decision>) -> Option<&'a Decision> {
    it.max_by_key(|d| (rank(&d.kind), d.id))
}

/// A decision's values as the sort stores them: one per comma, none for
/// nothing.
fn values_of(value: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect();
    v.sort();
    v
}

/// What a value is called in a line: the values joined, or `nothing`.
fn said(values: &[String]) -> String {
    if values.is_empty() {
        "nothing".to_string()
    } else {
        values.join(", ")
    }
}

/// Whether the caller reads every answer of a campaign (record 45): it is
/// closed, or the caller holds review:work or adjudicates it. The keyboard
/// reads every one.
fn reads_campaign(
    store: &mut Store,
    shown: &Shown,
    campaign: i64,
    status: &str,
    cache: &mut BTreeMap<i64, bool>,
) -> bool {
    if status == "closed" {
        return true;
    }
    let Some(caller) = shown.caller else {
        return true;
    };
    if let Some(known) = cache.get(&campaign) {
        return *known;
    }
    let sees = nils_registry::campaign::get(store, campaign)
        .ok()
        .flatten()
        .and_then(|c| crate::campaigns::sees_all(store, caller, &caller.principal, &c).ok())
        .unwrap_or(false);
    cache.insert(campaign, sees);
    sees
}

/// The answer: the target's stacks (those of a sealed sample withheld from a
/// caller who does not read them), each with its axes and their origins.
pub(crate) fn answer(store: &mut Store, t: &Target, shown: &Shown) -> Result<Value, String> {
    let all = stacks_of(store, t)?;
    if all.is_empty() && matches!(t, Target::Stack(_)) {
        return Err(match t {
            Target::Stack(s) => format!("no stack {s}"),
            _ => unreachable!(),
        });
    }
    let ids: Vec<i64> = all.iter().map(|s| s.id).collect();
    let sealed = match shown.caller {
        Some(c) => crate::sealed::withheld(store, &c.access, &ids).map_err(store_err)?,
        None => BTreeSet::new(),
    };
    let stacks: Vec<Of> = all
        .into_iter()
        .filter(|s| !sealed.contains(&s.id))
        .collect();
    let read = Read::of(store, &stacks)?;
    let mut cache: BTreeMap<i64, bool> = BTreeMap::new();
    let mut reads =
        |campaign: i64, status: &str| reads_campaign(store, shown, campaign, status, &mut cache);
    let mut doc = build(&read, &stacks, shown.quasi, &mut reads);
    doc["of"] = t.doc();
    if !sealed.is_empty() {
        doc["withheld"] = json!({
            "stacks": sealed.len(),
            "why": crate::sealed::refusal("a stack"),
        });
    }
    Ok(doc)
}

/// The answer's document from what was read: `reads` says whether the
/// caller reads every answer of a campaign not closed (its id and status).
fn build(
    read: &Read,
    stacks: &[Of],
    quasi: bool,
    reads: &mut dyn FnMut(i64, &str) -> bool,
) -> Value {
    let mut out: Vec<Value> = Vec::new();
    let mut unsorted_stacks = 0usize;
    for of in stacks {
        let sorted = read.sorted.get(&of.id);
        let empty = BTreeMap::new();
        let axes = read.axes.get(&of.id).unwrap_or(&empty);
        // every axis the sort holds, and every one a decision names that it
        // does not
        let mut names: BTreeSet<String> = axes.keys().cloned().collect();
        let writes = sorted.and_then(|(p, v, ..)| read.pack_axes.get(&(p.clone(), v.clone())));
        for d in &read.decisions {
            if writes.is_some_and(|w| w.contains(&d.axis))
                && read.in_force(of, &d.axis).is_some_and(|x| x.id == d.id)
            {
                names.insert(d.axis.clone());
            }
        }
        let no_rows = Vec::new();
        let mut rows: Vec<Value> = Vec::new();
        let mut unsorted = false;
        for axis in &names {
            let (held, confidence, tier) =
                axes.get(axis)
                    .cloned()
                    .unwrap_or((Vec::new(), 0.0, String::new()));
            let mut held_sorted = held.clone();
            held_sorted.sort();
            let evidence = read
                .evidence
                .get(&of.id)
                .and_then(|e| e.get(axis))
                .unwrap_or(&no_rows);
            // the row a decision wrote where the sort applied one
            let authored = evidence
                .iter()
                .find(|e| e["tier"] == "decision" && e["author_kind"].is_string());
            let rules: Vec<Value> = evidence
                .iter()
                .filter(|e| e["author_kind"].is_null() && e["tier"] != "decision")
                .filter(|e| {
                    held.is_empty()
                        || e["value"]
                            .as_str()
                            .is_some_and(|v| held.iter().any(|h| h == v))
                })
                .map(|e| {
                    let mut r = json!({
                        "rule_set": e["rule_set"], "rule": e["rule"], "source": e["source"],
                        "value": e["value"], "tier": e["tier"], "basis": e["basis"],
                        "confidence": e["confidence"], "pass": e["pass"],
                    });
                    if quasi {
                        r["matched"] = e["matched"].clone();
                    }
                    r
                })
                .collect();
            let model_named = |id: Option<i64>| {
                id.map(|m| {
                    let (name, version) = read.models.get(&m).cloned().unwrap_or_default();
                    json!({"id": m, "name": name, "version": version,
                           "label": if name.is_empty() { format!("model {m}") } else { format!("{name}@{version}") }})
                })
            };
            let (origin, from) = if tier == "decision" {
                match authored {
                    Some(e) if e["author_kind"] == "model" => (
                        "model",
                        json!({"kind": "model", "by": e["author"], "version": e["matched"],
                               "model": model_named(e["model_id"].as_i64()),
                               "campaign": e["campaign_id"], "scope": e["rule"]}),
                    ),
                    Some(e) => (
                        "decision",
                        json!({"kind": e["author_kind"], "by": e["author"],
                               "campaign": e["campaign_id"], "scope": e["rule"]}),
                    ),
                    None => ("decision", json!({"kind": "decision"})),
                }
            } else if held.is_empty() && rules.is_empty() {
                ("none", Value::Null)
            } else {
                ("rules", Value::Null)
            };
            // the decision in force, and whether the held value is it
            let decided = read.in_force(of, axis).map(|d| {
                let mut v = json!({
                    "id": d.id, "kind": d.kind, "scope": d.scope, "at": d.at,
                    "value": d.value.clone().unwrap_or_default(),
                    "values": values_of(d.value.as_deref()),
                    "by": d.actor,
                });
                if d.committed_by.as_deref().is_some_and(|c| c != d.actor) {
                    v["committed_by"] = json!(d.committed_by);
                }
                if d.kind == "model" {
                    let mut m = model_named(d.model).unwrap_or(Value::Null);
                    if m.is_null() {
                        m = json!({"label": d.version});
                    }
                    // a grouped proposal's member says its probability and run
                    if d.scope == "group"
                        && let Ok(item) = d.reference.parse::<i64>()
                        && let Some(ev) = read.members.get(&(item, of.id))
                    {
                        m["p"] = ev["confidence"].clone();
                        m["run"] = ev["run_id"].clone();
                    }
                    v["model"] = m;
                }
                if let Some(c) = d.campaign {
                    let (name, status) = read.campaigns.get(&c).cloned().unwrap_or_default();
                    let mut cv = json!({"id": c, "name": name, "status": status});
                    if let Some((item, position)) = read.items.get(&d.id) {
                        cv["item"] = json!(item);
                        cv["position"] = json!(position);
                    }
                    v["campaign"] = cv;
                }
                if let Some((item, kind)) = read.reviews.get(&d.id) {
                    v["review_item"] = json!({"id": item, "kind": kind});
                }
                (d, v)
            });
            let (state, words, decided) = match decided {
                Some((d, v)) => {
                    let blind = d.campaign.is_some_and(|c| {
                        let status = read
                            .campaigns
                            .get(&c)
                            .map(|x| x.1.clone())
                            .unwrap_or_default();
                        !reads(c, &status)
                    });
                    if blind {
                        let c = d.campaign.unwrap_or_default();
                        (
                            "withheld",
                            format!(
                                "a campaign not closed yet decided it; its answers are blind until it closes (campaign {c})"
                            ),
                            json!({"withheld": true, "campaign": {"id": c, "status": v["campaign"]["status"]}}),
                        )
                    } else {
                        let want = values_of(d.value.as_deref());
                        if want != held_sorted {
                            unsorted = true;
                            (
                                "unsorted",
                                format!("decided {}; sorted before the decision", said(&want)),
                                v,
                            )
                        } else {
                            ("current", String::new(), v)
                        }
                    }
                }
                None if tier == "decision" => {
                    unsorted = true;
                    (
                        "withdrawn",
                        format!(
                            "{} was decided, and the decision is no longer in force; sorted before it was withdrawn",
                            said(&held_sorted)
                        ),
                        Value::Null,
                    )
                }
                None => ("current", String::new(), Value::Null),
            };
            let line = match origin {
                "rules" => {
                    let rule = rules.first().map(|r| {
                        let set = r["rule_set"].as_str().unwrap_or_default();
                        let name = r["rule"].as_str().unwrap_or_default();
                        if set.is_empty() {
                            name.to_string()
                        } else {
                            format!("{set}/{name}")
                        }
                    });
                    format!(
                        "rules{}{}",
                        sorted.map_or(String::new(), |s| format!(" {}@{}", s.0, s.1)),
                        rule.map_or(String::new(), |r| format!(", {r}"))
                    )
                }
                "model" => format!(
                    "model {}",
                    from["model"]["label"].as_str().unwrap_or("unnamed")
                ),
                "decision" => format!("decided by {}", from["by"].as_str().unwrap_or("someone")),
                _ => "nothing answered it".to_string(),
            };
            let mut row = json!({
                "axis": axis,
                "values": held,
                "confidence": confidence,
                "tier": tier,
                "origin": origin,
                "line": line,
                "state": state,
                "words": if words.is_empty() { Value::Null } else { json!(words) },
                "rules": rules,
                "decided": decided,
            });
            if !from.is_null() {
                row["from"] = from;
            }
            rows.push(row);
        }
        unsorted_stacks += usize::from(unsorted);
        out.push(json!({
            "stack": of.id,
            "sorted": sorted.map(|(pack, version, overlay, job, at)| json!({
                "pack": pack, "version": version, "overlay": overlay, "job": job, "at": at,
            })),
            "unsorted": unsorted,
            "axes": rows,
        }));
    }
    json!({
        "count": out.len(),
        "unsorted": unsorted_stacks,
        "stacks": out,
    })
}

/// Sort the target's scans again, those of a sealed sample left out for a
/// caller who does not read them, by the pack each was sorted with.
pub(crate) fn resort(
    registry: &mut Registry,
    pack_dir: Option<&std::path::Path>,
    t: &Target,
    shown: &Shown,
    who: &str,
) -> Result<Value, String> {
    let all = stacks_of(registry.store(), t)?;
    if all.is_empty() && matches!(t, Target::Stack(_)) {
        return Err("no such stack".to_string());
    }
    let ids: Vec<i64> = all.iter().map(|s| s.id).collect();
    let sealed = match shown.caller {
        Some(c) => crate::sealed::withheld(registry.store(), &c.access, &ids).map_err(store_err)?,
        None => BTreeSet::new(),
    };
    let stacks: BTreeSet<i64> = ids.into_iter().filter(|s| !sealed.contains(s)).collect();
    let what = match t {
        Target::Stack(s) => format!("scan {s}"),
        Target::Session(s) => format!("session {s}"),
        Target::Studies(_) => "a visit".to_string(),
    };
    let done = if stacks.is_empty() {
        json!({"state": "done", "stacks": 0, "sorted": 0, "jobs": [], "words": "no scan to sort again"})
    } else {
        crate::review_questions::resort_stacks(
            registry,
            pack_dir,
            &format!("sort again by {who}: {what}"),
            &format!("sorted again from the viewer ({what})"),
            &stacks,
        )
    };
    let origins = answer(registry.store(), t, shown)?;
    Ok(json!({"resort": done, "origins": origins}))
}

/// The doors, or nothing when the path is not one of them. The grants were
/// checked by the table.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    method: &str,
    segs: &[&str],
    query: &HashMap<String, String>,
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let shown = Shown {
        quasi,
        caller: Some(caller),
    };
    Some(match (method, segs) {
        ("GET", ["api", "origins"]) => Target::of_query(query)
            .map_err(|e| Reply::error(400, e))
            .and_then(|t| {
                answer(registry.store(), &t, &shown).map_err(|e| {
                    let code = if e.starts_with("no stack") { 404 } else { 500 };
                    Reply::error(code, e)
                })
            })
            .map(Reply::ok),
        ("POST", ["api", "origins", "resort"]) => json_body(body).and_then(|b| {
            let t = Target::of_body(&b).map_err(|e| Reply::error(400, e))?;
            resort(
                registry,
                doors.pack_dir.as_deref(),
                &t,
                &shown,
                &caller.principal,
            )
            .map(Reply::ok)
            .map_err(|e| {
                let code = if e.starts_with("no such") { 404 } else { 500 };
                Reply::error(code, e)
            })
        }),
        _ => return None,
    })
}

/// `nils origins (--stack ID | --session ID | --studies A,B) [--resort]
/// [--json]`.
#[derive(Debug, clap::Args)]
pub(crate) struct OriginsArgs {
    /// One scan, by its stack id
    #[arg(long, value_name = "ID")]
    pub(crate) stack: Option<String>,
    /// One visit, by the id of its session
    #[arg(long, value_name = "ID")]
    pub(crate) session: Option<String>,
    /// One visit the session cache does not hold, by its studies' ids
    #[arg(long, value_name = "A,B")]
    pub(crate) studies: Option<String>,
    /// Sort the scans again first, by the pack each was sorted with
    #[arg(long)]
    pub(crate) resort: bool,
    /// Where the packs are, for a re-sort; $NILS_PACK_DIR, else `packs/` in the registry home
    #[arg(long, value_name = "DIR")]
    pub(crate) pack_dir: Option<std::path::PathBuf>,
    #[arg(long)]
    pub(crate) json: bool,
}

pub(crate) fn command(home: &Home, args: OriginsArgs) -> Result<(), Exit> {
    let t = Target::named(args.stack, args.session, args.studies).map_err(usage)?;
    let mut registry = crate::open(home)?;
    let shown = Shown {
        quasi: true,
        caller: None,
    };
    let (resort, v) = if args.resort {
        let pack_dir = crate::pack_dir(home, args.pack_dir).ok();
        let who = std::env::var("USER").unwrap_or_else(|_| "the keyboard".to_string());
        let mut done =
            resort(&mut registry, pack_dir.as_deref(), &t, &shown, &who).map_err(fail)?;
        (Some(done["resort"].take()), done["origins"].take())
    } else {
        (None, answer(registry.store(), &t, &shown).map_err(fail)?)
    };
    if args.json {
        let out = match resort {
            Some(r) => json!({"resort": r, "origins": v}),
            None => v,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| fail(e.to_string()))?
        );
        return Ok(());
    }
    if let Some(r) = resort {
        println!("sorted again: {}", r["words"].as_str().unwrap_or("not run"));
    }
    println!(
        "{} scan(s), {} with a label sorted before its decision",
        v["count"], v["unsorted"]
    );
    for s in v["stacks"].as_array().into_iter().flatten() {
        println!(
            "stack {}{}",
            s["stack"],
            s["sorted"]["pack"]
                .as_str()
                .map_or(String::new(), |p| format!(
                    " (sorted with {p}@{})",
                    s["sorted"]["version"].as_str().unwrap_or_default()
                ))
        );
        for a in s["axes"].as_array().into_iter().flatten() {
            let values: Vec<String> = a["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            println!(
                "  {}: {} ({}){}",
                a["axis"].as_str().unwrap_or_default(),
                said(&values),
                a["line"].as_str().unwrap_or_default(),
                a["words"]
                    .as_str()
                    .map_or(String::new(), |w| format!("; {w}"))
            );
        }
    }
    if let Some(w) = v["withheld"]["stacks"].as_u64() {
        println!("{w} stack(s) withheld");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_named_one_way() {
        assert_eq!(
            Target::named(Some("5".into()), None, None),
            Ok(Target::Stack(5))
        );
        assert_eq!(
            Target::named(None, Some("34".into()), None),
            Ok(Target::Session(34))
        );
        assert_eq!(
            Target::named(None, None, Some("1, 2".into())),
            Ok(Target::Studies(vec![1, 2]))
        );
        assert!(Target::named(None, None, None).is_err());
        assert!(Target::named(Some("5".into()), Some("3".into()), None).is_err());
        assert!(Target::named(Some("x".into()), None, None).is_err());
        assert_eq!(
            Target::of_body(&json!({"studies": [3, 4]})),
            Ok(Target::Studies(vec![3, 4]))
        );
        assert_eq!(Target::of_body(&json!({"stack": 9})), Ok(Target::Stack(9)));
    }

    fn decision(id: i64, scope: &str, reference: &str, value: &str, kind: &str) -> Decision {
        Decision {
            id,
            scope: scope.to_string(),
            reference: reference.to_string(),
            axis: "body_part".to_string(),
            value: Some(value.to_string()),
            actor: format!("{kind}-{id}"),
            kind: kind.to_string(),
            version: None,
            model: None,
            campaign: None,
            committed_by: None,
            at: Some("2026-11-05T10:00:00Z".to_string()),
        }
    }

    fn scan() -> Of {
        Of {
            id: 1,
            series: 10,
            subject: 100,
            origin: vec!["manufacturer=ge".to_string(), "station=mr1".to_string()],
        }
    }

    /// The sort's precedence: the highest rank, then the narrowest scope;
    /// within a scope the higher rank, then the later; a machine by the
    /// first of its names a decision names.
    #[test]
    fn the_decision_in_force_is_the_one_the_sort_applies() {
        let of = scan();
        let mut read = Read {
            decisions: vec![
                decision(1, "stack", "1", "brain", "agent"),
                decision(2, "subject", "100", "neck", "person"),
            ],
            ..Read::default()
        };
        assert_eq!(read.in_force(&of, "body_part").unwrap().id, 2);
        read.decisions
            .push(decision(3, "series", "10", "spine", "person"));
        assert_eq!(read.in_force(&of, "body_part").unwrap().id, 3);
        read.decisions
            .push(decision(4, "stack", "1", "head", "person"));
        read.decisions
            .push(decision(5, "stack", "1", "knee", "person"));
        assert_eq!(read.in_force(&of, "body_part").unwrap().id, 5);
        // another stack's, another axis's: not this one's
        read.decisions = vec![decision(6, "stack", "2", "brain", "person")];
        assert!(read.in_force(&of, "body_part").is_none());
        assert!(read.in_force(&of, "base").is_none());
        // a grouped question reaches its members alone
        read.decisions = vec![decision(7, "group", "40", "brain", "model")];
        assert!(read.in_force(&of, "body_part").is_none());
        read.members.insert((40, 1), json!({}));
        assert_eq!(read.in_force(&of, "body_part").unwrap().id, 7);
        // a machine: the manufacturer before the station, the later of two
        read.decisions = vec![
            decision(8, "origin", "station=MR1", "neck", "person"),
            decision(9, "origin", "manufacturer=GE", "brain", "person"),
            decision(10, "origin", "manufacturer=ge", "spine", "person"),
        ];
        assert_eq!(read.in_force(&of, "body_part").unwrap().id, 10);
    }

    fn row<'a>(doc: &'a Value, axis: &str) -> &'a Value {
        doc["stacks"][0]["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axis"] == axis)
            .unwrap()
    }

    fn held(read: &mut Read, values: &[&str], tier: &str) {
        read.axes.entry(1).or_default().insert(
            "body_part".to_string(),
            (
                values.iter().map(|v| v.to_string()).collect(),
                0.9,
                tier.to_string(),
            ),
        );
        read.sorted.insert(
            1,
            (
                "mri".to_string(),
                "1.4.0".to_string(),
                None,
                Some(3),
                Some("2026-11-01T08:00:00Z".to_string()),
            ),
        );
    }

    fn rule_row(read: &mut Read, value: &str) {
        read.evidence
            .entry(1)
            .or_default()
            .entry("body_part".to_string())
            .or_default()
            .push(json!({
                "value": value, "tier": "keyword", "basis": "name", "confidence": 0.9,
                "rule_set": "body_part", "rule": "spine_words", "source": "series_description",
                "matched": "spine", "pass": null, "reference": null,
                "author": null, "author_kind": null, "model_id": null, "campaign_id": null,
            }));
    }

    /// The rules' answer held while a campaign's decision says otherwise:
    /// the axis is unsorted, says so, and links the campaign's item.
    #[test]
    fn a_decision_the_sort_has_not_applied_is_said() {
        let mut read = Read::default();
        held(&mut read, &["spine"], "keyword");
        rule_row(&mut read, "spine");
        let mut d = decision(77, "stack", "1", "neck", "person");
        d.campaign = Some(6);
        read.decisions = vec![d];
        read.items.insert(77, (412, 3));
        read.campaigns
            .insert(6, ("curate".to_string(), "closed".to_string()));
        let doc = build(&read, &[scan()], false, &mut |_, _| true);
        let a = row(&doc, "body_part");
        assert_eq!(a["origin"], "rules", "{a}");
        assert_eq!(a["state"], "unsorted", "{a}");
        assert_eq!(a["words"], "decided neck; sorted before the decision");
        assert_eq!(a["decided"]["campaign"]["item"], 412, "{a}");
        assert_eq!(a["decided"]["campaign"]["name"], "curate", "{a}");
        assert_eq!(a["decided"]["by"], "person-77");
        assert_eq!(a["rules"][0]["rule"], "spine_words", "{a}");
        assert!(
            a["rules"][0].get("matched").is_none(),
            "matched only at quasi: {a}"
        );
        assert_eq!(a["line"], "rules mri@1.4.0, body_part/spine_words");
        assert_eq!(doc["unsorted"], 1);
        let quasi = build(&read, &[scan()], true, &mut |_, _| true);
        assert_eq!(row(&quasi, "body_part")["rules"][0]["matched"], "spine");
        // a campaign not closed whose answers the caller does not read
        read.campaigns
            .insert(6, ("curate".to_string(), "open".to_string()));
        let doc = build(&read, &[scan()], false, &mut |c, status| {
            assert_eq!((c, status), (6, "open"));
            false
        });
        let a = row(&doc, "body_part");
        assert_eq!(a["state"], "withheld", "{a}");
        assert!(a["decided"].get("value").is_none(), "{a}");
        assert!(a["decided"].get("by").is_none(), "{a}");
        assert!(a["decided"]["campaign"].get("item").is_none(), "{a}");
    }

    /// A model's grouped answer in force and applied: its name@version,
    /// the run and its probability on the scan.
    #[test]
    fn a_models_answer_names_the_model_the_run_and_the_probability() {
        let mut read = Read::default();
        held(&mut read, &["brain"], "decision");
        read.evidence
            .entry(1)
            .or_default()
            .entry("body_part".to_string())
            .or_default()
            .push(json!({
                "value": "brain", "tier": "decision", "basis": "decision", "confidence": 1.0,
                "rule_set": "decision", "rule": "group", "source": "decision", "matched": "2.1",
                "pass": null, "reference": null, "author": "bodypart@2.1", "author_kind": "model",
                "model_id": 7, "campaign_id": null,
            }));
        let mut d = decision(80, "group", "40", "brain", "model");
        d.model = Some(7);
        d.committed_by = Some("rae@lab".to_string());
        read.decisions = vec![d];
        read.members
            .insert((40, 1), json!({"confidence": 0.97, "run_id": 12}));
        read.models
            .insert(7, ("bodypart".to_string(), "2.1".to_string()));
        read.reviews.insert(80, (40, "body_part:model".to_string()));
        let doc = build(&read, &[scan()], false, &mut |_, _| true);
        let a = row(&doc, "body_part");
        assert_eq!(a["origin"], "model", "{a}");
        assert_eq!(a["state"], "current", "{a}");
        assert_eq!(a["line"], "model bodypart@2.1");
        assert_eq!(a["decided"]["model"]["label"], "bodypart@2.1");
        assert_eq!(a["decided"]["model"]["p"], 0.97);
        assert_eq!(a["decided"]["model"]["run"], 12);
        assert_eq!(a["decided"]["committed_by"], "rae@lab");
        assert_eq!(a["decided"]["review_item"]["id"], 40);
    }

    /// A decision held since withdrawn says so; a person's decision held
    /// and in force is current.
    #[test]
    fn a_withdrawn_decision_and_a_current_one() {
        let mut read = Read::default();
        held(&mut read, &["neck"], "decision");
        read.evidence
            .entry(1)
            .or_default()
            .entry("body_part".to_string())
            .or_default()
            .push(json!({
                "value": "neck", "tier": "decision", "basis": "decision", "confidence": 1.0,
                "rule_set": "decision", "rule": "stack", "source": "decision", "matched": null,
                "pass": null, "reference": null, "author": "rae@lab", "author_kind": "person",
                "model_id": null, "campaign_id": 6,
            }));
        let doc = build(&read, &[scan()], false, &mut |_, _| true);
        let a = row(&doc, "body_part");
        assert_eq!(a["origin"], "decision", "{a}");
        assert_eq!(a["state"], "withdrawn", "{a}");
        assert_eq!(a["from"]["by"], "rae@lab");
        read.decisions = vec![decision(90, "stack", "1", "neck", "person")];
        let doc = build(&read, &[scan()], false, &mut |_, _| true);
        let a = row(&doc, "body_part");
        assert_eq!(a["state"], "current", "{a}");
        assert!(a["words"].is_null(), "{a}");
        assert_eq!(doc["unsorted"], 0);
    }

    #[test]
    fn a_decision_value_reads_as_the_sort_stores_it() {
        assert_eq!(values_of(Some("b, a")), vec!["a", "b"]);
        assert!(values_of(Some("")).is_empty());
        assert!(values_of(None).is_empty());
        assert_eq!(said(&[]), "nothing");
    }
}
