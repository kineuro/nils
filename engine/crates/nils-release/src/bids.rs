// SPDX-License-Identifier: AGPL-3.0-only

//! The BIDS layout (`docs/specs/wave3-anonymize-and-bids.md`, §9.2 to §9.6).
//!
//! Less than half of what we hold has a BIDS name, and that is not a defect in
//! BIDS: a localizer, a reformat, a projection and a synthetic contrast are not
//! acquisitions and the standard has no word for them. So this is a second
//! layout beside the descriptive one of §9.1 and not a replacement for it, and
//! the parts of the archive it cannot name are routed rather than dropped.

pub mod convert;
pub mod dataset;
pub mod name;
pub mod place;
pub mod schema;
