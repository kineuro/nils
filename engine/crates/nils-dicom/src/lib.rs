// SPDX-License-Identifier: AGPL-3.0-only

//! The DICOM reader of NILS.
//!
//! What lives here, from the Wave 1 specification
//! (`docs/specs/wave1-parse-and-digest.md`, §5 and §6): the reader over `dicom-rs`
//! that opens a file and stops before Pixel Data, the field catalogue, value
//! normalization, the Enhanced MR and private-tag fallbacks, and the refusal
//! classes. Nothing in this crate assumes MRI: MR, CT and PT are read alike.
//!
//! The path of one file is [`extract::extract`]: [`sniff`] looks at the first
//! bytes, [`read`] parses the header with `dicom-rs`, [`charset`] repairs a
//! misspelled character set, [`catalogue`] says which elements become which
//! columns and [`value`] converts them. Whatever is refused carries a
//! [`refusal::QuarantineClass`]; whatever is merely odd carries a
//! [`diagnostic::Diagnostic`].

pub mod catalogue;
pub mod charset;
pub mod csa;
pub mod diagnostic;
pub mod extract;
pub mod private;
pub mod read;
pub mod refusal;
pub mod sniff;
pub mod synth;
pub mod value;

pub use catalogue::{CATALOGUE, Field, Level, Sensitivity, Source};
pub use diagnostic::{Diagnostic, DiagnosticKind};
pub use extract::{Extracted, Identity, extract, extract_header};
pub use read::{Form, Header, ParseKind, ReadFailure, read};
pub use refusal::{QuarantineClass, Refusal};
pub use value::{Converter, Value};
