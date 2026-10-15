// SPDX-License-Identifier: AGPL-3.0-only

//! A proposed rule change as a registry object (record 56, sections 5.3 to
//! 5.6, 6.4 and 6.5).
//!
//! Nima, 2026-10-09: "the change in rule should show how it affect the
//! sorting (like how many stack and to what)". A proposal is typed
//! operations (`nils_pack::patch`) with their reason, evidence and scope,
//! kept with the effect report they were rehearsed into, and decided by a
//! person: the pack is the auditable source of truth, and a rule never
//! changes by itself.
//!
//! - **A pack edit** (every operation scoped `pack`): adopted, it is the
//!   pack's next version, written where the engine reads its packs and
//!   recorded here as a rules version to release (`version`); every stack is
//!   sorted again by it.
//! - **A scoped change** (a site's, a dataset's, a scanner's or a batch's):
//!   adopted, its operations stay here, and every sort applies each of them
//!   to the stacks its scope holds and to no other (record 56 section 5.6:
//!   an overlay never leaves its scope); its scope is sorted again.
//!
//! Either re-sort records what it did on the proposal (`applied`). This
//! module keeps the rows; the doors and the sort read and write them.

use serde_json::Value;

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Row, Store};
use crate::time::now_iso;

pub const PROPOSED: &str = "proposed";
pub const ADOPTED: &str = "adopted";
pub const REJECTED: &str = "rejected";
/// An adopted scoped change a later one replaced: a newer version of the
/// same overlay adopted after it, or its overlay refused. No sort applies
/// it any more; the row stays, as every adoption does.
pub const RETIRED: &str = "retired";

/// Every status, in the order a list counts them.
pub const STATUSES: [&str; 4] = [PROPOSED, ADOPTED, REJECTED, RETIRED];

/// What the evidence of a scoped change the overlays door made says it was
/// made by (`made_by`): the door's own mark, so a person's proposal that
/// cites an overlay as its evidence is never taken for it.
pub const OVERLAY_MARK: &str = "overlay.adopt";

/// A pack edit, which ships as a rules release.
pub const PACK: &str = "pack";
/// A change for a site, a dataset, a scanner or a batch.
pub const SCOPED: &str = "scoped";

/// One proposal, as its row holds it.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub id: i64,
    pub pack: String,
    pub title: String,
    pub kind: String,
    /// The pack version it was last rehearsed against.
    pub on_version: String,
    /// Its operations' scopes, as text.
    pub scopes: Vec<String>,
    /// The patch as rehearsed: `{patch, pack, operations, cases?, ships?}`,
    /// every operation as written with its scope, reason and evidence.
    pub patch: Value,
    pub reason: Option<String>,
    pub evidence: Value,
    /// The datasets it touches, by name.
    pub datasets: Vec<String>,
    pub status: String,
    pub report: Value,
    pub card: Value,
    pub rehearsed_at: Option<String>,
    pub author: String,
    pub author_kind: String,
    pub actor: Value,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    pub why: Option<String>,
    pub job_id: Option<i64>,
    pub version: Value,
    pub applied: Value,
}

impl Proposal {
    pub fn is_scoped(&self) -> bool {
        self.kind == SCOPED
    }

    /// Whether it touches a dataset, by name.
    pub fn touches(&self, dataset: &str) -> bool {
        self.datasets.iter().any(|d| d == dataset)
    }
}

/// What a new proposal carries.
pub struct New<'a> {
    pub pack: &'a str,
    pub title: &'a str,
    pub kind: &'a str,
    pub on_version: &'a str,
    pub scopes: &'a [String],
    pub patch: &'a Value,
    pub reason: Option<&'a str>,
    pub evidence: &'a Value,
    pub datasets: &'a [String],
    pub report: &'a Value,
    pub card: &'a Value,
    pub author: &'a str,
    pub author_kind: &'a str,
    pub actor: &'a Value,
}

fn texts(v: &[String]) -> String {
    Value::from(v.to_vec()).to_string()
}

fn opt(v: Option<&str>) -> Param {
    match v {
        Some(s) => Param::from(s),
        None => Param::Null,
    }
}

/// Keep a new proposal. Answers its id.
pub fn create(store: &mut Store, p: &New<'_>) -> Result<i64, Error> {
    let now = now_iso();
    let rows = store.insert(
        &Insert::new(
            table("rule_proposal"),
            &[
                "pack",
                "title",
                "kind",
                "on_version",
                "scopes",
                "patch",
                "reason",
                "evidence",
                "datasets",
                "status",
                "report",
                "card",
                "rehearsed_at",
                "author",
                "author_kind",
                "actor",
                "created_at",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(p.pack),
            Param::from(p.title),
            Param::from(p.kind),
            Param::from(p.on_version),
            Param::from(texts(p.scopes)),
            Param::from(p.patch.to_string()),
            opt(p.reason),
            Param::from(p.evidence.to_string()),
            Param::from(texts(p.datasets)),
            Param::from(PROPOSED),
            Param::from(p.report.to_string()),
            Param::from(p.card.to_string()),
            Param::from(now.as_str()),
            Param::from(p.author),
            Param::from(p.author_kind),
            Param::from(p.actor.to_string()),
            Param::from(now.as_str()),
        ]],
    )?;
    rows.first()
        .ok_or(Error::Message("no id returned".into()))?
        .int(0)
}

fn select_sql(store: &Store, filter: &str, order: &str) -> String {
    let d = store.dialect();
    let t = table("rule_proposal");
    let text = |c: &str| d.text_of(t.column(c).expect("rule_proposal column"));
    format!(
        "SELECT id, pack, title, kind, on_version, {}, {}, reason, {}, {}, status, {}, {}, {}, author, author_kind, {}, {}, {}, decided_by, why, job_id, {}, {} FROM {}{filter} ORDER BY {order}",
        text("scopes"),
        text("patch"),
        text("evidence"),
        text("datasets"),
        text("report"),
        text("card"),
        text("rehearsed_at"),
        text("actor"),
        text("created_at"),
        text("decided_at"),
        text("version"),
        text("applied"),
        store.qualified("rule_proposal"),
    )
}

fn of(r: &Row) -> Result<Proposal, Error> {
    let json = |s: Option<&str>| {
        s.and_then(|t| serde_json::from_str::<Value>(t).ok())
            .unwrap_or(Value::Null)
    };
    let list = |s: Option<&str>| -> Vec<String> {
        json(s)
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(Proposal {
        id: r.int(0)?,
        pack: r.text(1)?.to_string(),
        title: r.text(2)?.to_string(),
        kind: r.text(3)?.to_string(),
        on_version: r.text(4)?.to_string(),
        scopes: list(r.opt_text(5)?),
        patch: json(r.opt_text(6)?),
        reason: r.opt_text(7)?.map(str::to_string),
        evidence: json(r.opt_text(8)?),
        datasets: list(r.opt_text(9)?),
        status: r.text(10)?.to_string(),
        report: json(r.opt_text(11)?),
        card: json(r.opt_text(12)?),
        rehearsed_at: r.opt_text(13)?.map(str::to_string),
        author: r.text(14)?.to_string(),
        author_kind: r.text(15)?.to_string(),
        actor: json(r.opt_text(16)?),
        created_at: r.text(17)?.to_string(),
        decided_at: r.opt_text(18)?.map(str::to_string),
        decided_by: r.opt_text(19)?.map(str::to_string),
        why: r.opt_text(20)?.map(str::to_string),
        job_id: r.opt_int(21)?,
        version: json(r.opt_text(22)?),
        applied: json(r.opt_text(23)?),
    })
}

/// One proposal.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Proposal>, Error> {
    let d = store.dialect();
    let sql = select_sql(
        store,
        &format!(" WHERE id = {}", d.param(1, Type::Int)),
        "id",
    );
    match store.query_opt(&sql, &[Param::Int(id)])? {
        Some(r) => Ok(Some(of(&r)?)),
        None => Ok(None),
    }
}

/// A pack's proposals, newest first; every pack's where none is named.
pub fn list(store: &mut Store, pack: Option<&str>) -> Result<Vec<Proposal>, Error> {
    let d = store.dialect();
    match pack {
        Some(p) => {
            let sql = select_sql(
                store,
                &format!(" WHERE pack = {}", d.param(1, Type::Text)),
                "id DESC",
            );
            store
                .query(&sql, &[Param::from(p)])?
                .iter()
                .map(of)
                .collect()
        }
        None => {
            let sql = select_sql(store, "", "id DESC");
            store.query(&sql, &[])?.iter().map(of).collect()
        }
    }
}

/// A pack's adopted scoped changes, in the order they were adopted: what
/// every sort applies, each operation to the stacks its scope holds.
pub fn adopted_scoped(store: &mut Store, pack: &str) -> Result<Vec<Proposal>, Error> {
    let d = store.dialect();
    let sql = select_sql(
        store,
        &format!(
            " WHERE pack = {} AND kind = {} AND status = {}",
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Text)
        ),
        "decided_at, id",
    );
    store
        .query(
            &sql,
            &[Param::from(pack), Param::from(SCOPED), Param::from(ADOPTED)],
        )?
        .iter()
        .map(of)
        .collect()
}

/// The scoped changes of a pack in force now, by id in order: the mark a
/// rehearsal takes before it reads the registry. A proposal whose mark is
/// not the one in force when it is decided was rehearsed against another
/// set of changes (one adopted or retired since, even within the same
/// second), and is rehearsed again before it is adopted.
pub fn in_force(store: &mut Store, pack: &str) -> Result<Vec<i64>, Error> {
    let mut ids: Vec<i64> = adopted_scoped(store, pack)?.iter().map(|p| p.id).collect();
    ids.sort_unstable();
    Ok(ids)
}

/// A proposal rehearsed again: its report and card, the version and the
/// datasets, while it is still proposed. Answers whether it was.
pub fn rehearsed(
    store: &mut Store,
    id: i64,
    on_version: &str,
    report: &Value,
    card: &Value,
    datasets: &[String],
) -> Result<bool, Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET on_version = {}, report = {}, card = {}, datasets = {}, rehearsed_at = {} WHERE id = {} AND status = {}",
        store.qualified("rule_proposal"),
        d.param(1, Type::Text),
        d.param(2, Type::Json),
        d.param(3, Type::Json),
        d.param(4, Type::Json),
        d.param(5, Type::Timestamp),
        d.param(6, Type::Int),
        d.param(7, Type::Text),
    );
    let n = store.execute(
        &sql,
        &[
            Param::from(on_version),
            Param::from(report.to_string()),
            Param::from(card.to_string()),
            Param::from(texts(datasets)),
            Param::from(now_iso().as_str()),
            Param::Int(id),
            Param::from(PROPOSED),
        ],
    )?;
    Ok(n > 0)
}

/// What a decision writes.
pub struct Decided<'a> {
    pub status: &'a str,
    pub by: &'a str,
    pub why: Option<&'a str>,
    /// The re-sort an adoption queued.
    pub job: Option<i64>,
    /// The rules version an adopted pack edit wrote.
    pub version: &'a Value,
    /// What the re-sort is expected to do, before it ran.
    pub applied: &'a Value,
    /// When it was decided, where that was before now: an overlay adopted
    /// before record 56's scoped changes keeps its own adoption time, so
    /// the order of adoption stays the real one.
    pub at: Option<&'a str>,
}

/// Decide a proposal that is still proposed, and nothing else: two people
/// deciding at once, the second finds it decided. Answers whether it was.
pub fn decide(store: &mut Store, id: i64, d: &Decided<'_>) -> Result<bool, Error> {
    let dialect = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = {}, decided_at = {}, decided_by = {}, why = {}, job_id = {}, version = {}, applied = {} WHERE id = {} AND status = {}",
        store.qualified("rule_proposal"),
        dialect.param(1, Type::Text),
        dialect.param(2, Type::Timestamp),
        dialect.param(3, Type::Text),
        dialect.param(4, Type::Text),
        dialect.param(5, Type::Int),
        dialect.param(6, Type::Json),
        dialect.param(7, Type::Json),
        dialect.param(8, Type::Int),
        dialect.param(9, Type::Text),
    );
    let json = |v: &Value| {
        if v.is_null() {
            Param::Null
        } else {
            Param::from(v.to_string())
        }
    };
    let n = store.execute(
        &sql,
        &[
            Param::from(d.status),
            Param::from(d.at.map_or_else(now_iso, str::to_string).as_str()),
            Param::from(d.by),
            opt(d.why),
            d.job.map_or(Param::Null, Param::Int),
            json(d.version),
            json(d.applied),
            Param::Int(id),
            Param::from(PROPOSED),
        ],
    )?;
    Ok(n > 0)
}

/// The re-sort an adoption queued, once it is queued.
pub fn set_job(store: &mut Store, id: i64, job: i64) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET job_id = {} WHERE id = {}",
        store.qualified("rule_proposal"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
    );
    store.execute(&sql, &[Param::Int(job), Param::Int(id)])?;
    Ok(())
}

/// What the re-sort after an adoption did: `measured` set beside what was
/// expected, and the state it ended in.
pub fn set_measured(store: &mut Store, id: i64, measured: &Value) -> Result<bool, Error> {
    let Some(p) = get(store, id)? else {
        return Ok(false);
    };
    let mut applied = match p.applied {
        Value::Object(m) => Value::Object(m),
        _ => serde_json::json!({}),
    };
    applied["state"] = measured["state"].clone();
    applied["measured"] = measured.clone();
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET applied = {} WHERE id = {}",
        store.qualified("rule_proposal"),
        d.param(1, Type::Json),
        d.param(2, Type::Int),
    );
    store.execute(&sql, &[Param::from(applied.to_string()), Param::Int(id)])?;
    Ok(true)
}

/// The scope an overlay is keyed on (C2: a manufacturer, a model, a station
/// or an ingest batch), as a rule change's scope reads it: `batch:ID` for a
/// batch alone, else `scanner:KEY=VALUE[,KEY=VALUE]`, its keys in order.
pub fn overlay_scope(document: &Value) -> Option<String> {
    let m = document["scope"].as_object()?;
    let mut keys: Vec<(String, String)> = m
        .iter()
        .filter_map(|(k, v)| {
            let v = match v {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            (!v.is_empty()).then(|| (k.clone(), v))
        })
        .collect();
    if keys.is_empty() {
        return None;
    }
    keys.sort();
    if keys.len() == 1 && keys[0].0 == "batch" {
        return Some(format!("batch:{}", keys[0].1));
    }
    Some(format!(
        "scanner:{}",
        keys.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    ))
}

/// A word overlay as the typed operations it is (record 56 section 5.5):
/// each bucket edit and each `axis.value` list edit an `add_words` and a
/// `remove_words`, all scoped to the overlay's origin, with the overlay as
/// their reason and evidence. None for a document that names no scope or
/// changes no word.
pub fn overlay_patch(id: i64, pack: &str, document: &Value) -> Option<(String, Value)> {
    let scope = overlay_scope(document)?;
    let name = format!(
        "{}@{}",
        document["overlay"].as_str().unwrap_or("overlay"),
        document["version"].as_str().unwrap_or("0")
    );
    let reason = format!("the overlay {name}");
    let evidence = serde_json::json!({"overlay": id});
    let words = |v: &Value| -> Vec<Value> {
        v.as_array()
            .map(|a| a.iter().filter(|w| w.is_string()).cloned().collect())
            .unwrap_or_default()
    };
    let mut operations: Vec<Value> = Vec::new();
    for (key, list) in [("buckets", false), ("lists", true)] {
        for (target, edit) in document[key].as_object().into_iter().flatten() {
            for (op, side) in [("add_words", "add"), ("remove_words", "remove")] {
                let w = words(&edit[side]);
                if w.is_empty() {
                    continue;
                }
                let mut o = serde_json::json!({
                    "op": op, "words": w, "scope": scope,
                    "reason": reason, "evidence": evidence,
                });
                if list {
                    let (axis, value) = target.split_once('.').unwrap_or((target, ""));
                    o["axis"] = Value::from(axis);
                    o["value"] = Value::from(value);
                } else {
                    o["bucket"] = Value::from(target.as_str());
                }
                operations.push(o);
            }
        }
    }
    if operations.is_empty() {
        return None;
    }
    let mut patch = serde_json::json!({
        "patch": 1, "pack": pack, "operations": operations,
        "reason": reason, "evidence": evidence,
    });
    // the overlay's own cases, which a rehearsal of it judges again
    if let Some(c) = document.get("cases").filter(|c| !c.is_null()) {
        patch["cases"] = c.clone();
    }
    Some((scope, patch))
}

/// The overlay a document is, by its own name (`overlay:`), else the name
/// its proposal was given: two versions of one overlay share it.
fn overlay_name(o: &crate::overlay::Overlay) -> String {
    o.document["overlay"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(&o.name)
        .to_string()
}

/// Whether a proposal is the scoped change the overlays door made of an
/// adopted overlay: its kind, its pack and its door's mark, never only an
/// evidence that cites the overlay (a person's proposal may).
pub fn made_of_overlay(p: &Proposal) -> bool {
    p.kind == SCOPED && p.evidence["made_by"].as_str() == Some(OVERLAY_MARK)
}

/// The scoped change the overlays door made of an overlay, where it made
/// one.
pub fn of_overlay(
    store: &mut Store,
    o: &crate::overlay::Overlay,
) -> Result<Option<Proposal>, Error> {
    Ok(list(store, Some(&o.pack))?.into_iter().find(|p| {
        made_of_overlay(p) && p.status != REJECTED && p.evidence["overlay"].as_i64() == Some(o.id)
    }))
}

/// The scoped change of a proposed overlay, as the proposal the overlays
/// door keeps beside it: the patch and the evidence an overlay is kept
/// with, or none where the overlay changes no word.
pub fn overlay_kept(o: &crate::overlay::Overlay) -> Option<(String, Value, Value, String)> {
    let (scope, patch) = overlay_patch(o.id, &o.pack, &o.document)?;
    let name = overlay_name(o);
    let evidence = serde_json::json!({
        "overlay": o.id, "made_by": OVERLAY_MARK,
        "name": name, "version": o.document["version"].as_str().unwrap_or(&o.version),
    });
    let title = format!("the overlay {} {}", o.name, o.version);
    Some((scope, patch, evidence, title))
}

/// The adopted scoped changes an adoption of this overlay retires: those
/// the overlays door made of another version of the same overlay (its own
/// name, on the same pack). The newer adopted version replaces the older
/// whole: its words are the overlay's words now, and an older one kept
/// beside it would collide with it (the same word added twice) and keep a
/// word the newer one took away.
pub fn superseded_by(
    store: &mut Store,
    o: &crate::overlay::Overlay,
) -> Result<Vec<Proposal>, Error> {
    let name = overlay_name(o);
    Ok(adopted_scoped(store, &o.pack)?
        .into_iter()
        .filter(|p| {
            made_of_overlay(p)
                && p.evidence["overlay"].as_i64() != Some(o.id)
                && p.evidence["name"].as_str() == Some(name.as_str())
        })
        .collect())
}

/// Retire an adopted scoped change: no sort applies it from now on. What
/// retired it is kept on its `applied` (`retired`: by, at, why). Answers
/// whether it was adopted.
pub fn retire(store: &mut Store, id: i64, by: &Value, why: &str) -> Result<bool, Error> {
    let Some(p) = get(store, id)? else {
        return Ok(false);
    };
    if p.status != ADOPTED {
        return Ok(false);
    }
    let mut applied = match p.applied {
        Value::Object(m) => Value::Object(m),
        _ => serde_json::json!({}),
    };
    applied["retired"] = serde_json::json!({"by": by, "at": now_iso(), "why": why});
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = {}, applied = {} WHERE id = {} AND status = {}",
        store.qualified("rule_proposal"),
        d.param(1, Type::Text),
        d.param(2, Type::Json),
        d.param(3, Type::Int),
        d.param(4, Type::Text),
    );
    let n = store.execute(
        &sql,
        &[
            Param::from(RETIRED),
            Param::from(applied.to_string()),
            Param::Int(id),
            Param::from(ADOPTED),
        ],
    )?;
    Ok(n > 0)
}

/// An overlay refused or withdrawn: the scoped change the overlays door
/// made of it retires with it where it is adopted, and is rejected where it
/// is still proposed. Answers the change retired or rejected, and whether it
/// had been adopted.
pub fn retire_overlay(
    store: &mut Store,
    o: &crate::overlay::Overlay,
    by: &str,
) -> Result<Option<(i64, bool)>, Error> {
    let Some(p) = of_overlay(store, o)? else {
        return Ok(None);
    };
    let why = format!("the overlay {} {} was refused by {by}", o.name, o.version);
    if p.status == PROPOSED {
        let done = decide(
            store,
            p.id,
            &Decided {
                status: REJECTED,
                by,
                why: Some(&why),
                job: None,
                version: &Value::Null,
                applied: &Value::Null,
                at: None,
            },
        )?;
        return Ok(done.then_some((p.id, false)));
    }
    Ok(retire(store, p.id, &serde_json::json!({"overlay": o.id}), &why)?.then_some((p.id, true)))
}

/// What keeping an overlay as its scoped change did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// The scoped change.
    pub id: i64,
    /// Whether it was made now (else the one made before is answered).
    pub made: bool,
    /// The scoped changes of the overlay's earlier versions it retired.
    pub retired: Vec<i64>,
}

/// The scoped change an adopted word overlay is (record 56 section 5.6: an
/// overlay never leaves its scope, and is applied by every sort, not by the
/// one re-sort its adoption queued): an adopted proposal of kind scoped
/// whose operations are the overlay's words, its evidence the overlay and
/// the door's mark. Made once: the one made before for the same overlay is
/// answered where there is one; none where the overlay changes no word.
///
/// The overlay's earlier versions adopted before (the same overlay name on
/// the same pack) are retired by it: the newer version is the overlay now.
/// `at` is when it was adopted, where that was before now (a migration
/// keeps the real order of adoption); the caller holds the transaction.
pub fn from_overlay(
    store: &mut Store,
    o: &crate::overlay::Overlay,
    by: &str,
    at: Option<&str>,
) -> Result<Option<Kept>, Error> {
    if let Some(p) = of_overlay(store, o)? {
        // the proposal an overlay was kept with, for an overlay adopted by
        // other means (a registry from before record 56): superseded by the
        // adopted scoped change made now
        if p.status == PROPOSED {
            decide(
                store,
                p.id,
                &Decided {
                    status: REJECTED,
                    by,
                    why: Some("the overlay was adopted without it"),
                    job: None,
                    version: &Value::Null,
                    applied: &Value::Null,
                    at,
                },
            )?;
            return from_overlay(store, o, by, at);
        }
        return Ok(Some(Kept {
            id: p.id,
            made: false,
            retired: Vec::new(),
        }));
    }
    let Some((scope, patch)) = overlay_patch(o.id, &o.pack, &o.document) else {
        return Ok(None);
    };
    let older = superseded_by(store, o)?;
    let title = format!("the overlay {} {}", o.name, o.version);
    let scopes = vec![scope];
    let name = overlay_name(o);
    let evidence = serde_json::json!({
        "overlay": o.id, "made_by": OVERLAY_MARK,
        "name": name, "version": o.document["version"].as_str().unwrap_or(&o.version),
    });
    let id = create(
        store,
        &New {
            pack: &o.pack,
            title: &title,
            kind: SCOPED,
            on_version: "",
            scopes: &scopes,
            patch: &patch,
            reason: o.why.as_deref(),
            evidence: &evidence,
            datasets: &[],
            report: &o.tried,
            card: &serde_json::json!({}),
            author: &o.author,
            author_kind: &o.author_kind,
            actor: &o.actor,
        },
    )?;
    let applied = serde_json::json!({
        "state": "queued", "expected": null, "measured": null, "overlay": o.id,
        "supersedes": older.iter().map(|p| p.id).collect::<Vec<_>>(),
    });
    decide(
        store,
        id,
        &Decided {
            status: ADOPTED,
            by,
            why: Some("an adopted overlay, kept as the scoped change it is"),
            job: o.job_id,
            version: &Value::Null,
            applied: &applied,
            at,
        },
    )?;
    let mut retired = Vec::new();
    for p in older {
        let why = format!(
            "the overlay {name} {} was adopted after it",
            evidence["version"].as_str().unwrap_or("")
        );
        if retire(
            store,
            p.id,
            &serde_json::json!({"proposal": id, "overlay": o.id}),
            &why,
        )? {
            retired.push(p.id);
        }
    }
    Ok(Some(Kept {
        id,
        made: true,
        retired,
    }))
}

/// Every adopted overlay, as the scoped change it is, made once: what a
/// registry from before record 56's scoped changes takes when it migrates,
/// so the overlays it adopted stay in their scope and survive every sort.
/// They are taken in the order they were adopted, each keeping its own
/// adoption time, so a later version of an overlay retires the earlier one
/// as its adoption did. Answers the proposals made now.
pub fn adopt_overlays(store: &mut Store) -> Result<Vec<i64>, Error> {
    let mut adopted: Vec<crate::overlay::Overlay> = crate::overlay::list(store)?
        .into_iter()
        .filter(|o| o.status == crate::overlay::ADOPTED)
        .collect();
    adopted.sort_by(|a, b| {
        a.decided_at
            .cmp(&b.decided_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut made = Vec::new();
    for o in adopted {
        let by = o.decided_by.clone().unwrap_or_else(|| o.author.clone());
        if let Some(k) = from_overlay(store, &o, &by, o.decided_at.as_deref())?
            && k.made
        {
            made.push(k.id);
        }
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> Store {
        let mut s = Store::sqlite_in_memory().unwrap();
        crate::migrate::migrate(&mut s, crate::migrate::Kind::Registry).unwrap();
        s
    }

    fn new<'a>(
        kind: &'a str,
        scopes: &'a [String],
        patch: &'a Value,
        datasets: &'a [String],
        empty: &'a Value,
    ) -> New<'a> {
        New {
            pack: "mri",
            title: "mdc for contrast",
            kind,
            on_version: "1.2.0",
            scopes,
            patch,
            reason: Some("the partner writes mdc"),
            evidence: empty,
            datasets,
            report: empty,
            card: empty,
            author: "nima@local",
            author_kind: "person",
            actor: empty,
        }
    }

    #[test]
    fn a_proposal_is_kept_listed_decided_once_and_measured() {
        let mut s = store();
        let patch = json!({"patch": 1, "pack": "mri", "operations": [{"op": "add_words"}]});
        let scopes = vec!["dataset:partner".to_string()];
        let datasets = vec!["partner".to_string()];
        let empty = json!({});
        let id = create(&mut s, &new(SCOPED, &scopes, &patch, &datasets, &empty)).unwrap();
        let p = get(&mut s, id).unwrap().unwrap();
        assert_eq!(p.status, PROPOSED);
        assert_eq!(p.scopes, scopes);
        assert!(p.touches("partner") && !p.touches("other"));
        assert_eq!(p.patch["operations"][0]["op"], "add_words");
        assert!(adopted_scoped(&mut s, "mri").unwrap().is_empty());

        // rehearsed again while proposed
        assert!(
            rehearsed(
                &mut s,
                id,
                "1.2.1",
                &json!({"moved": {"stacks": 3}}),
                &empty,
                &datasets
            )
            .unwrap()
        );
        assert_eq!(get(&mut s, id).unwrap().unwrap().on_version, "1.2.1");

        let applied = json!({"state": "queued", "expected": {"moved": {"stacks": 3}}});
        let d = Decided {
            status: ADOPTED,
            by: "nima@local",
            why: Some("fixes 3"),
            job: Some(7),
            version: &Value::Null,
            applied: &applied,
            at: None,
        };
        assert!(decide(&mut s, id, &d).unwrap());
        // the second decision finds it decided
        assert!(!decide(&mut s, id, &d).unwrap());
        // nor is a decided one rehearsed again
        assert!(!rehearsed(&mut s, id, "1.2.2", &empty, &empty, &datasets).unwrap());
        let scoped = adopted_scoped(&mut s, "mri").unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(in_force(&mut s, "mri").unwrap(), [id]);

        assert!(set_measured(&mut s, id, &json!({"state": "done", "stacks": 3})).unwrap());
        let p = get(&mut s, id).unwrap().unwrap();
        assert_eq!(p.applied["state"], "done");
        assert_eq!(p.applied["expected"]["moved"]["stacks"], 3);
        assert_eq!(p.applied["measured"]["stacks"], 3);
        assert_eq!(p.job_id, Some(7));
        assert_eq!(list(&mut s, Some("mri")).unwrap().len(), 1);
        assert!(list(&mut s, Some("ct")).unwrap().is_empty());
    }

    #[test]
    fn an_adopted_overlay_becomes_its_scoped_change_once() {
        let mut s = store();
        let doc = json!({
            "overlay": "site", "version": "1.0.0", "pack": "mri",
            "scope": {"manufacturer": "ACME", "station": "MR1"},
            "buckets": {"contrast_positive": {"add": ["zorvex"]}},
            "lists": {"technique.TSE": {"add": ["zzturbo"], "remove": ["turbo"]}},
        });
        let (scope, patch) = overlay_patch(4, "mri", &doc).unwrap();
        assert_eq!(scope, "scanner:manufacturer=ACME,station=MR1");
        let ops = patch["operations"].as_array().unwrap();
        assert_eq!(ops.len(), 3, "{patch}");
        assert_eq!(ops[0]["bucket"], "contrast_positive");
        assert_eq!(ops[1]["axis"], "technique");
        assert_eq!(ops[2]["op"], "remove_words");
        assert_eq!(
            overlay_scope(&json!({"scope": {"batch": "7"}})).as_deref(),
            Some("batch:7")
        );
        let p = crate::overlay::Proposal {
            name: "site",
            version: "1.0.0",
            pack: "mri",
            author: "rev@lab",
            author_kind: "person",
            actor: json!({}),
            scope: json!({"over": "batch:1"}),
            document: doc,
            tried: json!({}),
            why: Some("the site's words"),
        };
        let (overlay, _) = crate::overlay::propose(&mut s, &p).unwrap();
        // proposed only: nothing to keep
        assert!(adopt_overlays(&mut s).unwrap().is_empty());
        crate::overlay::decide(
            &mut s,
            overlay,
            crate::overlay::ADOPTED,
            "ops@lab",
            Some(3),
            None,
        )
        .unwrap();
        let made = adopt_overlays(&mut s).unwrap();
        assert_eq!(made.len(), 1);
        let kept = adopted_scoped(&mut s, "mri").unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].evidence["overlay"], overlay);
        assert_eq!(kept[0].scopes, ["scanner:manufacturer=ACME,station=MR1"]);
        assert_eq!(kept[0].decided_by.as_deref(), Some("ops@lab"));
        assert_eq!(kept[0].job_id, Some(3));
        // once
        assert!(adopt_overlays(&mut s).unwrap().is_empty());
        assert_eq!(adopted_scoped(&mut s, "mri").unwrap().len(), 1);
    }

    /// An overlay version as an overlay row, adopted at `at` where given.
    fn overlay_row(s: &mut Store, version: &str, words: &[&str], at: Option<&str>) -> i64 {
        let doc = json!({
            "overlay": "site", "version": version, "pack": "mri",
            "scope": {"batch": "1"},
            "lists": {"technique.TSE": {"add": words}},
            "cases": [{"name": "c", "stack": {"text_series_description": words[0]}, "axes": {"technique": "TSE"}}],
        });
        let p = crate::overlay::Proposal {
            name: "the site's words",
            version,
            pack: "mri",
            author: "rev@lab",
            author_kind: "person",
            actor: json!({}),
            scope: json!({"over": "batch:1"}),
            document: doc,
            tried: json!({}),
            why: None,
        };
        let (id, _) = crate::overlay::propose(s, &p).unwrap();
        if let Some(at) = at {
            crate::overlay::decide(s, id, crate::overlay::ADOPTED, "ops@lab", None, None).unwrap();
            let o = s.qualified("overlay");
            s.execute(
                &format!("UPDATE {o} SET decided_at = '{at}' WHERE id = {id}"),
                &[],
            )
            .unwrap();
        }
        id
    }

    #[test]
    fn a_later_version_of_an_overlay_retires_the_earlier_in_the_order_adopted() {
        let mut s = store();
        // 2.0.0 is proposed first and adopted last: the order of adoption,
        // not of the rows or of the migration, decides which is in force
        let v2 = overlay_row(&mut s, "2.0.0", &["zzturbo"], Some("2026-02-01T00:00:00Z"));
        let v1 = overlay_row(
            &mut s,
            "1.0.0",
            &["zzturbo", "oldword"],
            Some("2026-01-01T00:00:00Z"),
        );
        // a person's proposal citing the first version is not the door's
        let patch = json!({"patch": 1, "pack": "mri", "operations": [{"op": "add_words"}]});
        let scopes = vec!["batch:1".to_string()];
        let cites = json!({"overlay": v1});
        let empty = json!({});
        let mut person = new(SCOPED, &scopes, &patch, &[], &empty);
        person.evidence = &cites;
        let theirs = create(&mut s, &person).unwrap();
        let d = Decided {
            status: ADOPTED,
            by: "ops@lab",
            why: None,
            job: None,
            version: &Value::Null,
            applied: &empty,
            at: Some("2025-12-01T00:00:00Z"),
        };
        assert!(decide(&mut s, theirs, &d).unwrap());

        let made = adopt_overlays(&mut s).unwrap();
        assert_eq!(made.len(), 2, "{made:?}");
        let by_overlay = |s: &mut Store, o: i64| {
            list(s, Some("mri"))
                .unwrap()
                .into_iter()
                .find(|p| made_of_overlay(p) && p.evidence["overlay"] == o)
                .unwrap()
        };
        let one = by_overlay(&mut s, v1);
        let two = by_overlay(&mut s, v2);
        assert_eq!(one.status, RETIRED, "the earlier version is retired");
        assert_eq!(one.applied["retired"]["by"]["proposal"], two.id);
        assert_eq!(two.status, ADOPTED);
        assert_eq!(two.applied["supersedes"], json!([one.id]));
        // each keeps its own time of adoption
        assert!(
            two.decided_at.as_deref().unwrap().starts_with("2026-02-01"),
            "{:?}",
            two.decided_at
        );
        // the overlay's cases are kept with its words
        assert_eq!(two.patch["cases"][0]["name"], "c");
        // the person's proposal is theirs: neither taken for the overlay's
        // nor retired
        assert_eq!(get(&mut s, theirs).unwrap().unwrap().status, ADOPTED);
        assert_eq!(in_force(&mut s, "mri").unwrap(), [theirs, two.id]);
        assert_eq!(
            adopted_scoped(&mut s, "mri")
                .unwrap()
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            [theirs, two.id],
            "in the order adopted"
        );

        // a third version adopted at the door now retires the second
        let v3 = overlay_row(&mut s, "3.0.0", &["zzturbo3"], None);
        let o3 = crate::overlay::show(&mut s, v3).unwrap().unwrap();
        assert_eq!(
            superseded_by(&mut s, &o3)
                .unwrap()
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            [two.id]
        );
        let k = from_overlay(&mut s, &o3, "ops@lab", None).unwrap().unwrap();
        assert!(k.made);
        assert_eq!(k.retired, [two.id]);
        assert_eq!(in_force(&mut s, "mri").unwrap(), [theirs, k.id]);
        // once: the same overlay answers the change made before
        let again = from_overlay(&mut s, &o3, "ops@lab", None).unwrap().unwrap();
        assert_eq!((again.id, again.made), (k.id, false));

        // a refused overlay retires the change made of it, and nothing else
        assert_eq!(
            retire_overlay(&mut s, &o3, "ops@lab").unwrap(),
            Some((k.id, true))
        );
        assert_eq!(get(&mut s, k.id).unwrap().unwrap().status, RETIRED);
        assert_eq!(retire_overlay(&mut s, &o3, "ops@lab").unwrap(), None);
        assert_eq!(in_force(&mut s, "mri").unwrap(), [theirs]);
        assert!(!retire(&mut s, k.id, &json!({}), "twice").unwrap());
    }
}
