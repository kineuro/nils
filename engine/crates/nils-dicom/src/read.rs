// SPDX-License-Identifier: AGPL-3.0-only

//! Opening a file with `dicom-rs` and stopping before Pixel Data
//! (`docs/specs/wave1-parse-and-digest.md`, §6.1).
//!
//! A Part 10 file is read through [`OpenFileOptions`], which detects the
//! preamble and reads the file meta group. A bare data set has no meta group to
//! say its transfer syntax, so it is read with the collector under implicit VR
//! little endian and, when that fails, explicit VR little endian; the two are
//! told apart by looking at the bytes where the first element's VR would be. In
//! both cases reading stops at Pixel Data, so a file costs its header, not its
//! image.
//!
//! What fails is classified by walking the reader's error chain: an unexpected
//! end of file anywhere in the chain is a truncated file, another I/O error is
//! an unreadable one, and a transfer syntax the registry does not know is its
//! own kind. Everything else is a malformed header, with the chain as detail.

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::path::Path;

use dicom_dictionary_std::tags;
use dicom_object::collector::DicomCollectorOptions;
use dicom_object::file::ReadPreamble;
use dicom_object::meta::FileMetaTable;
use dicom_object::{InMemDicomObject, OpenFileOptions};

use crate::sniff::{Sniff, sniff};

/// Implicit VR little endian, the transfer syntax a bare data set is tried with
/// first.
pub const IMPLICIT_VR_LE: &str = "1.2.840.10008.1.2";
/// Explicit VR little endian, the second try.
pub const EXPLICIT_VR_LE: &str = "1.2.840.10008.1.2.1";

/// How the file was laid out on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// A meta group behind a `DICM` marker, with or without the preamble.
    Part10,
    /// A bare data set read as implicit VR little endian.
    BareImplicit,
    /// A bare data set read as explicit VR little endian.
    BareExplicit,
}

impl Form {
    pub fn name(self) -> &'static str {
        match self {
            Form::Part10 => "part10",
            Form::BareImplicit => "bare-implicit",
            Form::BareExplicit => "bare-explicit",
        }
    }
}

/// The header of one file: the data set up to Pixel Data and, for a Part 10
/// file, its meta group.
#[derive(Debug)]
pub struct Header {
    pub form: Form,
    pub meta: Option<FileMetaTable>,
    pub dataset: InMemDicomObject,
}

impl Header {
    /// The transfer syntax: the meta group's, or the one the bare data set was
    /// read with.
    pub fn transfer_syntax(&self) -> &str {
        match (&self.meta, self.form) {
            (Some(meta), _) => meta.transfer_syntax(),
            (None, Form::BareExplicit) => EXPLICIT_VR_LE,
            (None, _) => IMPLICIT_VR_LE,
        }
    }
}

/// What kind of parse failure it was; `detail` starts with the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ParseKind {
    /// The file ends inside the header.
    Truncated,
    /// A transfer syntax the reader does not know or cannot read.
    UnsupportedTransferSyntax,
    /// Anything else the reader refused.
    Malformed,
}

impl ParseKind {
    pub fn name(self) -> &'static str {
        match self {
            ParseKind::Truncated => "truncated",
            ParseKind::UnsupportedTransferSyntax => "unsupported_transfer_syntax",
            ParseKind::Malformed => "malformed",
        }
    }
}

impl fmt::Display for ParseKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a file could not be read into a [`Header`].
#[derive(Debug)]
pub enum ReadFailure {
    /// An I/O error opening or reading; the text is the error's.
    Unreadable(String),
    /// Neither a Part 10 file nor a readable bare data set.
    NotDicom,
    /// The reader failed inside the header.
    Parse { kind: ParseKind, chain: String },
}

impl ReadFailure {
    /// The text that goes into `source_file.detail`.
    pub fn detail(&self) -> Option<String> {
        match self {
            ReadFailure::Unreadable(text) => Some(text.clone()),
            ReadFailure::NotDicom => None,
            ReadFailure::Parse { kind, chain } => Some(format!("{kind}: {chain}")),
        }
    }
}

impl fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadFailure::Unreadable(text) => write!(f, "unreadable: {text}"),
            ReadFailure::NotDicom => f.write_str("not_dicom"),
            ReadFailure::Parse { kind, chain } => write!(f, "parse_error: {kind}: {chain}"),
        }
    }
}

/// Read the header of the file at `path`.
pub fn read(path: &Path) -> Result<Header, ReadFailure> {
    match sniff(path) {
        Sniff::Unreadable(e) => Err(ReadFailure::Unreadable(io_text(&e))),
        Sniff::Other => Err(ReadFailure::NotDicom),
        Sniff::Part10 => read_part10(path),
        Sniff::BareDataset => read_bare(path),
    }
}

fn read_part10(path: &Path) -> Result<Header, ReadFailure> {
    let opened = OpenFileOptions::new()
        .read_preamble(ReadPreamble::Auto)
        .read_until(tags::PIXEL_DATA)
        .open_file(path)
        .map_err(|e| classify(&e))?;
    let meta = opened.meta().clone();
    Ok(Header {
        form: Form::Part10,
        meta: Some(meta),
        dataset: opened.into_inner(),
    })
}

fn read_bare(path: &Path) -> Result<Header, ReadFailure> {
    // In explicit VR the bytes 4 and 5 of the first element are its VR, two
    // upper-case letters; in implicit VR they are the low half of a length.
    let explicit_first = looks_explicit(path);
    let order = if explicit_first {
        [Form::BareExplicit, Form::BareImplicit]
    } else {
        [Form::BareImplicit, Form::BareExplicit]
    };
    let mut first_failure = None;
    for form in order {
        match read_bare_as(path, form) {
            Ok(header) => return Ok(header),
            Err(failure) => {
                if first_failure.is_none() {
                    first_failure = Some(failure);
                }
            }
        }
    }
    Err(first_failure.unwrap_or(ReadFailure::NotDicom))
}

fn looks_explicit(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 6];
    match std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)) {
        Ok(()) => head[4].is_ascii_uppercase() && head[5].is_ascii_uppercase(),
        Err(_) => false,
    }
}

fn read_bare_as(path: &Path, form: Form) -> Result<Header, ReadFailure> {
    let ts = match form {
        Form::BareExplicit => EXPLICIT_VR_LE,
        _ => IMPLICIT_VR_LE,
    };
    let mut collector = DicomCollectorOptions::new()
        .read_preamble(ReadPreamble::Never)
        .expected_ts(ts)
        .open_file(path)
        .map_err(|e| classify(&e))?;
    let mut dataset = InMemDicomObject::new_empty();
    collector
        .read_dataset_up_to_pixeldata(&mut dataset)
        .map_err(|e| classify(&e))?;
    // A bare data set that parses but names no SOP instance is not DICOM in the
    // sense of §5.3: bytes that happened to decode as elements.
    if dataset.get(tags::SOP_INSTANCE_UID).is_none() {
        return Err(ReadFailure::NotDicom);
    }
    Ok(Header {
        form,
        meta: None,
        dataset,
    })
}

fn io_text(e: &io::Error) -> String {
    format!("{:?}: {e}", e.kind())
}

/// Classify a reader error by its chain.
fn classify(error: &(dyn StdError + 'static)) -> ReadFailure {
    let mut kind = ParseKind::Malformed;
    let mut chain = Vec::new();
    let mut cur: Option<&(dyn StdError + 'static)> = Some(error);
    while let Some(e) = cur {
        if let Some(io) = e.downcast_ref::<io::Error>() {
            match io.kind() {
                io::ErrorKind::UnexpectedEof => kind = ParseKind::Truncated,
                _ => return ReadFailure::Unreadable(io_text(io)),
            }
        }
        let text = e.to_string();
        if text.contains("transfer syntax") {
            kind = ParseKind::UnsupportedTransferSyntax;
        } else if text.contains("Premature data set end") && kind == ParseKind::Malformed {
            kind = ParseKind::Truncated;
        }
        chain.push(text);
        cur = e.source();
    }
    ReadFailure::Parse {
        kind,
        chain: chain.join(" <- "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Wrapped(Box<dyn StdError + Send + Sync>);

    impl fmt::Display for Wrapped {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Could not read data set token")
        }
    }

    impl StdError for Wrapped {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(self.0.as_ref())
        }
    }

    #[test]
    fn eof_in_the_chain_is_truncated() {
        let e = Wrapped(Box::new(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "eof",
        )));
        match classify(&e) {
            ReadFailure::Parse { kind, chain } => {
                assert_eq!(kind, ParseKind::Truncated);
                assert_eq!(chain, "Could not read data set token <- eof");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_io_errors_are_unreadable() {
        let e = Wrapped(Box::new(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "nope",
        )));
        assert!(matches!(classify(&e), ReadFailure::Unreadable(t) if t.contains("nope")));
    }

    #[test]
    fn transfer_syntax_text_is_its_own_kind() {
        let plain: Box<dyn StdError> = Box::from("Unsupported reading for transfer syntax `1.2.3`");
        assert!(matches!(
            classify(plain.as_ref()),
            ReadFailure::Parse {
                kind: ParseKind::UnsupportedTransferSyntax,
                ..
            }
        ));
    }

    #[test]
    fn premature_end_text_is_truncated() {
        let plain: Box<dyn StdError> = Box::from("Premature data set end");
        match classify(plain.as_ref()) {
            ReadFailure::Parse { kind, chain } => {
                assert_eq!(kind, ParseKind::Truncated);
                assert_eq!(chain, "Premature data set end");
            }
            other => panic!("{other:?}"),
        }
    }
}
