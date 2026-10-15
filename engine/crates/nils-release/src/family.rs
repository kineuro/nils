// SPDX-License-Identifier: AGPL-3.0-only

//! The acquisition family the pack's routes recognised in a stack
//! (2026-10-14): SyMRI's, an SWI's, a mix sequence's or STAGE's. A route is
//! a rule set with an entry condition (spec section 6.5), and every axis its
//! rules decide cites it on the stack's evidence, so the family is read from
//! what the registry holds and nothing is classified again. The scans doors
//! carry it as `acquisition_family`, and a release spells it in the names of
//! an acquisition's own images and writes it into every sidecar (record 55,
//! ruling 23: "on SWI we need the SWI in the name too").

use std::collections::HashMap;

use nils_registry::store::{Error as StoreError, Store};

pub use nils_classify::family::{ROUTE_FAMILIES, acquisition_family, spelled, word};

/// The routes of [`ROUTE_FAMILIES`] that decided an axis of each of
/// `stacks` (every stack of the registry where `stacks` is none), as their
/// evidence cites them.
pub fn routes_of(
    store: &mut Store,
    stacks: Option<&[i64]>,
) -> Result<HashMap<i64, Vec<String>>, StoreError> {
    let named = ROUTE_FAMILIES
        .iter()
        .map(|(set, _)| format!("'{set}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let table = store.qualified("classification_evidence");
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    let mut read = |store: &mut Store, within: String| -> Result<(), StoreError> {
        for r in store.query(
            &format!(
                "SELECT DISTINCT stack_id, rule_set FROM {table} WHERE rule_set IN ({named}){within}"
            ),
            &[],
        )? {
            out.entry(r.int(0)?)
                .or_default()
                .push(r.text(1)?.to_string());
        }
        Ok(())
    };
    match stacks {
        None => read(store, String::new())?,
        Some(stacks) => {
            for chunk in stacks.chunks(500) {
                let list = chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !list.is_empty() {
                    read(store, format!(" AND stack_id IN ({list})"))?;
                }
            }
        }
    }
    Ok(out)
}
