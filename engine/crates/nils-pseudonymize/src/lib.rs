// SPDX-License-Identifier: AGPL-3.0-only

//! The pseudonymiser of NILS (record 26 §3 and §4): `nils pseudonymize
//! @dataset` reads every file of a dataset's originals, resolves who it is
//! about through the linkage store exactly as the digest does, and writes a
//! copy into the dataset's pseudonymised tree with the code in `PatientID`,
//! the patient, provider, trial and institution groups removed, the age
//! written, the dates and UIDs kept, the private elements dropped except
//! what the pack names, the overlays and curves dropped, and the pixel
//! bytes copied untouched. Only that tree is ever digested.
//!
//! The shape is the digest's (`nils-digest`): a walker pool feeds a resume
//! stage, which feeds per-file workers over bounded channels; one thread
//! holds the registry and the linkage store, resolving identities for the
//! workers and recording a row per file. A file the linkage store does not
//! know is held with its shape and a keyed lookup when the dataset says
//! `hold`, or coded anyway under the key with its subject marked
//! provisional when it says `code`. A run resumes by size and modification
//! time, writes `.part` then renames, hashes while writing, syncs the tree
//! once at the end, and records the run as a batch of kind `pseudonymize`
//! that the digest after it shares a name with.

pub mod layout;
pub mod progress;
pub mod report;
pub mod resume;
pub mod rewrite;
pub mod run;
pub mod settings;

pub use nils_digest::cancel::{Cancel, Cancelled};
pub use report::Report;
pub use run::{PseudonymizeError, pseudonymize, pseudonymize_with};
pub use settings::{Settings, TagLists, Unmapped};
