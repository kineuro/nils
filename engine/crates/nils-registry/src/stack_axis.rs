// SPDX-License-Identifier: AGPL-3.0-only

//! The axes of a stack as one row (`stack_axis`): a read model of
//! `classification_axis`, which keeps a row per value and stays the place
//! of truth. A question that reads an axis of every stack of a set (a
//! predicate, a group's key, a level's signature, the rule that a stack the
//! pack ruled out is in no set) read `classification_axis` once per stack
//! and axis, and on a registry of half a million stacks and five million
//! axis rows the reading was most of the question's time. Here each stack
//! has one row:
//!
//! - `axes`, one JSON object: each axis the stack has a row of, with the
//!   array of its values, one element per row, a row with no value as null;
//! - `excluded`, 1 where the stack's `disposition` holds `excluded`, with a
//!   unique index over those rows alone, so the rule is one lookup in a
//!   list of the few stacks ruled out.
//!
//! It holds nothing `classification_axis` does not: the same values per
//! stack and axis, grouped. What a value is called, in which order values
//! are listed and what a stack with none reads as are all decided where the
//! row is read, as they were on the rows themselves.
//!
//! **It is never stale.** Every writer of `classification_axis` calls
//! [`refresh`] for the stacks it wrote, inside the transaction that wrote
//! them: the sort's class and disposition phases, a pass, the session
//! pass, the synthetic registries. A stack that is removed takes its row
//! with it (`crate::empty::STACK_TABLES`). [`verify`] compares the two for
//! every stack and [`rebuild`] makes the read model again from nothing
//! (`nils repair stack-axis`); the migration that added it is a rebuild.

use serde::{Deserialize, Serialize};

use crate::store::{Error, Store};

/// The axis and the value of a stack the pack ruled out (wave 4b, section
/// 4.4 rule 10): no set of a question sees it.
pub const RULED_OUT: (&str, &str) = ("disposition", "excluded");

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every stack's row as `classification_axis` says it now, `(stack_id,
/// axes, excluded)`, for the stacks `scope` names (`ax.stack_id IN (...)`)
/// or for all. The object's keys and each array's values are put in one
/// order, so that the same rows always make the same document.
fn fresh(store: &Store, scope: Option<&str>) -> String {
    let axis = store.qualified("classification_axis");
    let scope = scope.map_or(String::new(), |s| format!(" WHERE {s}"));
    let (ruled_axis, ruled_value) = RULED_OUT;
    let excluded = format!(
        "MAX(CASE WHEN ax.axis = '{ruled_axis}' AND ax.value = '{ruled_value}' THEN 1 ELSE 0 END)"
    );
    match store {
        Store::Sqlite(_) => format!(
            "SELECT a.stack_id AS stack_id, \
             json_group_object(a.axis, json(a.vals) ORDER BY a.axis) AS axes, \
             MAX(a.excluded) AS excluded \
             FROM (SELECT ax.stack_id AS stack_id, ax.axis AS axis, \
             json_group_array(ax.value ORDER BY ax.value) AS vals, {excluded} AS excluded \
             FROM {axis} ax{scope} GROUP BY ax.stack_id, ax.axis) a GROUP BY a.stack_id"
        ),
        Store::Postgres { .. } => format!(
            "SELECT a.stack_id AS stack_id, jsonb_object_agg(a.axis, a.vals) AS axes, \
             MAX(a.excluded) AS excluded \
             FROM (SELECT ax.stack_id AS stack_id, ax.axis AS axis, \
             jsonb_agg(ax.value ORDER BY ax.value) AS vals, {excluded} AS excluded \
             FROM {axis} ax{scope} GROUP BY ax.stack_id, ax.axis) a GROUP BY a.stack_id"
        ),
    }
}

/// Bring the rows of these stacks up to what `classification_axis` holds
/// for them now. Every writer of `classification_axis` calls this for the
/// stacks it wrote, inside its own transaction, after its last write: a
/// stack left with no axis row is left with no row here.
pub fn refresh(store: &mut Store, stacks: &[i64]) -> Result<(), Error> {
    let mine = store.qualified("stack_axis");
    for chunk in stacks.chunks(500) {
        let ids = list(chunk);
        store.execute(
            &format!("DELETE FROM {mine} WHERE stack_id IN ({ids})"),
            &[],
        )?;
        let rows = fresh(store, Some(&format!("ax.stack_id IN ({ids})")));
        store.execute(
            &format!("INSERT INTO {mine} (stack_id, axes, excluded) {rows}"),
            &[],
        )?;
    }
    Ok(())
}

/// Make every row again from `classification_axis`, and say how many
/// stacks have one. Runs inside the caller's transaction, if any.
pub fn rebuild(store: &mut Store) -> Result<u64, Error> {
    let mine = store.qualified("stack_axis");
    store.execute(&format!("DELETE FROM {mine}"), &[])?;
    let rows = fresh(store, None);
    store.execute(
        &format!("INSERT INTO {mine} (stack_id, axes, excluded) {rows}"),
        &[],
    )
}

/// How the read model stands against `classification_axis`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    /// Stacks with a row.
    pub stacks: u64,
    /// Stacks with axis rows and no row here.
    pub missing: u64,
    /// Rows here of stacks with no axis row.
    pub extra: u64,
    /// Rows that say something else than the axis rows do.
    pub different: u64,
    /// Some of the stacks that differ, lowest first.
    pub sample: Vec<i64>,
}

impl Standing {
    /// Every stack's row says what its axis rows say.
    pub fn agrees(&self) -> bool {
        self.missing == 0 && self.extra == 0 && self.different == 0
    }
}

/// Compare the read model against `classification_axis` for every stack.
pub fn verify(store: &mut Store) -> Result<Standing, Error> {
    let mine = store.qualified("stack_axis");
    let rows = fresh(store, None);
    let mut out = Standing {
        stacks: store.query(&format!("SELECT COUNT(*) FROM {mine}"), &[])?[0].int(0)? as u64,
        ..Standing::default()
    };
    let differing = format!(
        "SELECT COALESCE(n.stack_id, x.stack_id) AS stack_id, \
         CASE WHEN x.stack_id IS NULL THEN 0 WHEN n.stack_id IS NULL THEN 1 ELSE 2 END AS how \
         FROM ({rows}) n FULL JOIN {mine} x ON x.stack_id = n.stack_id \
         WHERE x.stack_id IS NULL OR n.stack_id IS NULL OR x.axes <> n.axes OR x.excluded <> n.excluded"
    );
    for r in store.query(
        &format!("SELECT d.how, COUNT(*) FROM ({differing}) d GROUP BY d.how"),
        &[],
    )? {
        let n = r.int(1)? as u64;
        match r.int(0)? {
            0 => out.missing = n,
            1 => out.extra = n,
            _ => out.different = n,
        }
    }
    if !out.agrees() {
        out.sample = store
            .query(
                &format!("SELECT d.stack_id FROM ({differing}) d ORDER BY d.stack_id LIMIT 20"),
                &[],
            )?
            .iter()
            .map(|r| r.int(0))
            .collect::<Result<_, _>>()?;
    }
    Ok(out)
}
