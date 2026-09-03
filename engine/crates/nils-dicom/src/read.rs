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
    /// Elements whose declared length their VR could not hold, repaired in
    /// memory so that the header could be read (§6.1).
    pub repaired: usize,
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
    let opened = match OpenFileOptions::new()
        .read_preamble(ReadPreamble::Auto)
        .read_until(tags::PIXEL_DATA)
        .open_file(path)
    {
        Ok(opened) => opened,
        Err(e) => {
            let failure = classify(&e);
            // A length a fixed-size VR cannot hold leaves the reader two
            // bytes behind and the rest of the header unreadable, so the
            // file looks truncated when it is not: repair it and read again.
            if matches!(
                failure,
                ReadFailure::Parse {
                    kind: ParseKind::Truncated,
                    ..
                }
            ) && let Some(header) = repaired_part10(path)
            {
                return Ok(header);
            }
            return Err(failure);
        }
    };
    let meta = opened.meta().clone();
    Ok(Header {
        form: Form::Part10,
        meta: Some(meta),
        dataset: opened.into_inner(),
        repaired: 0,
    })
}

/// The bytes one value of a fixed-size VR takes; none for the VRs whose
/// length is a byte count.
fn item_size(vr: [u8; 2]) -> Option<usize> {
    Some(match &vr {
        b"US" | b"SS" | b"OW" => 2,
        b"UL" | b"SL" | b"FL" | b"OF" | b"OL" | b"AT" => 4,
        b"FD" | b"OD" | b"SV" | b"UV" | b"OV" => 8,
        _ => return None,
    })
}

/// Whether an explicit VR carries a four-byte length after two reserved bytes.
fn long_header(vr: [u8; 2]) -> bool {
    matches!(
        &vr,
        b"OB"
            | b"OD"
            | b"OF"
            | b"OL"
            | b"OV"
            | b"OW"
            | b"SQ"
            | b"SV"
            | b"UC"
            | b"UN"
            | b"UR"
            | b"UT"
            | b"UV"
    )
}

/// Where the data set begins in a Part 10 file: past the preamble, the magic
/// and the file meta group, whose length the first element of the group gives.
fn dataset_start(raw: &[u8]) -> Option<usize> {
    let magic = if raw.len() >= 132 && &raw[128..132] == b"DICM" {
        132
    } else if raw.starts_with(b"DICM") {
        4
    } else {
        return None;
    };
    // (0002,0000) UL 4, the group's length in bytes after this element
    if raw.len() < magic + 12 || raw[magic..magic + 4] != [0x02, 0x00, 0x00, 0x00] {
        return None;
    }
    let group_length = u32::from_le_bytes(raw[magic + 8..magic + 12].try_into().ok()?) as usize;
    Some(magic + 12 + group_length)
}

/// One element whose declared length its VR cannot hold: where the length is
/// written, the value's bounds, and the length that fits.
struct Ragged {
    length_at: usize,
    long: bool,
    value_at: usize,
    declared: usize,
    fits: usize,
}

/// Walk the data set of `raw` from `start`, explicit VR little endian, and
/// list the elements whose length is not a whole number of values of their VR
/// (a `UL` of six bytes). Stops at Pixel Data, which is where the reader stops
/// too, and gives up (with what it has) on anything it cannot follow.
fn ragged_elements(raw: &[u8], start: usize) -> (Vec<Ragged>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i + 8 <= raw.len() {
        let group = u16::from_le_bytes([raw[i], raw[i + 1]]);
        let element = u16::from_le_bytes([raw[i + 2], raw[i + 3]]);
        if (group, element) == (0x7FE0, 0x0010) {
            return (out, i);
        }
        // an item or a delimiter: a tag and a four-byte length, no VR
        if group == 0xFFFE {
            let length = u32::from_le_bytes(raw[i + 4..i + 8].try_into().unwrap());
            i += 8;
            if length != u32::MAX && element == 0xE000 {
                // an item of a defined length: walk into it
                continue;
            }
            continue;
        }
        let vr: [u8; 2] = [raw[i + 4], raw[i + 5]];
        if !(vr[0].is_ascii_uppercase() && vr[1].is_ascii_uppercase()) {
            // implicit VR, or bytes we cannot follow: stop here
            return (out, raw.len());
        }
        let (length_at, header, declared) = if long_header(vr) {
            if i + 12 > raw.len() {
                return (out, raw.len());
            }
            (
                i + 8,
                12,
                u32::from_le_bytes(raw[i + 8..i + 12].try_into().unwrap()),
            )
        } else {
            (i + 6, 8, u32::from_le_bytes([raw[i + 6], raw[i + 7], 0, 0]))
        };
        let value_at = i + header;
        if declared == u32::MAX || &vr == b"SQ" {
            // a sequence: walk into its items, where ragged elements sit too
            i = value_at;
            continue;
        }
        if let Some(size) = item_size(vr) {
            let declared = declared as usize;
            if !declared.is_multiple_of(size) {
                out.push(Ragged {
                    length_at,
                    long: header == 12,
                    value_at,
                    declared,
                    fits: declared - declared % size,
                });
            }
        }
        i = value_at + declared as usize;
    }
    (out, raw.len())
}

/// The bytes of the header the repair reads at most: a file whose Pixel Data
/// is further in than this is left to the first reader's verdict.
const REPAIR_CAP: usize = 8 << 20;

/// Read a Part 10 file whose header the first reader could not follow,
/// repairing in memory the elements whose declared length their VR cannot
/// hold. The file on disk is untouched; the surplus bytes of such a value
/// are dropped, which is what any reader does with them anyway, and every
/// element after it is then where the reader expects it.
fn repaired_part10(path: &Path) -> Option<Header> {
    use std::io::Read;

    let mut raw = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(REPAIR_CAP as u64)
        .read_to_end(&mut raw)
        .ok()?;
    let start = dataset_start(&raw)?;
    let (ragged, _end) = ragged_elements(&raw, start);
    if ragged.is_empty() {
        return None;
    }
    let mut fixed = Vec::with_capacity(raw.len());
    let mut copied = 0;
    for r in &ragged {
        fixed.extend_from_slice(&raw[copied..r.length_at]);
        if r.long {
            fixed.extend_from_slice(&(r.fits as u32).to_le_bytes());
            copied = r.length_at + 4;
        } else {
            fixed.extend_from_slice(&(r.fits as u16).to_le_bytes());
            copied = r.length_at + 2;
        }
        fixed.extend_from_slice(&raw[copied..r.value_at + r.fits]);
        copied = r.value_at + r.declared;
    }
    fixed.extend_from_slice(&raw[copied..]);
    let opened = OpenFileOptions::new()
        .read_preamble(ReadPreamble::Auto)
        .read_until(tags::PIXEL_DATA)
        .from_reader(io::Cursor::new(fixed))
        .ok()?;
    let meta = opened.meta().clone();
    Some(Header {
        form: Form::Part10,
        meta: Some(meta),
        dataset: opened.into_inner(),
        repaired: ragged.len(),
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
        repaired: 0,
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
    fn a_sequence_of_undefined_length_is_read_to_the_end() {
        // PS3.5 §7.5: a sequence, and every item in it, may be written with
        // an undefined length and closed by a delimiter. Scanners do write
        // ProcedureCodeSequence that way, and the reader must not lose the
        // data set behind it.
        use crate::synth::{self, MetaFields, TempDir};
        use dicom_core::VR;
        use dicom_dictionary_std::tags;

        let dir = TempDir::new("undefined-length");
        for (i, ts) in [
            "1.2.840.10008.1.2.1",
            "1.2.840.10008.1.2.4.90",
            "1.2.840.10008.1.2.4.70",
        ]
        .into_iter()
        .enumerate()
        {
            let mut elems = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
            elems.push(synth::seq_undefined(
                tags::PROCEDURE_CODE_SEQUENCE,
                vec![vec![
                    synth::text(tags::CODE_VALUE, VR::SH, "AB1234"),
                    synth::text(tags::CODING_SCHEME_DESIGNATOR, VR::SH, "SECTRA"),
                    synth::text(tags::CODE_MEANING, VR::LO, "MR of the head"),
                ]],
            ));
            // an element after the sequence: the reader loses it when it
            // loses the alignment, which is how the archive's files were
            // quarantined
            elems.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1_mprage"));
            let meta = MetaFields::with(ts, "1.2.840.10008.5.1.4.1.1.4", "1.2.3.3");
            let path = dir.file(&format!("{i}.dcm"), &synth::part10(&meta, &elems, true));
            let header = read(&path).unwrap_or_else(|e| panic!("{ts}: {e}"));
            assert_eq!(
                header
                    .dataset
                    .get(tags::SERIES_DESCRIPTION)
                    .and_then(|e| e.string().ok().map(str::trim)),
                Some("t1_mprage"),
                "{ts}"
            );
            assert!(
                header.dataset.get(tags::PROCEDURE_CODE_SEQUENCE).is_some(),
                "{ts}"
            );
        }
    }

    #[test]
    fn a_binary_element_of_a_ragged_length_does_not_lose_the_alignment() {
        // A private `UL` whose length is 6, not a multiple of the four bytes
        // a UL takes. The archive is full of them (one vendor's 0009 block),
        // and a reader that consumes only the whole values it can make out of
        // the length is two bytes short from there on: the next tag it reads
        // is the middle of a text value, and the file looks truncated.
        use crate::synth::{self, MetaFields, TempDir};
        use dicom_core::{Tag, VR};
        use dicom_dictionary_std::tags;

        let dir = TempDir::new("ragged-length");
        let mut elems = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        elems.push(synth::text(Tag(0x0009, 0x0010), VR::LO, "A VENDOR"));
        elems.push(synth::bytes(
            Tag(0x0009, 0x1213),
            VR::UL,
            vec![1, 0, 0, 0, 2, 0],
        ));
        elems.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1_mprage"));
        let path = dir.file(
            "a.dcm",
            &synth::part10(&MetaFields::mr("1.2.3.3"), &elems, true),
        );
        let header = read(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            header
                .dataset
                .get(tags::SERIES_DESCRIPTION)
                .and_then(|e| e.string().ok().map(str::trim)),
            Some("t1_mprage")
        );
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
