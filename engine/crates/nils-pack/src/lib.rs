// SPDX-License-Identifier: AGPL-3.0-only

//! The pack format (`docs/specs/wave2-fingerprint-and-classify.md`, §5, §6).
//!
//! Classification knowledge as a versioned, diffable, third-party-shippable
//! bundle: vocabulary, grammar, editable buckets, overlays and the pack's own
//! corpus. The C11 prototype settled that this is expressible as data with no
//! code escape hatch, on the whole live corpus and with no disagreement
//! (`spikes/pack/README.md`); this crate is that written for keeps.
//!
//! It knows nothing about a registry. A pack plus a [`stack::Stack`] yields
//! facts, which is what makes a pack testable from a fixture and shippable by
//! someone who has never seen our schema.

pub mod corpus;
pub mod error;
pub mod eval;
pub mod expr;
pub mod normalize;
pub mod overlay;
pub mod pack;
pub mod pass;
pub mod rules;
pub mod stack;
pub mod verdict;
pub mod version;
mod yaml;

pub use error::Error;
pub use eval::Evaluated;
pub use overlay::Overlay;
pub use pack::{CONTRACT, Pack, load};
pub use stack::Stack;
pub use verdict::{AxisVerdict, Evidence, Verdict};
pub use version::Version;
