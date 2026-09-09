// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4c §6.6: the four diagnostics of Wave 2 §10, counted per batch and
//! per kind with samples, written to the batch's `diagnostic` table beside
//! the digest's own. The evaluator reports the first three per stack
//! ([`nils_pack::Diagnostic`]); `overlay_unused` is a batch-level fact, a
//! term the overlay added that no evidence row of the batch ever cited.
//!
//! A keyword shadowed on one stack is a fact about that stack; a keyword
//! shadowed on some stacks and cited on none, over a whole batch, is one
//! that can never match, and that is what the row counts.

use std::collections::{BTreeMap, HashMap, HashSet};

use nils_pack::Verdict;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error, Insert, Param, Store};

/// Samples kept per row, as the digest keeps them.
pub const SAMPLE_MAX: usize = 10;

pub const KINDS: &[&str] = &[
    "axis_conflict",
    "axis_unresolved",
    "keyword_shadowed",
    "overlay_unused",
];

#[derive(Default)]
struct Counted {
    count: i64,
    samples: Vec<String>,
}

impl Counted {
    fn add(&mut self, sample: String) {
        self.count += 1;
        if self.samples.len() < SAMPLE_MAX && !self.samples.contains(&sample) {
            self.samples.push(sample);
        }
    }
}

#[derive(Default)]
struct Tally {
    conflict: Counted,
    unresolved: Counted,
    /// Keyword (folded) to how often it was shadowed, and one sample.
    shadowed: BTreeMap<String, (i64, String)>,
    /// Every keyword cited in evidence, folded.
    cited: HashSet<String>,
}

/// Per batch.
#[derive(Default)]
pub struct Tallies {
    by_batch: HashMap<i64, Tally>,
}

impl Tallies {
    /// Note one stack's verdict against its batch.
    pub fn note(&mut self, batch: i64, verdict: &Verdict) {
        let t = self.by_batch.entry(batch).or_default();
        for e in &verdict.evidence {
            if e.source == "text" && !e.matched.is_empty() {
                t.cited.insert(e.matched.to_lowercase());
            }
        }
        for d in &verdict.diagnostics {
            match d.kind.as_str() {
                "axis_conflict" => t.conflict.add(format!(
                    "{}: {}/{}={} [{}] after {}/{}={} [{}]",
                    d.axis,
                    d.rule_set,
                    d.rule,
                    d.value,
                    d.matched,
                    d.by_rule_set,
                    d.by_rule,
                    d.by_value,
                    d.by_matched
                )),
                "axis_unresolved" => t.unresolved.add(d.axis.clone()),
                "keyword_shadowed" => {
                    let e = t
                        .shadowed
                        .entry(d.matched.to_lowercase())
                        .or_insert_with(|| {
                            (
                                0,
                                format!(
                                    "{}: {} in {}/{} behind {} [{}]",
                                    d.axis, d.matched, d.rule_set, d.rule, d.by_rule, d.by_matched
                                ),
                            )
                        });
                    e.0 += 1;
                }
                _ => {}
            }
        }
    }

    /// Write the rows, replacing this run's kinds for the batches it read,
    /// and answer the totals by kind.
    pub fn write(
        self,
        store: &mut Store,
        overlay_terms: &[String],
        now: &str,
    ) -> Result<BTreeMap<String, i64>, Error> {
        let mut totals: BTreeMap<String, i64> = BTreeMap::new();
        let mut rows: Vec<Vec<Param>> = Vec::new();
        let mut batches: Vec<i64> = self.by_batch.keys().copied().collect();
        batches.sort_unstable();
        for batch in &batches {
            let t = &self.by_batch[batch];
            let mut row = |kind: &str, count: i64, samples: &[String]| {
                if count == 0 {
                    return;
                }
                *totals.entry(kind.to_string()).or_insert(0) += count;
                rows.push(vec![
                    Param::Int(*batch),
                    Param::from(kind),
                    Param::from("batch"),
                    Param::Null,
                    Param::Int(count),
                    Param::from(serde_json::to_string(samples).unwrap_or_default()),
                    Param::from(now),
                ]);
            };
            row("axis_conflict", t.conflict.count, &t.conflict.samples);
            row("axis_unresolved", t.unresolved.count, &t.unresolved.samples);
            // A keyword that was shadowed somewhere and cited nowhere in
            // this batch can never match here.
            let never: Vec<(&String, &(i64, String))> = t
                .shadowed
                .iter()
                .filter(|(k, _)| !t.cited.contains(*k))
                .collect();
            let samples: Vec<String> = never
                .iter()
                .take(SAMPLE_MAX)
                .map(|(_, (n, s))| format!("{s} ({n})"))
                .collect();
            row("keyword_shadowed", never.len() as i64, &samples);
            let unused: Vec<String> = overlay_terms
                .iter()
                .filter(|term| !t.cited.contains(&term.to_lowercase()))
                .cloned()
                .collect();
            let samples: Vec<String> = unused.iter().take(SAMPLE_MAX).cloned().collect();
            row("overlay_unused", unused.len() as i64, &samples);
        }
        if batches.is_empty() {
            return Ok(totals);
        }
        let d = store.dialect();
        let kinds: Vec<String> = KINDS.iter().map(|k| format!("'{k}'")).collect();
        for chunk in batches.chunks(256) {
            let holes: Vec<String> = (0..chunk.len())
                .map(|i| d.param(i + 1, Type::Int))
                .collect();
            let sql = format!(
                "DELETE FROM {} WHERE kind IN ({}) AND batch_id IN ({})",
                store.qualified("diagnostic"),
                kinds.join(", "),
                holes.join(", ")
            );
            let params: Vec<Param> = chunk.iter().map(|b| Param::Int(*b)).collect();
            store.execute(&sql, &params)?;
        }
        if !rows.is_empty() {
            store.insert(&Insert::all(table("diagnostic")), &rows)?;
        }
        Ok(totals)
    }
}
