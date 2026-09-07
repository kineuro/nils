// SPDX-License-Identifier: AGPL-3.0-only

//! The MCP door (Wave 4b §12.3): one curated tool list over streamable
//! HTTP, in the engine and not generated from the endpoint list. Which
//! doors reach a model, what each says about itself, the grounding rules
//! and the worked examples all ship in the pack, so a model behaviour
//! change is a pack release and never an engine one. A domain refusal
//! comes back as text with `isError`, never as a transport error, because
//! a model reads text and retries. Every page is bounded by the caps.
//!
//! Identity is the doors' (§12.4): the same bearer token, the same roles,
//! the same catalog policy. What this door adds is RFC 9728: a protected
//! resource metadata document, a 401 that names it in `WWW-Authenticate`,
//! and a 403 that says `insufficient_scope`. Audience binding to the MCP
//! resource is a named, dated deviation (2026-09-07): the engine accepts
//! the audience its own doors accept until a client that speaks OAuth
//! exists to bind a second one.

use std::collections::HashMap;

use nils_registry::Registry;
use serde_json::{Value, json};

use crate::ask_doors::AskState;
use crate::serve::{Caller, Doors, Reply, Role};

/// The protocol versions this door speaks, newest first.
pub(crate) const PROTOCOL: &[&str] = &["2025-06-18", "2025-03-26"];

/// The path the door listens on.
pub(crate) const PATH: &str = "/mcp";

/// Bytes of rendered JSON one tool result may carry before it is cut.
const RESULT_BYTES: usize = 16 * 1024;

fn error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn result(id: &Value, value: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": value})
}

/// What a tool takes, by operation.
fn input_schema(operation: &str) -> Value {
    let document = json!({
        "type": "object",
        "description": "The ask as JSON, as the schema of GET /api/ask/schema describes it",
    });
    let document_id = json!({
        "type": "integer",
        "description": "A document handle from a previous call, instead of the document itself",
    });
    match operation {
        "catalog" => json!({
            "type": "object",
            "properties": {
                "level": {"type": "string", "description": "One level (subject, session, study, series, stack, instance, event, cohort); every level when absent"},
                "after": {"type": "string", "description": "The path the previous page ended on"},
            },
        }),
        "validate" => json!({
            "type": "object",
            "properties": {
                "document": document,
                "document_id": document_id,
                "mode": {"type": "string", "enum": ["strict", "repair"], "description": "repair fixes what is structural first and reports it"},
            },
        }),
        "options" => json!({
            "type": "object",
            "properties": {
                "document": document,
                "document_id": document_id,
                "set": {"type": "string", "description": "The set to offer moves on; the answer's set when absent"},
            },
        }),
        "apply" => json!({
            "type": "object",
            "required": ["document_id", "epoch", "token", "set", "moves"],
            "properties": {
                "document_id": document_id,
                "epoch": {"type": "integer"},
                "token": {"type": "string", "description": "The token the options answer carried"},
                "set": {"type": "string"},
                "moves": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["move_id"],
                        "properties": {
                            "move_id": {"type": "integer"},
                            "args": {"type": "object", "description": "One value per hole of the move's template"},
                        },
                    },
                },
            },
        }),
        "diagnose" => json!({
            "type": "object",
            "properties": {
                "document": document,
                "document_id": document_id,
                "keys": {"type": "boolean", "description": "Carry the surviving subject keys through the funnel"},
            },
        }),
        "preview" => json!({
            "type": "object",
            "properties": {
                "document": document,
                "document_id": document_id,
                "rows": {"type": "integer", "description": "Rows to show, inside the preview cap"},
            },
        }),
        "run" => json!({
            "type": "object",
            "properties": {
                "document": document,
                "document_id": document_id,
                "name": {"type": "string", "description": "A name for the handle; a named handle keeps its question with its rows"},
                "keep": {"type": "boolean", "description": "Leave a handle for every kept set too"},
            },
        }),
        "handle" => json!({
            "type": "object",
            "required": ["handle"],
            "properties": {"handle": {"type": "integer"}},
        }),
        "rows" => json!({
            "type": "object",
            "required": ["handle"],
            "properties": {
                "handle": {"type": "integer"},
                "page": {"type": "integer", "description": "The handle's page to read, from zero"},
                "offset": {"type": "integer", "description": "Where in that page to start; an answer says next_offset while rows remain"},
            },
        }),
        "selections" => json!({
            "type": "object",
            "properties": {"name": {"type": "string", "description": "The selection, as name or name@version"}},
        }),
        // explain and describe take a document and nothing else
        _ => json!({
            "type": "object",
            "properties": {"document": document, "document_id": document_id},
        }),
    }
}

/// The call a tool makes on the doors: a method, a path and a body.
fn call_of(operation: &str, args: &Value) -> Result<(String, String, Value), String> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(String::from);
    let int = |key: &str| args.get(key).and_then(Value::as_i64);
    let mut body = json!({});
    for key in [
        "document",
        "document_id",
        "mode",
        "set",
        "keys",
        "rows",
        "name",
        "keep",
        "epoch",
        "token",
        "moves",
    ] {
        if let Some(v) = args.get(key)
            && !v.is_null()
        {
            body[key] = v.clone();
        }
    }
    Ok(match operation {
        "catalog" => {
            let mut path = "/api/ask/catalog".to_string();
            if let Some(level) = text("level") {
                path.push('/');
                path.push_str(&level);
            }
            if let Some(after) = text("after") {
                path.push_str(&format!("?after={after}"));
            }
            ("GET".into(), path, Value::Null)
        }
        "handle" => {
            let id = int("handle").ok_or("handle: the id of a result handle")?;
            ("GET".into(), format!("/api/ask/handles/{id}"), Value::Null)
        }
        "rows" => {
            let id = int("handle").ok_or("handle: the id of a result handle")?;
            let page = int("page").unwrap_or(0);
            (
                "GET".into(),
                format!("/api/ask/handles/{id}/rows?page={page}"),
                Value::Null,
            )
        }
        "selections" => {
            let name = text("name").ok_or("name: a selection, as name or name@version")?;
            (
                "GET".into(),
                format!("/api/ask/selections/{name}"),
                Value::Null,
            )
        }
        other => ("POST".into(), format!("/api/ask/{other}"), body),
    })
}

/// The result a model sees: the document, bounded, and its own text.
fn tool_result(doc: &Value, bounded: bool) -> Value {
    let mut text = serde_json::to_string_pretty(doc).unwrap_or_default();
    let cut = text.len() > RESULT_BYTES;
    if cut {
        text.truncate(RESULT_BYTES);
        text.push_str("\n... cut at the result cap; read the rest by page or by handle");
    }
    let mut out = json!({
        "content": [{"type": "text", "text": text}],
        "isError": false,
    });
    if !cut {
        out["structuredContent"] = doc.clone();
    }
    if bounded {
        out["_meta"] = json!({"bounded": true});
    }
    out
}

/// A domain refusal: text a model can read and act on, never a transport
/// error.
fn refusal(reply: &Reply) -> Value {
    let message = reply.body["error"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| reply.body.to_string());
    let mut text = message;
    for i in reply.body["issues"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "\n  {} at {}: {} ({})",
            i["code"].as_str().unwrap_or_default(),
            i["path"].as_str().unwrap_or_default(),
            i["message"].as_str().unwrap_or_default(),
            i["next"].as_str().unwrap_or_default()
        ));
    }
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": true,
    })
}

/// Cut an answer's rows to the door's own page size, from `offset`, and
/// say where the rest is. A model reads a bounded slice and asks for the
/// next by offset; the handle's own pages are the door's.
fn bound_rows(doc: &mut Value, page_rows: usize, offset: usize, pageable: bool) -> bool {
    let Some(rows) = doc.get_mut("rows").and_then(|r| r.as_array_mut()) else {
        return false;
    };
    let total = rows.len();
    if offset == 0 && total <= page_rows {
        return false;
    }
    let start = offset.min(total);
    let end = (start + page_rows).min(total);
    *rows = rows[start..end].to_vec();
    doc["rows_in_page"] = json!(total);
    doc["offset"] = json!(start);
    doc["rows_shown"] = json!(end - start);
    match (end < total, pageable) {
        // The rows tool pages inside a page by offset; every other answer
        // sends the model to the handle for the rest.
        (true, true) => doc["next_offset"] = json!(end),
        (true, false) => {
            doc["more"] = json!("read the rest from the handle, page by page, with the rows tool");
        }
        (false, _) => doc["more"] = json!("this page ends here; read the next page, or the handle"),
    }
    true
}

/// The protected resource metadata of RFC 9728.
pub(crate) fn metadata(doors: &Doors) -> Value {
    json!({
        "resource": format!("http://{}{PATH}", doors.bound),
        "authorization_servers": doors.mcp_authorization_servers,
        "scopes_supported": ["reader", "reviewer", "operator", "admin"],
        "bearer_methods_supported": ["header"],
        "resource_documentation": "https://github.com/kineuro/nils/blob/main/docs/specs/wave4b-the-ask.md",
        "deviation": "audience binding to this resource is named and dated (2026-09-07): the engine accepts the audience its own doors accept until a client that speaks OAuth exists",
    })
}

fn unauthorized(doors: &Doors, message: &str) -> Reply {
    let url = format!(
        "http://{}/.well-known/oauth-protected-resource",
        doors.bound
    );
    Reply::error(401, message).with(
        "WWW-Authenticate",
        format!("Bearer realm=\"nils\", resource_metadata=\"{url}\""),
    )
}

fn forbidden(message: &str, scope: Role) -> Reply {
    Reply::error(403, message).with(
        "WWW-Authenticate",
        format!(
            "Bearer error=\"insufficient_scope\", scope=\"{}\"",
            scope.name()
        ),
    )
}

/// Route the MCP door, or none when the path is not its.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    state: &mut AskState,
    caller: Option<&Caller>,
    method: &str,
    path: &str,
    body: &str,
) -> Option<Reply> {
    // The metadata is public: a client reads it to learn where to
    // authenticate (RFC 9728).
    if method == "GET"
        && (path == "/.well-known/oauth-protected-resource"
            || path == "/.well-known/oauth-protected-resource/mcp")
    {
        return Some(Reply::ok(metadata(doors)));
    }
    if path != PATH {
        return None;
    }
    let Some(caller) = caller else {
        return Some(unauthorized(doors, "the MCP door takes a bearer token"));
    };
    if caller.roles.is_empty() {
        return Some(forbidden(
            "this token holds no role; an installer binds roles before a model reads",
            Role::Reader,
        ));
    }
    Some(match method {
        "POST" => serve(doors, registry, state, caller, body),
        // No server-initiated stream: every answer follows its request.
        "GET" => Reply::error(
            405,
            "this door answers a POST; it opens no stream of its own",
        ),
        "DELETE" => Reply::nothing(204),
        other => Reply::error(405, format!("{other} is not a method this door takes")),
    })
}

fn serve(
    doors: &Doors,
    registry: &mut Registry,
    state: &mut AskState,
    caller: &Caller,
    body: &str,
) -> Reply {
    let Ok(message): Result<Value, _> = serde_json::from_str(body) else {
        return Reply::ok(error(&Value::Null, -32700, "the body is not JSON"));
    };
    // A batch is a list; every answer comes back in one list.
    if let Some(batch) = message.as_array() {
        let mut answers = Vec::new();
        for m in batch {
            if let Some(a) = one(doors, registry, state, caller, m) {
                answers.push(a);
            }
        }
        return if answers.is_empty() {
            Reply::nothing(202)
        } else {
            Reply::ok(Value::Array(answers))
        };
    }
    match one(doors, registry, state, caller, &message) {
        Some(answer) => Reply::ok(answer),
        // A notification is answered with nothing at all.
        None => Reply::nothing(202),
    }
}

/// One JSON-RPC message; `None` for a notification.
fn one(
    doors: &Doors,
    registry: &mut Registry,
    state: &mut AskState,
    caller: &Caller,
    message: &Value,
) -> Option<Value> {
    let method = message["method"].as_str().unwrap_or_default().to_string();
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let notification = message.get("id").is_none();
    if notification {
        // initialized, cancelled, progress: nothing to answer.
        return None;
    }
    let params = message.get("params").cloned().unwrap_or(json!({}));
    Some(match method.as_str() {
        "initialize" => {
            let wanted = params["protocolVersion"].as_str().unwrap_or(PROTOCOL[0]);
            let version = if PROTOCOL.contains(&wanted) {
                wanted
            } else {
                PROTOCOL[0]
            };
            let model = state.model(doors, registry);
            result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "nils", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": model
                        .as_ref()
                        .map(|m| m.grounding.join("\n"))
                        .unwrap_or_else(|| "This engine's pack opts no tool in; ask its operator.".into()),
                }),
            )
        }
        "ping" => result(&id, json!({})),
        "tools/list" => {
            let Some(model) = state.model(doors, registry) else {
                return Some(result(&id, json!({"tools": []})));
            };
            let tools: Vec<Value> = model
                .tools
                .iter()
                .map(|t| {
                    let mut description = t.description.trim().to_string();
                    for r in &t.rules {
                        description.push_str(&format!("\n- {r}"));
                    }
                    json!({
                        "name": t.name,
                        "description": description,
                        "inputSchema": input_schema(&t.operation),
                    })
                })
                .collect();
            result(&id, json!({"tools": tools}))
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default().to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let Some(model) = state.model(doors, registry) else {
                return Some(result(
                    &id,
                    json!({
                        "content": [{"type": "text", "text": "this engine's pack opts no tool in"}],
                        "isError": true,
                    }),
                ));
            };
            let Some(tool) = model.tool(&name).cloned() else {
                return Some(error(
                    &id,
                    -32602,
                    format!("no tool named {name}; tools/list names them"),
                ));
            };
            let (method, path, call_body) = match call_of(&tool.operation, &args) {
                Ok(c) => c,
                Err(why) => {
                    return Some(result(
                        &id,
                        json!({"content": [{"type": "text", "text": why}], "isError": true}),
                    ));
                }
            };
            let (path, query) = split_query(&path);
            let reply = crate::serve::ask_call(
                doors,
                registry,
                state,
                caller,
                &method,
                &path,
                &query,
                &call_body.to_string(),
            );
            if (200..300).contains(&reply.status) {
                let mut doc = reply.body;
                let offset = args
                    .get("offset")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as usize;
                let bounded = bound_rows(
                    &mut doc,
                    doors.ask_caps.page_rows_mcp as usize,
                    offset,
                    tool.operation == "rows",
                );
                result(&id, tool_result(&doc, bounded))
            } else if reply.status == 401 || reply.status == 403 {
                // A role refusal is the caller's to fix, not the model's to
                // retry: it comes back as an error of the protocol.
                error(
                    &id,
                    -32003,
                    reply.body["error"].as_str().unwrap_or("refused"),
                )
            } else {
                result(&id, refusal(&reply))
            }
        }
        "resources/list" => result(&id, json!({"resources": []})),
        "prompts/list" => {
            let model = state.model(doors, registry);
            let prompts: Vec<Value> = model
                .map(|m| {
                    m.examples
                        .iter()
                        .enumerate()
                        .map(|(i, e)| {
                            json!({
                                "name": format!("example_{}", i + 1),
                                "title": e.question,
                                "description": e.note,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            result(&id, json!({"prompts": prompts}))
        }
        "prompts/get" => {
            let name = params["name"].as_str().unwrap_or_default();
            let index: usize = name
                .strip_prefix("example_")
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0);
            let model = state.model(doors, registry);
            match model.and_then(|m| m.examples.get(index.saturating_sub(1)).cloned()) {
                Some(e) => result(
                    &id,
                    json!({
                        "description": e.note,
                        "messages": [{
                            "role": "user",
                            "content": {"type": "text", "text": format!("{}\n\n{}", e.question, e.document)},
                        }],
                    }),
                ),
                None => error(&id, -32602, format!("no prompt named {name}")),
            }
        }
        other => error(
            &id,
            -32601,
            format!("{other} is not a method this door has"),
        ),
    })
}

fn split_query(path: &str) -> (String, HashMap<String, String>) {
    match path.split_once('?') {
        Some((p, q)) => (
            p.to_string(),
            q.split('&')
                .filter(|kv| !kv.is_empty())
                .filter_map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                })
                .collect(),
        ),
        None => (path.to_string(), HashMap::new()),
    }
}
