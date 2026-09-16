// SPDX-License-Identifier: AGPL-3.0-only

//! The report of a run: what was seen, written, unchanged, held and
//! refused, the subjects met and made, the tags removed by count, the
//! private elements dropped, the bytes and the rate. Nothing in it comes
//! out of a file but tag numbers, refusal classes and the shapes of held
//! identifiers.

use std::collections::BTreeMap;
use std::fmt;

use nils_digest::cancel::Cancelled;
use nils_digest::report::{human_bytes, human_secs, thousands};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Files {
    pub seen: u64,
    pub written: u64,
    pub unchanged: u64,
    pub held: u64,
    pub refused: u64,
    /// Symbolic links and special files, never read.
    pub skipped: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subjects {
    /// Made by this run.
    pub new: u64,
    /// Met by this run, made or found.
    pub seen: u64,
    /// Met by this run and marked provisional, whichever run made them.
    pub provisional: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub name: String,
    pub dataset: String,
    pub originals: String,
    pub root: String,
    pub dry_run: bool,
    /// Only the held rows a map released or a person coded anyway were read.
    pub held_only: bool,
    pub workers: usize,
    pub files: Files,
    pub subjects: Subjects,
    /// Tag, as `(gggg,eeee)`, `overlay`, `curve` or `private <creator>`, to
    /// how many times it was removed.
    pub tags_removed: BTreeMap<String, u64>,
    pub private_removed: u64,
    /// Refusal class to count.
    pub refused_by: BTreeMap<String, u64>,
    /// The shape of a held identifier to the files held under it, this
    /// run's and the ones still held from before.
    pub held_by_shape: BTreeMap<String, u64>,
    /// Originals left as they are because the tree held a file of the
    /// same SOP instance already, read by a digest (a v0 tree holds every
    /// original it was made from): counted among the unchanged.
    #[serde(default)]
    pub in_tree: u64,
    pub walk_errors: u64,
    pub bytes: u64,
    pub seconds: f64,
    pub files_per_s: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<Cancelled>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<i64>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ended = match (self.dry_run, self.cancelled) {
            (true, None) => " (dry run)",
            (true, Some(Cancelled::Stopped)) => " (dry run, stopped)",
            (true, Some(Cancelled::Aborted)) => " (dry run, aborted)",
            (false, Some(Cancelled::Stopped)) => " (stopped)",
            (false, Some(Cancelled::Aborted)) => " (aborted)",
            (false, None) => "",
        };
        writeln!(
            f,
            "nils pseudonymize{ended}   name {}   dataset {}{}",
            self.name,
            self.dataset,
            if self.held_only {
                "   held rows only"
            } else {
                ""
            }
        )?;
        writeln!(f, "  originals        {}", self.originals)?;
        writeln!(f, "  tree             {}", self.root)?;
        writeln!(
            f,
            "  files            {} seen   {} {}   {} unchanged   {} held   {} refused   {} skipped   {} walk errors",
            thousands(self.files.seen),
            thousands(self.files.written),
            if self.dry_run {
                "would write"
            } else {
                "written"
            },
            thousands(self.files.unchanged),
            thousands(self.files.held),
            thousands(self.files.refused),
            thousands(self.files.skipped),
            thousands(self.walk_errors),
        )?;
        writeln!(
            f,
            "  subjects         {} seen   {} new   {} provisional",
            thousands(self.subjects.seen),
            thousands(self.subjects.new),
            thousands(self.subjects.provisional),
        )?;
        let removed: u64 = self.tags_removed.values().sum();
        writeln!(
            f,
            "  removed          {} elements over {} tags   {} private",
            thousands(removed),
            thousands(self.tags_removed.len() as u64),
            thousands(self.private_removed),
        )?;
        if !self.refused_by.is_empty() {
            let by: Vec<String> = self
                .refused_by
                .iter()
                .map(|(c, n)| format!("{c} {}", thousands(*n)))
                .collect();
            writeln!(f, "  refused          {}", by.join("   "))?;
        }
        if !self.held_by_shape.is_empty() {
            let by: Vec<String> = self
                .held_by_shape
                .iter()
                .map(|(s, n)| format!("{s} {}", thousands(*n)))
                .collect();
            writeln!(f, "  held by shape    {}", by.join("   "))?;
        }
        if self.in_tree > 0 {
            writeln!(
                f,
                "  in the tree      {} original(s) the tree held already, left as they are",
                thousands(self.in_tree)
            )?;
        }
        writeln!(
            f,
            "  run              {} in {}   {} files/s   workers {}",
            human_bytes(self.bytes),
            human_secs(self.seconds),
            thousands(self.files_per_s.round() as u64),
            self.workers,
        )?;
        if let Some(batch) = self.batch_id {
            writeln!(f, "  batch            {batch}")?;
        }
        Ok(())
    }
}
