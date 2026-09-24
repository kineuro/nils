// SPDX-License-Identifier: AGPL-3.0-only

//! A pipeline's proposals (record 43 S6): the values its `results.json`
//! proposes on the axes its descriptor declares.
//!
//! This is the hook the runner calls after a run and the one place the
//! proposals go through. Until S6 lands it takes nothing: a proposal is
//! counted and kept in the run's `results.json`, and nothing reaches the
//! review spine, so nothing is in force. S6 replaces [`ingest`] with the
//! grouped `<axis>:model` review items and the staged model decisions.

use nils_registry::Registry;
use serde_json::Value;

/// What the hook did with a run's proposals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Ingested {
    /// Proposals on an axis the descriptor declares.
    pub declared: usize,
    /// Proposals on an axis it does not, which are never taken.
    pub undeclared: usize,
    /// Review items or staged decisions written.
    pub taken: usize,
}

/// The run a proposal came from, as the hook needs it.
pub(crate) struct From<'a> {
    pub run_id: i64,
    pub pipeline: &'a str,
    /// The axes the descriptor declares it may propose on.
    pub axes: &'a [String],
    pub model_ids: &'a [i64],
    pub job_id: Option<i64>,
}

/// Pass a run's proposals to the review spine. Until record 43 S6 this
/// counts them and writes nothing.
pub(crate) fn ingest(
    _registry: &mut Registry,
    from: &From<'_>,
    proposals: &[Value],
) -> Result<Ingested, String> {
    let declared = proposals
        .iter()
        .filter(|p| {
            p["axis"]
                .as_str()
                .is_some_and(|a| from.axes.iter().any(|x| x == a))
        })
        .count();
    let _ = (from.run_id, from.pipeline, from.model_ids, from.job_id);
    Ok(Ingested {
        declared,
        undeclared: proposals.len() - declared,
        taken: 0,
    })
}
