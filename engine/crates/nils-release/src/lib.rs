// SPDX-License-Identifier: AGPL-3.0-only

//! The release: selecting a dataset, de-identifying it and writing it out
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8).
//!
//! One verb over one selection. v0 has two exports and its own runner says the
//! two callers "only differ in scope, output root, and pipeline coupling"; all
//! three differences are gone in v1, so there is one.

pub mod bids;
pub mod blocks;
pub mod burned;
pub mod dates;
pub mod handover;
pub mod name;
pub mod policy;
pub mod run;
pub mod scrub;
pub mod tags;
pub mod uid;
pub mod version;
