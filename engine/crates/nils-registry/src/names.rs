// SPDX-License-Identifier: AGPL-3.0-only

//! Record 58 P1 (Wave 7b spec §12.5): the name an account goes by, so that
//! Results and Pace name a reader by their account's name and never by the
//! sign-in subject (the fix of Wave 7a's list).
//!
//! The engine keeps no user table (Wave 4c §5.3): a token carries the
//! account's `name` and `preferred_username` beside its subject, and the
//! desk's own issuer mints the display name a site gave the account. What a
//! person's token said is noted when they make, claim or answer in a
//! campaign ([`note`]) and read back by principal ([`of`]). A principal no
//! token has named yet is shown as itself ([`shown`]).

use std::collections::BTreeMap;

use crate::dialect::Conflict;
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

/// Note the name a principal's token carries; nothing where it carries
/// none, or only the principal again.
pub fn note(store: &mut Store, principal: &str, name: Option<&str>) -> Result<(), StoreError> {
    let Some(name) = name
        .map(str::trim)
        .filter(|n| !n.is_empty() && *n != principal)
    else {
        return Ok(());
    };
    store.insert(
        &Insert::new(table("principal_name"), &["principal", "name", "seen_at"]).on_conflict(
            Conflict::Update {
                target: &["principal"],
                set: &["name", "seen_at"],
            },
        ),
        &[vec![
            Param::from(principal),
            Param::from(name),
            Param::from(now_iso().as_str()),
        ]],
    )?;
    Ok(())
}

/// The names noted for these principals, by principal; one no token has
/// named is left out.
pub fn of(
    store: &mut Store,
    principals: &[String],
) -> Result<BTreeMap<String, String>, StoreError> {
    let mut out = BTreeMap::new();
    let d = store.dialect();
    for chunk in principals.chunks(500) {
        let marks: Vec<String> = (1..=chunk.len()).map(|i| d.param(i, Type::Text)).collect();
        let sql = format!(
            "SELECT principal, name FROM {} WHERE principal IN ({})",
            store.qualified("principal_name"),
            marks.join(", ")
        );
        let params: Vec<Param> = chunk.iter().map(|p| Param::from(p.as_str())).collect();
        for r in store.query(&sql, &params)? {
            out.insert(r.text(0)?.to_string(), r.text(1)?.to_string());
        }
    }
    Ok(out)
}

/// The name a principal is shown by: the account's where a token named it,
/// else the principal itself.
pub fn shown(names: &BTreeMap<String, String>, principal: &str) -> String {
    names
        .get(principal)
        .cloned()
        .unwrap_or_else(|| principal.to_string())
}
