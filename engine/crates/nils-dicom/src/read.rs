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

use crate::sniff::{Sniff, sniff, sniff_bytes};

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

/// A whole file, pixel data included, as a viewer reads it, and its transfer
/// syntax: a Part 10 file through its meta group, with or without the
/// preamble, and a bare data set as [`read`] reads its header, implicit or
/// explicit VR little endian as its first bytes say, the other when that
/// fails.
pub fn read_whole(path: &Path) -> Result<(InMemDicomObject, String), ReadFailure> {
    match sniff(path) {
        Sniff::Unreadable(e) => Err(ReadFailure::Unreadable(io_text(&e))),
        Sniff::Other => Err(ReadFailure::NotDicom),
        Sniff::Part10 => {
            let file = match OpenFileOptions::new()
                .read_preamble(ReadPreamble::Auto)
                .open_file(path)
            {
                Ok(file) => file,
                Err(e) => {
                    let failure = classify(&e);
                    // As the header's reader does: a length a fixed-size VR
                    // cannot hold (a private UL of six bytes, as one
                    // archive's writer left them) makes the file look
                    // truncated when it is not. Repaired in memory, the
                    // whole file this time, since its pixels are wanted.
                    if matches!(
                        failure,
                        ReadFailure::Parse {
                            kind: ParseKind::Truncated,
                            ..
                        }
                    ) && let Some(file) = repaired_whole(path)
                    {
                        file
                    } else {
                        return Err(failure);
                    }
                }
            };
            let ts = file
                .meta()
                .transfer_syntax()
                .trim_end_matches('\0')
                .to_string();
            Ok((file.into_inner(), ts))
        }
        Sniff::BareDataset => {
            let order = if looks_explicit(path) {
                [Form::BareExplicit, Form::BareImplicit]
            } else {
                [Form::BareImplicit, Form::BareExplicit]
            };
            let mut first_failure = None;
            for form in order {
                let ts = match form {
                    Form::BareExplicit => EXPLICIT_VR_LE,
                    _ => IMPLICIT_VR_LE,
                };
                let read = DicomCollectorOptions::new()
                    .read_preamble(ReadPreamble::Never)
                    .expected_ts(ts)
                    .open_file(path)
                    .and_then(|mut collector| {
                        let mut dataset = InMemDicomObject::new_empty();
                        collector.read_dataset_to_end(&mut dataset)?;
                        Ok(dataset)
                    })
                    .map_err(|e| classify(&e))
                    .and_then(|dataset| bare_header(form, dataset));
                match read {
                    Ok(header) => return Ok((header.dataset, ts.to_string())),
                    Err(failure) => {
                        first_failure.get_or_insert(failure);
                    }
                }
            }
            Err(first_failure.unwrap_or(ReadFailure::NotDicom))
        }
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
    repaired_bytes(&raw)
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
    bare_header(form, dataset)
}

/// A bare data set that parses but names no SOP instance is not DICOM in the
/// sense of §5.3: bytes that happened to decode as elements.
fn bare_header(form: Form, dataset: InMemDicomObject) -> Result<Header, ReadFailure> {
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

/// A header read for rewriting (record 26 §3): the header as [`read`] reads
/// it, and where in the file the pixel data element begins, so that a writer
/// can re-encode the header and copy everything from there verbatim. The
/// pixel data is never read.
///
/// The offset is found by walking the bytes of the data set, not by counting
/// what the parser consumed: the parser reads through a buffer of its own,
/// so a counter around its source sees the read-ahead and not the element
/// it stopped at.
#[derive(Debug)]
pub struct Framed {
    pub header: Header,
    /// The size of the file.
    pub size: u64,
    /// Where the pixel data element begins, its own header included; the
    /// size of the file when it has none.
    pub pixel_at: u64,
    /// Where the pixel data element ends: the file's end for one of
    /// undefined length, and for one whose length is written, its end, so
    /// that whatever a writer appended after the pixels is not copied.
    pub pixel_end: u64,
    /// The data set is explicit VR little endian (the meta group's syntax,
    /// or the bare data set's as sniffed); implicit VR little endian
    /// otherwise. Big endian is refused.
    pub explicit: bool,
}

/// The bytes read first, which cover nearly every header.
const FRAME_CHUNK: usize = 256 << 10;

/// The most a header may take: an enhanced file's per-frame groups run to
/// megabytes; beyond this the file is left to the whole-file readers.
const FRAME_CAP: usize = 64 << 20;

/// Explicit VR big endian, retired, which a rewrite does not produce.
const EXPLICIT_VR_BE: &str = "1.2.840.10008.1.2.2";

fn is_truncated(failure: &ReadFailure) -> bool {
    matches!(
        failure,
        ReadFailure::Parse {
            kind: ParseKind::Truncated,
            ..
        }
    )
}

/// Read the header of the file at `path` and frame its pixel data.
pub fn read_framed(path: &Path) -> Result<Framed, ReadFailure> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| ReadFailure::Unreadable(io_text(&e)))?;
    let size = file
        .metadata()
        .map_err(|e| ReadFailure::Unreadable(io_text(&e)))?
        .len();
    let mut raw: Vec<u8> = Vec::new();
    let mut want = FRAME_CHUNK;
    loop {
        let more = want.saturating_sub(raw.len()) as u64;
        file.by_ref()
            .take(more)
            .read_to_end(&mut raw)
            .map_err(|e| ReadFailure::Unreadable(io_text(&e)))?;
        let at_end = raw.len() < want || raw.len() as u64 >= size;
        match frame_bytes(&raw, size) {
            Ok(framed) => return Ok(framed),
            Err(failure) if is_truncated(&failure) && !at_end && raw.len() < FRAME_CAP => {
                want *= 2;
            }
            Err(failure) => return Err(failure),
        }
    }
}

/// Frame a file from its first bytes: parse the header, then find the pixel
/// data element in the same bytes.
fn frame_bytes(raw: &[u8], size: u64) -> Result<Framed, ReadFailure> {
    let (header, start) = match sniff_bytes(raw) {
        Sniff::Part10 => {
            let magic = if raw.len() >= 132 && &raw[128..132] == b"DICM" {
                128
            } else {
                0
            };
            let header = match OpenFileOptions::new()
                .read_preamble(ReadPreamble::Never)
                .read_until(tags::PIXEL_DATA)
                .from_reader(io::Cursor::new(&raw[magic..]))
            {
                Ok(opened) => {
                    let meta = opened.meta().clone();
                    Header {
                        form: Form::Part10,
                        meta: Some(meta),
                        dataset: opened.into_inner(),
                        repaired: 0,
                    }
                }
                Err(e) => {
                    let failure = classify(&e);
                    // the repair of §6.1, on the bytes in hand, once the
                    // whole file is there to be repaired
                    let whole = raw.len() as u64 >= size;
                    match (is_truncated(&failure) && whole).then(|| repaired_bytes(raw)) {
                        Some(Some(header)) => header,
                        _ => return Err(failure),
                    }
                }
            };
            let start = dataset_start(raw).ok_or_else(|| ReadFailure::Parse {
                kind: ParseKind::Malformed,
                chain: "the file meta group does not say its length".into(),
            })?;
            (header, start)
        }
        Sniff::BareDataset => {
            let explicit =
                raw.len() >= 6 && raw[4].is_ascii_uppercase() && raw[5].is_ascii_uppercase();
            let order = if explicit {
                [Form::BareExplicit, Form::BareImplicit]
            } else {
                [Form::BareImplicit, Form::BareExplicit]
            };
            let mut first_failure = None;
            let mut found = None;
            for form in order {
                match bare_from_bytes(raw, form) {
                    Ok(header) => {
                        found = Some(header);
                        break;
                    }
                    Err(failure) => {
                        if first_failure.is_none() {
                            first_failure = Some(failure);
                        }
                    }
                }
            }
            match found {
                Some(header) => (header, 0),
                None => return Err(first_failure.unwrap_or(ReadFailure::NotDicom)),
            }
        }
        Sniff::Other => return Err(ReadFailure::NotDicom),
        Sniff::Unreadable(e) => return Err(ReadFailure::Unreadable(io_text(&e))),
    };
    let ts = header.transfer_syntax();
    if ts == EXPLICIT_VR_BE {
        return Err(ReadFailure::Parse {
            kind: ParseKind::UnsupportedTransferSyntax,
            chain: "explicit VR big endian is not rewritten".into(),
        });
    }
    let explicit = ts != IMPLICIT_VR_LE;
    let found = if explicit {
        pixel_offset_explicit(raw, start)
    } else {
        pixel_offset_implicit(raw, start)
    };
    let (pixel_at, pixel_end) = match found {
        Ok(Some(at)) => {
            let end = pixel_end(raw, at, explicit).min(size);
            (at as u64, end)
        }
        // no pixel data before the bytes ran out: the whole file was read
        // and holds none, or the bytes were not enough to say
        Ok(None) if raw.len() as u64 >= size => (size, size),
        Ok(None) => {
            return Err(ReadFailure::Parse {
                kind: ParseKind::Truncated,
                chain: "the pixel data was not reached".into(),
            });
        }
        Err(why) => {
            return Err(ReadFailure::Parse {
                kind: ParseKind::Malformed,
                chain: format!("the pixel data could not be framed: {why}"),
            });
        }
    };
    Ok(Framed {
        header,
        size,
        pixel_at,
        pixel_end,
        explicit,
    })
}

fn bare_from_bytes(raw: &[u8], form: Form) -> Result<Header, ReadFailure> {
    let ts = match form {
        Form::BareExplicit => EXPLICIT_VR_LE,
        _ => IMPLICIT_VR_LE,
    };
    let mut collector = DicomCollectorOptions::new()
        .read_preamble(ReadPreamble::Never)
        .expected_ts(ts)
        .from_reader(io::BufReader::new(io::Cursor::new(raw)));
    let mut dataset = InMemDicomObject::new_empty();
    collector
        .read_dataset_up_to_pixeldata(&mut dataset)
        .map_err(|e| classify(&e))?;
    bare_header(form, dataset)
}

/// A whole Part 10 file, pixel data included, read with the repair of
/// [`repaired_part10`]: the file on disk is untouched.
fn repaired_whole(path: &Path) -> Option<dicom_object::DefaultDicomObject> {
    let raw = std::fs::read(path).ok()?;
    let (fixed, _) = repair(&raw)?;
    OpenFileOptions::new()
        .read_preamble(ReadPreamble::Auto)
        .from_reader(io::Cursor::new(fixed))
        .ok()
}

/// The repair of [`repaired_part10`], on bytes already in hand.
fn repaired_bytes(raw: &[u8]) -> Option<Header> {
    let (fixed, repaired) = repair(raw)?;
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
        repaired,
    })
}

/// The bytes of a Part 10 file with each element whose declared length its
/// fixed-size VR cannot hold cut to the length it can, and how many were;
/// none where there is none.
fn repair(raw: &[u8]) -> Option<(Vec<u8>, usize)> {
    let start = dataset_start(raw)?;
    let (ragged, _end) = ragged_elements(raw, start);
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
    Some((fixed, ragged.len()))
}

/// Where the top-level pixel data element begins in an explicit VR little
/// endian data set starting at `start`: none when the bytes run out first,
/// an error when they cannot be followed. A sequence or item of undefined
/// length is walked into with its depth counted, so that a pixel data
/// element inside an item (an icon's) is not taken for the image's; one of
/// a written length is stepped over whole.
fn pixel_offset_explicit(raw: &[u8], start: usize) -> Result<Option<usize>, String> {
    let mut i = start;
    let mut depth = 0usize;
    while i + 8 <= raw.len() {
        let group = u16::from_le_bytes([raw[i], raw[i + 1]]);
        let element = u16::from_le_bytes([raw[i + 2], raw[i + 3]]);
        if group == 0xFFFE {
            let length = u32::from_le_bytes(raw[i + 4..i + 8].try_into().unwrap());
            i += 8;
            match element {
                0xE000 if length == u32::MAX => depth += 1,
                0xE000 => i += length as usize,
                0xE00D | 0xE0DD => depth = depth.saturating_sub(1),
                other => return Err(format!("item tag (FFFE,{other:04X})")),
            }
            continue;
        }
        if (group, element) == (0x7FE0, 0x0010) && depth == 0 {
            return Ok(Some(i));
        }
        let vr: [u8; 2] = [raw[i + 4], raw[i + 5]];
        if !(vr[0].is_ascii_uppercase() && vr[1].is_ascii_uppercase()) {
            return Err(format!("no VR at ({group:04X},{element:04X})"));
        }
        let (value_at, length) = if long_header(vr) {
            if i + 12 > raw.len() {
                return Ok(None);
            }
            (
                i + 12,
                u32::from_le_bytes(raw[i + 8..i + 12].try_into().unwrap()),
            )
        } else {
            (i + 8, u32::from_le_bytes([raw[i + 6], raw[i + 7], 0, 0]))
        };
        if length == u32::MAX {
            depth += 1;
            i = value_at;
        } else {
            i = value_at + length as usize;
        }
    }
    Ok(None)
}

/// [`pixel_offset_explicit`] for implicit VR little endian, where every
/// element header is a tag and a four-byte length.
fn pixel_offset_implicit(raw: &[u8], start: usize) -> Result<Option<usize>, String> {
    let mut i = start;
    let mut depth = 0usize;
    while i + 8 <= raw.len() {
        let group = u16::from_le_bytes([raw[i], raw[i + 1]]);
        let element = u16::from_le_bytes([raw[i + 2], raw[i + 3]]);
        let length = u32::from_le_bytes(raw[i + 4..i + 8].try_into().unwrap());
        i += 8;
        if group == 0xFFFE {
            match element {
                0xE000 if length == u32::MAX => depth += 1,
                0xE000 => i += length as usize,
                0xE00D | 0xE0DD => depth = depth.saturating_sub(1),
                other => return Err(format!("item tag (FFFE,{other:04X})")),
            }
            continue;
        }
        if (group, element) == (0x7FE0, 0x0010) && depth == 0 {
            return Ok(Some(i - 8));
        }
        if length == u32::MAX {
            depth += 1;
        } else {
            i += length as usize;
        }
    }
    Ok(None)
}

/// Where the pixel data element at `at` ends, as its header says: the end of
/// the file's bytes for an undefined length, or where a length that cannot
/// be read leaves it. The caller caps it at the file's size.
fn pixel_end(raw: &[u8], at: usize, explicit: bool) -> u64 {
    let (header, length) = if explicit {
        let vr: [u8; 2] = [
            raw.get(at + 4).copied().unwrap_or(0),
            raw.get(at + 5).copied().unwrap_or(0),
        ];
        if long_header(vr) {
            (
                12,
                raw.get(at + 8..at + 12)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .unwrap_or(u32::MAX),
            )
        } else {
            (
                8,
                raw.get(at + 6..at + 8)
                    .map(|b| u32::from(u16::from_le_bytes(b.try_into().unwrap())))
                    .unwrap_or(u32::MAX),
            )
        }
    } else {
        (
            8,
            raw.get(at + 4..at + 8)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .unwrap_or(u32::MAX),
        )
    };
    if length == u32::MAX {
        u64::MAX
    } else {
        at as u64 + header + u64::from(length)
    }
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
    fn a_whole_file_with_an_element_of_a_ragged_length_is_read_with_its_pixels() {
        // The viewer's read, as the pyramid makes it (Phase 0: eleven
        // stacks of one archive's old scanner, a private UL of six bytes in
        // every file, were counted unreadable though their headers were
        // read): the same repair, the whole file this time.
        use crate::synth::{self, MetaFields, TempDir};
        use dicom_core::{Tag, VR};
        use dicom_dictionary_std::tags;

        let dir = TempDir::new("ragged-whole");
        let pixels: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        let mut elems = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        elems.push(synth::text(Tag(0x0009, 0x0010), VR::LO, "A VENDOR"));
        elems.push(synth::bytes(
            Tag(0x0009, 0x1213),
            VR::UL,
            vec![1, 0, 0, 0, 2, 0],
        ));
        elems.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1_mprage"));
        elems.push(synth::bytes(tags::PIXEL_DATA, VR::OW, pixels.clone()));
        let path = dir.file(
            "a.dcm",
            &synth::part10(&MetaFields::mr("1.2.3.3"), &elems, true),
        );
        let (dataset, ts) = read_whole(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ts, "1.2.840.10008.1.2.1");
        assert_eq!(
            dataset
                .get(tags::SERIES_DESCRIPTION)
                .and_then(|e| e.string().ok().map(str::trim)),
            Some("t1_mprage")
        );
        let read = dataset
            .get(tags::PIXEL_DATA)
            .and_then(|e| e.to_bytes().ok())
            .map(|b| b.to_vec());
        assert_eq!(read.as_deref(), Some(pixels.as_slice()));
        // and a file that is truly cut short is still refused
        let bytes = std::fs::read(&path).unwrap();
        let cut = dir.file("cut.dcm", &bytes[..bytes.len() - 1500]);
        assert!(read_whole(&cut).is_err());
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

    /// The offset of the first `(7FE0,0010)` tag in the bytes, the way a
    /// test finds it without a parser.
    fn tag_at(bytes: &[u8], from: usize) -> Option<usize> {
        (from..bytes.len().saturating_sub(4)).find(|&i| bytes[i..i + 4] == [0xE0, 0x7F, 0x10, 0x00])
    }

    #[test]
    fn a_framed_file_names_where_its_pixel_data_begins_and_ends() {
        use crate::synth::{self, MetaFields, TempDir};
        use dicom_core::VR;
        use dicom_dictionary_std::tags;

        let dir = TempDir::new("framed");
        let pixels: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        // explicit VR LE, with an icon whose own pixel data sits inside an
        // item of undefined length before the image's
        let mut elems = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        elems.push(synth::text(tags::PATIENT_NAME, VR::PN, "Doe^Jane"));
        elems.push(synth::seq_undefined(
            tags::ICON_IMAGE_SEQUENCE,
            vec![vec![
                synth::us(tags::ROWS, 8),
                synth::bytes(tags::PIXEL_DATA, VR::OB, vec![1, 2, 3, 4]),
            ]],
        ));
        elems.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1_mprage"));
        elems.push(synth::bytes(tags::PIXEL_DATA, VR::OW, pixels.clone()));
        let bytes = synth::part10(&MetaFields::mr("1.2.3.3"), &elems, true);
        let path = dir.file("explicit.dcm", &bytes);
        let framed = read_framed(&path).unwrap_or_else(|e| panic!("{e}"));
        let icon = tag_at(&bytes, 0).unwrap();
        let real = tag_at(&bytes, icon + 4).unwrap();
        assert_eq!(
            framed.pixel_at as usize, real,
            "the icon's is not the image's"
        );
        assert_eq!(framed.pixel_end, bytes.len() as u64);
        assert_eq!(framed.size, bytes.len() as u64);
        assert!(framed.explicit);
        assert_eq!(framed.header.form, Form::Part10);
        assert!(framed.header.dataset.get(tags::PIXEL_DATA).is_none());
        assert_eq!(
            framed
                .header
                .dataset
                .get(tags::SERIES_DESCRIPTION)
                .and_then(|e| e.string().ok().map(str::trim)),
            Some("t1_mprage")
        );

        // bytes after a pixel data of a written length are not the pixels
        let mut trailing = bytes.clone();
        trailing.extend_from_slice(&[0xFC, 0xFF, 0xFC, 0xFF, b'O', b'B', 0, 0, 2, 0, 0, 0, 9, 9]);
        let path = dir.file("trailing.dcm", &trailing);
        let framed = read_framed(&path).unwrap();
        assert_eq!(framed.pixel_at as usize, real);
        assert_eq!(framed.pixel_end, bytes.len() as u64);
        assert_eq!(framed.size, trailing.len() as u64);

        // implicit VR LE
        let meta = MetaFields::with(IMPLICIT_VR_LE, "1.2.840.10008.5.1.4.1.1.4", "1.2.3.3");
        let mut elems = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        elems.push(synth::seq_undefined(
            tags::REFERENCED_IMAGE_SEQUENCE,
            vec![vec![synth::text(
                tags::REFERENCED_SOP_INSTANCE_UID,
                VR::UI,
                "1.2.9",
            )]],
        ));
        elems.push(synth::bytes(tags::PIXEL_DATA, VR::OW, pixels.clone()));
        let bytes = synth::part10(&meta, &elems, true);
        let path = dir.file("implicit.dcm", &bytes);
        let framed = read_framed(&path).unwrap_or_else(|e| panic!("{e}"));
        assert!(!framed.explicit);
        assert_eq!(framed.pixel_at as usize, tag_at(&bytes, 0).unwrap());
        assert_eq!(framed.pixel_end, bytes.len() as u64);

        // a bare data set, explicit, no preamble and no meta
        let mut elems = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        elems.push(synth::bytes(tags::PIXEL_DATA, VR::OW, pixels.clone()));
        let bytes = synth::bare(&elems, true);
        let path = dir.file("bare", &bytes);
        let framed = read_framed(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(framed.header.form, Form::BareExplicit);
        assert!(framed.header.meta.is_none());
        assert_eq!(framed.pixel_at as usize, tag_at(&bytes, 0).unwrap());

        // a file with no pixel data at all: the tail is empty
        let elems = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        let bytes = synth::part10(&MetaFields::mr("1.2.3.3"), &elems, true);
        let path = dir.file("headless.dcm", &bytes);
        let framed = read_framed(&path).unwrap();
        assert_eq!(framed.pixel_at, bytes.len() as u64);
        assert_eq!(framed.pixel_end, bytes.len() as u64);

        // a header longer than the first chunk is read in more chunks
        let mut elems = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        elems.push(synth::bytes(
            dicom_core::Tag(0x0009, 0x1010),
            VR::OB,
            vec![7u8; FRAME_CHUNK + 1000],
        ));
        elems.push(synth::bytes(tags::PIXEL_DATA, VR::OW, pixels));
        let bytes = synth::part10(&MetaFields::mr("1.2.3.3"), &elems, true);
        let path = dir.file("long.dcm", &bytes);
        let framed = read_framed(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            framed.pixel_at as usize,
            tag_at(&bytes, FRAME_CHUNK).unwrap()
        );

        // what the reader refuses, the framing refuses the same way
        let path = dir.file("text.txt", b"not a dicom file at all, not at all");
        assert!(matches!(read_framed(&path), Err(ReadFailure::NotDicom)));
        let path = dir.file("cut.dcm", &bytes[..bytes.len() / 3]);
        assert!(matches!(
            read_framed(&path),
            Err(ReadFailure::Parse {
                kind: ParseKind::Truncated,
                ..
            })
        ));
    }
}
