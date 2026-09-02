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
//! Slice 1 of the build is the skeleton; the walker lands with slice 2 and the
//! writer with slice 3 (§14).
