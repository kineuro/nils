// SPDX-License-Identifier: AGPL-3.0-only

//! The digest of NILS: from a tree of files to rows in the registry.
//!
//! What lives here, from the Wave 1 specification
//! (`docs/specs/wave1-parse-and-digest.md`): the walker and its quarantine
//! classes (§5), identity resolution (§7), stack signatures (§8), the pipeline
//! from reader to writer with its bounds (§9), and jobs, resume and cancellation
//! (§10). A stage's input is always a predicate over columns, never a list
//! carried in memory.
//!
//! Slice 2 of the build (§14) is the walker, the parser pool and the dry run:
//! [`dry_run`] walks a root, reads every candidate file through
//! `nils_dicom::extract` and returns the [`Report`]; the writer lands with
//! slice 3.

pub mod dryrun;
pub mod knobs;
pub mod report;
mod rss;
pub mod walk;

pub use dryrun::{DigestError, dry_run};
pub use knobs::{KNOBS, Knob, Settings};
pub use report::Report;
pub use walk::{Filter, SkipReason, WalkEvent, walk};
