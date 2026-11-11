// SPDX-License-Identifier: AGPL-3.0-only

//! The one pass that drops what a store from before kept of a value shaped
//! like a personnummer (2026-11-11).
//!
//! The stores keep no personnummer, not even sealed, and until this pass the
//! type's name alone decided it: a dataset whose rule filed PatientID under
//! `patient-id`, or under a hospital's own type, kept what PatientID held,
//! and where that was a personnummer the linkage store sealed one. Since
//! [`crate::linkage::keeps_value`] nothing of that shape is sealed under any
//! type. This pass does the same for what was sealed before: it opens every
//! identity row that keeps a value and every held file's sealed identifier,
//! and blanks those shaped like a personnummer or a coordination number. The
//! keyed lookup stays, so the identifier still finds its subject, and the
//! subject is shown by its code.
//!
//! It needs the registry's pseudonym key, which a migration does not hold,
//! so it runs where the key is: at the first [`crate::Home::open`] of a
//! registry that carries no [`MARKER`] row, which is the first command or
//! engine start after the update. A registry made by this binary carries the
//! row from `nils init` and is never swept. The row is written in the
//! transaction that blanks the held files, so a pass that did not finish
//! leaves none and the next open runs it again; a second pass finds nothing
//! left to blank. Later opens read the row with the rest of `registry_meta`
//! and cost nothing.
//!
//! Nothing here stops the engine: a key that cannot be read, a linkage store
//! that does not open and a statement that fails all leave the registry open
//! and unmarked, for the next open to try again. A ciphertext that does not
//! open under the key is counted and left as it is: nobody holding this key
//! reads it either. What the pass reports is counts, in the marker row and in
//! one audit row; never a value, a lookup or a subject.

use serde_json::{Value, json};

use crate::audit::{self, Action, Entry};
use crate::home::{HomeError, Registry};
use crate::linkage::{self, Subkeys};
use crate::principal::Principal;
use crate::schema::{Type, table};
use crate::store::{Error, Param, Store};
use crate::time::now_iso;

/// The `registry_meta` row that says the pass ran, or that the registry was
/// made after it and needs none.
pub const MARKER: &str = "never_kept_by_shape";

/// How many rows one read takes.
const PAGE: usize = 5000;

/// What the pass met in one table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// The rows that kept a value.
    pub checked: i64,
    /// Those whose value was shaped like a personnummer, now blanked.
    pub blanked: i64,
    /// Those whose ciphertext did not open under the registry's key, left
    /// as they were.
    pub unreadable: i64,
}

impl Counts {
    fn as_json(self) -> Value {
        json!({
            "checked": self.checked,
            "blanked": self.blanked,
            "unreadable": self.unreadable,
        })
    }
}

/// What one pass did: the linkage store's identities and the registry's
/// held files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Swept {
    pub identities: Counts,
    pub held_files: Counts,
}

impl Swept {
    pub fn as_json(self) -> Value {
        json!({
            "identities": self.identities.as_json(),
            "held_files": self.held_files.as_json(),
        })
    }
}

/// The marker of a registry made by this binary: nothing in it was sealed
/// before the rule, so there is nothing to sweep.
pub(crate) fn fresh_marker() -> String {
    json!({"at": now_iso(), "fresh": true}).to_string()
}

/// The rows of `table_name` whose `column` keeps a value shaped like a
/// personnummer, by id, with what was met on the way. Nothing is changed and
/// no value leaves this function.
pub fn shaped(
    store: &mut Store,
    keys: &Subkeys,
    table_name: &str,
    column: &str,
) -> Result<(Counts, Vec<i64>), Error> {
    let sql = format!(
        "SELECT id, {column} FROM {} WHERE id > {} AND {column} IS NOT NULL AND length({column}) > 0 ORDER BY id LIMIT {PAGE}",
        store.qualified(table_name),
        store.dialect().param(1, Type::Int)
    );
    let mut counts = Counts::default();
    let mut ids = Vec::new();
    let mut after = i64::MIN;
    loop {
        let rows = store.query(&sql, &[Param::Int(after)])?;
        let Some(last) = rows.last() else {
            break;
        };
        after = last.int(0)?;
        for r in &rows {
            counts.checked += 1;
            match keys.open(r.bytes(1)?) {
                Ok(value) if linkage::personnummer_shaped(&value) => ids.push(r.int(0)?),
                Ok(_) => {}
                Err(_) => counts.unreadable += 1,
            }
        }
        if rows.len() < PAGE {
            break;
        }
    }
    counts.blanked = ids.len() as i64;
    Ok((counts, ids))
}

/// Run the pass on a registry that carries no marker. None where another
/// process marked the registry first, which then did the work.
pub(crate) fn run(registry: &mut Registry) -> Result<Option<Swept>, HomeError> {
    let keys = Subkeys::derive(&registry.pseudonym_key()?);
    let mut linkage = registry.open_linkage()?;
    let (identities, identity_ids) = shaped(&mut linkage, &keys, "identity", "ciphertext")?;
    let store = registry.store();
    let (held_files, held_ids) = shaped(store, &keys, "pseudonym_file", "sealed")?;
    let swept = Swept {
        identities,
        held_files,
    };
    store.begin()?;
    match blank(store, &mut linkage, &identity_ids, &held_ids, swept) {
        Ok(true) => {
            store.commit()?;
            Ok(Some(swept))
        }
        Ok(false) => {
            store.rollback()?;
            Ok(None)
        }
        Err(e) => {
            let _ = store.rollback();
            Err(e.into())
        }
    }
}

/// Inside the registry's transaction: take the marker, which only one
/// process gets, then blank the identities in a transaction of the linkage
/// store, the held files here, and write the audit row. False where the
/// marker was already taken.
fn blank(
    store: &mut Store,
    linkage: &mut Store,
    identity_ids: &[i64],
    held_ids: &[i64],
    swept: Swept,
) -> Result<bool, Error> {
    let mut marker = swept.as_json();
    marker["at"] = json!(now_iso());
    let d = store.dialect();
    let taken = store.execute(
        &format!(
            "INSERT INTO {} (key, value) VALUES ({}, {}) ON CONFLICT (key) DO NOTHING",
            store.qualified("registry_meta"),
            d.param(1, Type::Text),
            d.param(2, Type::Text)
        ),
        &[Param::from(MARKER), Param::from(marker.to_string())],
    )?;
    if taken == 0 {
        return Ok(false);
    }
    linkage.begin()?;
    match linkage.update_by_ids(
        table("identity"),
        &[("ciphertext", Param::Bytes(Vec::new()))],
        "id",
        identity_ids,
    ) {
        Ok(_) => linkage.commit()?,
        Err(e) => {
            let _ = linkage.rollback();
            return Err(e);
        }
    }
    store.update_by_ids(
        table("pseudonym_file"),
        &[("sealed", Param::Null)],
        "id",
        held_ids,
    )?;
    audit::record_in(
        store,
        &Entry {
            principal: &Principal::current().to_string(),
            action: Action::LinkageNeverKept,
            scope: json!({"what": "values shaped like a personnummer"}),
            policy: None,
            job_id: None,
            details: Some(swept.as_json()),
        },
    )?;
    Ok(true)
}
