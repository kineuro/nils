// SPDX-License-Identifier: AGPL-3.0-only

//! The ask doors of `nils serve` (Wave 4b §12.2): one door per operation,
//! authenticated per request, the catalog policy reading the caller's roles
//! from whichever auth mode supplied them, every statement run through a
//! read only reader (§12.4), handles and audit rows written through the
//! registry. A capped run is flagged truncated and has no hash; a token
//! with no role never reaches a door.

use std::collections::{BTreeSet, HashMap};

use nils_ask::affordance::{self, AffordanceError, Setting};
use nils_ask::ast::{Ask, SchemeRef};
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

use crate::serve::{Caller, Doors, Reply, Role, job_err, json_body};

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

/// The scope the catalog's policy gives a caller (§9, rule 15): a reader
/// asks; a reviewer projects quasi identifying fields; an operator sees
/// sensitive kinds and may project identifiers.
fn scope_of(caller: &Caller) -> Scope {
    let mut classes = BTreeSet::new();
    if caller.can(Role::Reviewer) {
        classes.insert(Class::QuasiIdentifying);
    }
    if caller.can(Role::Operator) {
        classes.insert(Class::Sensitive);
    }
    Scope {
        federated: false,
        classes,
    }
}

fn may_project_raw(caller: &Caller) -> bool {
    caller.can(Role::Operator)
}

/// The same scope from a role list alone, for the worker that runs a
/// queued job under the roles the door recorded (Wave 4c §6.1).
pub(crate) fn scope_of_roles(roles: &[Role]) -> Scope {
    let mut classes = BTreeSet::new();
    if roles.iter().any(|r| *r >= Role::Reviewer) {
        classes.insert(Class::QuasiIdentifying);
    }
    if roles.iter().any(|r| *r >= Role::Operator) {
        classes.insert(Class::Sensitive);
    }
    Scope {
        federated: false,
        classes,
    }
}

pub(crate) fn may_project_raw_of_roles(roles: &[Role]) -> bool {
    roles.iter().any(|r| *r >= Role::Operator)
}

/// Wave 4c §6.1: a handle's pages carry the classes the producing scope
/// allowed, recorded on the handle; a caller whose own scope is narrower is
/// refused, whoever produced it.
fn handle_within_scope(h: &handle::Handle, scope: &Scope, id: i64) -> Result<(), Reply> {
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
                "handle {id} holds {} fields, which this role's scope does not reach",
                beyond.join(" and ")
            ),
        ))
    }
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
            "issues": issues,
            "next": issues.iter().map(|i| i.next.clone()).collect::<Vec<_>>(),
        }),
        headers: Vec::new(),
        empty: false,
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
        RunError::Exec(ExecError::Timeout(ms)) => Reply::error(
            504,
            format!("the statement ran past {ms} ms; POST /api/ask/jobs runs it unbounded"),
        ),
        RunError::Selection(SelectionError::NotFound(n, v)) => {
            Reply::error(404, SelectionError::NotFound(n, v).to_string())
        }
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
    let principal = caller.principal.as_str();
    let (get, post, put) = (method == "GET", method == "POST", method == "PUT");
    let path = format!("/{}", segs.join("/"));
    let needs = match (method, segs) {
        ("PUT", ["api", "ask", "selections", _]) => Role::Reviewer,
        ("POST", ["api", "ask", "handles", _, "promote"])
        | ("POST", ["api", "sessions", "rebuild"]) => Role::Operator,
        _ => Role::Reader,
    };
    if !caller.can(needs) {
        return Err(Reply::error(
            403,
            format!(
                "{path} asks for the {} role; {principal} holds {}",
                needs.name(),
                if caller.roles.is_empty() {
                    "no role".to_string()
                } else {
                    caller
                        .roles
                        .iter()
                        .map(|r| r.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
        ));
    }
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
    let catalog: &Catalog = &catalog.as_ref().expect("the catalog is built").1;
    let reader: &mut Store = reader.as_mut().expect("the reader is open");
    let pack_version = pack.as_ref().map(|p| p.version.to_string());
    let doc = if post || put {
        json_body(body)?
    } else {
        json!({})
    };
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
            let prepared = prepare(ask, catalog, &scope).map_err(ask_err)?;
            Ok(Reply::ok(json!({
                "hash": prepared.hash,
                "warnings": prepared.validated.warnings,
                "pinned": prepared.pinned.iter().map(|(s, n, v)| json!({"set": s, "selection": n, "version": v})).collect::<Vec<_>>(),
                "repairs": repairs.iter().map(|r| json!({"path": r.path, "what": r.what})).collect::<Vec<_>>(),
                "order": prepared.validated.order,
            })))
        }
        ["api", "ask", "run"] if post => {
            let (ask, _) = document_of(registry, &doc)?;
            let scheme = scheme_of(registry, &ask)?;
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
            Ok(Reply::ok(json!({
                "handle": out.handle.id,
                "hash": out.hash,
                "grain": out.handle.grain,
                "row_count": n,
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
                    "identifiers are projected only by a role that may read them",
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
            // it under these roles and never its own.
            let job = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                doc["name"].as_str(),
                Some(principal),
                json!({
                    "roles": caller.roles.iter().map(|r| r.name()).collect::<Vec<_>>(),
                    "may_project_raw": may_project_raw(caller),
                }),
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(
                json!({"job": job, "document": id, "state": "queued"}),
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
            Ok(Reply::ok(serde_json::to_value(p).unwrap_or(Value::Null)))
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
            let d = affordance::describe(&ask, &s).map_err(affordance_err)?;
            Ok(Reply::ok(serde_json::to_value(d).unwrap_or(Value::Null)))
        }
        ["api", "ask", "documents"] if post => {
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
            let d = affordance::post(registry, &ask, &s).map_err(affordance_err)?;
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
        ["api", "ask", "selections", name] if put => {
            let (ask, _) = document_of(registry, &doc)?;
            let prepared = prepare(ask, catalog, &scope).map_err(ask_err)?;
            let saved = selection::save(
                registry,
                name,
                &prepared.ask,
                &prepared.hash,
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
            let pages = handle::page_count(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let rows = handle::page(registry.store(), id, page)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("handle {id} has {pages} pages")))?;
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
            Ok(Reply::ok(
                json!({"handle": id, "page": page, "pages": pages, "columns": h.columns, "rows": rows}),
            ))
        }
        ["api", "ask", "handles", _, "promote"] if post => {
            let id = id_at(3)?;
            let cohort = doc["cohort"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "cohort is required"))?;
            handle::get(registry.store(), id)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .ok_or_else(|| Reply::error(404, format!("no handle {id}")))?;
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
            let job = nils_registry::job::enqueue(
                registry.store(),
                &command,
                Some(cohort),
                Some(principal),
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
            let job = nils_registry::job::enqueue(
                registry.store(),
                &command,
                doc["scheme_name"].as_str(),
                Some(principal),
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
    "POST /api/ask/validate",
    "POST /api/ask/run",
    "POST /api/ask/jobs",
    "POST /api/ask/explain",
    "POST /api/ask/options",
    "POST /api/ask/apply",
    "POST /api/ask/diagnose",
    "POST /api/ask/preview",
    "POST /api/ask/describe",
    "POST /api/ask/documents",
    "GET /api/ask/documents/{id}",
    "PUT /api/ask/selections/{name}",
    "GET /api/ask/selections/{name}",
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
