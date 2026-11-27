// SPDX-License-Identifier: AGPL-3.0-only
//! The kept session schemes at a door (record 55 H2: no command line inside
//! a desk flow). `GET /api/session-schemes` lists what `nils session scheme
//! list` lists, each with its definition, and the scheme a release or a pick
//! uses when it names none; `POST /api/session-schemes` keeps a new one as
//! `nils session scheme add` does, and never replaces one.

use nils_registry::schema::Type;
use nils_registry::session::Scheme;
use nils_registry::store::Insert;
use nils_registry::{Param, Registry};
use serde_json::{Value, json};

use crate::serve::Reply;
use crate::{Exit, fail, usage};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 2] = ["GET /api/session-schemes", "POST /api/session-schemes"];

/// Whether the registry keeps a scheme under this name.
fn kept(registry: &mut Registry, name: &str) -> Result<bool, Exit> {
    let store = registry.store();
    let row = store
        .query_opt(
            &format!(
                "SELECT id FROM {} WHERE name = {}",
                store.qualified("session_scheme"),
                store.dialect().param(1, Type::Text)
            ),
            &[Param::Text(name.to_string())],
        )
        .map_err(|e| fail(e.to_string()))?;
    Ok(row.is_some())
}

/// What keeping a scheme did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kept {
    New,
    Replaced,
}

/// Keep a scheme under a name, as `nils session scheme add` does: a name
/// kept already is refused unless `force` replaces it.
pub(crate) fn keep(
    registry: &mut Registry,
    name: &str,
    scheme: &Scheme,
    note: Option<&str>,
    force: bool,
) -> Result<Kept, Exit> {
    let json = serde_json::to_string(scheme)
        .map_err(|e| fail(format!("the scheme will not serialize: {e}")))?;
    let note = || match note {
        Some(n) => Param::Text(n.to_string()),
        None => Param::Null,
    };
    if kept(registry, name)? {
        if !force {
            return Err(fail(format!(
                "a scheme called {name} is already kept; --force replaces it, and every \
                 label derived with the old one changes"
            )));
        }
        let store = registry.store();
        store
            .execute(
                &format!(
                    "UPDATE {} SET definition = {}, note = {}, created_at = {}, digest = {} WHERE name = {}",
                    store.qualified("session_scheme"),
                    store.dialect().param(1, Type::Json),
                    store.dialect().param(2, Type::Text),
                    store.dialect().param(3, Type::Timestamp),
                    store.dialect().param(4, Type::Text),
                    store.dialect().param(5, Type::Text),
                ),
                &[
                    Param::Text(json),
                    note(),
                    Param::Text(nils_registry::time::now_iso()),
                    Param::Text(scheme.digest()),
                    Param::Text(name.to_string()),
                ],
            )
            .map_err(|e| fail(e.to_string()))?;
        return Ok(Kept::Replaced);
    }
    registry
        .store()
        .insert(
            &Insert::new(
                nils_registry::schema::table("session_scheme"),
                &["name", "definition", "created_at", "note", "digest"],
            ),
            &[vec![
                Param::Text(name.to_string()),
                Param::Text(json),
                Param::Text(nils_registry::time::now_iso()),
                note(),
                Param::Text(scheme.digest()),
            ]],
        )
        .map_err(|e| fail(e.to_string()))?;
    Ok(Kept::New)
}

/// The kept schemes, by name, each with its definition, note, digest and
/// when it was kept.
pub(crate) fn list(registry: &mut Registry) -> Result<Vec<Value>, Exit> {
    let store = registry.store();
    let rows = store
        .query(
            &format!(
                "SELECT name, definition, created_at, note, digest FROM {} ORDER BY name",
                store.qualified("session_scheme")
            ),
            &[],
        )
        .map_err(|e| fail(e.to_string()))?;
    Ok(rows
        .iter()
        .map(|r| {
            let definition = r.text(1).unwrap_or_default();
            json!({
                "name": r.text(0).unwrap_or_default(),
                "scheme": serde_json::from_str::<Value>(definition).unwrap_or(Value::Null),
                "added_at": r.text(2).unwrap_or_default(),
                "note": r.opt_text(3).ok().flatten(),
                "digest": r.opt_text(4).ok().flatten(),
            })
        })
        .collect())
}

/// What [`rebuild_each`] did: a build under one scheme is committed before
/// the next begins, so a failure under a later scheme leaves the earlier
/// ones built.
pub(crate) struct Built {
    /// The sessions the subjects have under the default scheme.
    pub sessions: usize,
    /// Whether any build wrote a session or a label (and so moved the
    /// epoch).
    pub wrote: bool,
    /// The first kept scheme that could not be built, and why.
    pub failed: Option<String>,
}

/// The sessions of some subjects built again once a write has dropped
/// them: a merge takes away the cache rows of both its subjects, and the
/// subject that is left had no session, in any question or on any page that
/// reads the cache as it stands, until some reader built them. Built under
/// the default scheme and under every kept one, since the merge dropped the
/// rows of every window. Answers the sessions the subjects have under the
/// default scheme. The default scheme's failure is the error; a kept
/// scheme's is too, once every scheme was tried.
pub(crate) fn rebuild_for(
    registry: &mut Registry,
    subjects: &std::collections::BTreeSet<i64>,
) -> Result<usize, String> {
    let built = rebuild_each(registry, subjects)?;
    match built.failed {
        Some(e) => Err(e),
        None => Ok(built.sessions),
    }
}

/// [`rebuild_for`] for a caller that must account for what was built when a
/// kept scheme fails: the default scheme's failure is the error, with
/// nothing written; a kept scheme's is told beside what the others built.
pub(crate) fn rebuild_each(
    registry: &mut Registry,
    subjects: &std::collections::BTreeSet<i64>,
) -> Result<Built, String> {
    let mut out = Built {
        sessions: 0,
        wrote: false,
        failed: None,
    };
    if subjects.is_empty() {
        return Ok(out);
    }
    let default = Scheme::default();
    let built =
        nils_session::refresh_for(registry, &default, subjects).map_err(|e| e.to_string())?;
    out.sessions = built.sessions;
    out.wrote = built.epoch.is_some();
    let store = registry.store();
    let rows = store
        .query(
            &format!(
                "SELECT name, definition FROM {} ORDER BY name",
                store.qualified("session_scheme")
            ),
            &[],
        )
        .map_err(|e| e.to_string())?;
    let mut done = vec![default.digest()];
    for r in &rows {
        let name = r.text(0).unwrap_or_default();
        let Ok(scheme) = Scheme::from_json(r.text(1).unwrap_or_default()) else {
            continue;
        };
        if done.contains(&scheme.digest()) {
            continue;
        }
        done.push(scheme.digest());
        match nils_session::refresh_for(registry, &scheme, subjects) {
            Ok(built) => out.wrote |= built.epoch.is_some(),
            Err(e) => {
                out.failed
                    .get_or_insert(format!("under the scheme {name}: {e}"));
            }
        }
    }
    Ok(out)
}

/// [`rebuild_for`] for the subjects an identifier map merged others into,
/// by their codes.
pub(crate) fn rebuild_after_merges(
    registry: &mut Registry,
    merges: &[nils_registry::identity_map::Merge],
) -> Result<usize, String> {
    let codes: Vec<String> = merges
        .iter()
        .map(|m| m.canonical.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let subjects = nils_registry::linkage::subjects_by_code(registry.store(), &codes)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|s| s.merged_into.unwrap_or(s.id))
        .collect();
    rebuild_for(registry, &subjects)
}

/// A scheme's name: what a release, a pick and a question name it by.
fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The scheme in a request: an object, as the registry stores it, or the
/// text of a scheme file (`session:` and its fields), as the command line
/// reads it.
fn scheme_of(v: &Value) -> Result<Scheme, Exit> {
    match v {
        Value::Object(_) => Scheme::from_json(&v.to_string()).map_err(|e| {
            usage(
                e.to_string()
                    .replacen("the stored session scheme", "the session scheme", 1),
            )
        }),
        Value::String(text) => Scheme::parse(text).map_err(|e| usage(e.to_string())),
        _ => Err(usage(
            "scheme is the session scheme, as an object or as the text of a scheme file",
        )),
    }
}

/// The doors. The grants are the door table's: release:see to read,
/// release:work to keep one.
pub(crate) fn route(
    registry: &mut Registry,
    method: &str,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    Some(match (method, segs) {
        ("GET", ["api", "session-schemes"]) => (|| {
            let schemes = list(registry)?;
            let default = serde_json::to_value(Scheme::default())
                .map_err(|e| Reply::error(500, e.to_string()))?;
            Ok(Reply::ok(json!({
                "schemes": schemes,
                // what a release or a pick that names no scheme uses
                "default": default,
            })))
        })(),
        ("POST", ["api", "session-schemes"]) => (|| {
            let v: Value = serde_json::from_str(body)
                .map_err(|e| Reply::error(400, format!("the body is not JSON: {e}")))?;
            let name = v["name"].as_str().map(str::trim).unwrap_or_default();
            if !name_ok(name) {
                return Err(Reply::error(
                    400,
                    "name is a scheme's name: letters, digits, '-', '_' and '.', at most 64",
                ));
            }
            let scheme = scheme_of(&v["scheme"])?;
            let note = v["note"].as_str().map(str::trim).filter(|n| !n.is_empty());
            if kept(registry, name)? {
                return Err(Reply::error(
                    409,
                    format!(
                        "a scheme called {name} is kept already; give the new one another name, since every label made with the old one would change"
                    ),
                ));
            }
            keep(registry, name, &scheme, note, false)?;
            crate::audit(
                registry,
                nils_registry::audit::Action::SessionScheme,
                json!({"scheme": name, "digest": scheme.digest()}),
                None,
            )?;
            let row = list(registry)?
                .into_iter()
                .find(|r| r["name"] == name)
                .unwrap_or_else(|| json!({"name": name}));
            Ok(Reply::created(row))
        })(),
        _ => return None,
    })
}
