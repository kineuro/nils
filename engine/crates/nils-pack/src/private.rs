// SPDX-License-Identifier: AGPL-3.0-only

//! Which private elements a release keeps
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.4).
//!
//! Private elements are **dropped by default**, because a private element is
//! by definition one whose meaning the standard does not fix: some carry a
//! diffusion direction and some carry the operator's name, and nothing in the
//! file says which. The allowlist is the exception, and it is pack-shaped data
//! rather than a table in the engine, because which vendor element carries a
//! gradient is knowledge about scanners that changes without the engine
//! changing.
//!
//! v0 removes 119 named standard tags and **touches no private element at
//! all**, so every vendor block leaves the building. Siemens CSA headers alone
//! have carried the patient name, the operator and the institution in
//! shipping firmware.

/// One private element a release keeps, by the block its creator reserves.
///
/// Addressed by creator rather than by position, because the block a creator
/// reserves moves from file to file: `(0019,0010)` in one and `(0019,0011)` in
/// the next, with the elements at `10xx` and `11xx`. v0's reader takes the
/// fixed slot and reads whatever is there, which is how a value can be read
/// from the wrong vendor's block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowed {
    pub creator: String,
    pub group: u16,
    /// The offset within the block: the low byte of the element.
    pub element: u8,
    /// What it carries, so a reader of the pack can judge the exception.
    pub why: String,
}

impl Allowed {
    /// How the row and the report name it.
    pub fn text(&self) -> String {
        format!(
            "({:04X},xx{:02X}) {}",
            self.group, self.element, self.creator
        )
    }
}
