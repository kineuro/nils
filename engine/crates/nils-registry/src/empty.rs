// SPDX-License-Identifier: AGPL-3.0-only

//! Stacks and series that hold no instance. A file whose instance another
//! file already holds under another series is filed as a duplicate, but the
//! digest has made its series and its stack by then, so a registry could
//! keep rows that describe nothing (203 stacks in a 518,365-stack registry).
//! The digest sweeps the ones it made at the end of its run, and
//! `nils repair empty-stacks` sweeps a registry's whole.
//!
//! A stack is removed only when nothing a person or a release did names it:
//! no decision, no pick, no seal, no campaign item, no review item but an
//! open one, no release row, no derivative and no measure. Such a stack is
//! kept and reported instead, with the first reason found. What is derived
//! from the stack alone (its fingerprint, its classification and the open
//! review items the classifier raised on it) goes with it.
//!
//! Which tables carry a stack or a series is not remembered here by hand:
//! every table with a `stack_id` or a `series_id` column is listed in
//! [`STACK_TABLES`] or [`SERIES_TABLES`] with what a sweep does to it, and a
//! test fails when the schema gains one the lists do not name.

use std::collections::{BTreeMap, BTreeSet};

use crate::schema::{Type, table};
use crate::store::{Error, Param, Store};

/// What a sweep does to a table's rows for a stack or a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Derived from the stack or series alone: removed with it.
    Goes,
    /// Names what someone did: the stack or series is kept and reported.
    Keeps(&'static str),
    /// Checked on its own before anything is swept (the instance rows,
    /// the stack rows of a series).
    Holds,
}

/// Every table with a `stack_id` column, and what a sweep does to it.
pub const STACK_TABLES: &[(&str, Row)] = &[
    ("instance", Row::Holds),
    ("instance_frame", Row::Holds),
    ("stack_fingerprint", Row::Goes),
    ("classification", Row::Goes),
    ("classification_axis", Row::Goes),
    ("classification_evidence", Row::Goes),
    ("classification_vote", Row::Goes),
    ("pick_stack", Row::Keeps("a pick names it")),
    ("sealed_stack", Row::Keeps("it is of a sealed sample")),
    ("campaign_item", Row::Keeps("a campaign asks it")),
    (
        "review_member",
        Row::Keeps("a grouped review item holds it"),
    ),
    ("derivative", Row::Keeps("a derivative was made of it")),
    ("measure", Row::Keeps("a measure was taken of it")),
    ("release_stack", Row::Keeps("a release holds it")),
    ("release_plan", Row::Keeps("a release planned it")),
    ("release_absent", Row::Keeps("a release names it")),
    ("release_move", Row::Keeps("a release moved it")),
];

/// Every table with a `series_id` column, and what a sweep does to it.
pub const SERIES_TABLES: &[(&str, Row)] = &[
    ("stack", Row::Holds),
    ("instance", Row::Holds),
    ("stack_fingerprint", Row::Goes),
    ("series_private", Row::Goes),
    ("series_mr", Row::Goes),
    ("series_ct", Row::Goes),
    ("series_pet", Row::Goes),
    ("derivative", Row::Keeps("a derivative was made of it")),
];

/// What a sweep did, or would do on a dry run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Swept {
    pub stacks: u64,
    pub series: u64,
    /// Empty stacks kept, each with why.
    pub kept_stacks: Vec<(i64, &'static str)>,
    /// Empty series kept, each with why.
    pub kept_series: Vec<(i64, &'static str)>,
}

impl Swept {
    pub fn as_json(&self) -> serde_json::Value {
        let why = |kept: &[(i64, &str)]| -> serde_json::Value {
            kept.iter()
                .map(|(id, why)| serde_json::json!({"id": id, "why": why}))
                .collect()
        };
        serde_json::json!({
            "stacks_removed": self.stacks,
            "series_removed": self.series,
            "stacks_kept": why(&self.kept_stacks),
            "series_kept": why(&self.kept_series),
        })
    }
}

fn ids_of(store: &mut Store, sql: &str, params: &[Param]) -> Result<Vec<i64>, Error> {
    store.query(sql, params)?.iter().map(|r| r.int(0)).collect()
}

fn list(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// The ids among `ids` that `column` of `t` names.
fn named_in(store: &mut Store, t: &str, column: &str, ids: &[i64]) -> Result<BTreeSet<i64>, Error> {
    let mut out = BTreeSet::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT DISTINCT {column} FROM {} WHERE {column} IN ({})",
            store.qualified(t),
            list(chunk)
        );
        out.extend(ids_of(store, &sql, &[])?);
    }
    Ok(out)
}

/// The ids among `ids` a decision at `scope` names.
fn decided(store: &mut Store, scope: &str, ids: &[i64]) -> Result<BTreeSet<i64>, Error> {
    let mut out = BTreeSet::new();
    for chunk in ids.chunks(500) {
        let refs = chunk
            .iter()
            .map(|i| format!("'{i}'"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT DISTINCT ref FROM {} WHERE scope = {} AND ref IN ({refs})",
            store.qualified("decision"),
            store.dialect().param(1, Type::Text)
        );
        for r in store.query(&sql, &[Param::from(scope)])? {
            if let Ok(id) = r.text(0)?.parse::<i64>() {
                out.insert(id);
            }
        }
    }
    Ok(out)
}

/// The stack review items that name one of `ids`: (item, stack, open). A
/// campaign's item is kept by its `campaign_item` row already.
fn reviewed(store: &mut Store, ids: &BTreeSet<i64>) -> Result<Vec<(i64, i64, bool)>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let t = table("review_item");
    let d = store.dialect();
    let sql = format!(
        "SELECT id, {}, status FROM {} WHERE scope = 'stack'",
        d.text_of(t.column("ref").expect("ref")),
        store.qualified("review_item"),
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        let Some(stack) = r
            .opt_text(1)?
            .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            .and_then(|v| v["stack_id"].as_i64())
        else {
            continue;
        };
        if ids.contains(&stack) {
            let open = r.text(2)? == "open";
            out.push((r.int(0)?, stack, open));
        }
    }
    Ok(out)
}

fn delete_where(store: &mut Store, t: &str, column: &str, ids: &[i64]) -> Result<(), Error> {
    for chunk in ids.chunks(500) {
        store.execute(
            &format!(
                "DELETE FROM {} WHERE {column} IN ({})",
                store.qualified(t),
                list(chunk)
            ),
            &[],
        )?;
    }
    Ok(())
}

/// Sweep the stacks and series that hold no instance: those the digest
/// batch `batch` made, or every one when `batch` is None. Writes nothing
/// when `dry_run`. Runs inside the caller's transaction, if any.
pub fn sweep(store: &mut Store, batch: Option<i64>, dry_run: bool) -> Result<Swept, Error> {
    let d = store.dialect();
    let scoped = |alias: &str| match batch {
        Some(_) => format!(" AND {alias}.first_batch_id = {}", d.param(1, Type::Int)),
        None => String::new(),
    };
    let params: Vec<Param> = batch.map(Param::Int).into_iter().collect();
    let sql = format!(
        "SELECT s.id FROM {} s WHERE s.n_instances = 0{} \
         AND NOT EXISTS (SELECT 1 FROM {} i WHERE i.stack_id = s.id) \
         AND NOT EXISTS (SELECT 1 FROM {} f WHERE f.stack_id = s.id) ORDER BY s.id",
        store.qualified("stack"),
        scoped("s"),
        store.qualified("instance"),
        store.qualified("instance_frame"),
    );
    let empty = ids_of(store, &sql, &params)?;
    let mut out = Swept::default();
    let mut kept: BTreeMap<i64, &'static str> = BTreeMap::new();
    if !empty.is_empty() {
        for id in decided(store, "stack", &empty)? {
            kept.entry(id).or_insert("a decision names it");
        }
        for (t, row) in STACK_TABLES {
            if let Row::Keeps(why) = row {
                for id in named_in(store, t, "stack_id", &empty)? {
                    kept.entry(id).or_insert(why);
                }
            }
        }
    }
    let set: BTreeSet<i64> = empty.iter().copied().collect();
    let items = reviewed(store, &set)?;
    for (_, stack, open) in &items {
        if !open {
            kept.entry(*stack)
                .or_insert("a review item on it was answered");
        }
    }
    let gone: Vec<i64> = empty
        .iter()
        .copied()
        .filter(|id| !kept.contains_key(id))
        .collect();
    out.kept_stacks = kept.into_iter().collect();
    out.stacks = gone.len() as u64;

    // the series of the stacks that go, and the empty series the scope made
    let mut series: BTreeMap<i64, i64> = BTreeMap::new();
    for chunk in gone.chunks(500) {
        let sql = format!(
            "SELECT series_id, COUNT(*) FROM {} WHERE id IN ({}) GROUP BY series_id",
            store.qualified("stack"),
            list(chunk)
        );
        for r in store.query(&sql, &[])? {
            *series.entry(r.int(0)?).or_default() += r.int(1)?;
        }
    }
    let sql = format!(
        "SELECT se.id FROM {} se WHERE se.n_instances = 0{} \
         AND NOT EXISTS (SELECT 1 FROM {} i WHERE i.series_id = se.id)",
        store.qualified("series"),
        scoped("se"),
        store.qualified("instance"),
    );
    let mut maybe: BTreeSet<i64> = ids_of(store, &sql, &params)?.into_iter().collect();
    // a series of a stack that goes is empty only if no instance names it
    let touched: Vec<i64> = series.keys().copied().collect();
    let with_instances = named_in(store, "instance", "series_id", &touched)?;
    maybe.extend(touched.iter().filter(|s| !with_instances.contains(s)));
    let maybe: Vec<i64> = maybe.into_iter().collect();
    // a series still holding a stack that stays is not empty
    let mut stacks_left: BTreeMap<i64, i64> = BTreeMap::new();
    for chunk in maybe.chunks(500) {
        let sql = format!(
            "SELECT series_id, COUNT(*) FROM {} WHERE series_id IN ({}) GROUP BY series_id",
            store.qualified("stack"),
            list(chunk)
        );
        for r in store.query(&sql, &[])? {
            stacks_left.insert(r.int(0)?, r.int(1)?);
        }
    }
    let empty_series: Vec<i64> = maybe
        .into_iter()
        .filter(|s| stacks_left.get(s).copied().unwrap_or(0) == series.get(s).copied().unwrap_or(0))
        .collect();
    let mut kept_series: BTreeMap<i64, &'static str> = BTreeMap::new();
    if !empty_series.is_empty() {
        for id in decided(store, "series", &empty_series)? {
            kept_series.entry(id).or_insert("a decision names it");
        }
        for (t, row) in SERIES_TABLES {
            if let Row::Keeps(why) = row {
                for id in named_in(store, t, "series_id", &empty_series)? {
                    kept_series.entry(id).or_insert(why);
                }
            }
        }
    }
    let series_gone: Vec<i64> = empty_series
        .iter()
        .copied()
        .filter(|id| !kept_series.contains_key(id))
        .collect();
    out.kept_series = kept_series.into_iter().collect();
    out.series = series_gone.len() as u64;
    if dry_run {
        return Ok(out);
    }

    let open_items: Vec<i64> = items
        .iter()
        .filter(|(_, stack, open)| *open && gone.binary_search(stack).is_ok())
        .map(|(item, _, _)| *item)
        .collect();
    delete_where(store, "review_item", "id", &open_items)?;
    for (t, row) in STACK_TABLES {
        if *row == Row::Goes {
            delete_where(store, t, "stack_id", &gone)?;
        }
    }
    delete_where(store, "stack", "id", &gone)?;
    for (t, row) in SERIES_TABLES {
        if *row == Row::Goes {
            delete_where(store, t, "series_id", &series_gone)?;
        }
    }
    delete_where(store, "series", "id", &series_gone)?;
    // a series that stays counts the stacks it has left
    let fewer: Vec<(i64, i64)> = series
        .into_iter()
        .filter(|(s, _)| series_gone.binary_search(s).is_err())
        .collect();
    if !fewer.is_empty() {
        store.update_from_values(table("series"), "n_stacks = n_stacks - v.val", "id", &fewer)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::registry_tables;

    /// Every table that carries a stack or a series is named, so a new one
    /// is a choice and not a row left behind.
    #[test]
    fn every_table_with_a_stack_or_a_series_is_named() {
        for t in registry_tables() {
            if t.column("stack_id").is_some() {
                assert!(
                    STACK_TABLES.iter().any(|(n, _)| *n == t.name),
                    "{} has a stack_id; name it in STACK_TABLES",
                    t.name
                );
            }
            if t.column("series_id").is_some() {
                assert!(
                    SERIES_TABLES.iter().any(|(n, _)| *n == t.name),
                    "{} has a series_id; name it in SERIES_TABLES",
                    t.name
                );
            }
        }
        for (n, _) in STACK_TABLES.iter().chain(SERIES_TABLES) {
            assert!(
                registry_tables().iter().any(|t| t.name == *n),
                "{n} is not a registry table"
            );
        }
    }
}
