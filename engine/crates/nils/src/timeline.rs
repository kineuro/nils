// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 5 section 12.2, slice A2: the timeline door. Every event on one
//! object in order, read from the rows the engine already keeps (documents
//! and their parent chain, handles, cohort membership, audit, jobs, batches,
//! classification, decisions, review items, picks, releases, handovers),
//! each typed and each naming the object it produced. Nothing here is a row
//! of a person: a subject's events are dated arrivals and memberships, never
//! a clinical value.

use nils_registry::schema::Type;
use nils_registry::store::Param;
use nils_registry::{Registry, Store};
use serde_json::{Value, json};

/// One event on an object.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Event {
    /// An ISO stamp; the order key.
    pub at: String,
    /// What happened: `version`, `run`, `promotion`, `decision`, ...
    pub kind: String,
    /// Who did it, when a row says.
    pub actor: Option<String>,
    pub summary: String,
    /// The object the event produced, `{kind, id}` or `{kind, name, ...}`.
    pub produced: Value,
    /// The table the event was read from.
    pub source: &'static str,
}

/// The kinds the door serves.
pub(crate) const KINDS: &[&str] = &[
    "document", "handle", "subject", "session", "stack", "job", "release", "review",
];

pub(crate) enum Outcome {
    Events(Vec<Event>),
    /// No such kind.
    NoKind,
    /// The kind is known and no row has that id.
    NoObject,
}

pub(crate) fn of(registry: &mut Registry, kind: &str, id: i64) -> Result<Outcome, String> {
    let found = match kind {
        "document" => document(registry, id),
        "handle" => handle(registry, id),
        "subject" => subject(registry.store(), id),
        "session" => session(registry.store(), id),
        "stack" => stack(registry.store(), id),
        "job" => job(registry, id),
        "release" => release(registry, id),
        "review" => review(registry, id),
        _ => return Ok(Outcome::NoKind),
    }?;
    Ok(match found {
        Some(mut events) => {
            // stable: equal stamps keep the order they were collected in
            events.sort_by(|a, b| a.at.cmp(&b.at));
            Outcome::Events(events)
        }
        None => Outcome::NoObject,
    })
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn text_of(store: &Store, t: &str, c: &str) -> String {
    crate::text_of(store, t, c)
}

fn produced(kind: &str, id: i64) -> Value {
    json!({ "kind": kind, "id": id })
}

fn event(
    at: impl Into<String>,
    kind: &str,
    actor: Option<&str>,
    summary: String,
    produced: Value,
    source: &'static str,
) -> Event {
    Event {
        at: at.into(),
        kind: kind.to_string(),
        actor: actor.map(str::to_string),
        summary,
        produced,
        source,
    }
}

/// Audit rows whose action is one of `actions`, oldest first, each with its
/// scope parsed; the caller keeps the ones that name its object.
/// An audit row as the timeline reads it: at, principal, action, scope, job.
type AuditRow = (String, String, String, Value, Option<i64>);

fn audit_rows(store: &mut Store, actions: &[&str]) -> Result<Vec<AuditRow>, String> {
    let d = store.dialect();
    let at = text_of(store, "audit", "at");
    let scope = text_of(store, "audit", "scope");
    let holes: Vec<String> = (1..=actions.len())
        .map(|i| d.param(i, Type::Text))
        .collect();
    let params: Vec<Param> = actions.iter().map(|a| Param::from(*a)).collect();
    let sql = format!(
        "SELECT {at}, principal, action, {scope}, job_id FROM {} WHERE action IN ({}) ORDER BY id",
        store.qualified("audit"),
        holes.join(", ")
    );
    store
        .query(&sql, &params)
        .map_err(err)?
        .iter()
        .map(|r| {
            Ok((
                r.opt_text(0).map_err(err)?.unwrap_or_default().to_string(),
                r.text(1).map_err(err)?.to_string(),
                r.text(2).map_err(err)?.to_string(),
                r.opt_text(3)
                    .map_err(err)?
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null),
                r.opt_int(4).map_err(err)?,
            ))
        })
        .collect()
}

/// When a batch finished, or started; the stamp a landed row carries.
fn batch_at(store: &mut Store, batch: i64) -> Result<Option<String>, String> {
    let d = store.dialect();
    let finished = text_of(store, "ingest_batch", "finished_at");
    let started = text_of(store, "ingest_batch", "started_at");
    let sql = format!(
        "SELECT {finished}, {started} FROM {} WHERE id = {}",
        store.qualified("ingest_batch"),
        d.param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(batch)])
        .map_err(err)?
        .and_then(|r| {
            r.opt_text(0)
                .ok()
                .flatten()
                .or(r.opt_text(1).ok().flatten())
                .map(str::to_string)
        }))
}

// ---------------------------------------------------------------- document

/// The chain a document belongs to: its ancestors to the root, then every
/// descendant, oldest first.
fn chain(store: &mut Store, id: i64) -> Result<Vec<nils_ask::document::Document>, String> {
    use nils_ask::document;
    let Some(start) = document::get(store, id).map_err(err)? else {
        return Ok(Vec::new());
    };
    let mut root = start;
    let mut seen = std::collections::HashSet::new();
    seen.insert(root.id);
    while let Some(parent) = root.parent_id {
        if !seen.insert(parent) {
            break;
        }
        match document::get(store, parent).map_err(err)? {
            Some(p) => root = p,
            None => break,
        }
    }
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE parent_id = {} ORDER BY id",
        store.qualified("ask_document"),
        d.param(1, Type::Int)
    );
    let mut out = vec![root];
    let mut queue = std::collections::VecDeque::from([out[0].id]);
    let mut visited = std::collections::HashSet::from([out[0].id]);
    while let Some(next) = queue.pop_front() {
        let children: Vec<i64> = store
            .query(&sql, &[Param::Int(next)])
            .map_err(err)?
            .iter()
            .filter_map(|r| r.int(0).ok())
            .collect();
        for c in children {
            if !visited.insert(c) {
                continue;
            }
            if let Some(doc) = document::get(store, c).map_err(err)? {
                out.push(doc);
                queue.push_back(c);
            }
        }
    }
    Ok(out)
}

fn document(registry: &mut Registry, id: i64) -> Result<Option<Vec<Event>>, String> {
    let store = registry.store();
    let versions = chain(store, id)?;
    if versions.is_empty() {
        return Ok(None);
    }
    let mut events = Vec::new();
    for (i, v) in versions.iter().enumerate() {
        let from = v
            .parent_id
            .map(|p| format!(", from document {p}"))
            .unwrap_or_default();
        events.push(event(
            v.created_at.clone(),
            "version",
            Some(&v.principal),
            format!(
                "version {} of the document, {}{from}",
                i + 1,
                v.ask.name.as_deref().unwrap_or("unnamed")
            ),
            produced("document", v.id),
            "ask_document",
        ));
    }
    let hashes: std::collections::HashSet<String> =
        versions.iter().map(|v| v.hash.clone()).collect();
    let handles: Vec<nils_ask::handle::Handle> = nils_ask::handle::list(store, true)
        .map_err(err)?
        .into_iter()
        .filter(|h| h.ask_hash().is_some_and(|x| hashes.contains(&x)))
        .collect();
    let handle_ids: std::collections::HashSet<i64> = handles.iter().map(|h| h.id).collect();
    for h in &handles {
        events.extend(handle_events(h));
    }
    for (at, who, action, scope, _) in audit_rows(store, &["cohort.promote", "selection.save"])? {
        match action.as_str() {
            "cohort.promote" => {
                if scope["handle"]
                    .as_i64()
                    .is_some_and(|h| handle_ids.contains(&h))
                {
                    events.push(promotion(at, &who, &scope));
                }
            }
            "selection.save" if scope["hash"].as_str().is_some_and(|x| hashes.contains(x)) => {
                events.push(event(
                    at,
                    "selection",
                    Some(&who),
                    format!(
                        "saved as selection {} version {}",
                        scope["selection"].as_str().unwrap_or("?"),
                        scope["version"]
                    ),
                    json!({ "kind": "selection", "name": scope["selection"], "version": scope["version"] }),
                    "audit",
                ));
            }
            _ => {}
        }
    }
    Ok(Some(events))
}

fn promotion(at: String, who: &str, scope: &Value) -> Event {
    event(
        at,
        "promotion",
        Some(who),
        format!(
            "handle {} promoted to cohort {}: {} added, {} already members",
            scope["handle"],
            scope["cohort"].as_str().unwrap_or("?"),
            scope["added"],
            scope["already"]
        ),
        match scope["id"].as_i64() {
            Some(c) => produced("cohort", c),
            None => json!({ "kind": "cohort", "name": scope["cohort"] }),
        },
        "audit",
    )
}

/// The events a handle's own row carries.
fn handle_events(h: &nils_ask::handle::Handle) -> Vec<Event> {
    let mut out = vec![event(
        h.created_at.clone(),
        "run",
        Some(&h.principal),
        format!(
            "run to handle {}{}: {} {} rows{}",
            h.id,
            h.name
                .as_ref()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default(),
            h.row_count,
            h.grain.name(),
            if h.truncated { ", truncated" } else { "" }
        ),
        produced("handle", h.id),
        "handle",
    )];
    if let Some(at) = &h.last_read_at {
        out.push(event(
            at.clone(),
            "read",
            None,
            format!("handle {} last read", h.id),
            Value::Null,
            "handle",
        ));
    }
    if let Some(at) = &h.rows_dropped_at {
        out.push(event(
            at.clone(),
            "rows_dropped",
            None,
            format!("the rows of handle {} were dropped by retention", h.id),
            Value::Null,
            "handle",
        ));
    }
    if let Some(at) = &h.withdrawn_at {
        out.push(event(
            at.clone(),
            "withdrawn",
            h.withdrawn_by.as_deref(),
            format!(
                "handle {} withdrawn{}",
                h.id,
                h.withdrawn_why
                    .as_ref()
                    .map(|w| format!(": {w}"))
                    .unwrap_or_default()
            ),
            Value::Null,
            "handle",
        ));
    }
    out
}

// ------------------------------------------------------------------ handle

fn handle(registry: &mut Registry, id: i64) -> Result<Option<Vec<Event>>, String> {
    let store = registry.store();
    let Some(h) = nils_ask::handle::get(store, id).map_err(err)? else {
        return Ok(None);
    };
    // the document it came from, when one was stored; first, so that a
    // document and a run in the same second read in the order they happened
    let mut events = Vec::new();
    if let Some(hash) = h.ask_hash() {
        let d = store.dialect();
        let created = text_of(store, "ask_document", "created_at");
        let sql = format!(
            "SELECT id, {created}, principal FROM {} WHERE hash = {} ORDER BY id",
            store.qualified("ask_document"),
            d.param(1, Type::Text)
        );
        for r in store
            .query(&sql, &[Param::from(hash.as_str())])
            .map_err(err)?
        {
            let doc = r.int(0).map_err(err)?;
            events.push(event(
                r.opt_text(1).map_err(err)?.unwrap_or_default(),
                "document",
                r.opt_text(2).map_err(err)?,
                format!("document {doc} holds the ask this handle ran"),
                produced("document", doc),
                "ask_document",
            ));
        }
    }
    events.extend(handle_events(&h));
    // the reads that were audited
    {
        let d = store.dialect();
        let at = text_of(store, "handle_read_audit", "read_at");
        let sql = format!(
            "SELECT {at}, principal, rows, purpose FROM {} WHERE handle_id = {} ORDER BY id",
            store.qualified("handle_read_audit"),
            d.param(1, Type::Int)
        );
        if let Ok(rows) = store.query(&sql, &[Param::Int(id)]) {
            for r in rows {
                events.push(event(
                    r.opt_text(0).map_err(err)?.unwrap_or_default(),
                    "read",
                    r.opt_text(1).map_err(err)?,
                    format!(
                        "{} rows read{}",
                        r.opt_int(2).map_err(err)?.unwrap_or(0),
                        r.opt_text(3)
                            .map_err(err)?
                            .map(|p| format!(" for {p}"))
                            .unwrap_or_default()
                    ),
                    Value::Null,
                    "handle_read_audit",
                ));
            }
        }
    }
    for (at, who, _, scope, _) in audit_rows(store, &["cohort.promote"])? {
        if scope["handle"].as_i64() == Some(id) {
            events.push(promotion(at, &who, &scope));
        }
    }
    Ok(Some(events))
}

// ----------------------------------------------------------------- subject

fn subject(store: &mut Store, id: i64) -> Result<Option<Vec<Event>>, String> {
    let d = store.dialect();
    let created = text_of(store, "subject", "created_at");
    let sql = format!(
        "SELECT {created}, first_batch_id FROM {} WHERE id = {}",
        store.qualified("subject"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(id)]).map_err(err)? else {
        return Ok(None);
    };
    let mut events = Vec::new();
    let created_at = row
        .opt_text(0)
        .map_err(err)?
        .unwrap_or_default()
        .to_string();
    let first_batch = row.opt_int(1).map_err(err)?;
    events.push(event(
        created_at,
        "arrived",
        None,
        "the subject arrived".to_string(),
        first_batch.map_or(Value::Null, |b| produced("batch", b)),
        "subject",
    ));
    // studies, each dated by the batch that brought it
    let sql = format!(
        "SELECT id, first_batch_id FROM {} WHERE subject_id = {} ORDER BY id",
        store.qualified("study"),
        d.param(1, Type::Int)
    );
    let studies: Vec<(i64, Option<i64>)> = store
        .query(&sql, &[Param::Int(id)])
        .map_err(err)?
        .iter()
        .map(|r| Ok((r.int(0).map_err(err)?, r.opt_int(1).map_err(err)?)))
        .collect::<Result<_, String>>()?;
    for (study, batch) in studies {
        let at = match batch {
            Some(b) => batch_at(store, b)?.unwrap_or_default(),
            None => String::new(),
        };
        events.push(event(
            at,
            "study",
            None,
            format!("study {study} landed"),
            produced("study", study),
            "study",
        ));
    }
    // cohort membership
    let joined = text_of(store, "cohort_member", "joined_at");
    let left = text_of(store, "cohort_member", "left_at");
    let sql = format!(
        "SELECT m.cohort_id, c.name, {joined}, {left}, m.actor, m.left_by, m.handle_id FROM {} m \
         JOIN {} c ON c.id = m.cohort_id WHERE m.subject_id = {} ORDER BY m.id",
        store.qualified("cohort_member"),
        store.qualified("cohort"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let cohort = r.int(0).map_err(err)?;
        let name = r.text(1).map_err(err)?.to_string();
        let handle = r.opt_int(6).map_err(err)?;
        events.push(event(
            r.opt_text(2).map_err(err)?.unwrap_or_default(),
            "joined",
            r.opt_text(4).map_err(err)?,
            format!(
                "joined cohort {name}{}",
                handle
                    .map(|h| format!(" through handle {h}"))
                    .unwrap_or_default()
            ),
            produced("cohort", cohort),
            "cohort_member",
        ));
        if let Some(at) = r.opt_text(3).map_err(err)? {
            events.push(event(
                at,
                "left",
                r.opt_text(5).map_err(err)?,
                format!("left cohort {name}"),
                produced("cohort", cohort),
                "cohort_member",
            ));
        }
    }
    // sessions as the cache built them
    let built = text_of(store, "session_cache", "built_at");
    let sql = format!(
        "SELECT id, {built}, n_studies FROM {} WHERE subject_id = {} ORDER BY id",
        store.qualified("session_cache"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let session = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(1).map_err(err)?.unwrap_or_default(),
            "session",
            None,
            format!(
                "session {session} built over {} studies",
                r.opt_int(2).map_err(err)?.unwrap_or(0)
            ),
            produced("session", session),
            "session_cache",
        ));
    }
    // releases holding one of the subject's stacks
    let started = text_of(store, "release", "started_at");
    let sql = format!(
        "SELECT rel.id, rel.name, rel.version, {started}, COUNT(*) FROM {} rs \
         JOIN {} st ON st.id = rs.stack_id JOIN {} se ON se.id = st.series_id \
         JOIN {} rel ON rel.id = rs.release_id \
         WHERE se.subject_id = {} GROUP BY rel.id, rel.name, rel.version, {started} ORDER BY rel.id",
        store.qualified("release_stack"),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("release"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let rel = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(3).map_err(err)?.unwrap_or_default(),
            "released",
            None,
            format!(
                "{} stacks released in {} {}",
                r.opt_int(4).map_err(err)?.unwrap_or(0),
                r.text(1).map_err(err)?,
                r.text(2).map_err(err)?
            ),
            produced("release", rel),
            "release_stack",
        ));
    }
    Ok(Some(events))
}

// ----------------------------------------------------------------- session

fn session(store: &mut Store, id: i64) -> Result<Option<Vec<Event>>, String> {
    let d = store.dialect();
    let built = text_of(store, "session_cache", "built_at");
    let first = text_of(store, "session_cache", "first");
    let last = text_of(store, "session_cache", "last");
    let sql = format!(
        "SELECT subject_id, {built}, n_studies, {first}, {last}, epoch FROM {} WHERE id = {}",
        store.qualified("session_cache"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(id)]).map_err(err)? else {
        return Ok(None);
    };
    let subject = row.int(0).map_err(err)?;
    let mut events = vec![event(
        row.opt_text(1).map_err(err)?.unwrap_or_default(),
        "built",
        None,
        format!(
            "session built over {} studies at epoch {}",
            row.opt_int(2).map_err(err)?.unwrap_or(0),
            row.opt_int(5).map_err(err)?.unwrap_or(0)
        ),
        produced("subject", subject),
        "session_cache",
    )];
    let sql = format!(
        "SELECT s.study_id, st.first_batch_id FROM {} s JOIN {} st ON st.id = s.study_id \
         WHERE s.session_id = {} ORDER BY s.id",
        store.qualified("session_cache_study"),
        store.qualified("study"),
        d.param(1, Type::Int)
    );
    let studies: Vec<(i64, Option<i64>)> = store
        .query(&sql, &[Param::Int(id)])
        .map_err(err)?
        .iter()
        .map(|r| Ok((r.int(0).map_err(err)?, r.opt_int(1).map_err(err)?)))
        .collect::<Result<_, String>>()?;
    for (study, batch) in studies {
        let at = match batch {
            Some(b) => batch_at(store, b)?.unwrap_or_default(),
            None => String::new(),
        };
        events.push(event(
            at,
            "study",
            None,
            format!("study {study} landed"),
            produced("study", study),
            "session_cache_study",
        ));
    }
    let sql = format!(
        "SELECT label, scheme_digest, flagged, reason FROM {} WHERE session_id = {} ORDER BY id",
        store.qualified("session_label"),
        d.param(1, Type::Int)
    );
    let built_at = events[0].at.clone();
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let flagged = r.opt_int(2).map_err(err)?.unwrap_or(0) != 0;
        events.push(event(
            built_at.clone(),
            "labelled",
            None,
            format!(
                "labelled {} under scheme {}{}",
                r.opt_text(0).map_err(err)?.unwrap_or("?"),
                r.opt_text(1)
                    .map_err(err)?
                    .map(|s| s.chars().take(12).collect::<String>())
                    .unwrap_or_default(),
                if flagged {
                    format!(
                        ", flagged{}",
                        r.opt_text(3)
                            .map_err(err)?
                            .map(|w| format!(": {w}"))
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                }
            ),
            Value::Null,
            "session_label",
        ));
    }
    Ok(Some(events))
}

// ------------------------------------------------------------------- stack

fn stack(store: &mut Store, id: i64) -> Result<Option<Vec<Event>>, String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT first_batch_id, modality, n_instances FROM {} WHERE id = {}",
        store.qualified("stack"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(id)]).map_err(err)? else {
        return Ok(None);
    };
    let batch = row.opt_int(0).map_err(err)?;
    let modality = row.opt_text(1).map_err(err)?.unwrap_or("?").to_string();
    let n = row.opt_int(2).map_err(err)?.unwrap_or(0);
    let mut events = Vec::new();
    let landed = match batch {
        Some(b) => batch_at(store, b)?.unwrap_or_default(),
        None => String::new(),
    };
    events.push(event(
        landed,
        "landed",
        None,
        format!("stack landed: {modality}, {n} instances"),
        batch.map_or(Value::Null, |b| produced("batch", b)),
        "stack",
    ));
    // classified
    let finished = text_of(store, "job", "finished_at");
    let started = text_of(store, "job", "started_at");
    let sql = format!(
        "SELECT c.job_id, c.pack, c.pack_version, c.epoch, c.review_items, c.overlay, {finished}, {started} \
         FROM {} c LEFT JOIN {} j ON j.id = c.job_id WHERE c.stack_id = {} ORDER BY c.id",
        store.qualified("classification"),
        store.qualified("job"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let job = r.opt_int(0).map_err(err)?;
        let at = r
            .opt_text(6)
            .map_err(err)?
            .or(r.opt_text(7).map_err(err)?)
            .unwrap_or_default();
        events.push(event(
            at,
            "classified",
            None,
            format!(
                "classified by pack {} {} at epoch {}{}{}",
                r.opt_text(1).map_err(err)?.unwrap_or("?"),
                r.opt_text(2).map_err(err)?.unwrap_or("?"),
                r.opt_int(3).map_err(err)?.unwrap_or(0),
                r.opt_text(5)
                    .map_err(err)?
                    .map(|o| format!(" under overlay {o}"))
                    .unwrap_or_default(),
                match r.opt_int(4).map_err(err)?.unwrap_or(0) {
                    0 => String::new(),
                    k => format!(", {k} review items opened"),
                }
            ),
            job.map_or(Value::Null, |j| produced("job", j)),
            "classification",
        ));
    }
    // decisions at stack scope
    let decided = text_of(store, "decision", "decided_at");
    let staged = text_of(store, "decision", "staged_at");
    let committed = text_of(store, "decision", "committed_at");
    let withdrawn = text_of(store, "decision", "withdrawn_at");
    let sql = format!(
        "SELECT id, axis, value, actor, author_kind, why, {decided}, {staged}, {committed}, {withdrawn} \
         FROM {} WHERE scope = 'stack' AND ref = {} ORDER BY id",
        store.qualified("decision"),
        d.param(1, Type::Text)
    );
    let key = id.to_string();
    for r in store
        .query(&sql, &[Param::from(key.as_str())])
        .map_err(err)?
    {
        events.extend(decision_events(&r)?);
    }
    // review items holding the stack
    let created = text_of(store, "review_item", "created_at");
    let reference = text_of(store, "review_item", "ref");
    let sql = format!(
        "SELECT i.id, i.kind, i.scope, i.status, {created}, i.actor FROM {} i \
         WHERE i.id IN (SELECT item_id FROM {} WHERE stack_id = {}) OR {reference} LIKE {} ORDER BY i.id",
        store.qualified("review_item"),
        store.qualified("review_member"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    let like = format!("%\"stack_id\":{id}%");
    for r in store
        .query(&sql, &[Param::Int(id), Param::from(like.as_str())])
        .map_err(err)?
    {
        let item = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(4).map_err(err)?.unwrap_or_default(),
            "review",
            r.opt_text(5).map_err(err)?,
            format!(
                "review item {item} opened: {} at {} scope, {}",
                r.text(1).map_err(err)?,
                r.text(2).map_err(err)?,
                r.text(3).map_err(err)?
            ),
            produced("review", item),
            "review_item",
        ));
    }
    // picked
    let pick_at = text_of(store, "pick", "decided_at");
    let sql = format!(
        "SELECT p.id, p.role, p.actor, {pick_at} FROM {} ps JOIN {} p ON p.id = ps.pick_id \
         WHERE ps.stack_id = {} ORDER BY p.id",
        store.qualified("pick_stack"),
        store.qualified("pick"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let pick = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(3).map_err(err)?.unwrap_or_default(),
            "picked",
            r.opt_text(2).map_err(err)?,
            format!("picked as {}", r.opt_text(1).map_err(err)?.unwrap_or("?")),
            produced("pick", pick),
            "pick",
        ));
    }
    // released
    let started = text_of(store, "release", "started_at");
    let sql = format!(
        "SELECT rel.id, rel.name, rel.version, {started}, rs.route FROM {} rs \
         JOIN {} rel ON rel.id = rs.release_id WHERE rs.stack_id = {} ORDER BY rel.id",
        store.qualified("release_stack"),
        store.qualified("release"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let rel = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(3).map_err(err)?.unwrap_or_default(),
            "released",
            None,
            format!(
                "released in {} {}{}",
                r.text(1).map_err(err)?,
                r.text(2).map_err(err)?,
                r.opt_text(4)
                    .map_err(err)?
                    .map(|route| format!(" as {route}"))
                    .unwrap_or_default()
            ),
            produced("release", rel),
            "release_stack",
        ));
    }
    Ok(Some(events))
}

/// The events one decision row carries: staged, committed, decided,
/// withdrawn, whichever stamps it has.
fn decision_events(r: &nils_registry::store::Row) -> Result<Vec<Event>, String> {
    let id = r.int(0).map_err(err)?;
    let axis = r.opt_text(1).map_err(err)?.unwrap_or("?").to_string();
    let value = r.opt_text(2).map_err(err)?.map(str::to_string);
    let actor = r.opt_text(3).map_err(err)?.map(str::to_string);
    let kind = r.opt_text(4).map_err(err)?.unwrap_or("person").to_string();
    let why = r.opt_text(5).map_err(err)?.map(str::to_string);
    let decided = r.opt_text(6).map_err(err)?.map(str::to_string);
    let staged = r.opt_text(7).map_err(err)?.map(str::to_string);
    let committed = r.opt_text(8).map_err(err)?.map(str::to_string);
    let withdrawn = r.opt_text(9).map_err(err)?.map(str::to_string);
    let what = format!(
        "{axis} = {}{}",
        value.as_deref().unwrap_or("no value"),
        why.as_ref().map(|w| format!(" ({w})")).unwrap_or_default()
    );
    let mut out = Vec::new();
    if let Some(at) = staged {
        out.push(event(
            at,
            "staged",
            actor.as_deref(),
            format!("decision {id} staged by a {kind}: {what}"),
            produced("decision", id),
            "decision",
        ));
    }
    if let Some(at) = committed {
        out.push(event(
            at,
            "committed",
            actor.as_deref(),
            format!("decision {id} committed: {what}"),
            produced("decision", id),
            "decision",
        ));
    } else if let Some(at) = decided {
        out.push(event(
            at,
            "decision",
            actor.as_deref(),
            format!("decision {id} by a {kind}: {what}"),
            produced("decision", id),
            "decision",
        ));
    }
    if let Some(at) = withdrawn {
        out.push(event(
            at,
            "withdrawn",
            None,
            format!("decision {id} withdrawn"),
            produced("decision", id),
            "decision",
        ));
    }
    Ok(out)
}

// --------------------------------------------------------------------- job

fn job(registry: &mut Registry, id: i64) -> Result<Option<Vec<Event>>, String> {
    let store = registry.store();
    let Some(j) = nils_registry::job::show(store, id).map_err(err)? else {
        return Ok(None);
    };
    let mut events = vec![event(
        j.started_at.clone(),
        "started",
        j.principal(),
        format!(
            "job {} started: {}{}",
            j.id,
            j.kind,
            j.name.as_ref().map(|n| format!(" {n}")).unwrap_or_default()
        ),
        Value::Null,
        "job",
    )];
    if let Some(at) = &j.finished_at {
        let produced = j
            .result
            .as_ref()
            .and_then(|r| {
                [
                    ("cohort_id", "cohort"),
                    ("handle", "handle"),
                    ("release", "release"),
                    ("batch", "batch"),
                    ("overlay", "overlay"),
                    ("handover", "handover"),
                ]
                .iter()
                .find_map(|(key, kind)| r[key].as_i64().map(|v| json!({ "kind": kind, "id": v })))
            })
            .unwrap_or(Value::Null);
        events.push(event(
            at.clone(),
            "finished",
            None,
            format!(
                "job {} {}{}",
                j.id,
                j.state.name(),
                j.error
                    .as_ref()
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default()
            ),
            produced,
            "job",
        ));
    }
    // a promotion names its handle in the result and its audit row
    if let Some(promoted) = j.result.as_ref().and_then(|r| r["handle_id"].as_i64()) {
        for (at, who, _, scope, _) in audit_rows(store, &["cohort.promote"])? {
            if scope["handle"].as_i64() == Some(promoted) {
                events.push(promotion(at, &who, &scope));
            }
        }
    }
    let d = store.dialect();
    // the batches, overlays and review items it wrote
    let finished = text_of(store, "ingest_batch", "finished_at");
    let started = text_of(store, "ingest_batch", "started_at");
    let sql = format!(
        "SELECT id, name, state, {finished}, {started} FROM {} WHERE job_id = {} ORDER BY id",
        store.qualified("ingest_batch"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let batch = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(3)
                .map_err(err)?
                .or(r.opt_text(4).map_err(err)?)
                .unwrap_or_default(),
            "batch",
            None,
            format!(
                "batch {batch} {}: {}",
                r.opt_text(1).map_err(err)?.unwrap_or(""),
                r.opt_text(2).map_err(err)?.unwrap_or("")
            ),
            produced("batch", batch),
            "ingest_batch",
        ));
    }
    let created = text_of(store, "overlay", "created_at");
    let sql = format!(
        "SELECT id, name, status, {created} FROM {} WHERE job_id = {} ORDER BY id",
        store.qualified("overlay"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let overlay = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(3).map_err(err)?.unwrap_or_default(),
            "overlay",
            None,
            format!(
                "overlay {} proposed, {}",
                r.opt_text(1).map_err(err)?.unwrap_or("?"),
                r.opt_text(2).map_err(err)?.unwrap_or("?")
            ),
            produced("overlay", overlay),
            "overlay",
        ));
    }
    let sql = format!(
        "SELECT COUNT(*), MIN(id), MAX(id) FROM {} WHERE job_id = {}",
        store.qualified("review_item"),
        d.param(1, Type::Int)
    );
    if let Some(r) = store.query_opt(&sql, &[Param::Int(id)]).map_err(err)? {
        let n = r.opt_int(0).map_err(err)?.unwrap_or(0);
        if n > 0 {
            events.push(event(
                j.finished_at.clone().unwrap_or_else(|| j.started_at.clone()),
                "review",
                None,
                format!(
                    "{n} review items opened, {} to {}",
                    r.opt_int(1).map_err(err)?.unwrap_or(0),
                    r.opt_int(2).map_err(err)?.unwrap_or(0)
                ),
                json!({ "kind": "review", "from": r.opt_int(1).map_err(err)?, "to": r.opt_int(2).map_err(err)? }),
                "review_item",
            ));
        }
    }
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE job_id = {}",
        store.qualified("classification"),
        d.param(1, Type::Int)
    );
    if let Some(r) = store.query_opt(&sql, &[Param::Int(id)]).map_err(err)? {
        let n = r.opt_int(0).map_err(err)?.unwrap_or(0);
        if n > 0 {
            events.push(event(
                j.finished_at
                    .clone()
                    .unwrap_or_else(|| j.started_at.clone()),
                "classified",
                None,
                format!("{n} stacks classified"),
                Value::Null,
                "classification",
            ));
        }
    }
    // what the audit recorded under this job
    let at = text_of(store, "audit", "at");
    let scope = text_of(store, "audit", "scope");
    let sql = format!(
        "SELECT {at}, principal, action, {scope} FROM {} WHERE job_id = {} ORDER BY id",
        store.qualified("audit"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let action = r.text(2).map_err(err)?.to_string();
        let scope: Value = r
            .opt_text(3)
            .map_err(err)?
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        events.push(event(
            r.opt_text(0).map_err(err)?.unwrap_or_default(),
            &action,
            r.opt_text(1).map_err(err)?,
            format!("{action}: {}", scope_summary(&scope)),
            audit_produced(&action, &scope),
            "audit",
        ));
    }
    Ok(Some(events))
}

fn scope_summary(scope: &Value) -> String {
    match scope.as_object() {
        Some(o) => o
            .iter()
            .filter(|(_, v)| !v.is_object() && !v.is_array())
            .map(|(k, v)| format!("{k} {}", v.as_str().map_or(v.to_string(), str::to_string)))
            .collect::<Vec<_>>()
            .join(", "),
        None => String::new(),
    }
}

fn audit_produced(action: &str, scope: &Value) -> Value {
    match action {
        "release" => {
            json!({ "kind": "release", "name": scope["release"], "version": scope["version"] })
        }
        "cohort.promote" | "cohort.create" => match scope["id"].as_i64() {
            Some(c) => produced("cohort", c),
            None => json!({ "kind": "cohort", "name": scope["cohort"] }),
        },
        "overlay.adopt" | "overlay.propose" => scope["overlay"]
            .as_i64()
            .map_or(Value::Null, |o| produced("overlay", o)),
        "decision" => scope["decision"]
            .as_i64()
            .map_or(Value::Null, |d| produced("decision", d)),
        "handover" => scope["handover"]
            .as_i64()
            .map_or(Value::Null, |h| produced("handover", h)),
        _ => Value::Null,
    }
}

// ----------------------------------------------------------------- release

fn release(registry: &mut Registry, id: i64) -> Result<Option<Vec<Event>>, String> {
    let store = registry.store();
    let d = store.dialect();
    let started = text_of(store, "release", "started_at");
    let finished = text_of(store, "release", "finished_at");
    let withdrawn = text_of(store, "release", "withdrawn_at");
    let sql = format!(
        "SELECT name, version, dataset_id, actor, {started}, {finished}, {withdrawn}, withdrawn_by, \
         withdrawn_why, files, subjects, unchanged, moved, rewritten, added, removed, error, previous_id \
         FROM {} WHERE id = {}",
        store.qualified("release"),
        d.param(1, Type::Int)
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(id)]).map_err(err)? else {
        return Ok(None);
    };
    let name = r.text(0).map_err(err)?.to_string();
    let version = r.text(1).map_err(err)?.to_string();
    let dataset = r.opt_int(2).map_err(err)?;
    let actor = r.opt_text(3).map_err(err)?.map(str::to_string);
    let mut events = vec![event(
        r.opt_text(4).map_err(err)?.unwrap_or_default(),
        "started",
        actor.as_deref(),
        format!(
            "release {name} {version} started{}",
            r.opt_int(17)
                .map_err(err)?
                .map(|p| format!(", after release {p}"))
                .unwrap_or_default()
        ),
        dataset.map_or(Value::Null, |ds| produced("dataset", ds)),
        "release",
    )];
    if let Some(at) = r.opt_text(5).map_err(err)? {
        let n = |i: usize| r.opt_int(i).map_err(err).map(|v| v.unwrap_or(0));
        events.push(event(
            at,
            "finished",
            actor.as_deref(),
            match r.opt_text(16).map_err(err)? {
                Some(e) => format!("release {name} {version} failed: {e}"),
                None => format!(
                    "release {name} {version} finished: {} files, {} subjects, {} unchanged, {} moved, {} rewritten, {} added, {} removed",
                    n(9)?,
                    n(10)?,
                    n(11)?,
                    n(12)?,
                    n(13)?,
                    n(14)?,
                    n(15)?
                ),
            },
            produced("release", id),
            "release",
        ));
    }
    if let Some(at) = r.opt_text(6).map_err(err)? {
        events.push(event(
            at,
            "withdrawn",
            r.opt_text(7).map_err(err)?,
            format!(
                "release {name} {version} withdrawn{}",
                r.opt_text(8)
                    .map_err(err)?
                    .map(|w| format!(": {w}"))
                    .unwrap_or_default()
            ),
            Value::Null,
            "release",
        ));
    }
    // handovers of this release
    let h_started = text_of(store, "handover", "started_at");
    let h_finished = text_of(store, "handover", "finished_at");
    let sql = format!(
        "SELECT id, strategy, actor, {h_started}, {h_finished}, archives, files, error FROM {} \
         WHERE release_id = {} ORDER BY id",
        store.qualified("handover"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let handover = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(3).map_err(err)?.unwrap_or_default(),
            "handover",
            r.opt_text(2).map_err(err)?,
            format!(
                "handover {handover} started, {}",
                r.opt_text(1).map_err(err)?.unwrap_or("?")
            ),
            produced("handover", handover),
            "handover",
        ));
        if let Some(at) = r.opt_text(4).map_err(err)? {
            events.push(event(
                at,
                "handover_finished",
                r.opt_text(2).map_err(err)?,
                match r.opt_text(7).map_err(err)? {
                    Some(e) => format!("handover {handover} failed: {e}"),
                    None => format!(
                        "handover {handover} finished: {} archives, {} files",
                        r.opt_int(5).map_err(err)?.unwrap_or(0),
                        r.opt_int(6).map_err(err)?.unwrap_or(0)
                    ),
                },
                produced("handover", handover),
                "handover",
            ));
        }
    }
    // the audit rows that name it
    for (at, who, action, scope, _) in audit_rows(store, &["release", "release.withdraw"])? {
        let named = scope["release"].as_str() == Some(name.as_str())
            && (scope["version"].as_str() == Some(version.as_str())
                || scope["version"] == json!(version)
                || scope["version"].is_null());
        let by_id = scope["id"].as_i64() == Some(id) || scope["release_id"].as_i64() == Some(id);
        if named || by_id {
            events.push(event(
                at,
                &action,
                Some(&who),
                format!("{action} recorded: {}", scope_summary(&scope)),
                produced("release", id),
                "audit",
            ));
        }
    }
    Ok(Some(events))
}

// ------------------------------------------------------------------ review

fn review(registry: &mut Registry, id: i64) -> Result<Option<Vec<Event>>, String> {
    let store = registry.store();
    let Some(item) = nils_registry::review::item(store, id).map_err(err)? else {
        return Ok(None);
    };
    let d = store.dialect();
    let created = text_of(store, "review_item", "created_at");
    let decided = text_of(store, "review_item", "decided_at");
    let accepted = text_of(store, "review_item", "accepted_at");
    let sql = format!(
        "SELECT {created}, actor, job_id, {decided}, decision, accepted_by, {accepted}, decision_id \
         FROM {} WHERE id = {}",
        store.qualified("review_item"),
        d.param(1, Type::Int)
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(id)]).map_err(err)? else {
        return Ok(None);
    };
    let job = r.opt_int(2).map_err(err)?;
    let mut events = vec![event(
        r.opt_text(0).map_err(err)?.unwrap_or_default(),
        "opened",
        r.opt_text(1).map_err(err)?,
        format!(
            "review item {id} opened: {} at {} scope{}",
            item.kind,
            item.scope,
            if item.members > 0 {
                format!(", {} members", item.members)
            } else {
                String::new()
            }
        ),
        job.map_or(Value::Null, |j| produced("job", j)),
        "review_item",
    )];
    if let Some(at) = r.opt_text(3).map_err(err)? {
        events.push(event(
            at,
            "decided",
            None,
            format!(
                "review item {id} decided{}: now {}",
                r.opt_text(4)
                    .map_err(err)?
                    .map(|v| format!(" as {v}"))
                    .unwrap_or_default(),
                item.status
            ),
            r.opt_int(7)
                .map_err(err)?
                .map_or(Value::Null, |dec| produced("decision", dec)),
            "review_item",
        ));
    }
    if let Some(at) = r.opt_text(6).map_err(err)? {
        events.push(event(
            at,
            "accepted",
            r.opt_text(5).map_err(err)?,
            format!("review item {id} accepted as it stands"),
            Value::Null,
            "review_item",
        ));
    }
    // members decided on their own
    let m_decided = text_of(store, "review_member", "decided_at");
    let sql = format!(
        "SELECT stack_id, {m_decided} FROM {} WHERE item_id = {} AND {m_decided} IS NOT NULL ORDER BY id",
        store.qualified("review_member"),
        d.param(1, Type::Int)
    );
    for r in store.query(&sql, &[Param::Int(id)]).map_err(err)? {
        let stack = r.int(0).map_err(err)?;
        events.push(event(
            r.opt_text(1).map_err(err)?.unwrap_or_default(),
            "member_decided",
            None,
            format!("member stack {stack} decided on its own"),
            produced("stack", stack),
            "review_member",
        ));
    }
    // the decisions the audit ties to this item, and their withdrawals
    let mut decisions = std::collections::HashSet::new();
    if let Some(dec) = r.opt_int(7).map_err(err)? {
        decisions.insert(dec);
    }
    let rows = audit_rows(store, &["decision", "review.accept"])?;
    for (at, who, action, scope, _) in &rows {
        if scope["review_item"].as_i64() == Some(id) || scope["item"].as_i64() == Some(id) {
            if let Some(dec) = scope["decision"].as_i64() {
                decisions.insert(dec);
            }
            events.push(event(
                at.clone(),
                action,
                Some(who),
                format!("{action}: {}", scope_summary(scope)),
                audit_produced(action, scope),
                "audit",
            ));
        }
    }
    for (at, who, _, scope, _) in &rows {
        if let Some(w) = scope["withdrawn"]
            .as_i64()
            .filter(|w| decisions.contains(w))
        {
            events.push(event(
                at.clone(),
                "withdrawn",
                Some(who),
                format!(
                    "decision {w} withdrawn, {} items reopened",
                    scope["reopened"]
                ),
                produced("decision", w),
                "audit",
            ));
        }
    }
    // the decision rows themselves, when the item names one
    if !decisions.is_empty() {
        let decided = text_of(store, "decision", "decided_at");
        let staged = text_of(store, "decision", "staged_at");
        let committed = text_of(store, "decision", "committed_at");
        let withdrawn = text_of(store, "decision", "withdrawn_at");
        let ids: Vec<String> = decisions.iter().map(|d| d.to_string()).collect();
        let sql = format!(
            "SELECT id, axis, value, actor, author_kind, why, {decided}, {staged}, {committed}, {withdrawn} \
             FROM {} WHERE id IN ({}) ORDER BY id",
            store.qualified("decision"),
            ids.join(", ")
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            events.extend(decision_events(&r)?);
        }
    }
    Ok(Some(events))
}
