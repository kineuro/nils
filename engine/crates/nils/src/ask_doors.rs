// SPDX-License-Identifier: AGPL-3.0-only

//! The ask doors of `nils serve` (Wave 4b §12.2): one door per operation,
//! authenticated per request, the catalog policy reading the caller's
//! detail from whichever auth mode supplied it, every statement run through
//! a read only reader (§12.4), handles and audit rows written through the
//! registry. A capped run is flagged truncated and has no hash; a token
//! with no grant never reaches a door.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nils_ask::affordance::{self, AffordanceError, Setting};
use nils_ask::ast::{Ask, Grain, SchemeRef};
use nils_ask::exec::{Bounds, ExecError};
use nils_ask::handle::{self, HandleError};
use nils_ask::moves::{MOVE_KINDS_CAP, MoveCall};
use nils_ask::run::{self, Request, RunError};
use nils_ask::selection::{self, SelectionError};
use nils_ask::validate::{Class, Scope};
use nils_ask::{Error as AskError, diagnose, document, parse, parse_repaired, prepare, values};
use nils_catalog::Catalog;
use nils_pack::Pack;
use nils_registry::Registry;
use nils_registry::session::Scheme;
use nils_registry::store::Store;
use serde_json::{Value, json};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply, job_err, json_body};

/// What a handler thread keeps between ask requests: the pack, the catalog
/// at an epoch, and the reader.
#[derive(Default)]
pub(crate) struct AskState {
    reader: Option<Store>,
    catalog: Option<(i64, Catalog)>,
    pack: Option<Pack>,
}

impl AskState {
    /// What the pack tells a model (§12.3), or none when it says nothing.
    pub(crate) fn model(
        &mut self,
        doors: &Doors,
        registry: &mut Registry,
    ) -> Option<nils_pack::mcp::Model> {
        self.ensure(doors, registry).ok()?;
        self.pack.as_ref().and_then(|p| p.mcp.clone())
    }

    fn ensure(&mut self, doors: &Doors, registry: &mut Registry) -> Result<(), Reply> {
        if self.pack.is_none() {
            let dir = doors
                .pack_dir
                .as_ref()
                .ok_or_else(|| Reply::error(500, "the ask doors need --pack-dir"))?;
            let path = dir.join(&doors.ask_pack);
            let pack = nils_pack::load(&path, None)
                .map_err(|e| Reply::error(500, format!("the pack {}: {e}", doors.ask_pack)))?;
            self.pack = Some(pack);
        }
        registry
            .refresh_meta()
            .map_err(|e| Reply::error(500, e.to_string()))?;
        let epoch = registry.meta().epoch;
        if self.catalog.as_ref().is_none_or(|(e, _)| *e != epoch) {
            let pack = self.pack.as_ref().expect("the pack");
            let catalog = Catalog::build(registry, pack)
                .map_err(|e| Reply::error(500, format!("the catalog: {e}")))?;
            self.catalog = Some((epoch, catalog));
        }
        if self.reader.is_none() {
            let r = registry
                .open_ask_reader(doors.ask_dsn.as_deref())
                .map_err(|e| Reply::error(500, format!("the reader: {e}")))?;
            self.reader = Some(r);
        }
        Ok(())
    }
}

/// The scope the catalog's policy gives a caller (§9, rule 15), read from
/// its detail: plain asks; quasi projects quasi identifying fields;
/// sensitive sees sensitive kinds and may project identifiers.
fn scope_of(caller: &Caller) -> Scope {
    let mut scope = scope_of_detail(caller.access.detail);
    // record 48, D1 of the move: the certificate's grant alone reads the
    // stacks of a sample sealed now
    scope.unsealed = crate::sealed::reads(&caller.access);
    scope
}

fn may_project_raw(caller: &Caller) -> bool {
    caller.access.detail >= Detail::Sensitive
}

/// The same scope from a detail alone, for the worker that runs a queued
/// job under the detail the door recorded (Wave 4c §6.1).
pub(crate) fn scope_of_detail(detail: Detail) -> Scope {
    let mut classes = BTreeSet::new();
    if detail >= Detail::Quasi {
        classes.insert(Class::QuasiIdentifying);
    }
    if detail >= Detail::Sensitive {
        classes.insert(Class::Sensitive);
    }
    Scope {
        federated: false,
        classes,
        unsealed: false,
    }
}

/// Wave 4c §6.1: a handle's pages carry the classes the producing scope
/// allowed, recorded on the handle; a caller whose own scope is narrower is
/// refused, whoever produced it.
/// Wave 5 §6.6: why a handle's answer is not `what` (hashed, exported,
/// released, pinned, promoted), in the order the desk shows: truncated,
/// stale (the registry moved, or the handle was invalidated), incomplete
/// (its rows dropped). None when it reproduces. Withdrawn is refused before
/// any of these by the doors that read rows.
pub(crate) fn not_reproducible(
    registry: &mut Registry,
    h: &handle::Handle,
    what: &str,
) -> Result<Option<String>, Reply> {
    if h.withdrawn_at.is_some() {
        return Ok(Some(format!(
            "handle {} was withdrawn and is not {what}",
            h.id
        )));
    }
    if h.truncated {
        return Ok(Some(format!(
            "a truncated answer is not {what}; narrow the question or run it as a job"
        )));
    }
    let epoch = registry.meta().epoch;
    if let Some(inv) = handle::invalidation(registry.store(), h.id)
        .map_err(|e| Reply::error(500, e.to_string()))?
    {
        return Ok(Some(format!(
            "a stale answer is not {what}; handle {} was invalidated at {}: {}",
            h.id, inv.at, inv.reason
        )));
    }
    if h.epoch != epoch {
        return Ok(Some(format!(
            "a stale answer is not {what}; the registry moved to epoch {epoch} since handle {} ran at {}",
            h.id, h.epoch
        )));
    }
    if !h.has_rows() {
        return Ok(Some(format!(
            "an incomplete answer is not {what}; the rows of handle {} were dropped by retention",
            h.id
        )));
    }
    Ok(None)
}

/// Record 55 H2: the selection a handle answered whole, when it did: its
/// answer is the set that read the selection, or a set of another grain
/// made of that set and nothing more (how a campaign, a run or a pyramid
/// job freezes a selection), so its row count is the selection's size.
fn whole_selection(h: &handle::Handle) -> Option<&str> {
    let ask = h.ask.as_ref()?;
    let out = ask.out.set.as_str();
    let pins = h.selection_versions.as_array()?;
    pins.iter().find_map(|p| {
        let set = p["set"].as_str()?;
        let whole = set == out
            || ask.sets.get(out).is_some_and(|s| {
                s.of.as_deref() == Some(set)
                    && serde_json::to_value(s)
                        .ok()
                        .and_then(|v| v.as_object().map(|o| o.len() == 2))
                        .unwrap_or(false)
            });
        whole.then(|| p["selection"].as_str()).flatten()
    })
}

/// Whether a handle's fields are all within the caller's detail.
fn classes_within(h: &handle::Handle, scope: &Scope) -> bool {
    let classes: BTreeSet<Class> =
        serde_json::from_value(h.suppression["classes"].clone()).unwrap_or_default();
    classes.iter().all(|c| scope.classes.contains(c))
}

fn handle_within_scope(h: &handle::Handle, scope: &Scope, id: i64) -> Result<(), Reply> {
    // record 48, D1 of the move: a handle that read the stacks of a sample
    // sealed now, or was answered before they were withheld, is opened only
    // by a scope that reads them
    if !scope.unsealed && h.suppression["sealed"] != "withheld" {
        return Err(Reply::error(
            403,
            format!(
                "handle {id} was answered with the stacks of a sealed certification sample in it (record 48); run its question again"
            ),
        ));
    }
    let classes: BTreeSet<Class> =
        serde_json::from_value(h.suppression["classes"].clone()).unwrap_or_default();
    let beyond: Vec<String> = classes
        .iter()
        .filter(|c| !scope.classes.contains(c))
        .map(|c| {
            serde_json::to_value(c)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        })
        .collect();
    if beyond.is_empty() {
        Ok(())
    } else {
        Err(Reply::error(
            403,
            format!(
                "handle {id} holds {} fields, which this caller's detail does not reach",
                beyond.join(" and ")
            ),
        ))
    }
}

/// Record 48, D1 of the move: a handle's page as a scope that does not read
/// sealed stacks reads it: a row of a stack (or an instance of one) sealed
/// since the handle was answered is left out. Answers how many were.
pub(crate) fn withhold_rows(
    store: &mut Store,
    h: &handle::Handle,
    scope: &Scope,
    rows: &mut Vec<Vec<Value>>,
) -> Result<usize, Reply> {
    // a page of keyed rows alone: a count or a group's row is no stack's
    if scope.unsealed
        || rows.is_empty()
        || h.columns.first().map(|c| c.name.as_str()) != Some("_key")
    {
        return Ok(0);
    }
    let keys: Vec<i64> = rows.iter().filter_map(|r| r.first()?.as_i64()).collect();
    let stack_of: BTreeMap<i64, i64> = match h.grain {
        Grain::Stack => keys.iter().map(|k| (*k, *k)).collect(),
        Grain::Instance => {
            let mut m = BTreeMap::new();
            for chunk in keys.chunks(500) {
                let list = chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT id, stack_id FROM {} WHERE id IN ({list})",
                    store.qualified("instance")
                );
                for r in store.query(&sql, &[])? {
                    m.insert(r.int(0)?, r.int(1)?);
                }
            }
            m
        }
        _ => return Ok(0),
    };
    let stacks: Vec<i64> = stack_of.values().copied().collect();
    let sealed = crate::sealed::now(store, &stacks)?;
    if sealed.is_empty() {
        return Ok(0);
    }
    let before = rows.len();
    rows.retain(|r| {
        !r.first()
            .and_then(Value::as_i64)
            .and_then(|k| stack_of.get(&k))
            .is_some_and(|s| sealed.contains(s))
    });
    Ok(before - rows.len())
}

fn scheme_of(registry: &mut Registry, ask: &Ask) -> Result<Scheme, Reply> {
    match &ask.scheme {
        None => Ok(Scheme::default()),
        Some(SchemeRef::Name(n)) if n == "default" || n == "day" => Ok(Scheme::default()),
        Some(SchemeRef::Name(n)) => crate::stored_scheme(registry, n).map_err(Reply::from),
        Some(SchemeRef::Inline(m)) => {
            let text = serde_json::to_string(m).unwrap_or_default();
            Scheme::from_json(&text)
                .map_err(|e| Reply::error(400, format!("the inline scheme: {e}")))
        }
    }
}

/// The document a body carries: the ask itself, or a document handle.
fn document_of(registry: &mut Registry, body: &Value) -> Result<(Ask, Option<i64>), Reply> {
    if let Some(id) = body["document_id"].as_i64() {
        let d = document::get(registry.store(), id)
            .map_err(|e| Reply::error(500, e.to_string()))?
            .ok_or_else(|| Reply::error(404, format!("no document {id}")))?;
        return Ok((d.ask, Some(id)));
    }
    if body["document"].is_object() {
        let ask = parse(&body["document"].to_string())
            .map_err(|e| Reply::error(400, format!("the document is refused: {e}")))?;
        return Ok((ask, None));
    }
    Err(Reply::error(
        400,
        "document (the ask as JSON) or document_id (a handle from POST /api/ask/documents)",
    ))
}

fn issues_reply(status: u16, what: &str, issues: &[nils_ask::validate::Issue]) -> Reply {
    Reply {
        status,
        body: json!({
            "error": what,
            // taxonomy issues name paths of the document, never a row
            "disclosure": "safe",
            "issues": issues,
            "next": issues.iter().map(|i| i.next.clone()).collect::<Vec<_>>(),
        }),
        headers: Vec::new(),
        empty: false,
        raw: None,
        file: None,
    }
}

fn ask_err(e: AskError) -> Reply {
    match e {
        AskError::Invalid(issues) => issues_reply(400, "the document is refused", &issues),
        other => Reply::error(400, other.to_string()),
    }
}

fn run_err(e: RunError) -> Reply {
    match e {
        RunError::Ask(a) => ask_err(a),
        RunError::Forbidden(m) => Reply::error(403, m),
        RunError::Handle(HandleError::NotFound(id)) => Reply::error(404, format!("no handle {id}")),
        RunError::Handle(
            e @ (HandleError::Expired(_) | HandleError::Withdrawn(_) | HandleError::Truncated(_)),
        ) => Reply::error(409, e.to_string()),
        e @ RunError::NameTaken(_) => Reply::error(409, e.to_string()),
        RunError::Exec(ExecError::Timeout(ms)) => Reply::error(
            504,
            format!("the statement ran past {ms} ms; POST /api/ask/jobs runs it unbounded"),
        ),
        RunError::Selection(SelectionError::NotFound(n, v)) => {
            Reply::error(404, SelectionError::NotFound(n, v).to_string())
        }
        // kineuro/nils#99: a measure that fails is the document's to change
        RunError::Measure(m) => issues_reply(400, "the document is refused", &[m.issue()]),
        other => Reply::error(500, other.to_string()),
    }
}

fn affordance_err(e: AffordanceError) -> Reply {
    match e {
        AffordanceError::Ask(a) => ask_err(a),
        AffordanceError::StaleOptions(i) => issues_reply(409, "stale_options", &[i]),
        AffordanceError::Move(m) => Reply::error(400, m.to_string()),
        AffordanceError::Document(document::DocumentError::NotFound(id)) => {
            Reply::error(404, format!("no document {id}"))
        }
        AffordanceError::Run(r) => run_err(r),
        other => Reply::error(500, other.to_string()),
    }
}

/// Route an ask door, or none when the path is not one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    state: &mut AskState,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    query: &HashMap<String, String>,
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let is_ask =
        matches!(segs, ["api", "ask", ..]) || matches!(segs, ["api", "sessions", "rebuild"]);
    if !is_ask {
        return None;
    }
    Some(routed(
        doors, registry, state, caller, method, segs, query, body,
    ))
}

/// Wave 4c §6.3: the doors whose repeat creates a row with an audit
/// consequence, so they take an idempotency key.
pub(crate) const IDEMPOTENT_DOORS: &[&str] = &[
    "POST /api/ask/run",
    "POST /api/ask/jobs",
    "POST /api/ask/apply",
    "POST /api/ask/handles/{id}/promote",
];

/// Wave 4c §6.3: on a writing door, a key the caller carried is looked up
/// first: the same body is answered again with `deduplicated: true`, a
/// different body under the same key is refused with 409, and a fresh key
/// records the answer for a day. Everything else goes straight through.
#[allow(clippy::too_many_arguments)]
fn routed(
    doors: &Doors,
    registry: &mut Registry,
    state: &mut AskState,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    query: &HashMap<String, String>,
    body: &str,
) -> Result<Reply, Reply> {
    let idempotent = method == "POST"
        && matches!(
            segs,
            ["api", "ask", "run"]
                | ["api", "ask", "jobs"]
                | ["api", "ask", "apply"]
                | ["api", "ask", "handles", _, "promote"]
        );
    let Some(key) = caller.idempotency_key.as_deref().filter(|_| idempotent) else {
        return answer(doors, registry, state, caller, method, segs, query, body);
    };
    let principal = caller.principal.as_str();
    let digest = nils_registry::idempotency::digest(body);
    match nils_registry::idempotency::lookup(registry.store(), principal, key)
        .map_err(|e| Reply::error(500, e.to_string()))?
    {
        Some(record) if record.digest == digest => {
            let mut body = record.reply;
            body["deduplicated"] = json!(true);
            return Ok(Reply {
                status: record.status as u16,
                body,
                headers: Vec::new(),
                empty: false,
                raw: None,
                file: None,
            });
        }
        Some(_) => {
            return Err(Reply::error(
                409,
                format!("Idempotency-Key {key} was already used with a different body"),
            ));
        }
        None => {}
    }
    let reply = answer(doors, registry, state, caller, method, segs, query, body)?;
    if reply.status < 300 {
        nils_registry::idempotency::record(
            registry.store(),
            principal,
            key,
            &digest,
            i64::from(reply.status),
            &reply.body,
        )
        .map_err(|e| Reply::error(500, e.to_string()))?;
    }
    Ok(reply)
}

#[allow(clippy::too_many_arguments)]
fn answer(
    doors: &Doors,
    registry: &mut Registry,
    state: &mut AskState,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    query: &HashMap<String, String>,
    body: &str,
) -> Result<Reply, Reply> {
    let principal = caller.principal.as_str();
    let (get, post, put) = (method == "GET", method == "POST", method == "PUT");
    let path = format!("/{}", segs.join("/"));
    // The door table every door reads (the suite contract, version 2).
    // Wave 5 §12.1: an identifier list is quasi-identifying territory, so
    // uploading one, and starting a question from one, needs query:work at
    // detail quasi; the resolver checks the second itself.
    let (need, detail) = crate::serve::door(method, segs);
    caller.allowed(&path, need, detail)?;
    state.ensure(doors, registry)?;
    let caps = &doors.ask_caps;
    let bounds = Bounds {
        timeout_ms: caps.sync_timeout_ms,
        max_rows: caps.sync_max_rows,
        max_bytes: caps.sync_max_bytes,
    };
    let scope = scope_of(caller);
    let epoch = registry.meta().epoch;
    let AskState {
        reader,
        catalog,
        pack,
    } = state;
    let doc = if post || put {
        json_body(body)?
    } else {
        json!({})
    };
    // A catalog is built at an epoch and a run moves no epoch, so a handle
    // written since is unknown to it until the next build. The resolver of
    // Wave 5 §12.1 starts from the handle a person just ran, so it reads the
    // handle itself and tells the catalog before it validates.
    if matches!(segs, ["api", "ask", "start"])
        && let Some(id) = doc["from"]["handle"].as_i64()
        && let Some((_, c)) = catalog.as_mut()
        && !c.handles.contains_key(&id.to_string())
        && let Ok(Some(h)) = handle::get(registry.store(), id)
        && h.withdrawn_at.is_none()
    {
        c.handles.insert(id.to_string(), h.grain);
    }
    let catalog: &Catalog = &catalog.as_ref().expect("the catalog is built").1;
    let reader: &mut Store = reader.as_mut().expect("the reader is open");
    let pack_version = pack.as_ref().map(|p| p.version.to_string());
    let id_at = |i: usize| -> Result<i64, Reply> {
        segs.get(i)
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| Reply::error(404, format!("{path} names no id")))
    };
    match segs {
        ["api", "ask", "schema"] if get => Ok(Reply::ok(json!({
            "generated": nils_ask::schema::generated(),
            "tightened": nils_ask::schema::tightened(),
            "digest": nils_ask::schema::digest(),
        }))),
        ["api", "ask", "catalog"] if get => Ok(Reply::ok(catalog.document(&scope))),
        ["api", "ask", "catalog", level] if get => {
            if !nils_ask::validate::levels().contains(level) {
                return Err(Reply::error(
                    404,
                    format!("{level} is not a level of the catalog"),
                ));
            }
            let page = catalog.page(
                level,
                &scope,
                query.get("after").map(String::as_str),
                caps.catalog_page_bytes as usize,
            );
            Ok(Reply::ok(serde_json::to_value(page).unwrap_or(Value::Null)))
        }
        ["api", "ask", "validate"] if post => {
            let repair = doc["mode"].as_str() == Some("repair");
            let (ask, repairs) = if repair {
                let text = doc["document"].to_string();
                parse_repaired(&text).map_err(ask_err)?
            } else {
                (document_of(registry, &doc)?.0, Vec::new())
            };
            let prepared = prepare(ask.clone(), catalog, &scope).map_err(ask_err)?;
            // Wave 4c: strict validate compiles what run would (kineuro/nils#93)
            if !repair {
                let scheme = scheme_of(registry, &ask)?;
                let s = Setting {
                    names: catalog,
                    scope: &scope,
                    scheme: &scheme,
                    principal,
                    bounds,
                    values_cap: caps.options_values as usize,
                };
                let issues = affordance::compile_issues(registry, &ask, &s);
                if !issues.is_empty() {
                    return Err(issues_reply(400, "the document is refused", &issues));
                }
            }
            Ok(Reply::ok(json!({
                "hash": prepared.hash,
                "bound_hash": prepared.bound_hash,
                "warnings": prepared.validated.warnings,
                "pinned": prepared.pinned.iter().map(|(s, n, v)| json!({"set": s, "selection": n, "version": v})).collect::<Vec<_>>(),
                "repairs": repairs.iter().map(|r| json!({"path": r.path, "what": r.what})).collect::<Vec<_>>(),
                "order": prepared.validated.order,
            })))
        }
        ["api", "ask", "run"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            let scheme = scheme_of(registry, &ask)?;
            // Wave 5 §12.8: the content hash as a cache key. A core that ran
            // at this epoch and pack answers its handle again, unless the
            // caller asks for a fresh run.
            // A named or kept run, or a paged or capped one, is a new
            // artefact by request and never the cache's.
            let plain = doc["name"].is_null()
                && !doc["keep"].as_bool().unwrap_or(false)
                && doc["after"].is_null()
                && doc["limit"].is_null();
            if plain
                && !doc["fresh"].as_bool().unwrap_or(false)
                && let Some((h, hash)) = run::cached(
                    registry,
                    ask.clone(),
                    catalog,
                    &scope,
                    pack_version.as_deref(),
                    &scheme,
                )
                .map_err(run_err)?
            {
                handle_within_scope(&h, &scope, h.id)?;
                let described = affordance::describe(
                    &ask,
                    &Setting {
                        names: catalog,
                        scope: &scope,
                        scheme: &scheme,
                        principal,
                        bounds,
                        values_cap: caps.options_values as usize,
                    },
                )
                .ok();
                let declaration = described.map(|d| {
                    nils_ask::describe::declaration(
                        &ask,
                        &d,
                        &scheme.digest(),
                        h.truncated,
                        &catalog.locale,
                    )
                });
                let pages = handle::page_count(registry.store(), h.id)
                    .map_err(|e| Reply::error(500, e.to_string()))?;
                let mut rows = handle::page(registry.store(), h.id, 0)
                    .map_err(|e| Reply::error(500, e.to_string()))?
                    .unwrap_or_default();
                withhold_rows(registry.store(), &h, &scope, &mut rows)?;
                handle::touch(registry.store(), h.id)
                    .map_err(|e| Reply::error(500, e.to_string()))?;
                let page_rows = caps.page_rows.max(1) as usize;
                return Ok(Reply::ok(json!({
                    "handle": h.id,
                    "hash": hash,
                    "grain": h.grain,
                    "row_count": h.row_count,
                    "declaration": declaration,
                    "content_hash": h.content_hash,
                    "truncated": h.truncated,
                    "columns": h.columns,
                    "rows": rows,
                    "page_rows": page_rows,
                    "pages": pages,
                    "measures": Value::Null,
                    "drift": [],
                    "kept": [],
                    "identifiers": [],
                    "inlined": [],
                    "epoch": epoch,
                    "cached": true,
                    "produced_at": h.created_at,
                    "produced_by": h.principal,
                    "fresh": "POST again with fresh: true for a new run",
                })));
            }
            // Wave 4c §6.4: the declaration block travels with the answer.
            let described = affordance::describe(
                &ask,
                &Setting {
                    names: catalog,
                    scope: &scope,
                    scheme: &scheme,
                    principal,
                    bounds,
                    values_cap: caps.options_values as usize,
                },
            )
            .ok();
            let asked = ask.clone();
            let out = run::run(
                registry,
                Request {
                    ask,
                    names: catalog,
                    scope: &scope,
                    principal,
                    node: &doors.node,
                    pack_version: pack_version.as_deref(),
                    scheme: &scheme,
                    bounds,
                    page_rows: caps.page_rows as usize,
                    name: doc["name"].as_str(),
                    keep: doc["keep"].as_bool().unwrap_or(false),
                    after: doc["after"].as_i64(),
                    limit: doc["limit"].as_u64(),
                    may_project_raw: may_project_raw(caller),
                    purpose: doc["purpose"].as_str(),
                    reader: Some(reader),
                },
            )
            .map_err(run_err)?;
            let page_rows = caps.page_rows.max(1) as usize;
            let rows: Vec<Value> = out
                .answer
                .rows
                .iter()
                .take(page_rows)
                .map(|r| Value::Array(r.0.iter().map(handle::cell_json).collect()))
                .collect();
            let n = out.answer.rows.len();
            let declaration = described.map(|d| {
                nils_ask::describe::declaration(
                    &asked,
                    &d,
                    &scheme.digest(),
                    out.answer.truncated,
                    &catalog.locale,
                )
            });
            Ok(Reply::ok(json!({
                "handle": out.handle.id,
                "hash": out.hash,
                "grain": out.handle.grain,
                "row_count": n,
                "declaration": declaration,
                "content_hash": out.handle.content_hash,
                "truncated": out.answer.truncated,
                "columns": out.answer.columns,
                "rows": rows,
                "page_rows": page_rows,
                "pages": n.div_ceil(page_rows),
                "measures": out.measured,
                "drift": out.drift,
                "kept": out.kept.iter().map(|k| json!({"handle": k.id, "name": k.name, "grain": k.grain, "row_count": k.row_count})).collect::<Vec<_>>(),
                "identifiers": out.identifiers,
                "inlined": out.inlined,
                "epoch": epoch,
            })))
        }
        ["api", "ask", "jobs"] if post => {
            let (ask, id) = document_of(registry, &doc)?;
            // Wave 4c §6.1: the job path refuses what the synchronous door
            // refuses, before anything is queued.
            if !ask.out.identifiers.is_empty() && !may_project_raw(caller) {
                return Err(Reply::error(
                    403,
                    "identifiers are projected only at detail sensitive",
                ));
            }
            let id = match id {
                Some(id) => id,
                None => {
                    let prepared = prepare(ask.clone(), catalog, &scope).map_err(ask_err)?;
                    document::put(registry.store(), &ask, &prepared.hash, principal, None)
                        .map_err(|e| Reply::error(500, e.to_string()))?
                        .id
                }
            };
            let mut command = vec![
                "ask".to_string(),
                "run".to_string(),
                "--document".to_string(),
                id.to_string(),
            ];
            if let Some(n) = doc["name"].as_str() {
                command.extend(["--name".into(), n.into()]);
            }
            if doc["keep"].as_bool().unwrap_or(false) {
                command.push("--keep".into());
            }
            if let Some(dir) = &doors.pack_dir {
                command.extend(["--pack-dir".into(), dir.display().to_string()]);
            }
            command.extend(["--pack".into(), doors.ask_pack.clone()]);
            // Wave 4c §6.1: the job carries the caller, so the worker runs
            // it under this detail and never its own.
            let mut by = crate::serve::queued_by(caller);
            by["may_project_raw"] = json!(may_project_raw(caller));
            let job = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                doc["name"].as_str(),
                Some(principal),
                by,
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(
                json!({"job": job, "document": id, "state": "queued"}),
            ))
        }
        ["api", "ask", "guide"] if get => {
            // Wave 4c §6.4: one source for what a caller is told, on both
            // transports: the pack's grounding and examples, the schema
            // digest, the doors' policy and the caps in force.
            let model = state.model(doors, registry);
            let schema_digest = state.catalog.as_ref().map(|(_, c)| c.schema_digest.clone());
            Ok(Reply::ok(json!({
                "grounding": model.as_ref().map(|m| m.grounding.clone()).unwrap_or_default(),
                "examples": model.as_ref().map(|m| m.examples.iter().map(|e| json!({"question": e.question, "document": e.document, "note": e.note})).collect::<Vec<_>>()).unwrap_or_default(),
                "content_version": model.as_ref().map(|m| m.version.clone()),
                "schema_digest": schema_digest,
                "policy": DOORS,
                "caps": caps,
            })))
        }
        ["api", "ask", "draft"] if post => {
            // Wave 4c §6.4: authored text in, add only repair, a diagnosis,
            // a stored document when it validates. Words stay outside.
            let text = doc["text"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "text: the document, as YAML or JSON"))?;
            let scheme = Scheme::default();
            let s = Setting {
                names: catalog,
                scope: &scope,
                scheme: &scheme,
                principal,
                bounds,
                values_cap: caps.options_values as usize,
            };
            let drafted =
                affordance::draft(registry, text, &s, Some(reader)).map_err(affordance_err)?;
            Ok(Reply::ok(
                serde_json::to_value(drafted).unwrap_or(Value::Null),
            ))
        }
        ["api", "ask", "diff"] if post => {
            // Wave 4c §6.4: one diff over the canonical form. Two documents,
            // two handles, or one of each.
            let side = |which: &str| -> Result<Value, Reply> {
                let v = &doc[which];
                if !v.is_object() {
                    return Err(Reply::error(
                        400,
                        format!("{which}: {{document}}, {{document_id}} or {{handle}}"),
                    ));
                }
                Ok(v.clone())
            };
            let a = side("a")?;
            let b = side("b")?;
            let mut handle_of = |v: &Value| -> Result<Option<handle::Handle>, Reply> {
                let Some(id) = v["handle"].as_i64() else {
                    return Ok(None);
                };
                let h = handle::get(registry.store(), id)
                    .map_err(|e| Reply::error(500, e.to_string()))?
                    .ok_or_else(|| Reply::error(404, format!("no handle {id}")))?;
                handle_within_scope(&h, &scope, id)?;
                Ok(Some(h))
            };
            let ha = handle_of(&a)?;
            let hb = handle_of(&b)?;
            match (ha, hb) {
                (Some(x), Some(y)) => Ok(Reply::ok(nils_ask::diff::handles(&x, &y))),
                (x, y) => {
                    let mut ask_of = |v: &Value, h: Option<handle::Handle>| -> Result<Ask, Reply> {
                        match h {
                            Some(h) => h.ask.ok_or_else(|| {
                                Reply::error(409, format!("handle {} keeps no document", h.id))
                            }),
                            None => Ok(document_of(registry, v)?.0),
                        }
                    };
                    let da = ask_of(&a, x)?;
                    let db = ask_of(&b, y)?;
                    Ok(Reply::ok(
                        serde_json::to_value(nils_ask::diff::documents(&da, &db))
                            .unwrap_or(Value::Null),
                    ))
                }
            }
        }
        ["api", "ask", "catalog", level, field, "values"] if get => {
            // Wave 4c §6.4: the value sampler, under the caller's scope.
            if !nils_ask::validate::levels().contains(level) {
                return Err(Reply::error(
                    404,
                    format!("{level} is not a level of the catalog"),
                ));
            }
            let scheme = Scheme::default();
            let s = Setting {
                names: catalog,
                scope: &scope,
                scheme: &scheme,
                principal,
                bounds,
                values_cap: caps.options_values as usize,
            };
            let cap = query
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(caps.options_values as usize)
                .clamp(1, caps.options_values as usize);
            let sample = affordance::values(registry, level, field, &s, cap, Some(reader))
                .map_err(affordance_err)?;
            Ok(Reply::ok(
                serde_json::to_value(sample).unwrap_or(Value::Null),
            ))
        }
        ["api", "ask", "explain"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            let scheme = scheme_of(registry, &ask)?;
            let ex = run::explain(registry, ask, catalog, &scope, &scheme).map_err(run_err)?;
            Ok(Reply::ok(serde_json::to_value(ex).unwrap_or(Value::Null)))
        }
        ["api", "ask", "options"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            let scheme = scheme_of(registry, &ask)?;
            let s = Setting {
                names: catalog,
                scope: &scope,
                scheme: &scheme,
                principal,
                bounds,
                values_cap: caps.options_values as usize,
            };
            let opts = affordance::options(epoch, &ask, doc["set"].as_str(), &s)
                .map_err(affordance_err)?;
            Ok(Reply::ok(serde_json::to_value(opts).unwrap_or(Value::Null)))
        }
        ["api", "ask", "apply"] if post => {
            let id = doc["document_id"].as_i64().ok_or_else(|| {
                Reply::error(
                    400,
                    "document_id is required; POST /api/ask/documents makes one",
                )
            })?;
            let at_epoch = doc["epoch"]
                .as_i64()
                .ok_or_else(|| Reply::error(400, "epoch is required"))?;
            let token = doc["token"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "token is required"))?;
            let set = doc["set"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "set is required"))?;
            let moves: Vec<MoveCall> = serde_json::from_value(doc["moves"].clone())
                .map_err(|e| Reply::error(400, format!("moves: {e}")))?;
            let d = document::get(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("no document {id}")))?;
            let scheme = scheme_of(registry, &d.ask)?;
            let s = Setting {
                names: catalog,
                scope: &scope,
                scheme: &scheme,
                principal,
                bounds,
                values_cap: caps.options_values as usize,
            };
            let applied = affordance::apply(registry, id, at_epoch, token, set, &moves, &s)
                .map_err(affordance_err)?;
            Ok(Reply::ok(
                serde_json::to_value(applied).unwrap_or(Value::Null),
            ))
        }
        ["api", "ask", "diagnose"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            let scheme = scheme_of(registry, &ask)?;
            let out_set = ask.out.set.clone();
            let d = diagnose::diagnose(
                registry,
                ask,
                Vec::new(),
                catalog,
                &scope,
                &scheme,
                bounds,
                doc["keys"].as_bool().unwrap_or(false),
                Some(reader),
            )
            .map_err(run_err)?;
            let d = match doc["by"].as_str() {
                None | Some("set") => d,
                Some("clause") => d.by_clause(&out_set),
                Some(other) => {
                    return Err(Reply::error(
                        400,
                        format!("by: {other} is not a keying of the funnel; set or clause"),
                    ));
                }
            };
            Ok(Reply::ok(serde_json::to_value(d).unwrap_or(Value::Null)))
        }
        ["api", "ask", "preview"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            let scheme = scheme_of(registry, &ask)?;
            let s = Setting {
                names: catalog,
                scope: &scope,
                scheme: &scheme,
                principal,
                bounds,
                values_cap: caps.options_values as usize,
            };
            let rows = doc["rows"]
                .as_u64()
                .unwrap_or(caps.preview_rows)
                .min(caps.page_rows_max);
            let p = affordance::preview(registry, &ask, rows, &s, Some(reader))
                .map_err(affordance_err)?;
            let declaration = affordance::describe(&ask, &s).ok().map(|d| {
                nils_ask::describe::declaration(
                    &ask,
                    &d,
                    &scheme.digest(),
                    p.truncated,
                    &catalog.locale,
                )
            });
            let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
            v["declaration"] = json!(declaration);
            Ok(Reply::ok(v))
        }
        ["api", "ask", "profile"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            let scheme = scheme_of(registry, &ask)?;
            let s = Setting {
                names: catalog,
                scope: &scope,
                scheme: &scheme,
                principal,
                bounds,
                values_cap: caps.options_values as usize,
            };
            let profile = crate::profile::document(
                registry,
                reader,
                &s,
                &ask,
                doc["set"].as_str(),
                doc["field"].as_str(),
            )
            .map_err(|why| Reply::error(404, why))?;
            Ok(Reply::ok(profile))
        }
        ["api", "ask", "describe"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            let scheme = scheme_of(registry, &ask)?;
            let s = Setting {
                names: catalog,
                scope: &scope,
                scheme: &scheme,
                principal,
                bounds,
                values_cap: caps.options_values as usize,
            };
            // Wave 4c §6.4: one node, so a chip label, a model's tool output
            // and an audit line are one string.
            if let Some(node) = doc.get("node").filter(|n| n.is_object()) {
                let set = node["set"].as_str().unwrap_or_default();
                let part = node["part"].as_str().unwrap_or("set");
                let index = node["index"].as_u64().unwrap_or(0) as usize;
                let found = nils_ask::describe::node(&ask, set, part, index).ok_or_else(|| {
                    Reply::error(404, format!("no node {part}[{index}] in set {set}"))
                })?;
                return Ok(Reply::ok(
                    serde_json::to_value(found).unwrap_or(Value::Null),
                ));
            }
            let d = affordance::describe(&ask, &s).map_err(affordance_err)?;
            let declaration =
                nils_ask::describe::declaration(&ask, &d, &scheme.digest(), false, &catalog.locale);
            let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
            v["declaration"] = json!(declaration);
            Ok(Reply::ok(v))
        }
        ["api", "ask", "start"] if post => {
            // Wave 5 §12.1, §7.1: anything a person may start from becomes the
            // opening set of a document, with its count and the sessions under it
            let from = &doc["from"];
            let mut sets = serde_json::Map::new();
            let mut params = serde_json::Map::new();
            let mut values_decl = serde_json::Map::new();
            let mut whole: Option<Ask> = None;
            let set_name;
            let grain: Grain;
            if let Some(list) = from["cohorts"].as_array() {
                let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
                if names.is_empty() {
                    return Err(Reply::error(400, "from.cohorts: a list of cohort names"));
                }
                params.insert("cohorts".into(), json!({"type": "list", "value": names}));
                sets.insert(
                    "scope".into(),
                    json!({"grain": "cohort", "where": [["in", {}, ["field", {}, "name"], ["param", {}, "cohorts"]]]}),
                );
                sets.insert("people".into(), json!({"grain": "subject", "of": "scope"}));
                set_name = "people".to_string();
                grain = Grain::Subject;
            } else if let Some(spec) = from["selection"].as_str() {
                let (name, version) = match spec.rsplit_once('@') {
                    Some((n, v)) => (n, v.parse::<u64>().ok()),
                    None => (spec, None),
                };
                let saved = selection::get(registry.store(), name, version)
                    .map_err(|e| Reply::error(500, e.to_string()))?
                    .ok_or_else(|| Reply::error(404, format!("no selection {spec}")))?;
                let g = saved
                    .ask
                    .sets
                    .get(&saved.ask.out.set)
                    .map(|s| s.grain)
                    .unwrap_or(Grain::Subject);
                sets.insert(
                    "start".into(),
                    json!({"grain": g.name(), "from": format!("selection:{name}@{}", saved.version)}),
                );
                set_name = "start".to_string();
                grain = g;
            } else if let Some(id) = from["handle"].as_i64() {
                let h = handle::get(registry.store(), id)
                    .map_err(|e| Reply::error(500, e.to_string()))?
                    .ok_or_else(|| Reply::error(404, format!("no handle {id}")))?;
                handle_within_scope(&h, &scope, id)?;
                sets.insert(
                    "start".into(),
                    json!({"grain": h.grain.name(), "from": format!("handle:{id}")}),
                );
                set_name = "start".to_string();
                grain = h.grain;
            } else if let Some(id) = from["document"].as_i64() {
                let d = document::get(registry.store(), id)
                    .map_err(|e| Reply::error(500, e.to_string()))?
                    .ok_or_else(|| Reply::error(404, format!("no document {id}")))?;
                grain = d
                    .ask
                    .sets
                    .get(&d.ask.out.set)
                    .map(|s| s.grain)
                    .unwrap_or(Grain::Subject);
                set_name = d.ask.out.set.clone();
                whole = Some(d.ask);
            } else if let Some(upload) = from["values"].as_str() {
                caller.allowed(
                    "starting from an uploaded list",
                    Need::One("query:work"),
                    Detail::Quasi,
                )?;
                values_decl.insert("list".into(), json!({"upload": upload}));
                sets.insert(
                    "people".into(),
                    json!({"grain": "subject", "from": "values:list"}),
                );
                set_name = "people".to_string();
                grain = Grain::Subject;
            } else if from.is_null() || from.as_object().is_some_and(|o| o.is_empty()) {
                sets.insert("everyone".into(), json!({"grain": "subject"}));
                set_name = "everyone".to_string();
                grain = Grain::Subject;
            } else {
                return Err(Reply::error(
                    400,
                    "from: {cohorts: [..]} | {selection: name@v} | {handle: id} | {document: id} | {values: upload_id} | {}",
                ));
            }
            // the sessions under the opening set, when the grain has any
            let under = matches!(grain, Grain::Cohort | Grain::Subject) && whole.is_none();
            let ask = match whole {
                Some(a) => a,
                None => {
                    if under {
                        sets.insert(
                            "sessions_under".into(),
                            json!({"grain": "session", "of": set_name}),
                        );
                    }
                    let mut d = json!({
                        "ast_version": 1,
                        "sets": sets,
                        "out": {"set": set_name, "level": "count"},
                    });
                    if !params.is_empty() {
                        d["params"] = Value::Object(params);
                    }
                    if !values_decl.is_empty() {
                        d["values"] = Value::Object(values_decl);
                    }
                    parse(&d.to_string()).map_err(|e| {
                        Reply::error(400, format!("the opening document is refused: {e}"))
                    })?
                }
            };
            let scheme = scheme_of(registry, &ask)?;
            let opening = ask.clone();
            let diagnosis = diagnose::diagnose(
                registry,
                ask,
                Vec::new(),
                catalog,
                &scope,
                &scheme,
                bounds,
                false,
                Some(reader),
            )
            .map_err(run_err)?;
            if !diagnosis.valid {
                return Err(issues_reply(400, "the opening document", &diagnosis.issues));
            }
            let stage_of = |name: &str| {
                diagnosis
                    .funnel
                    .iter()
                    .rfind(|s| s.set == name)
                    .map(|s| (s.rows, s.subjects))
            };
            let (count, subjects) = stage_of(&set_name).unwrap_or((0, 0));
            let sessions = if under {
                stage_of("sessions_under").map(|(rows, _)| rows)
            } else if grain == Grain::Session {
                Some(count)
            } else {
                None
            };
            // the sessions helper set is the resolver's, not the document's
            let mut document = serde_json::to_value(&opening).unwrap_or(Value::Null);
            if under && let Some(m) = document["sets"].as_object_mut() {
                m.remove("sessions_under");
            }
            Ok(Reply::ok(json!({
                "document": document,
                "set": set_name,
                "grain": grain.name(),
                "count": count,
                "subjects": subjects,
                "sessions": sessions,
                "epoch": epoch,
            })))
        }
        ["api", "ask", "documents"] if get => {
            // Wave 5 §12.1: documents folded into lineages by their parent
            // chain, newest lineage first, each with its last run
            let limit = query
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(100)
                .clamp(1, caps.page_rows_max as usize);
            let after = query.get("after").and_then(|a| a.parse::<i64>().ok());
            let all =
                document::list(registry.store()).map_err(|e| Reply::error(500, e.to_string()))?;
            let by_id: HashMap<i64, usize> =
                all.iter().enumerate().map(|(i, d)| (d.id, i)).collect();
            let root_of = |mut i: usize| {
                let mut seen = 0;
                while let Some(&p) = all[i].parent_id.and_then(|p| by_id.get(&p)) {
                    i = p;
                    seen += 1;
                    if seen > 10_000 {
                        break;
                    }
                }
                all[i].id
            };
            let mut lineages: std::collections::BTreeMap<i64, Vec<usize>> =
                std::collections::BTreeMap::new();
            for i in 0..all.len() {
                lineages.entry(root_of(i)).or_default().push(i);
            }
            // the last run of any version: the newest handle whose ask hash is
            // one of the lineage's hashes
            let handles = handle::list(registry.store(), true)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let mut last_run: HashMap<String, (i64, String)> = HashMap::new();
            for h in &handles {
                if let Some(hash) = h.ask_hash() {
                    last_run
                        .entry(hash)
                        .and_modify(|(id, at)| {
                            if h.id > *id {
                                *id = h.id;
                                *at = h.created_at.clone();
                            }
                        })
                        .or_insert((h.id, h.created_at.clone()));
                }
            }
            let mut rows: Vec<Value> = lineages
                .values()
                .map(|members| {
                    let versions: Vec<&document::Document> =
                        members.iter().map(|&i| &all[i]).collect();
                    let latest = *versions
                        .iter()
                        .max_by_key(|d| d.id)
                        .expect("a lineage has a member");
                    let root = versions[0];
                    let grain = latest
                        .ask
                        .sets
                        .get(&latest.ask.out.set)
                        .map(|s| s.grain.name().to_string());
                    let run = versions
                        .iter()
                        .filter_map(|d| last_run.get(&d.hash))
                        .max_by_key(|(id, _)| *id);
                    json!({
                        "document": latest.id,
                        "root": root.id,
                        "name": latest.ask.name,
                        "grain": grain,
                        "out": latest.ask.out.set,
                        "level": latest.ask.out.level,
                        "versions": versions.len(),
                        "author": root.principal,
                        "created_at": root.created_at,
                        "updated_at": latest.created_at,
                        "last_used_at": versions.iter().map(|d| d.last_used_at.as_str()).max(),
                        "hash": latest.hash,
                        "last_run": run.map(|(id, at)| json!({"handle": id, "at": at})),
                    })
                })
                .collect();
            rows.sort_by_key(|r| std::cmp::Reverse(r["document"].as_i64().unwrap_or(0)));
            let page: Vec<Value> = rows
                .into_iter()
                .filter(|r| after.is_none_or(|a| r["document"].as_i64().unwrap_or(0) < a))
                .take(limit)
                .collect();
            let next = if page.len() == limit {
                page.last().and_then(|r| r["document"].as_i64())
            } else {
                None
            };
            Ok(Reply::ok(
                json!({"count": page.len(), "documents": page, "next": next}),
            ))
        }
        ["api", "ask", "documents"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            // stored under another document, it is that document's next
            // version: an accepted proposal joins the line it was proposed for
            let parent = match doc.get("parent") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    let p = v.as_i64().ok_or_else(|| {
                        Reply::error(400, "parent: the id of the document this one follows")
                    })?;
                    document::get(registry.store(), p)
                        .map_err(|e| Reply::error(500, e.to_string()))?
                        .ok_or_else(|| Reply::error(404, format!("no document {p}")))?;
                    Some(p)
                }
            };
            let scheme = scheme_of(registry, &ask)?;
            let s = Setting {
                names: catalog,
                scope: &scope,
                scheme: &scheme,
                principal,
                bounds,
                values_cap: caps.options_values as usize,
            };
            let d = affordance::post(registry, &ask, &s, parent).map_err(affordance_err)?;
            Ok(Reply::ok(
                json!({"document": d.id, "hash": d.hash, "digest": d.digest, "parent": d.parent_id}),
            ))
        }
        ["api", "ask", "documents", _] if get => {
            let id = id_at(3)?;
            let d = document::get(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("no document {id}")))?;
            Ok(Reply::ok(json!({
                "document": d.id, "hash": d.hash, "digest": d.digest, "principal": d.principal,
                "created_at": d.created_at, "last_used_at": d.last_used_at, "parent": d.parent_id,
                "ask": d.ask,
            })))
        }
        ["api", "ask", "selections"] if get => {
            // record 55 H2: the saved selections as a list, by name, a page
            // at a time: each with its versions, who made it and the last
            // version, when, and its size as the newest answer this caller
            // may open that froze it whole says it
            let limit = query
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(50)
                .clamp(1, caps.page_rows_max as usize);
            let after = query.get("after").map(String::as_str);
            let needle = query
                .get("q")
                .map(|q| q.trim().to_lowercase())
                .filter(|q| !q.is_empty());
            let store = registry.store();
            let all = selection::list(store).map_err(|e| Reply::error(500, e.to_string()))?;
            let total = all.len();
            let matching: Vec<&selection::Listed> = all
                .iter()
                .filter(|l| {
                    needle
                        .as_ref()
                        .is_none_or(|q| l.name.to_lowercase().contains(q))
                })
                .collect();
            let page: Vec<&selection::Listed> = matching
                .iter()
                .filter(|l| after.is_none_or(|a| l.name.as_str() > a))
                .take(limit)
                .copied()
                .collect();
            let cohorts: HashMap<i64, String> = nils_registry::cohort::all(store)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .into_iter()
                .map(|c| (c.id, c.name))
                .collect();
            // the newest handle of each selection that froze it whole: its
            // answer is the selection's set. Its fields must be within the
            // caller's detail; a frozen handle keeps the stacks of a sample
            // sealed now, which a caller who does not read them has left out
            // of the count, as a run of the question would
            let mut handles =
                handle::list(store, false).map_err(|e| Reply::error(500, e.to_string()))?;
            handles.sort_by_key(|h| std::cmp::Reverse(h.id));
            let mut sized: HashMap<String, (&handle::Handle, i64)> = HashMap::new();
            for h in &handles {
                let Some(name) = whole_selection(h) else {
                    continue;
                };
                if sized.contains_key(name) || !page.iter().any(|l| l.name == name) {
                    continue;
                }
                if !classes_within(h, &scope) {
                    continue;
                }
                let rows = if scope.unsealed || h.suppression["sealed"] == "withheld" {
                    h.row_count
                } else if h.grain == Grain::Stack && h.has_rows() {
                    let keys: Vec<i64> = crate::campaigns::handle_keys(store, h.id, Grain::Stack)?
                        .into_iter()
                        .map(|(k, _)| k)
                        .collect();
                    let sealed = crate::sealed::now(store, &keys)?;
                    (keys.len() - sealed.len()) as i64
                } else {
                    continue;
                };
                sized.insert(name.to_string(), (h, rows));
            }
            let mut rows = Vec::with_capacity(page.len());
            for l in &page {
                let current = selection::get(store, &l.name, None)
                    .map_err(|e| Reply::error(500, e.to_string()))?;
                let grain = current.as_ref().and_then(|v| {
                    v.ask
                        .sets
                        .get(&v.ask.out.set)
                        .map(|s| s.grain.name().to_string())
                });
                let size = sized.get(l.name.as_str()).map(|(h, count)| {
                    let version = h
                        .selection_versions
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|p| p["selection"].as_str() == Some(l.name.as_str()))
                        .and_then(|p| p["version"].as_u64());
                    json!({
                        "rows": count, "grain": h.grain, "version": version,
                        "handle": h.id, "at": h.created_at, "truncated": h.truncated,
                    })
                });
                rows.push(json!({
                    "id": l.id,
                    "name": l.name,
                    "spec": format!("selection:{}@{}", l.name, l.current_version),
                    "description": l.description,
                    "versions": l.current_version,
                    "grain": grain,
                    "owner": l.owner,
                    "created_at": l.created_at,
                    "updated_at": current.as_ref().map(|v| v.created_at.clone()),
                    "updated_by": current.as_ref().map(|v| v.actor.clone()),
                    "note": current.as_ref().and_then(|v| v.note.clone()),
                    "cohort": l.cohort_id.and_then(|c| cohorts.get(&c).cloned()),
                    "size": size,
                }));
            }
            let next = if page.len() == limit
                && matching
                    .iter()
                    .any(|l| page.last().is_some_and(|p| l.name > p.name))
            {
                page.last().map(|l| l.name.clone())
            } else {
                None
            };
            Ok(Reply::ok(json!({
                "count": rows.len(),
                "total": total,
                "matching": matching.len(),
                "selections": rows,
                "next": next,
            })))
        }
        ["api", "ask", "selections", name] if put => {
            let (ask, _) = document_of(registry, &doc)?;
            let prepared = prepare(ask, catalog, &scope).map_err(ask_err)?;
            let saved = selection::save(
                registry,
                name,
                &prepared.ask,
                &prepared.bound_hash,
                principal,
                doc["note"].as_str(),
                doc["description"].as_str(),
            )
            .map_err(|e| match e {
                SelectionError::NameIsACohort(_) => Reply::error(409, e.to_string()),
                other => Reply::error(500, other.to_string()),
            })?;
            Ok(Reply::ok(
                serde_json::to_value(saved).unwrap_or(Value::Null),
            ))
        }
        ["api", "ask", "selections", spec] if get => {
            let (name, version) = match spec.rsplit_once('@') {
                Some((n, v)) => (
                    n,
                    Some(
                        v.parse::<u64>()
                            .map_err(|_| Reply::error(400, format!("{v} is not a version")))?,
                    ),
                ),
                None => (*spec, None),
            };
            let v = selection::get(registry.store(), name, version)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("no selection {spec}")))?;
            Ok(Reply::ok(serde_json::to_value(v).unwrap_or(Value::Null)))
        }
        ["api", "ask", "handles"] if get => {
            // Wave 4c §7.4: the result surface lists what runs left. Newest
            // first, within the caller's scope: a handle holding fields
            // beyond the caller's detail is left out, not refused, so the list
            // is what this caller may open. `withdrawn=1` includes the
            // withdrawn ones; `limit` caps the answer at page_rows_max.
            let withdrawn = query
                .get("withdrawn")
                .is_some_and(|w| w == "1" || w == "true");
            let limit = query
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(100)
                .clamp(1, caps.page_rows_max as usize);
            let all = handle::list(registry.store(), withdrawn)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let out = handle::invalidated(registry.store())
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let mine: Vec<Value> = all
                .iter()
                .filter(|h| handle_within_scope(h, &scope, h.id).is_ok())
                .take(limit)
                .map(|h| {
                    json!({
                        "invalidated": out.contains(&h.id),
                        "stale": out.contains(&h.id) || h.epoch != epoch,
                        "id": h.id, "name": h.name, "grain": h.grain, "row_count": h.row_count,
                        "content_hash": h.content_hash, "principal": h.principal, "actor": h.actor,
                        "created_at": h.created_at, "epoch": h.epoch, "pack_version": h.pack_version,
                        "disclosure": h.disclosure, "truncated": h.truncated,
                        "limit": h.ask.as_ref().and_then(|a| a.out.limit),
                        "kept": h.has_rows(), "last_read_at": h.last_read_at,
                        "withdrawn_at": h.withdrawn_at, "ask_hash": h.ask_hash(),
                        "columns": h.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(Reply::ok(json!({"count": mine.len(), "handles": mine})))
        }
        ["api", "ask", "handles", _] if get => {
            let id = id_at(3)?;
            let h = handle::get(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("no handle {id}")))?;
            handle_within_scope(&h, &scope, id)?;
            let pins = handle::pinned_by(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let mut v = serde_json::to_value(&h).unwrap_or(Value::Null);
            v["pinned_by"] = json!(pins);
            // Wave 5 §12.8: the invalidation, when there is one, and what
            // it means for the answer
            let inv = handle::invalidation(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            v["stale"] = json!(inv.is_some() || h.epoch != epoch);
            v["invalidated"] = serde_json::to_value(inv).unwrap_or(Value::Null);
            v["reads"] = json!(
                handle::read_count(registry.store(), id)
                    .map_err(|e| Reply::error(500, e.to_string()))?
            );
            v["pages"] = json!(
                handle::page_count(registry.store(), id)
                    .map_err(|e| Reply::error(500, e.to_string()))?
            );
            Ok(Reply::ok(v))
        }
        ["api", "ask", "handles", _, "rows"] if get => {
            let id = id_at(3)?;
            let page = query
                .get("page")
                .and_then(|p| p.parse::<i64>().ok())
                .unwrap_or(0);
            let h = handle::get(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("no handle {id}")))?;
            handle_within_scope(&h, &scope, id)?;
            if !h.has_rows() {
                return Err(Reply::error(404, HandleError::Expired(id).to_string()));
            }
            // Wave 5 §6.6: a page read for an export or a promotion is
            // refused at the server when the answer no longer reproduces
            if let Some(purpose) = query.get("purpose").map(String::as_str)
                && matches!(purpose, "export" | "promote" | "release" | "pin")
                && let Some(why) = not_reproducible(registry, &h, &format!("{purpose}d"))?
            {
                return Err(Reply::error(409, why));
            }
            let pages = handle::page_count(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let mut rows = handle::page(registry.store(), id, page)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("handle {id} has {pages} pages")))?;
            let withheld = withhold_rows(registry.store(), &h, &scope, &mut rows)?;
            // Wave 4c §6.1: every page read is audited, not only a reveal.
            let epoch = registry.meta().epoch;
            let columns: Vec<String> = h.columns.iter().map(|c| c.name.clone()).collect();
            handle::read_audit(
                registry.store(),
                principal,
                id,
                &columns,
                rows.len(),
                query.get("purpose").map(String::as_str),
                epoch,
            )
            .map_err(|e| Reply::error(500, e.to_string()))?;
            let mut v = json!({"handle": id, "page": page, "pages": pages, "columns": h.columns, "rows": rows});
            if withheld > 0 {
                v["withheld_sealed"] = json!(withheld);
            }
            Ok(Reply::ok(v))
        }
        ["api", "ask", "handles", _, "promote"] if post => {
            let id = id_at(3)?;
            let cohort = doc["cohort"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "cohort is required"))?;
            let h = handle::get(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("no handle {id}")))?;
            // Wave 5 §6.6: a stale answer is not promoted, and the reason
            // comes in the desk's order
            if let Some(why) = not_reproducible(registry, &h, "promoted")? {
                return Err(Reply::error(409, why));
            }
            let mut command = vec![
                "ask".to_string(),
                "promote".to_string(),
                "--handle".to_string(),
                id.to_string(),
                "--cohort".to_string(),
                cohort.to_string(),
            ];
            if doc["create"].as_bool().unwrap_or(false) {
                command.push("--create".into());
            }
            if let Some(r) = doc["reason"].as_str() {
                command.extend(["--reason".into(), r.into()]);
            }
            let job = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                Some(cohort),
                Some(principal),
                crate::serve::queued_by(caller),
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(
                json!({"job": job, "state": "queued", "handle": id, "cohort": cohort}),
            ))
        }
        ["api", "ask", "values"] if post => {
            let namespace = doc["namespace"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "namespace is required"))?;
            let list: Vec<String> = doc["values"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if list.is_empty() {
                return Err(Reply::error(400, "values: a list of identifiers"));
            }
            if list.len() as u64 > caps.values_inline_rows {
                return Err(Reply::error(
                    400,
                    format!(
                        "{} values; the inline cap is {}",
                        list.len(),
                        caps.values_inline_rows
                    ),
                ));
            }
            let up =
                values::upload(registry, namespace, &list, principal).map_err(|e| match e {
                    values::ValuesError::UnknownNamespace(_) | values::ValuesError::Empty => {
                        Reply::error(400, e.to_string())
                    }
                    other => Reply::error(500, other.to_string()),
                })?;
            Ok(Reply::ok(serde_json::to_value(up).unwrap_or(Value::Null)))
        }
        ["api", "sessions", "rebuild"] if post => {
            let mut command = vec!["session".to_string(), "rebuild".to_string()];
            if let Some(n) = doc["scheme_name"].as_str() {
                command.extend(["--scheme-name".into(), n.into()]);
            }
            if doc["force"].as_bool().unwrap_or(false) {
                command.push("--force".into());
            }
            let job = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                doc["scheme_name"].as_str(),
                Some(principal),
                crate::serve::queued_by(caller),
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(json!({"job": job, "state": "queued"})))
        }
        _ => Err(Reply::error(
            404,
            format!("{method} {path} is not a door; GET /api/capabilities lists them"),
        )),
    }
}

/// The ask doors, by method and path.
pub(crate) const DOORS: &[&str] = &[
    "GET /api/ask/schema",
    "GET /api/ask/catalog",
    "GET /api/ask/catalog/{level}",
    "GET /api/ask/catalog/{level}/{field}/values",
    "GET /api/ask/guide",
    "POST /api/ask/draft",
    "POST /api/ask/diff",
    "POST /api/ask/validate",
    "POST /api/ask/run",
    "POST /api/ask/jobs",
    "POST /api/ask/explain",
    "POST /api/ask/options",
    "POST /api/ask/apply",
    "POST /api/ask/diagnose",
    "POST /api/ask/preview",
    "POST /api/ask/profile",
    "POST /api/ask/describe",
    "POST /api/ask/start",
    "GET /api/ask/documents",
    "POST /api/ask/documents",
    "GET /api/ask/documents/{id}",
    "GET /api/ask/selections",
    "PUT /api/ask/selections/{name}",
    "GET /api/ask/selections/{name}",
    "GET /api/ask/handles",
    "GET /api/ask/handles/{id}",
    "GET /api/ask/handles/{id}/rows",
    "POST /api/ask/handles/{id}/promote",
    "POST /api/ask/values",
    "POST /api/sessions/rebuild",
];

/// `capabilities.ask`: the caps, the schema digests, the epoch, the move
/// cap, how the reader is opened, and the doors.
pub(crate) fn capabilities(doors: &Doors, registry: &mut Registry, state: &mut AskState) -> Value {
    let schema_digest = state
        .ensure(doors, registry)
        .ok()
        .and_then(|_| state.catalog.as_ref().map(|(_, c)| c.schema_digest.clone()));
    json!({
        "caps": doors.ask_caps,
        "schema_digest": schema_digest,
        "ask_schema_digest": nils_ask::schema::digest(),
        "epoch": registry.meta().epoch,
        "move_kinds_cap": MOVE_KINDS_CAP,
        "pack": doors.ask_pack,
        "reader": if doors.ask_dsn.is_some() { "a dedicated role" } else { "the registry, read only" },
        "doors": DOORS,
    })
}
