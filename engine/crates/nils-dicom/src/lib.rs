// SPDX-License-Identifier: AGPL-3.0-only

//! The DICOM reader of NILS.
//!
//! What lives here, from the Wave 1 specification
//! (`docs/specs/wave1-parse-and-digest.md`, §6): the reader over `dicom-rs` that
//! opens a file and stops before Pixel Data, the field catalogue, value
//! normalization, the Enhanced MR and private-tag fallbacks, and the refusal
//! classes. Nothing in this crate assumes MRI: MR, CT and PT are read alike.
//!
//! Slice 1 of the build is the skeleton; the reader lands with slice 2 (§14).
