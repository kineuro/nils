// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55, ruling 35 (c): the key store and a handover's password at the
//! door, so the desk has a place for what `nils key` and `nils handover
//! password` do at the keyboard.
//!
//! - `GET /api/keys`: the keys by name, with their length, fingerprint and
//!   which one the registry's pseudonyms come from; never a byte of one.
//! - `POST /api/keys {name, passphrase}`: a key made from its passphrase,
//!   as `nils key add NAME` makes it from its input. After a restore it
//!   makes the pseudonym key again; the key is first tried on what the
//!   linkage store keeps, and a passphrase that does not open it is refused
//!   with nothing kept.
//! - `POST /api/keys/{name}/remove {confirm}`: a key the registry does not
//!   use taken away, its name typed again to confirm.
//! - `POST /api/handovers/password {key}`: the password a handover under a
//!   key was packed with, for the person receiving the archives.
//!
//! A secret given here or answered here goes only over the authenticated
//! door: it is never written to the registry, the jobs, the audit or a log,
//! never put in an error, and an answer that carries one says `no-store`.
//! The acts are a person's, never an agent's or a model's, and each is
//! audited by the key's name alone. The grants are the door table's:
//! reading the store is the Backups page's, changing it is an admin's
//! (`database:work` and `install:work` at detail sensitive), and the
//! password is the work of whoever hands a release over, at detail
//! sensitive.

use nils_registry::keys::{self, KeyError, KeyStore};
use nils_registry::linkage::Subkeys;
use nils_registry::store::Param;
use nils_registry::{Registry, audit};
use serde_json::{Value, json};

use crate::serve::{Caller, Reply, author_of, json_body};

/// The doors this module answers, for the capabilities.
pub(crate) const DOORS: &[&str] = &[
    "GET /api/keys",
    "POST /api/keys",
    "POST /api/keys/{name}/remove",
    "POST /api/handovers/password",
];

/// The header every answer that carries a secret, or that follows one, is
/// sent with: no cache along the way keeps it.
pub(crate) const NO_STORE: (&str, &str) = ("Cache-Control", "no-store");

/// The doors under `/api/keys` and the handover's password, or nothing
/// when the path is not one. The grants were checked by the table.
pub(crate) fn route(
    registry: &mut Registry,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let get = method == "GET";
    let post = method == "POST";
    Some(match segs {
        ["api", "keys"] if get => list(registry),
        ["api", "keys"] if post => add(registry, caller, body),
        ["api", "keys", name, "remove"] if post => remove(registry, caller, name, body),
        ["api", "handovers", "password"] if post => password(registry, caller, body),
        _ => return None,
    })
}

/// A key act is a person's: a call that says an agent or a model acts is
/// refused whatever its grants, so no assistant ever handles a secret.
pub(crate) fn person_only(caller: &Caller, act: &str) -> Result<(), Reply> {
    let (kind, _) = author_of(caller);
    if kind != "person" {
        return Err(Reply::error(
            403,
            format!(
                "{act} is a person's act, and this caller acts as {}",
                nils_registry::a_or_an(kind)
            ),
        ));
    }
    Ok(())
}

/// What the key store said, as a refusal that never carries a key: the
/// store's own words hold names and lengths only.
fn refused(e: KeyError) -> Reply {
    match e {
        KeyError::BadName(_) | KeyError::BadLength(_) => Reply::error(400, e.to_string()),
        KeyError::Exists(_) | KeyError::InUse(_) => Reply::error(409, e.to_string()),
        KeyError::Missing(_) => Reply::error(404, e.to_string()),
        KeyError::Io { .. } => Reply::error(500, "the key store could not be read or written"),
    }
}

fn in_use(registry: &Registry) -> String {
    registry.meta().pseudonym_key.clone()
}

/// `GET /api/keys`: `{pseudonym_key, keys: [{name, bytes, fingerprint,
/// in_use}]}`. A fingerprint is the first eight hex characters of the key's
/// hash, what `nils key list` prints, so a key made again is checked
/// against the one written down.
fn list(registry: &mut Registry) -> Result<Reply, Reply> {
    let store = registry.keys();
    let used = in_use(registry);
    let rows = store.list().map_err(refused)?;
    let keys: Vec<Value> = rows
        .iter()
        .map(|k| {
            json!({
                "name": k.name,
                "bytes": k.bytes,
                "fingerprint": k.fingerprint,
                "in_use": k.name == used,
            })
        })
        .collect();
    let present = rows.iter().any(|k| k.name == used);
    Ok(Reply::ok(json!({
        "pseudonym_key": used,
        "present": present,
        "max_bytes": keys::MAX_KEY_BYTES,
        "keys": keys,
    })))
}

/// What the linkage store keeps under the registry's key: whether `key`
/// opens it. `None` when it keeps nothing a key could be tried on (only
/// keyed lookups, or nothing at all).
fn opens_linkage(registry: &mut Registry, key: &[u8]) -> Result<Option<bool>, Reply> {
    let mut store = registry
        .open_linkage()
        .map_err(|e| Reply::error(500, e.to_string()))?;
    let sql = format!(
        "SELECT ciphertext FROM {} WHERE ciphertext IS NOT NULL AND length(ciphertext) > 0 ORDER BY id LIMIT 1",
        store.qualified("identity")
    );
    let rows = store.query(&sql, &[] as &[Param])?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let sealed = row.bytes(0)?.to_vec();
    Ok(Some(Subkeys::derive(key).open(&sealed).is_ok()))
}

/// `POST /api/keys {name, passphrase}`: the key made from its passphrase,
/// as `nils key add` makes it from its input (one final newline dropped).
/// A name the store has already is refused (409): a key is never replaced.
/// The registry's pseudonym key, made again, is tried on the linkage store
/// first, and refused (409) with nothing kept where it does not open what
/// the store keeps. Answers `{name, bytes, fingerprint, in_use, checked}`.
fn add(registry: &mut Registry, caller: &Caller, body: &str) -> Result<Reply, Reply> {
    person_only(caller, "adding a key")?;
    let doc = json_body(body)?;
    let name = doc["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Reply::error(400, "name: the key's name"))?
        .to_string();
    // the passphrase is read once and never named again: no refusal below
    // quotes it, and nothing but the key store's file keeps it
    let Some(given) = doc.get("passphrase").and_then(Value::as_str) else {
        return Err(Reply::error(
            400,
            "passphrase: the key's passphrase, as text",
        ));
    };
    let (bytes, _) = keys::strip_newline(given.as_bytes());
    if bytes.is_empty() || bytes.len() > keys::MAX_KEY_BYTES {
        return Err(refused(KeyError::BadLength(bytes.len())));
    }
    let used = in_use(registry);
    let store: KeyStore = registry.keys();
    let checked = if name == used {
        match opens_linkage(registry, bytes)? {
            Some(false) => {
                return Err(Reply::error(
                    409,
                    format!(
                        "that passphrase does not open the identifiers the linkage store keeps under {name}; nothing was kept"
                    ),
                ));
            }
            Some(true) => "it opens the identifiers the linkage store keeps",
            None => {
                "the linkage store keeps nothing it could be tried on; a wrong passphrase gives every new subject another code"
            }
        }
    } else {
        "not the pseudonym key; nothing to try it on"
    };
    let info = store.add(&name, bytes).map_err(refused)?;
    audit::record(
        registry,
        &audit::Entry {
            principal: &caller.principal,
            action: audit::Action::KeyAdd,
            scope: json!({ "key": info.name, "bytes": info.bytes, "in_use": info.name == used }),
            policy: None,
            job_id: None,
            details: None,
        },
    )?;
    Ok(Reply::created(json!({
        "name": info.name,
        "bytes": info.bytes,
        "fingerprint": info.fingerprint,
        "in_use": info.name == used,
        "checked": checked,
    }))
    .with(NO_STORE.0, NO_STORE.1))
}

/// `POST /api/keys/{name}/remove {confirm}`: the key taken away, `confirm`
/// its name typed again. The registry's pseudonym key is refused (409), as
/// `nils key remove` refuses it.
fn remove(
    registry: &mut Registry,
    caller: &Caller,
    name: &str,
    body: &str,
) -> Result<Reply, Reply> {
    person_only(caller, "removing a key")?;
    let doc = json_body(body)?;
    let name = crate::serve::decoded(name);
    if doc["confirm"].as_str().map(str::trim) != Some(name.as_str()) {
        return Err(Reply::error(
            400,
            "confirm: the key's name, typed again; a key taken away cannot be made again without its passphrase",
        ));
    }
    let used = in_use(registry);
    registry
        .keys()
        .remove(&name, Some(used.as_str()))
        .map_err(refused)?;
    audit::record(
        registry,
        &audit::Entry {
            principal: &caller.principal,
            action: audit::Action::KeyRemove,
            scope: json!({ "key": name }),
            policy: None,
            job_id: None,
            details: None,
        },
    )?;
    Ok(Reply::ok(json!({ "name": name, "removed": true })))
}

/// `POST /api/handovers/password {key}`: `{key, password}`, the password
/// every handover under that key was packed with (`nils handover password
/// --key`), sent `no-store`. It is derived, never stored, and the audit
/// names the key alone.
fn password(registry: &mut Registry, caller: &Caller, body: &str) -> Result<Reply, Reply> {
    person_only(caller, "reading a handover's password")?;
    let doc = json_body(body)?;
    let name = doc["key"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| {
            Reply::error(
                400,
                "key: the name of the key the handover was packed under",
            )
        })?
        .to_string();
    let key = registry.keys().read(&name).map_err(refused)?;
    let password = nils_release::handover::password(&key);
    audit::record(
        registry,
        &audit::Entry {
            principal: &caller.principal,
            action: audit::Action::HandoverPassword,
            scope: json!({ "key": name }),
            policy: None,
            job_id: None,
            details: None,
        },
    )?;
    Ok(Reply::ok(json!({ "key": name, "password": password })).with(NO_STORE.0, NO_STORE.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grants::Detail;

    /// The door table: reading the store is the Backups page's; changing
    /// it is an admin's at detail sensitive; the password is release work
    /// at detail sensitive. Every door has its policy row.
    #[test]
    fn the_key_doors_need_their_grants_and_detail() {
        let need = |method: &str, path: &str| {
            let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
            let (need, detail) = crate::serve::door(method, &segs);
            (need.words(), detail)
        };
        assert_eq!(
            need("GET", "/api/keys"),
            ("the database:see grant".to_string(), Detail::Plain)
        );
        for (m, p) in [("POST", "/api/keys"), ("POST", "/api/keys/k2/remove")] {
            assert_eq!(
                need(m, p),
                (
                    "the database:work and install:work grants".to_string(),
                    Detail::Sensitive
                ),
                "{m} {p}"
            );
        }
        assert_eq!(
            need("POST", "/api/handovers/password"),
            ("the release:work grant".to_string(), Detail::Sensitive)
        );
        let policy = crate::serve::policy();
        for d in DOORS {
            assert!(
                policy.iter().any(|r| r["door"] == *d),
                "{d} has no policy row"
            );
        }
    }
}
