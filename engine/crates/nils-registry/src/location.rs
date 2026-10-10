// SPDX-License-Identifier: AGPL-3.0-only

//! Where a scan's files are (record 55, Nima's duplicate policy of
//! 2026-10-10). Every file the digest reads is a location of its instance:
//! the instance's own, or another copy of it whose subject, study, series
//! and instance UIDs are all the instance's. A copy is never read as a new
//! instance; its `source_file` row names the instance, and `source_stack`
//! says which sources hold a file of each stack. A dataset therefore holds
//! every scan its tree has a file of, whoever read the scan first, and a
//! subject's sessions and scans are the same whichever dataset is asked.

use std::collections::BTreeSet;

use crate::dialect::Conflict;
use crate::empty::list;
use crate::schema::table;
use crate::store::{Error, Insert, Param, Store};

/// Record that a source holds a file of each of `stacks`, first seen `now`.
/// A pair already recorded keeps its first sighting. Runs inside the
/// caller's transaction, if any.
pub fn record(
    store: &mut Store,
    source_id: i64,
    stacks: &BTreeSet<i64>,
    now: &str,
) -> Result<(), Error> {
    if stacks.is_empty() {
        return Ok(());
    }
    let spec = Insert::new(
        table("source_stack"),
        &["source_id", "stack_id", "first_seen_at"],
    )
    .on_conflict(Conflict::Nothing(&["source_id", "stack_id"]));
    let rows: Vec<Vec<Param>> = stacks
        .iter()
        .map(|&stack| vec![Param::Int(source_id), Param::Int(stack), Param::from(now)])
        .collect();
    store.insert(&spec, &rows)?;
    Ok(())
}

/// An echo fold moved the instances of each `(from, to)` stack to the
/// other: the sources that held a file of `from` hold one of `to` now. The
/// rows of `from` stay for the stack's removal to take.
pub fn follow(store: &mut Store, moves: &[(i64, i64)]) -> Result<(), Error> {
    if moves.is_empty() {
        return Ok(());
    }
    let ss = store.qualified("source_stack");
    let mut held: Vec<(i64, i64, String)> = Vec::new();
    for chunk in moves.chunks(500) {
        let from: Vec<i64> = chunk.iter().map(|(f, _)| *f).collect();
        let sql = format!(
            "SELECT source_id, stack_id, {} FROM {ss} WHERE stack_id IN ({})",
            store.dialect().text_of(
                table("source_stack")
                    .column("first_seen_at")
                    .expect("first_seen_at")
            ),
            list(&from)
        );
        for r in store.query(&sql, &[])? {
            let stack = r.int(1)?;
            if let Some((_, to)) = chunk.iter().find(|(f, _)| *f == stack) {
                held.push((r.int(0)?, *to, r.text(2)?.to_string()));
            }
        }
    }
    let spec = Insert::new(
        table("source_stack"),
        &["source_id", "stack_id", "first_seen_at"],
    )
    .on_conflict(Conflict::Nothing(&["source_id", "stack_id"]));
    let rows: Vec<Vec<Param>> = held
        .into_iter()
        .map(|(source, stack, seen)| vec![Param::Int(source), Param::Int(stack), Param::from(seen)])
        .collect();
    store.insert(&spec, &rows)?;
    Ok(())
}

/// Fill `source_stack` from the files a registry from before recorded:
/// every source that has an instance's own file in a stack, first seen when
/// the earliest of them was last seen (the row kept no earlier sighting). A
/// file whose frames are in several stacks holds each. A copy a registry
/// from before filed by its instance UID alone holds nothing yet: the next
/// read of its source compares it, and records it if it is one.
pub fn backfill(store: &mut Store) -> Result<(), Error> {
    let (ss, file, instance, frame) = (
        store.qualified("source_stack"),
        store.qualified("source_file"),
        store.qualified("instance"),
        store.qualified("instance_frame"),
    );
    store.execute(
        &format!(
            "INSERT INTO {ss} (source_id, stack_id, first_seen_at) \
             SELECT u.source_id, u.stack_id, MIN(u.seen) FROM (\
               SELECT f.source_id AS source_id, i.stack_id AS stack_id, f.seen_at AS seen \
               FROM {file} f JOIN {instance} i ON i.id = f.instance_id \
               WHERE f.status = 'ingested' AND i.stack_id IS NOT NULL \
               UNION ALL \
               SELECT f.source_id, fr.stack_id, f.seen_at \
               FROM {file} f JOIN {frame} fr ON fr.instance_id = f.instance_id \
               WHERE f.status = 'ingested'\
             ) u GROUP BY u.source_id, u.stack_id"
        ),
        &[],
    )?;
    Ok(())
}
