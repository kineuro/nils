// SPDX-License-Identifier: AGPL-3.0-only

//! The pass over a registry (`docs/specs/wave2-fingerprint-and-classify.md`,
//! §3): the fingerprint builder today, the modality router, the batch
//! pipeline and the evidence and decision writers as the wave's slices land.
//!
//! The line this crate keeps: **the fingerprint holds what is true of the
//! file, a pack holds what is true of the knowledge** (§4.2). So there is no
//! MRI in here. Folding text is here because folding is a fact about text;
//! deciding that `ir` means inversion recovery is not, and it lives in a pack.

pub mod classify;
pub mod derived;
pub mod fingerprint;
pub mod fold;
pub mod job;
pub mod passes;
pub mod report;

pub use job::{Error, Settings, fingerprint as run};
pub use report::{Classified, Report};
