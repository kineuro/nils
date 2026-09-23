// SPDX-License-Identifier: AGPL-3.0-only

//! `nils classify votes` (record 41, S2): the vote matrix a label model
//! reads.
//!
//! One line per vote: the stack, the axis, the rule set, the rule, the
//! clause's place in it, the value it said, the clause's tier, and 1 where
//! the clause only restates another axis (a schema implication, which a
//! label model reads as a constraint rather than a witness). Identities
//! and the pack's own vocabulary only: no text a stack carried, no keyword
//! it matched, nothing a person typed. Read in windows of stacks, so memory
//! does not follow the size of the registry.

use std::collections::{BTreeSet, HashMap};
use std::io::Write;

use nils_registry::schema::Type;
use nils_registry::store::{Param, Store};
use serde::Serialize;

use crate::job::Error;

/// The columns, in order, as the first line says them.
pub const HEADER: &str = "stack_id\taxis\trule_set\trule\tclause\tvalue\ttier\trestates";

/// What was written.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Written {
    /// Stacks with a vote list, whether or not it held a vote.
    pub stacks: i64,
    pub votes: i64,
    /// The axes the lines name.
    pub axes: Vec<String>,
}

/// Which votes to write.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only this axis.
    pub axis: Option<String>,
}

/// Write the vote matrix as tab-separated lines, a header first, in stack
/// order and within a stack in the order the rules were heard, the class
/// phase before the disposition.
pub fn write(store: &mut Store, filter: &Filter, out: &mut dyn Write) -> Result<Written, Error> {
    let io = |e: std::io::Error| {
        Error::Store(nils_registry::store::Error::Message(format!(
            "the votes will not write: {e}"
        )))
    };
    let mut voters: HashMap<i64, (String, String, String, i64, String, i64)> = HashMap::new();
    let sql = format!(
        "SELECT id, axis, rule_set, rule, clause, tier, restates FROM {}",
        store.qualified("classification_voter")
    );
    for r in store.query(&sql, &[])? {
        voters.insert(
            r.int(0)?,
            (
                r.text(1)?.to_string(),
                r.text(2)?.to_string(),
                r.text(3)?.to_string(),
                r.int(4)?,
                r.text(5)?.to_string(),
                r.int(6)?,
            ),
        );
    }

    writeln!(out, "{HEADER}").map_err(io)?;
    let mut written = Written::default();
    let mut axes: BTreeSet<String> = BTreeSet::new();
    let window: i64 = 4_096;
    let sql = format!(
        "SELECT stack_id, votes FROM {} WHERE stack_id > {} AND stack_id <= {} ORDER BY stack_id, phase",
        store.qualified("classification_vote"),
        store.dialect().param(1, Type::Int),
        store.dialect().param(2, Type::Int),
    );
    let bounds = store.query(
        &format!(
            "SELECT MIN(stack_id), MAX(stack_id) FROM {}",
            store.qualified("classification_vote")
        ),
        &[],
    )?;
    let (Some(first), Some(last)) = (
        bounds.first().and_then(|r| r.int(0).ok()),
        bounds.first().and_then(|r| r.int(1).ok()),
    ) else {
        return Ok(written);
    };
    let mut after = first - 1;
    let mut seen_stack: Option<i64> = None;
    while after < last {
        let upto = after.saturating_add(window);
        for r in store.query(&sql, &[Param::Int(after), Param::Int(upto)])? {
            let stack = r.int(0)?;
            if seen_stack != Some(stack) {
                written.stacks += 1;
                seen_stack = Some(stack);
            }
            let pairs: Vec<(i64, String)> = serde_json::from_str(r.text(1)?).map_err(|e| {
                Error::Store(nils_registry::store::Error::Message(format!(
                    "stack {stack}: its votes do not read: {e}"
                )))
            })?;
            for (voter, value) in pairs {
                let Some((axis, rule_set, rule, clause, tier, restates)) = voters.get(&voter)
                else {
                    continue;
                };
                if filter.axis.as_ref().is_some_and(|a| a != axis) {
                    continue;
                }
                writeln!(
                    out,
                    "{stack}\t{axis}\t{rule_set}\t{rule}\t{clause}\t{value}\t{tier}\t{restates}"
                )
                .map_err(io)?;
                written.votes += 1;
                if !axes.contains(axis) {
                    axes.insert(axis.clone());
                }
            }
        }
        after = upto;
    }
    out.flush().map_err(io)?;
    written.axes = axes.into_iter().collect();
    Ok(written)
}
