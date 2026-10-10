// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4c §6.6: a candidate overlay rehearsed over a bounded sample of a
//! scope, writing nothing. The answer is the change manifest's
//! `predicted_impact`, computed: what would move per axis, from which value
//! to which and how many; how many review items would close and open; and
//! the pass or fail of the overlay's own cases.

use std::collections::BTreeMap;

use nils_pack::{Evaluated, Pack};
use nils_registry::store::{Error, Store};
use serde_json::{Value, json};

use crate::classify::{scoped_select, to_stack};
use crate::scope::Scope;

/// The sample when the caller names none, and the most a door reads.
pub const SAMPLE_DEFAULT: usize = 2_000;
pub const SAMPLE_MAX: usize = 20_000;

/// Whether the classifier would ask a person about this axis of this
/// verdict, as `classify` decides it. Record 55 H3 (2026-10-09): an axis no
/// rule answered is asked where it matters (`missing_asked`); a rule's low
/// confidence alone is asked only when a person names a threshold
/// (`review_below`), never by the pack's; a conflict is never asked.
fn asks(
    review_below: Option<f64>,
    verdict: &nils_pack::Verdict,
    axis: &str,
    missing_asked: &[String],
    by_model: &[String],
) -> bool {
    if verdict.silent {
        return false;
    }
    if verdict.unresolved.iter().any(|a| a == axis) {
        return missing_asked.iter().any(|a| a == axis);
    }
    let Some(below) = review_below else {
        return false;
    };
    if by_model.iter().any(|a| a == axis) {
        return false;
    }
    match verdict.axis(axis) {
        Some(a) if !a.stored().is_empty() => {
            a.confidence > 0.0 && nils_pack::weaker_than(a.confidence, below)
        }
        _ => false,
    }
}

/// Rehearse `after` against `before` over the sample. `cases` is the
/// overlay's own case failure, when its cases did not hold, and
/// `assertions` how many it made.
pub fn run(
    store: &mut Store,
    before: &Pack,
    after: &Pack,
    scope: &Scope,
    sample: usize,
    review_below: Option<f64>,
    cases: (usize, Option<String>),
) -> Result<Value, Error> {
    let sample = sample.clamp(1, SAMPLE_MAX);
    let (sql, params) = scoped_select(store, &before.modality, scope, sample);
    let rows = store.query(&sql, &params)?;
    // (axis, from, to) -> stacks
    let mut moves: BTreeMap<(String, String, String), i64> = BTreeMap::new();
    let mut close = 0i64;
    let mut open = 0i64;
    let mut read = 0i64;
    let asked_before = nils_pack::matters::missing_asked(before);
    let asked_after = nils_pack::matters::missing_asked(after);
    for r in &rows {
        let (_, stack, private) =
            to_stack(r, false, before).map_err(|e| Error::Message(e.to_string()))?;
        read += 1;
        let was = Evaluated::with_private(before, &stack, private.clone()).classify();
        let now = Evaluated::with_private(after, &stack, private).classify();
        let mut axes: Vec<&str> = was.axes.iter().map(|a| a.axis.as_str()).collect();
        for a in now
            .axes
            .iter()
            .map(|a| &a.axis)
            .chain(&was.unresolved)
            .chain(&now.unresolved)
        {
            if !axes.contains(&a.as_str()) {
                axes.push(a);
            }
        }
        for axis in axes {
            let from = was.stored(axis);
            let to = now.stored(axis);
            if from != to {
                *moves
                    .entry((axis.to_string(), from.clone(), to.clone()))
                    .or_insert(0) += 1;
            }
            let asked = asks(
                review_below,
                &was,
                axis,
                &asked_before,
                &before.review.by_model,
            );
            let asks_now = asks(
                review_below,
                &now,
                axis,
                &asked_after,
                &after.review.by_model,
            );
            match (asked, asks_now) {
                (true, false) => close += 1,
                (false, true) => open += 1,
                _ => {}
            }
        }
    }
    let moves: Vec<Value> = moves
        .into_iter()
        .map(|((axis, from, to), stacks)| json!({"axis": axis, "from": from, "to": to, "stacks": stacks}))
        .collect();
    let (assertions, failure) = cases;
    let failed = failure
        .as_deref()
        .and_then(|f| f.split(" of ").next())
        .and_then(|n| n.rsplit(' ').next())
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    Ok(json!({
        "scope": scope.text(),
        "sample": {"asked": sample, "read": read},
        "moves": moves,
        "review_items": {"close": close, "open": open},
        "cases": {
            "passed": assertions.saturating_sub(failed),
            "failed": failed,
            "failures": failure,
        },
        "overlay": after.overlay,
        "pack": after.id(),
    }))
}

/// Wave 5 §12.4: the stacks an overlay moves, by id. The same pass as
/// `run`, keeping the stack instead of counting it; the closure the
/// dependency door names. The sample is bounded like a rehearsal's.
pub fn moved_stacks(
    store: &mut Store,
    before: &Pack,
    after: &Pack,
    scope: &Scope,
    sample: usize,
) -> Result<Vec<i64>, Error> {
    let sample = sample.clamp(1, SAMPLE_MAX);
    let (sql, params) = scoped_select(store, &before.modality, scope, sample);
    let rows = store.query(&sql, &params)?;
    let mut moved = Vec::new();
    for r in &rows {
        let id = r.int(0)?;
        let (_, stack, private) =
            to_stack(r, false, before).map_err(|e| Error::Message(e.to_string()))?;
        let was = Evaluated::with_private(before, &stack, private.clone()).classify();
        let now = Evaluated::with_private(after, &stack, private).classify();
        let mut axes: Vec<&str> = was.axes.iter().map(|a| a.axis.as_str()).collect();
        for a in &now.axes {
            if !axes.contains(&a.axis.as_str()) {
                axes.push(&a.axis);
            }
        }
        if axes.iter().any(|axis| was.stored(axis) != now.stored(axis)) {
            moved.push(id);
        }
    }
    Ok(moved)
}

/// The sample size a caller asked for, bounded.
pub fn sample_of(asked: Option<i64>) -> usize {
    match asked {
        Some(n) if n > 0 => (n as usize).min(SAMPLE_MAX),
        _ => SAMPLE_DEFAULT,
    }
}
