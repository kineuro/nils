// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55, ruling 35 (c), Nima 2026-10-16: a handover's password. A
//! release stays a plain folder and a handover an optional step; inside it
//! encryption is a choice, on by default, and turning it off needs a reason
//! that the handover and the audit keep. Each handover has a password of its
//! own, never one shared by every handover under a key: made by NILS
//! (derived from a key and the handover's id, never stored) or typed by the
//! person handing over (kept sealed under the registry's key, never in
//! clear).
//!
//! - `POST /api/handovers {release, out, encrypt?, why?, password?, typed?,
//!   key?}` queues the handover, a typed password sealed before it enters
//!   the job.
//! - `GET /api/handovers?release=` lists the handovers, how each is kept
//!   closed and whether the caller may show its password.
//! - `POST /api/handovers/{id}/password` answers one handover's password,
//!   `no-store`.
//!
//! Who may show a handover's password, or type one when handing over, is a
//! grant given like any other (`handover-password:see` and `:work`, the
//! admin's set holds both), or the release's maker for the handovers of
//! their own release. Every show and every typed password is audited by the
//! handover's id, never the password. A call that acts as an agent or a
//! model is refused.

use std::collections::HashMap;

use nils_registry::Registry;
use nils_registry::linkage::Subkeys;
use nils_registry::schema::Type;
use nils_registry::store::Param;
use nils_release::handover::run::{self, Kept};
use serde_json::json;

use crate::keys_doors::{NO_STORE, person_only};
use crate::serve::{Caller, Reply, job_err, json_body, queued_by};

/// The doors this module answers beside `POST /api/handovers`, which the
/// door list names with the release doors.
pub(crate) const DOORS: &[&str] = &["GET /api/handovers", "POST /api/handovers/{id}/password"];

/// Show a handover's password.
pub(crate) const SEE: &str = "handover-password:see";
/// Type one when handing over.
pub(crate) const WORK: &str = "handover-password:work";

/// The shortest reason for handing over unencrypted.
const WHY_SHORTEST: usize = 4;

/// Whether the caller may show this handover's password: the grant, or the
/// release's maker.
pub(crate) fn may_show(caller: &Caller, kept: &Kept) -> bool {
    caller.access.holds(SEE) || caller.principal == kept.maker
}

/// Who made the newest finished version of a release, which a handover packs.
fn maker_of(registry: &mut Registry, release: &str) -> Result<Option<String>, Reply> {
    let store = registry.store();
    let sql = format!(
        "SELECT actor FROM {} WHERE name = {} AND finished_at IS NOT NULL ORDER BY id DESC LIMIT 1",
        store.qualified("release"),
        store.dialect().param(1, Type::Text),
    );
    Ok(store
        .query_opt(&sql, &[Param::from(release)])?
        .map(|r| r.text(0).map(str::to_string))
        .transpose()?)
}

/// `POST /api/handovers`: the handover queued as `nils handover run`, with
/// its password made by NILS (the default, from `key` or the registry's
/// key), typed (`password: "typed"` and `typed`, for a holder of the grant
/// or the release's maker; sealed under the registry's key before it enters
/// the job), or none (`encrypt: false` and a `why`).
pub(crate) fn queue(registry: &mut Registry, caller: &Caller, body: &str) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let release = doc["release"]
        .as_str()
        .ok_or_else(|| Reply::error(400, "release is required"))?
        .to_string();
    let out = doc["out"]
        .as_str()
        .ok_or_else(|| Reply::error(400, "out is required"))?
        .to_string();
    // Wave 5 §10.2: a handover writes only to an exchange place.
    crate::places::require(
        registry.store(),
        nils_registry::place::Role::Exchange,
        std::path::Path::new(&out),
    )
    .map_err(|r| Reply::error(409, r.message))?;
    // Wave 7b (the maintenance track): a folder a newer release stopped in,
    // or is writing, is a mixture of versions and is not handed over
    if let Some(words) = run::refusal_for(registry.store(), &release)
        .map_err(|e| Reply::error(500, e.to_string()))?
    {
        return Err(Reply::error(409, words));
    }
    let encrypt = match &doc["encrypt"] {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => *b,
        _ => return Err(Reply::error(400, "encrypt: true or false")),
    };
    let mut command = vec![
        "handover".to_string(),
        "run".to_string(),
        "--release".to_string(),
        release.clone(),
        "--out".to_string(),
        out,
    ];
    let mut shown = command.clone();
    if !encrypt {
        let why = doc["why"]
            .as_str()
            .map(str::trim)
            .filter(|w| w.chars().count() >= WHY_SHORTEST)
            .ok_or_else(|| {
                Reply::error(
                    400,
                    "why: the reason the archives go unencrypted, a few words; the handover and the audit keep it",
                )
            })?;
        for w in ["--no-encrypt", "--why", why] {
            command.push(w.to_string());
            shown.push(w.to_string());
        }
    } else {
        match doc["password"].as_str().unwrap_or("made") {
            "made" => {
                if let Some(k) = doc["key"].as_str().filter(|k| !k.trim().is_empty()) {
                    for w in ["--key", k.trim()] {
                        command.push(w.to_string());
                        shown.push(w.to_string());
                    }
                }
            }
            "typed" => {
                person_only(caller, "typing a handover's password")?;
                let maker = maker_of(registry, &release)?;
                if !caller.access.holds(WORK) && maker.as_deref() != Some(caller.principal.as_str())
                {
                    return Err(Reply::error(
                        403,
                        format!(
                            "typing a handover's password needs the {WORK} grant, or to be who made the release"
                        ),
                    ));
                }
                // read once, checked, sealed; never named again
                let Some(typed) = doc.get("typed").and_then(serde_json::Value::as_str) else {
                    return Err(Reply::error(400, "typed: the password, as text"));
                };
                if let Some(why) = nils_release::handover::typed_refusal(typed) {
                    return Err(Reply::error(400, why));
                }
                let key = registry
                    .pseudonym_key()
                    .map_err(|e| Reply::error(500, e.to_string()))?;
                let sealed = hex::encode(Subkeys::derive(&key).seal(typed));
                command.extend(["--sealed-password".to_string(), sealed]);
                shown.extend(["--sealed-password".to_string(), "(sealed)".to_string()]);
            }
            other => {
                return Err(Reply::error(
                    400,
                    format!("password: made or typed, not {other}"),
                ));
            }
        }
    }
    let id = nils_registry::job::enqueue_with(
        registry.store(),
        &command,
        Some(&release),
        Some(&caller.principal),
        queued_by(caller),
    )
    .map_err(job_err)?;
    Ok(
        Reply::accepted(json!({ "job": id, "state": "queued", "command": shown }))
            .with(NO_STORE.0, NO_STORE.1),
    )
}

/// `GET /api/handovers?release=`: the handovers, newest first, each as its
/// row keeps it (never a password) and `may_show`, whether this caller may
/// show its password.
pub(crate) fn list(
    registry: &mut Registry,
    caller: &Caller,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let kept = run::kept(
        registry.store(),
        query.get("release").map(String::as_str),
        None,
    )
    .map_err(|e| Reply::error(500, e.to_string()))?;
    let rows: Vec<serde_json::Value> = kept
        .iter()
        .map(|k| {
            let mut v = k.as_json();
            v["may_show"] =
                json!(k.kind != nils_release::handover::Kind::None && may_show(caller, k));
            v
        })
        .collect();
    Ok(Reply::ok(json!({ "count": rows.len(), "handovers": rows })))
}

/// `POST /api/handovers/{id}/password`: `{handover, password, kind}`, sent
/// `no-store`, for a holder of `handover-password:see` or the release's
/// maker; audited as `handover.password` with the handover's id alone. A
/// handover from before each had its own answers its key's shared password
/// and says so (`kind: key`); one not encrypted is refused (409) with why.
pub(crate) fn password(registry: &mut Registry, caller: &Caller, id: &str) -> Result<Reply, Reply> {
    person_only(caller, "reading a handover's password")?;
    let id: i64 = id
        .parse()
        .map_err(|_| Reply::error(404, format!("{id} is not a handover's id")))?;
    let kept = run::kept(registry.store(), None, Some(id))
        .map_err(|e| Reply::error(500, e.to_string()))?
        .into_iter()
        .next()
        .ok_or_else(|| Reply::error(404, format!("no handover {id}")))?;
    if !may_show(caller, &kept) {
        return Err(Reply::error(
            403,
            format!("a handover's password needs the {SEE} grant, or to be who made the release"),
        ));
    }
    let shown = run::password_of(registry, &kept).map_err(|e| match e {
        run::Error::Refused(m) => Reply::error(409, m),
        other => Reply::error(500, other.to_string()),
    })?;
    let Some(password) = shown else {
        return Err(Reply::error(
            409,
            format!(
                "handover {id} was handed over unencrypted: {}",
                kept.why.as_deref().unwrap_or("no reason kept")
            ),
        ));
    };
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &caller.principal,
            action: nils_registry::audit::Action::HandoverPassword,
            scope: json!({ "handover": id }),
            policy: None,
            job_id: None,
            details: None,
        },
    )?;
    let kind = match kept.kind {
        nils_release::handover::Kind::Key => "key",
        k => k.word(),
    };
    Ok(
        Reply::ok(json!({ "handover": id, "password": password, "kind": kind }))
            .with(NO_STORE.0, NO_STORE.1),
    )
}

#[cfg(test)]
mod tests {
    use crate::grants::Detail;

    #[test]
    fn the_handover_doors_need_their_grants() {
        let need = |method: &str, path: &str| {
            let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
            let (need, detail) = crate::serve::door(method, &segs);
            (need.words(), detail)
        };
        assert_eq!(
            need("GET", "/api/handovers"),
            (
                "one of the grants release:see, handover-password:see".to_string(),
                Detail::Plain
            )
        );
        assert_eq!(
            need("POST", "/api/handovers/3/password"),
            (
                "one of the grants handover-password:see, release:see".to_string(),
                Detail::Plain
            )
        );
        let policy = crate::serve::policy();
        for d in super::DOORS {
            assert!(
                policy.iter().any(|r| r["door"] == *d),
                "{d} has no policy row"
            );
        }
    }
}
