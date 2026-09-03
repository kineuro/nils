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
//! Slices 2 to 5 of the build (§14) are here: [`dry_run`] walks a root and
//! reads every candidate file through `nils_dicom::extract`; [`digest`] does
//! the same and writes what it read into a registry, one transaction per
//! batch, with the resume check of §5.2 in front of the parsers, the identity
//! rule of §7 applied per batch and every instance filed under its stack (§8).
//! Slice 6 adds the rest of §10: the job record with its heartbeat and the
//! takeover of a dead one, the two-step cancellation of [`Cancel`], and the
//! resume of a killed run to the same rows as an uninterrupted one.

pub mod batch;
pub mod cancel;
pub mod digest;
pub mod knobs;
pub mod progress;
pub mod report;
pub mod resume;
pub mod rss;
pub mod rule;
pub mod stack;
pub mod walk;
pub mod writer;

pub use cancel::{Cancel, Cancelled, Scripted};
pub use digest::{DigestError, digest, digest_with, dry_run};
pub use knobs::{KNOBS, Knob, Settings};
pub use report::{Report, Written};
pub use rule::{Ident, Rule, RuleError};
pub use walk::{Filter, SkipReason, WalkEvent, walk};
