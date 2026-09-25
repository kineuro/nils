// SPDX-License-Identifier: AGPL-3.0-only
//! The viewing pyramid and the gated instance doors (Wave 5 §12.7), shaped
//! by the viewer study: a browser must never hold a whole scan, so a stack
//! is precomputed at digest as four in-plane levels of 256 by 256 HTJ2K
//! tiles, every slice at every level, written to a `working` place; the
//! doors answer one plane's tiles in one response, a slab of up to 32
//! planes, and a server-rendered plane for the first picture, thin clients
//! and the gated case. The codec is HTJ2K through a pure Rust port of
//! OpenJPH, reversible, in-process.
//!
//! Record 45 E2: the manifest says where the planes are in the patient, so
//! a viewer can draw MPR and label its sides: `orientation`, the six
//! direction cosines of a plane's rows and columns (Image Orientation
//! Patient), `origin`, the position of the first plane's first pixel (Image
//! Position Patient), and `frame`, whether the planes are parallel and
//! evenly spaced, which is what a volume needs. The planes are ordered along
//! the normal the orientation gives. A manifest written before is read as
//! axial with `orientation_known` false. E1: `nils pyramid build --select`
//! builds a selection's pyramids as one job, skipping what is built.
//!
//! The planes are read from native pixel data (little endian, deflated or
//! big endian) or decoded from JPEG, JPEG-LS, JPEG 2000 and RLE, and the
//! manifest says when a plane came from a lossy source.
//!
//! Every frame of a multi-frame file is a plane. An enhanced object says
//! each frame's position, orientation, pixel spacing and rescale in its
//! functional groups; where the digest split a file's frames between stacks
//! (`instance_frame`), a stack's pyramid takes the frames that are its own.
//! A multi-frame file whose frames do not say where they are is stacked in
//! frame order, and the manifest's `order` says so.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use dicom_dictionary_std::tags;
use dicom_encoding::adapters::PixelDataObject;
use dicom_encoding::transfer_syntax::TransferSyntaxIndex;
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use nils_registry::Param;
use nils_registry::schema::Type;
use nils_registry::store::Store;
use openjph_core::codestream::Codestream;
use openjph_core::file::{MemInfile, MemOutfile};
use openjph_core::types::{Point, Size};
use serde::{Deserialize, Serialize};

use crate::grants::Detail;
use crate::serve::{Caller, Reply};
use nils_registry::place::{self, Place, Role as PlaceRole};

pub const TILE: u32 = 256;
pub const LEVELS: u32 = 4;
/// The most planes one slab answer carries.
pub const SLAB_MAX: u32 = 32;
pub const CODEC: &str = "htj2k";
pub const CONTENT_TYPE: &str = "application/x-nils-tiles";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Level {
    pub level: u32,
    /// `[nz, ny, nx]` at this level.
    pub shape: [u32; 3],
    /// `[ty, tx]`: tiles per plane.
    pub tiles: [u32; 2],
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub percentiles: [i64; 2],
    pub center: f64,
    pub width: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub burned_in: bool,
    /// Where the header said it is: `"header"` when the tag named it, else
    /// the bands the render blanks, `"top-and-bottom-eighths"`.
    #[serde(rename = "where")]
    pub place: String,
}

/// Whether a stack's planes make a volume: parallel to each other, and
/// evenly spaced along their normal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub parallel: bool,
    pub evenly_spaced: bool,
}

/// The orientation a manifest from before record 45 is read with: axial.
pub const AXIAL: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

fn axial() -> [f64; 6] {
    AXIAL
}

/// The plane nearest the stack's, by the largest component of its normal.
fn plane_of(orientation: &[f64; 6]) -> (&'static str, bool) {
    let n = normal(orientation);
    let (i, largest) = n
        .iter()
        .map(|c| c.abs())
        .enumerate()
        .fold(
            (2, 0.0),
            |(bi, bv), (i, v)| if v > bv { (i, v) } else { (bi, bv) },
        );
    let plane = match i {
        0 => "sagittal",
        1 => "coronal",
        _ => "axial",
    };
    // more than about a degree off the nearest plane is oblique
    (plane, largest < 0.9998)
}

/// The normal of a plane: the rows' direction crossed with the columns'.
pub fn normal(o: &[f64; 6]) -> [f64; 3] {
    let (r, c) = ([o[0], o[1], o[2]], [o[3], o[4], o[5]]);
    let n = [
        r[1] * c[2] - r[2] * c[1],
        r[2] * c[0] - r[0] * c[2],
        r[0] * c[1] - r[1] * c[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > 0.0 {
        [n[0] / len, n[1] / len, n[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// What the desk's loader reads first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub stack: i64,
    pub codec: String,
    pub tile: u32,
    pub levels: u32,
    /// `[nz, ny, nx]` at level 0.
    pub shape: [u32; 3],
    /// `[dz, dy, dx]` in millimetres.
    pub spacing: [f64; 3],
    pub dtype: String,
    /// A stored value is the modality's value as `stored * slope +
    /// intercept` (record 45: the file's Rescale Slope and Intercept, with
    /// a signed volume's shift by 32768 into u16 folded in). A manifest from
    /// before has no slope, which reads as one, and its intercept is the
    /// shift alone.
    #[serde(serialize_with = "whole_when_whole")]
    pub intercept: f64,
    #[serde(default = "one", serialize_with = "whole_when_whole")]
    pub slope: f64,
    /// The files did not share one rescale; the first file's is the
    /// manifest's.
    #[serde(default)]
    pub rescale_varies: bool,
    pub window: Window,
    pub bytes_per_level: Vec<u64>,
    pub level_shapes: Vec<Level>,
    pub annotation: Annotation,
    pub built_at: String,
    pub pack_version: Option<String>,
    pub precompute: Precompute,
    /// Record 45 E2: the direction cosines of a plane's rows, then of its
    /// columns, in the patient's frame (LPS, as DICOM has it).
    #[serde(default = "axial")]
    pub orientation: [f64; 6],
    /// The patient position of the first plane's first pixel, millimetres.
    #[serde(default)]
    pub origin: [f64; 3],
    /// Whether the planes make a volume; none in a manifest from before.
    #[serde(default)]
    pub frame: Option<Frame>,
    /// False when the files did not say (or the manifest is from before
    /// record 45), and `orientation` is the axial it is read as.
    #[serde(default)]
    pub orientation_known: bool,
    /// The plane nearest the stack's (axial, coronal or sagittal), and
    /// whether it is oblique to it.
    #[serde(default = "plane_axial")]
    pub plane: String,
    #[serde(default)]
    pub oblique: bool,
    /// Some of the pixels come from a lossy source: a transfer syntax that
    /// may be lossy (JPEG baseline and extended, JPEG-LS near-lossless,
    /// JPEG 2000 that is not lossless-only) or a file that says Lossy Image
    /// Compression 01. False in a manifest from before.
    #[serde(default)]
    pub lossy: bool,
    /// The transfer syntaxes the stack's files were read in, each once.
    #[serde(default)]
    pub source_syntaxes: Vec<String>,
    /// How the planes were put in order: `position`, along the normal by
    /// where each plane says it is (a frame of an enhanced object by its
    /// functional groups); `instance`, by instance number, where a file
    /// did not say; `frames`, the same with a multi-frame file's frames in
    /// the file's order, where a frame did not say where it is (a classic
    /// multi-frame file, a cine), and `spacing[0]` is then the file's
    /// Spacing Between Slices or Slice Thickness, not measured. None in a
    /// manifest from before.
    #[serde(default)]
    pub order: Option<String>,
    /// How many of the stack's files held more than one frame, each frame
    /// a plane; zero in a manifest from before.
    #[serde(default)]
    pub multiframe_files: u32,
}

fn plane_axial() -> String {
    "axial".to_string()
}

fn one() -> f64 {
    1.0
}

/// A number that is whole is written as an integer, as the manifest wrote
/// its intercept before it could be anything else.
fn whole_when_whole<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Precompute {
    pub wall_seconds: f64,
    pub workers: usize,
    pub raw_bytes: u64,
}

/// One volume in memory: `nz` planes of `ny` by `nx` u16.
pub struct Volume {
    pub shape: [u32; 3],
    pub spacing: [f64; 3],
    /// What was added to a raw value to store it in u16: 32768 for a
    /// signed volume, else nothing.
    pub intercept: i64,
    /// The file's Rescale Slope and Intercept: the modality's value is
    /// `raw * slope + intercept`.
    pub rescale: (f64, f64),
    pub rescale_varies: bool,
    pub burned_in: Option<bool>,
    pub data: Vec<u16>,
    /// Where the planes are, when the files said (record 45 E2).
    pub geometry: Option<Geometry>,
    /// Some plane came from a lossy source ([`Manifest::lossy`]).
    pub lossy: bool,
    /// The transfer syntaxes of the files, sorted, each once.
    pub syntaxes: Vec<String>,
    /// How the planes were put in order ([`Manifest::order`]).
    pub order: &'static str,
    /// How many of the files held more than one frame.
    pub multiframe_files: u32,
}

/// A stack's place in the patient, from its files.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub orientation: [f64; 6],
    pub origin: [f64; 3],
    pub frame: Frame,
}

impl Volume {
    fn plane(&self, z: usize) -> &[u16] {
        let n = (self.shape[1] * self.shape[2]) as usize;
        &self.data[z * n..(z + 1) * n]
    }
}

/// The directory a stack's pyramid lives in.
pub fn dir(working: &Path, stack: i64) -> PathBuf {
    working.join("pyramids").join(stack.to_string())
}

/// One file of a stack, and which of its frames are the stack's planes.
#[derive(Debug, Clone, PartialEq)]
pub struct StackFile {
    pub path: PathBuf,
    /// The frames that are the stack's, counting from one, when the digest
    /// split the file's frames between stacks (record 37 S8, the
    /// `instance_frame` rows); none is every frame of the file.
    pub frames: Option<Vec<u32>>,
}

impl StackFile {
    pub fn whole(path: impl Into<PathBuf>) -> StackFile {
        StackFile {
            path: path.into(),
            frames: None,
        }
    }
}

/// The frames a digest wrote down, `1-4,9,12-20`, as frame numbers.
pub fn frame_list(list: &str) -> Result<Vec<u32>, String> {
    let bad = || format!("the frame list {list:?} is not ranges of frame numbers");
    let mut out = Vec::new();
    for part in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let (a, b): (u32, u32) = (
            a.trim().parse().map_err(|_| bad())?,
            b.trim().parse().map_err(|_| bad())?,
        );
        if a == 0 || b < a {
            return Err(bad());
        }
        out.extend(a..=b);
    }
    if out.is_empty() {
        return Err(bad());
    }
    Ok(out)
}

/// Read a stack's files from the registry and make them one volume. A
/// file is the stack's whole when its instance is filed under the stack
/// and the digest wrote no frame rows for it; a multi-frame file whose
/// frames the digest split between stacks gives the frames its
/// `instance_frame` row lists for this stack, whichever stack its instance
/// row names. The planes are ordered along the stack's normal.
pub fn read_volume(store: &mut Store, stack: i64) -> Result<Volume, String> {
    let p = store.dialect().param(1, Type::Int);
    let whole = format!(
        "SELECT so.root, f.path FROM {i} i JOIN {f} f ON f.id = i.source_file_id \
         JOIN {so} so ON so.id = f.source_id WHERE i.stack_id = {p} \
         AND NOT EXISTS (SELECT 1 FROM {fr} fr WHERE fr.instance_id = i.id)",
        i = store.qualified("instance"),
        f = store.qualified("source_file"),
        so = store.qualified("source"),
        fr = store.qualified("instance_frame"),
    );
    let framed = format!(
        "SELECT so.root, f.path, fr.frames FROM {fr} fr JOIN {i} i ON i.id = fr.instance_id \
         JOIN {f} f ON f.id = i.source_file_id JOIN {so} so ON so.id = f.source_id \
         WHERE fr.stack_id = {p}",
        i = store.qualified("instance"),
        f = store.qualified("source_file"),
        so = store.qualified("source"),
        fr = store.qualified("instance_frame"),
    );
    let mut files = Vec::new();
    for r in store
        .query(&whole, &[Param::Int(stack)])
        .map_err(|e| e.to_string())?
    {
        let root = r.text(0).map_err(|e| e.to_string())?;
        let path = r.text(1).map_err(|e| e.to_string())?;
        files.push(StackFile::whole(Path::new(root).join(path)));
    }
    for r in store
        .query(&framed, &[Param::Int(stack)])
        .map_err(|e| e.to_string())?
    {
        let root = r.text(0).map_err(|e| e.to_string())?;
        let path = r.text(1).map_err(|e| e.to_string())?;
        let frames = frame_list(r.text(2).map_err(|e| e.to_string())?)?;
        files.push(StackFile {
            path: Path::new(root).join(path),
            frames: Some(frames),
        });
    }
    if files.is_empty() {
        return Err(format!("stack {stack} has no files the registry can read"));
    }
    read_stack(&files)
}

/// One plane: a single-frame file, or one frame of a multi-frame file.
struct Slice {
    z: f64,
    position: Option<[f64; 3]>,
    orientation: Option<[f64; 6]>,
    instance: i64,
    /// The file's place in the stack's list, and the frame's in the file,
    /// from zero: the order when nothing says where a plane is.
    file: usize,
    frame: u32,
    /// The plane is a frame of a file that holds more than one.
    multiframe: bool,
    rows: u32,
    cols: u32,
    signed: bool,
    bits: u16,
    pixels: Vec<u8>,
    spacing: [f64; 2],
    thickness: f64,
    burned_in: Option<bool>,
    rescale: (f64, f64),
    syntax: String,
    lossy: bool,
}

fn f64s(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Option<Vec<f64>> {
    let el = obj.element(tag).ok()?;
    let s = el.to_str().ok()?;
    let v: Vec<f64> = s
        .split('\\')
        .filter_map(|p| p.trim().parse::<f64>().ok())
        .collect();
    (!v.is_empty()).then_some(v)
}

fn int(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Option<i64> {
    obj.element(tag).ok()?.to_int::<i64>().ok()
}

fn text(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Option<String> {
    obj.element(tag)
        .ok()?
        .to_str()
        .ok()
        .map(|s| s.trim().to_string())
}

/// The first item of a sequence.
fn first_item(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Option<&InMemDicomObject> {
    obj.element(tag).ok()?.items()?.first()
}

/// The transfer syntaxes whose pixel data the parser hands over as it is:
/// little endian, explicit or implicit, deflated (the parser inflates the
/// data set) and explicit big endian (the parser reads the words in their
/// order). Any other is encapsulated and decoded through the registry's
/// codecs; one without a codec is refused with the syntax named.
const NATIVE: [&str; 4] = [
    "1.2.840.10008.1.2",
    "1.2.840.10008.1.2.1",
    "1.2.840.10008.1.2.1.99",
    "1.2.840.10008.1.2.2",
];

/// The encapsulated syntaxes that may be lossy: JPEG baseline and extended
/// and the other DCT processes, JPEG-LS near-lossless, JPEG 2000 and its
/// part 2 and high-throughput forms that are not lossless-only, JPEG XL,
/// and the video syntaxes. A plane read in one of them is marked lossy
/// whatever the encoder chose, as is one whose file says so.
const LOSSY: [&str; 8] = [
    "1.2.840.10008.1.2.4.50",
    "1.2.840.10008.1.2.4.51",
    "1.2.840.10008.1.2.4.81",
    "1.2.840.10008.1.2.4.91",
    "1.2.840.10008.1.2.4.93",
    "1.2.840.10008.1.2.4.203",
    "1.2.840.10008.1.2.4.111",
    "1.2.840.10008.1.2.4.112",
];

/// JPEG-LS, lossless and near-lossless, which the pyramid decodes itself.
const JPEG_LS: [&str; 2] = ["1.2.840.10008.1.2.4.80", "1.2.840.10008.1.2.4.81"];

/// RLE Lossless, whose every frame is one fragment by the standard.
const RLE: &str = "1.2.840.10008.1.2.5";

fn lossy_syntax(ts: &str) -> bool {
    // the DCT processes .52 to .65 are retired lossy JPEG, the .100s video
    LOSSY.contains(&ts)
        || ts
            .strip_prefix("1.2.840.10008.1.2.4.")
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|n| {
                (52..=56).contains(&n) || (58..=65).contains(&n) || (100..=108).contains(&n)
            })
}

/// The words for pixel data a codec refused: the syntax, never the file.
fn undecodable(ts: &str, e: &dyn std::fmt::Display) -> String {
    format!("pixel data in {ts} did not decode: {e}")
}

/// Refuse a syntax the pyramid has no decoder for, before its pixel data
/// is looked at.
fn decoder_for(ts: &str) -> Result<(), String> {
    let codec = TransferSyntaxRegistry
        .get(ts)
        .and_then(|entry| entry.pixel_data_reader());
    if codec.is_none() && !JPEG_LS.contains(&ts) {
        return Err(format!(
            "transfer syntax {ts} is one the pyramid has no decoder for"
        ));
    }
    Ok(())
}

/// Which fragments of an encapsulated object are which frame: one each when
/// there are as many as frames, else by the Basic Offset Table, else by
/// where a codestream starts (a JPEG or JPEG-LS start of image, a JPEG
/// 2000 start of codestream). A frame is a range of fragment indices.
fn frame_fragments(
    fragments: &[Vec<u8>],
    table: &[u32],
    frames: usize,
    ts: &str,
) -> Result<Vec<std::ops::Range<usize>>, String> {
    if frames <= 1 {
        return Ok(std::iter::once(0..fragments.len()).collect());
    }
    if fragments.len() == frames {
        return Ok((0..frames).map(|i| i..i + 1).collect());
    }
    let apart = || {
        undecodable(
            ts,
            &format!(
                "{} fragments could not be told apart into {frames} frames",
                fragments.len()
            ),
        )
    };
    let starts: Vec<usize> = if table.len() == frames {
        // an offset is from the first fragment's item tag, eight bytes of
        // item header before every fragment
        let mut at = Vec::with_capacity(fragments.len());
        let mut offset = 0u64;
        for f in fragments {
            at.push(offset);
            offset += f.len() as u64 + 8;
        }
        table
            .iter()
            .map(|&o| at.iter().position(|&a| a == u64::from(o)).ok_or_else(apart))
            .collect::<Result<_, _>>()?
    } else if ts == RLE {
        return Err(apart());
    } else {
        let marker: &[u8] = if ts.starts_with("1.2.840.10008.1.2.4.9")
            || ts.starts_with("1.2.840.10008.1.2.4.20")
        {
            &[0xFF, 0x4F]
        } else {
            &[0xFF, 0xD8]
        };
        fragments
            .iter()
            .enumerate()
            .filter(|(_, f)| f.starts_with(marker))
            .map(|(i, _)| i)
            .collect()
    };
    if starts.len() != frames || starts.first() != Some(&0) || !starts.is_sorted() {
        return Err(apart());
    }
    Ok((0..frames)
        .map(|i| starts[i]..starts.get(i + 1).copied().unwrap_or(fragments.len()))
        .collect())
}

/// One frame's codestream, decoded by the registry's codec for `ts` (or
/// the JPEG-LS decoder) into little endian samples of the object's Bits
/// Allocated. `header` holds the image pixel attributes a codec reads; the
/// frame is handed over as a single-frame object of its own, so a codec
/// never copies the file's other frames. The words say which syntax and
/// never the file.
fn decode_frame(header: &InMemDicomObject, ts: &str, stream: Vec<u8>) -> Result<Vec<u8>, String> {
    let failed = |e: &dyn std::fmt::Display| undecodable(ts, e);
    let mut obj = header.clone();
    obj.put(dicom_core::DataElement::new(
        tags::PIXEL_DATA,
        dicom_core::VR::OB,
        dicom_core::DicomValue::from(dicom_core::value::PixelFragmentSequence::new_fragments(
            vec![stream],
        )),
    ));
    // the codecs read the syntax from a file's meta group; the SOP UIDs
    // are stand-ins
    let meta = FileMetaTableBuilder::new()
        .transfer_syntax(ts)
        .media_storage_sop_class_uid("1.2")
        .media_storage_sop_instance_uid("1.2");
    let file = obj.with_meta(meta).map_err(|e| failed(&e))?;
    let mut out = Vec::new();
    match TransferSyntaxRegistry
        .get(ts)
        .and_then(|entry| entry.pixel_data_reader())
    {
        Some(codec) => codec
            .decode_frame(&file, 0, &mut out)
            .map_err(|e| failed(&e))?,
        None => {
            let cols = int(&file, tags::COLUMNS).unwrap_or(0) as u32;
            let rows = int(&file, tags::ROWS).unwrap_or(0) as u32;
            let bits = int(&file, tags::BITS_ALLOCATED).unwrap_or(16);
            let frame = file
                .frame_pixel_data(0)
                .ok_or_else(|| failed(&"no frame"))?;
            let (samples, _, _) =
                jpegls::decode(&jpeg_ls_trimmed(&frame), cols, rows).map_err(|e| failed(&e))?;
            out = if bits == 8 {
                samples.iter().map(|&v| v as u8).collect()
            } else {
                samples.iter().flat_map(|v| v.to_le_bytes()).collect()
            };
        }
    }
    Ok(out)
}

/// A JPEG-LS codestream as the decoder takes it: an encapsulated fragment
/// is padded to an even length, with a zero after the end-of-image marker
/// or a fill byte before it (DCMTK's default writes FF FF D9), and the
/// decoder wants the marker last and alone.
fn jpeg_ls_trimmed(frame: &[u8]) -> Vec<u8> {
    let mut end = frame.len();
    while end > 0 && frame[end - 1] == 0 {
        end -= 1;
    }
    let mut out = frame[..end].to_vec();
    while out.len() >= 3 && out.ends_with(&[0xFF, 0xFF, 0xD9]) {
        out.remove(out.len() - 3);
    }
    out
}

/// The words of 16-bit signed samples with fewer bits stored, extended
/// from their sign bit: a JPEG lossless or JPEG-LS codec hands back the
/// stored bits alone, where the native form has them extended already.
fn extend_sign(pixels: &mut [u8], stored: u16) {
    if !(1..16).contains(&stored) {
        return;
    }
    let mask = (1u16 << stored) - 1;
    let sign = 1u16 << (stored - 1);
    for px in pixels.as_chunks_mut::<2>().0 {
        let v = u16::from_le_bytes(*px) & mask;
        *px = (if v & sign != 0 { v | !mask } else { v }).to_le_bytes();
    }
}

/// The image pixel attributes a codec reads, copied from the file.
const PIXEL_HEADER: [dicom_core::Tag; 9] = [
    tags::ROWS,
    tags::COLUMNS,
    tags::SAMPLES_PER_PIXEL,
    tags::PHOTOMETRIC_INTERPRETATION,
    tags::PLANAR_CONFIGURATION,
    tags::BITS_ALLOCATED,
    tags::BITS_STORED,
    tags::HIGH_BIT,
    tags::PIXEL_REPRESENTATION,
];

/// The planes of one file: every frame, or the frames `wanted` names
/// (counting from one), each with its own place in the patient. An
/// enhanced object says a frame's position, orientation, pixel spacing and
/// rescale in its Per-frame Functional Groups, with the Shared Functional
/// Groups as the fallback, and a classic file at the top level. A frame of
/// a multi-frame file that says no position of its own has none: the
/// file's one position is its first frame's at best. `threads` decode
/// compressed frames at once.
fn read_frames(
    path: &Path,
    file_index: usize,
    wanted: Option<&[u32]>,
    threads: usize,
) -> Result<Vec<Slice>, String> {
    // record 48: a file without the Part 10 preamble or meta group is read
    // as the digest reads it, a bare data set, so one such file does not
    // fail its stack's pyramid
    let (obj, ts) = nils_dicom::read_whole(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let obj = &obj;
    let rows = int(obj, tags::ROWS).ok_or("no Rows")? as u32;
    let cols = int(obj, tags::COLUMNS).ok_or("no Columns")? as u32;
    let bits = int(obj, tags::BITS_ALLOCATED).unwrap_or(16) as u16;
    if bits != 16 && bits != 8 {
        return Err(format!("{bits} bits allocated; the pyramid reads 8 and 16"));
    }
    let signed = int(obj, tags::PIXEL_REPRESENTATION).unwrap_or(0) == 1;
    let samples = int(obj, tags::SAMPLES_PER_PIXEL).unwrap_or(1);
    if samples != 1 {
        return Err(format!(
            "{samples} samples per pixel; the pyramid reads one"
        ));
    }
    let stored = int(obj, tags::BITS_STORED).unwrap_or(bits as i64) as u16;
    let lossy =
        lossy_syntax(&ts) || text(obj, tags::LOSSY_IMAGE_COMPRESSION).is_some_and(|s| s == "01");
    let Ok(pixel_data) = obj.element(tags::PIXEL_DATA) else {
        return Err("no Pixel Data".to_string());
    };
    let count = int(obj, tags::NUMBER_OF_FRAMES)
        .filter(|n| *n > 0)
        .unwrap_or(1) as u32;
    let frames: Vec<u32> = match wanted {
        None => (0..count).collect(),
        Some(w) => w
            .iter()
            .map(|&f| {
                if f == 0 || f > count {
                    Err(format!(
                        "pixel data holds {count} frames, the stack names frame {f}"
                    ))
                } else {
                    Ok(f - 1)
                }
            })
            .collect::<Result<_, _>>()?,
    };
    let multiframe = count > 1;
    let need = (rows * cols) as usize * (bits as usize / 8);
    // the pixels of each wanted frame, in the order of `frames`
    let pixels: Vec<Vec<u8>> = if NATIVE.contains(&ts.as_str()) {
        let all = pixel_data.to_bytes().map_err(|e| e.to_string())?;
        frames
            .iter()
            .map(|&i| {
                let at = i as usize * need;
                all.get(at..at + need).map(<[u8]>::to_vec).ok_or_else(|| {
                    format!(
                        "pixel data holds {} bytes, the header says {}",
                        all.len(),
                        need * count as usize
                    )
                })
            })
            .collect::<Result<_, _>>()?
    } else {
        decoder_for(&ts)?;
        let dicom_core::DicomValue::PixelSequence(seq) = pixel_data.value() else {
            return Err(undecodable(&ts, &"the pixel data is not encapsulated"));
        };
        let fragments = seq.fragments();
        let ranges = frame_fragments(fragments, seq.offset_table(), count as usize, &ts)?;
        let mut header = InMemDicomObject::new_empty();
        for tag in PIXEL_HEADER {
            if let Ok(e) = obj.element(tag) {
                header.put(e.clone());
            }
        }
        let streams: Vec<Vec<u8>> = frames
            .iter()
            .map(|&i| fragments[ranges[i as usize].clone()].concat())
            .collect();
        let chunk = streams.len().div_ceil(threads.max(1)).max(1);
        let (header, ts) = (&header, ts.as_str());
        let decoded: Vec<Result<Vec<Vec<u8>>, String>> = std::thread::scope(|s| {
            let handles: Vec<_> = streams
                .chunks(chunk)
                .map(|part| {
                    s.spawn(move || {
                        part.iter()
                            .map(|stream| {
                                let mut px = decode_frame(header, ts, stream.clone())?;
                                if signed && bits == 16 {
                                    extend_sign(&mut px, stored);
                                }
                                Ok(px)
                            })
                            .collect()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err("a decoder panicked".to_string()))
                })
                .collect()
        });
        let mut out = Vec::with_capacity(frames.len());
        for part in decoded {
            out.extend(part?);
        }
        out
    };
    if let Some(short) = pixels.iter().find(|p| p.len() < need) {
        return Err(format!(
            "pixel data holds {} bytes, the header says {need}",
            short.len()
        ));
    }
    // the file's own values, which a frame's functional groups override
    let shared = first_item(obj, tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE);
    let per_frame = obj
        .element(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
        .ok()
        .and_then(|e| e.items());
    let instance = int(obj, tags::INSTANCE_NUMBER).unwrap_or(0);
    let burned_in = text(obj, tags::BURNED_IN_ANNOTATION).map(|s| s.eq_ignore_ascii_case("YES"));
    let mut out = Vec::with_capacity(frames.len());
    for (&i, pixels) in frames.iter().zip(pixels) {
        let item = per_frame.and_then(|items| items.get(i as usize));
        // a functional group of the frame's own, else the shared one
        let group = |seq: dicom_core::Tag| {
            item.and_then(|it| first_item(it, seq))
                .or_else(|| shared.and_then(|s| first_item(s, seq)))
        };
        let in_group =
            |seq: dicom_core::Tag, tag: dicom_core::Tag| group(seq).and_then(|g| f64s(g, tag));
        let position = in_group(tags::PLANE_POSITION_SEQUENCE, tags::IMAGE_POSITION_PATIENT)
            .or_else(|| {
                // the file's one position is no frame's but a single frame's
                (!multiframe)
                    .then(|| f64s(obj, tags::IMAGE_POSITION_PATIENT))
                    .flatten()
            })
            .filter(|v| v.len() >= 3)
            .map(|v| [v[0], v[1], v[2]]);
        let orientation = in_group(
            tags::PLANE_ORIENTATION_SEQUENCE,
            tags::IMAGE_ORIENTATION_PATIENT,
        )
        .or_else(|| f64s(obj, tags::IMAGE_ORIENTATION_PATIENT))
        .filter(|v| v.len() >= 6)
        .map(|v| [v[0], v[1], v[2], v[3], v[4], v[5]]);
        let spacing = in_group(tags::PIXEL_MEASURES_SEQUENCE, tags::PIXEL_SPACING)
            .or_else(|| f64s(obj, tags::PIXEL_SPACING))
            .map(|v| [v[0], *v.get(1).unwrap_or(&v[0])])
            .unwrap_or([1.0, 1.0]);
        // the first of these that is a distance: a spacing some files
        // write negative is its size
        let thickness = [
            in_group(tags::PIXEL_MEASURES_SEQUENCE, tags::SPACING_BETWEEN_SLICES),
            f64s(obj, tags::SPACING_BETWEEN_SLICES),
            in_group(tags::PIXEL_MEASURES_SEQUENCE, tags::SLICE_THICKNESS),
            f64s(obj, tags::SLICE_THICKNESS),
        ]
        .into_iter()
        .flatten()
        .map(|v| v[0].abs())
        .find(|t| t.is_finite() && *t > 0.0)
        .unwrap_or(1.0);
        let transform = |tag| {
            in_group(tags::PIXEL_VALUE_TRANSFORMATION_SEQUENCE, tag)
                .or_else(|| f64s(obj, tag))
                .and_then(|v| v.first().copied())
        };
        let slope = transform(tags::RESCALE_SLOPE)
            .filter(|s| s.is_finite() && *s != 0.0)
            .unwrap_or(1.0);
        let rescale_intercept = transform(tags::RESCALE_INTERCEPT)
            .filter(|b| b.is_finite())
            .unwrap_or(0.0);
        out.push(Slice {
            z: position.map(|p| p[2]).unwrap_or(f64::NAN),
            position,
            orientation,
            instance,
            file: file_index,
            frame: i,
            multiframe,
            rows,
            cols,
            signed,
            bits,
            pixels,
            spacing,
            thickness,
            burned_in,
            rescale: (slope, rescale_intercept),
            syntax: ts.clone(),
            lossy,
        });
    }
    Ok(out)
}

/// The files of one stack into one volume, in order, every frame of each.
#[cfg(test)]
pub fn read_files(files: &[PathBuf]) -> Result<Volume, String> {
    let files: Vec<StackFile> = files.iter().map(StackFile::whole).collect();
    read_stack(&files)
}

/// How a volume's planes were put in order ([`Manifest::order`]).
pub const ORDER_POSITION: &str = "position";
pub const ORDER_INSTANCE: &str = "instance";
pub const ORDER_FRAMES: &str = "frames";

/// The planes of one stack's files, and of the frames each names, into one
/// volume, in order.
pub fn read_stack(files: &[StackFile]) -> Result<Volume, String> {
    // a compressed plane costs its decode, so the files are read a few at
    // a time, and the frames of a file a few at a time within it; the first
    // file that fails, in the files' order, is the error
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8);
    let chunk = files.len().div_ceil(threads).max(1);
    let within = (threads / files.len().min(threads).max(1)).max(1);
    let read: Vec<Result<Vec<Slice>, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .enumerate()
            .map(|(c, part)| {
                s.spawn(move || {
                    let mut slices = Vec::new();
                    for (k, f) in part.iter().enumerate() {
                        slices.extend(read_frames(
                            &f.path,
                            c * chunk + k,
                            f.frames.as_deref(),
                            within,
                        )?);
                    }
                    Ok(slices)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("a reader panicked".to_string()))
            })
            .collect()
    });
    let mut slices = Vec::with_capacity(files.len());
    for part in read {
        slices.extend(part?);
    }
    if slices.is_empty() {
        return Err("stack has no files to read".to_string());
    }
    let (rows, cols, bits, signed) = (
        slices[0].rows,
        slices[0].cols,
        slices[0].bits,
        slices[0].signed,
    );
    if slices
        .iter()
        .any(|s| s.rows != rows || s.cols != cols || s.bits != bits)
    {
        return Err("the stack's files do not share one matrix".to_string());
    }
    // Record 45 E2: along the normal of the planes when every plane says
    // where it is and how it is turned, which for an axial stack is the
    // third coordinate as before; else by that coordinate; else by the
    // instance number, and a multi-frame file's frames in the file's order.
    // Each plane's distance along the normal is its z.
    let oriented = slices
        .iter()
        .all(|s| s.position.is_some() && s.orientation.is_some());
    if oriented {
        let n = normal(&slices[0].orientation.expect("every plane is oriented"));
        for s in &mut slices {
            let p = s.position.expect("every plane has a position");
            s.z = p[0] * n[0] + p[1] * n[1] + p[2] * n[2];
        }
    }
    let placed = slices.iter().all(|s| s.z.is_finite());
    let order = if placed {
        // stable: planes at one place keep the files' and frames' order
        slices.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
        ORDER_POSITION
    } else {
        slices.sort_by_key(|s| (s.instance, s.file, s.frame));
        if slices.iter().any(|s| s.multiframe) {
            ORDER_FRAMES
        } else {
            ORDER_INSTANCE
        }
    };
    let nz = slices.len() as u32;
    let intercept: i64 = if signed { 32768 } else { 0 };
    let mut data = Vec::with_capacity((nz * rows * cols) as usize);
    for s in &slices {
        let n = (rows * cols) as usize;
        if bits == 8 {
            // a signed byte is shifted as a signed word is
            data.extend(s.pixels[..n].iter().map(|&b| {
                if signed {
                    (b as i8 as i32 + 32768) as u16
                } else {
                    b as u16
                }
            }));
        } else {
            for px in s.pixels[..n * 2].as_chunks::<2>().0 {
                let raw = u16::from_le_bytes(*px);
                data.push(if signed {
                    (raw as i16 as i32 + 32768) as u16
                } else {
                    raw
                });
            }
        }
    }
    // measured along the normal where the planes say where they are, else
    // the files' Spacing Between Slices or Slice Thickness
    let dz = if nz > 1 && placed {
        ((slices[nz as usize - 1].z - slices[0].z) / (nz as f64 - 1.0)).abs()
    } else {
        slices[0].thickness
    };
    let burned_in = slices.iter().find_map(|s| s.burned_in);
    let geometry = if oriented {
        let first = slices[0].orientation.expect("every plane is oriented");
        let parallel = slices.iter().all(|s| {
            s.orientation.is_some_and(|o| {
                o.iter()
                    .zip(first.iter())
                    .all(|(a, b)| (a - b).abs() < 1e-3)
            })
        });
        let gaps: Vec<f64> = slices.windows(2).map(|w| w[1].z - w[0].z).collect();
        let evenly_spaced = match gaps.first() {
            None => true,
            Some(_) => {
                let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
                let tolerance = (mean.abs() * 0.01).max(1e-3);
                mean.abs() > 1e-6 && gaps.iter().all(|g| (g - mean).abs() <= tolerance)
            }
        };
        Some(Geometry {
            orientation: first,
            origin: slices[0].position.expect("every plane has a position"),
            frame: Frame {
                parallel,
                evenly_spaced,
            },
        })
    } else {
        None
    };
    let lossy = slices.iter().any(|s| s.lossy);
    let syntaxes: Vec<String> = slices
        .iter()
        .map(|s| s.syntax.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let rescale = slices[0].rescale;
    let rescale_varies = slices
        .iter()
        .any(|s| (s.rescale.0 - rescale.0).abs() > 1e-9 || (s.rescale.1 - rescale.1).abs() > 1e-9);
    let multiframe_files = slices
        .iter()
        .filter(|s| s.multiframe)
        .map(|s| s.file)
        .collect::<std::collections::BTreeSet<_>>()
        .len() as u32;
    Ok(Volume {
        shape: [nz, rows, cols],
        spacing: [dz, slices[0].spacing[0], slices[0].spacing[1]],
        intercept,
        rescale,
        rescale_varies,
        burned_in,
        data,
        geometry,
        lossy,
        syntaxes,
        order,
        multiframe_files,
    })
}

/// One tile as an HTJ2K codestream, reversible, single component, 16 bits.
pub fn encode_tile(pixels: &[u16], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut cs = Codestream::new();
    cs.access_siz_mut()
        .set_image_extent(Point::new(width, height));
    cs.access_siz_mut().set_num_components(1);
    cs.access_siz_mut()
        .set_comp_info(0, Point::new(1, 1), 16, false);
    cs.access_siz_mut().set_tile_size(Size::new(width, height));
    let decompositions = if width.min(height) >= 32 { 5 } else { 2 };
    cs.access_cod_mut().set_num_decomposition(decompositions);
    cs.access_cod_mut().set_reversible(true);
    cs.access_cod_mut().set_color_transform(false);
    cs.set_planar(0);
    let mut out = MemOutfile::new();
    cs.write_headers(&mut out, &[]).map_err(|e| e.to_string())?;
    let mut line = vec![0i32; width as usize];
    for y in 0..height as usize {
        let row = &pixels[y * width as usize..(y + 1) * width as usize];
        for (l, p) in line.iter_mut().zip(row) {
            *l = *p as i32;
        }
        cs.exchange(&line, 0).map_err(|e| e.to_string())?;
    }
    cs.flush(&mut out).map_err(|e| e.to_string())?;
    Ok(out.get_data().to_vec())
}

/// A tile back to pixels: `(width, height, pixels)`.
pub fn decode_tile(bytes: &[u8]) -> Result<(u32, u32, Vec<u16>), String> {
    let mut infile = MemInfile::new(bytes);
    let mut cs = Codestream::new();
    cs.read_headers(&mut infile).map_err(|e| e.to_string())?;
    let extent = cs.access_siz().get_image_extent();
    let (width, height) = (extent.x, extent.y);
    cs.create(&mut infile).map_err(|e| e.to_string())?;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for _ in 0..height {
        let line = cs.pull(0).ok_or("the codestream ended early")?;
        pixels.extend(line.iter().map(|&v| v.clamp(0, 65535) as u16));
    }
    Ok((width, height, pixels))
}

/// The tile grid of a plane `ny` by `nx`.
fn grid(ny: u32, nx: u32) -> (u32, u32) {
    (ny.div_ceil(TILE), nx.div_ceil(TILE))
}

/// One plane at half resolution in-plane: the 2 by 2 mean.
fn halve(plane: &[u16], ny: u32, nx: u32) -> (Vec<u16>, u32, u32) {
    let (hy, hx) = (ny / 2, nx / 2);
    let mut out = Vec::with_capacity((hy * hx) as usize);
    for y in 0..hy as usize {
        for x in 0..hx as usize {
            let a = plane[(2 * y) * nx as usize + 2 * x] as u32;
            let b = plane[(2 * y) * nx as usize + 2 * x + 1] as u32;
            let c = plane[(2 * y + 1) * nx as usize + 2 * x] as u32;
            let d = plane[(2 * y + 1) * nx as usize + 2 * x + 1] as u32;
            out.push(((a + b + c + d) / 4) as u16);
        }
    }
    (out, hy, hx)
}

/// The tile file name.
fn tile_path(root: &Path, level: u32, z: u32, ty: u32, tx: u32) -> PathBuf {
    root.join(level.to_string())
        .join(z.to_string())
        .join(format!("{ty}_{tx}.j2c"))
}

/// Encode one plane's tiles at one level; returns the bytes written.
fn write_plane(
    root: &Path,
    level: u32,
    z: u32,
    plane: &[u16],
    ny: u32,
    nx: u32,
) -> Result<u64, String> {
    let (ty, tx) = grid(ny, nx);
    let dir = root.join(level.to_string()).join(z.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut bytes = 0u64;
    let mut tile = Vec::with_capacity((TILE * TILE) as usize);
    for j in 0..ty {
        for i in 0..tx {
            let y0 = j * TILE;
            let x0 = i * TILE;
            let h = TILE.min(ny - y0);
            let w = TILE.min(nx - x0);
            tile.clear();
            for y in y0..y0 + h {
                let row = &plane[(y * nx + x0) as usize..(y * nx + x0 + w) as usize];
                tile.extend_from_slice(row);
            }
            let encoded = encode_tile(&tile, w, h)?;
            bytes += encoded.len() as u64;
            std::fs::write(tile_path(root, level, z, j, i), &encoded).map_err(|e| e.to_string())?;
        }
    }
    Ok(bytes)
}

/// The window the viewer opens at: the first and ninety-ninth percentiles
/// of a sample of planes.
fn window(vol: &Volume) -> Window {
    let nz = vol.shape[0] as usize;
    let step = (nz / 16).max(1);
    let mut sample: Vec<u16> = Vec::new();
    for z in (0..nz).step_by(step) {
        let p = vol.plane(z);
        let every = (p.len() / 65536).max(1);
        sample.extend(p.iter().step_by(every));
    }
    if sample.is_empty() {
        return Window {
            percentiles: [0, 0],
            center: 0.0,
            width: 1.0,
        };
    }
    // in the modality's values, which a descending slope reverses
    let (slope, b) = vol.rescale;
    let mut values: Vec<f64> = sample
        .iter()
        .map(|s| (*s as i64 - vol.intercept) as f64 * slope + b)
        .collect();
    values.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let at = |q: f64| values[((values.len() - 1) as f64 * q) as usize];
    let (p1, p99) = (at(0.01), at(0.99));
    Window {
        percentiles: [p1.round() as i64, p99.round() as i64],
        center: (p1 + p99) / 2.0,
        width: (p99 - p1).max(1.0),
    }
}

/// Build the pyramid of a volume under `root`, `workers` planes at a time.
pub fn build(
    vol: &Volume,
    stack: i64,
    root: &Path,
    workers: usize,
    pack_version: Option<String>,
) -> Result<Manifest, String> {
    let started = std::time::Instant::now();
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let [nz, ny, nx] = vol.shape;
    let workers = workers.max(1);
    let mut level_shapes = Vec::new();
    let mut bytes_per_level = Vec::new();
    // every level from the level before it, plane by plane, in parallel over planes
    let mut current: Vec<Vec<u16>> = (0..nz as usize).map(|z| vol.plane(z).to_vec()).collect();
    let (mut ly, mut lx) = (ny, nx);
    for level in 0..LEVELS {
        let (ty, tx) = grid(ly, lx);
        let planes = &current;
        let chunk = (planes.len() / workers).max(1);
        let results: Vec<Result<u64, String>> = std::thread::scope(|s| {
            let mut handles = Vec::new();
            for (c, part) in planes.chunks(chunk).enumerate() {
                let (ly, lx) = (ly, lx);
                handles.push(s.spawn(move || {
                    let mut total = 0u64;
                    for (k, plane) in part.iter().enumerate() {
                        let z = (c * chunk + k) as u32;
                        total += write_plane(root, level, z, plane, ly, lx)?;
                    }
                    Ok::<u64, String>(total)
                }));
            }
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err("a worker panicked".into())))
                .collect()
        });
        let mut bytes = 0u64;
        for r in results {
            bytes += r?;
        }
        level_shapes.push(Level {
            level,
            shape: [nz, ly, lx],
            tiles: [ty, tx],
            bytes,
        });
        bytes_per_level.push(bytes);
        if level + 1 < LEVELS {
            if ly < 2 || lx < 2 {
                break;
            }
            let next: Vec<Vec<u16>> = std::thread::scope(|s| {
                let handles: Vec<_> = current
                    .chunks(chunk)
                    .map(|part| {
                        s.spawn(move || part.iter().map(|p| halve(p, ly, lx).0).collect::<Vec<_>>())
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap_or_default())
                    .collect()
            });
            current = next;
            ly /= 2;
            lx /= 2;
        }
    }
    let annotation = Annotation {
        burned_in: vol.burned_in.unwrap_or(false),
        place: if vol.burned_in.is_some() {
            "header".to_string()
        } else {
            "top-and-bottom-eighths".to_string()
        },
    };
    let orientation = vol.geometry.map_or(AXIAL, |g| g.orientation);
    let (plane, oblique) = plane_of(&orientation);
    let manifest = Manifest {
        orientation,
        origin: vol.geometry.map_or([0.0; 3], |g| g.origin),
        frame: vol.geometry.map(|g| g.frame),
        orientation_known: vol.geometry.is_some(),
        plane: plane.to_string(),
        oblique,
        lossy: vol.lossy,
        source_syntaxes: vol.syntaxes.clone(),
        order: Some(vol.order.to_string()),
        multiframe_files: vol.multiframe_files,
        stack,
        codec: CODEC.to_string(),
        tile: TILE,
        levels: level_shapes.len() as u32,
        shape: [nz, ny, nx],
        spacing: vol.spacing,
        dtype: "uint16".to_string(),
        // stored * slope + intercept: the shift undone, then the rescale
        slope: vol.rescale.0,
        intercept: vol.rescale.1 - vol.intercept as f64 * vol.rescale.0,
        rescale_varies: vol.rescale_varies,
        window: window(vol),
        bytes_per_level,
        level_shapes,
        annotation,
        built_at: nils_registry::time::now_iso(),
        pack_version,
        precompute: Precompute {
            wall_seconds: started.elapsed().as_secs_f64(),
            workers,
            raw_bytes: vol.data.len() as u64 * 2,
        },
    };
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(root.join("manifest.json"), text).map_err(|e| e.to_string())?;
    Ok(manifest)
}

/// The manifest of a built pyramid, or none.
pub fn manifest(root: &Path) -> Result<Option<Manifest>, String> {
    let p = root.join("manifest.json");
    if !p.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// One plane's tiles in one container: `[u32 count][u32 offset...][tiles]`,
/// little endian; the offsets are from the start of the container, tiles
/// in row-major order of the plane's grid.
pub fn plane_container(root: &Path, m: &Manifest, level: u32, z: u32) -> Result<Vec<u8>, String> {
    let lv = m
        .level_shapes
        .get(level as usize)
        .ok_or_else(|| format!("level {level} is not in the pyramid; it has {}", m.levels))?;
    if z >= lv.shape[0] {
        return Err(format!("plane {z} is past the stack's {}", lv.shape[0]));
    }
    let [ty, tx] = lv.tiles;
    let mut tiles = Vec::with_capacity((ty * tx) as usize);
    for j in 0..ty {
        for i in 0..tx {
            tiles.push(std::fs::read(tile_path(root, level, z, j, i)).map_err(|e| e.to_string())?);
        }
    }
    Ok(container(&tiles))
}

pub fn container(tiles: &[Vec<u8>]) -> Vec<u8> {
    let head = 4 + 4 * tiles.len();
    let mut out = Vec::with_capacity(head + tiles.iter().map(Vec::len).sum::<usize>());
    out.write_all(&(tiles.len() as u32).to_le_bytes()).ok();
    let mut offset = head as u32;
    for t in tiles {
        out.write_all(&offset.to_le_bytes()).ok();
        offset += t.len() as u32;
    }
    for t in tiles {
        out.extend_from_slice(t);
    }
    out
}

/// A slab: the planes `z0..z1` each as a container, concatenated behind a
/// count and offsets of their own (the same shape one level up).
pub fn slab_container(
    root: &Path,
    m: &Manifest,
    level: u32,
    z0: u32,
    z1: u32,
) -> Result<Vec<u8>, String> {
    let planes: Vec<Vec<u8>> = (z0..z1)
        .map(|z| plane_container(root, m, level, z))
        .collect::<Result<_, _>>()?;
    Ok(container(&planes))
}

/// A whole plane decoded from its tiles.
pub fn decode_plane(
    root: &Path,
    m: &Manifest,
    level: u32,
    z: u32,
) -> Result<(u32, u32, Vec<u16>), String> {
    let lv = m
        .level_shapes
        .get(level as usize)
        .ok_or_else(|| format!("level {level} is not in the pyramid"))?;
    let [_, ny, nx] = lv.shape;
    let [ty, tx] = lv.tiles;
    let mut plane = vec![0u16; (ny * nx) as usize];
    for j in 0..ty {
        for i in 0..tx {
            let bytes =
                std::fs::read(tile_path(root, level, z, j, i)).map_err(|e| e.to_string())?;
            let (w, h, px) = decode_tile(&bytes)?;
            for y in 0..h {
                let src = &px[(y * w) as usize..((y + 1) * w) as usize];
                let dy = j * TILE + y;
                let dx = i * TILE;
                plane[(dy * nx + dx) as usize..(dy * nx + dx + w) as usize].copy_from_slice(src);
            }
        }
    }
    Ok((nx, ny, plane))
}

/// The plane along an axis: `z` is the index along that axis. `y` and `x`
/// planes are assembled from every plane's row or column (the slab door's
/// job in the browser; here for the server render).
pub fn plane_along(
    root: &Path,
    m: &Manifest,
    level: u32,
    axis: char,
    index: u32,
) -> Result<(u32, u32, Vec<u16>), String> {
    let lv = m
        .level_shapes
        .get(level as usize)
        .ok_or_else(|| format!("level {level} is not in the pyramid"))?;
    let [nz, ny, nx] = lv.shape;
    match axis {
        'z' => decode_plane(root, m, level, index.min(nz.saturating_sub(1))),
        'y' => {
            let row = index.min(ny.saturating_sub(1));
            let mut out = Vec::with_capacity((nz * nx) as usize);
            for z in 0..nz {
                let (w, _, px) = decode_plane(root, m, level, z)?;
                out.extend_from_slice(&px[(row * w) as usize..((row + 1) * w) as usize]);
            }
            Ok((nx, nz, out))
        }
        _ => {
            let col = index.min(nx.saturating_sub(1));
            let mut out = Vec::with_capacity((nz * ny) as usize);
            for z in 0..nz {
                let (w, h, px) = decode_plane(root, m, level, z)?;
                for y in 0..h {
                    out.push(px[(y * w + col) as usize]);
                }
            }
            Ok((ny, nz, out))
        }
    }
}

/// The plane with window and level applied, as a JPEG; `blank` blanks the
/// top and bottom eighths, where burned-in annotation is held. A stored
/// value is the modality's as `stored * slope + intercept` (record 45), and
/// the window is in the modality's values.
#[allow(clippy::too_many_arguments)]
pub fn render_jpeg(
    width: u32,
    height: u32,
    pixels: &[u16],
    slope: f64,
    intercept: f64,
    center: f64,
    wwidth: f64,
    blank: bool,
) -> Result<Vec<u8>, String> {
    // a width below one (0 from a header, or a caller's) is one: a narrower
    // window would divide into NaN or infinity and render black
    let wwidth = if wwidth.is_finite() {
        wwidth.max(1.0)
    } else {
        1.0
    };
    let lo = center - wwidth / 2.0;
    let scale = 255.0 / wwidth;
    let mut gray = Vec::with_capacity(pixels.len());
    let band = height / 8;
    // the value to grey as one line: stored * (slope * scale) + offset
    let (k, offset) = (slope * scale, (intercept - lo) * scale);
    for (n, &p) in pixels.iter().enumerate() {
        let y = n as u32 / width.max(1);
        if blank && (y < band || y >= height - band) {
            gray.push(0u8);
            continue;
        }
        gray.push((p as f64 * k + offset).clamp(0.0, 255.0) as u8);
    }
    let img =
        image::GrayImage::from_raw(width, height, gray).ok_or("the plane's size does not match")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&img)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// The pyramids a working place holds, by stack.
pub fn built(working: &Path) -> BTreeMap<i64, Manifest> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(working.join("pyramids")) else {
        return out;
    };
    for e in entries.flatten() {
        if let Ok(id) = e.file_name().to_string_lossy().parse::<i64>()
            && let Ok(Some(m)) = manifest(&e.path())
        {
            out.insert(id, m);
        }
    }
    out
}

/// What a build over many stacks did (record 45 E1).
#[derive(Debug, Default)]
pub struct Many {
    pub built: Vec<i64>,
    pub skipped: Vec<i64>,
    /// A stack whose pyramid could not be built, and a reason class
    /// ([`reason_of`]): never the reader's words, which can hold a file's
    /// path (a subject code, a series name) or header text, since a job's
    /// result is served at detail plain.
    pub failed: Vec<(i64, &'static str)>,
    pub bytes: u64,
    /// Cancelled part way: what was built stays built.
    pub stopped: bool,
}

impl Many {
    pub fn as_json(&self, place: &str) -> serde_json::Value {
        serde_json::json!({
            "place": place,
            "stacks": self.built.len() + self.skipped.len() + self.failed.len(),
            "built": self.built.len(),
            "skipped": self.skipped.len(),
            "failed": self.failed.len(),
            "bytes": self.bytes,
            "stopped": self.stopped,
            // the first few: a stack id and a reason class, never a path
            "failures": self.failed.iter().take(20).map(|(s, reason)| serde_json::json!({"stack": s, "reason": reason})).collect::<Vec<_>>(),
            // and every failure, counted by its reason class
            "failures_by_reason": self.failed.iter().fold(BTreeMap::<&str, usize>::new(), |mut by, (_, reason)| {
                *by.entry(reason).or_default() += 1;
                by
            }),
        })
    }
}

/// The class of a reason a stack's pyramid was not built, from the
/// reader's or the builder's words, which stay on the machine: `no_files`,
/// `compressed` for a transfer syntax the pyramid has no decoder for,
/// `undecodable` for pixel data its codec refused, `unsupported_pixels`,
/// `mixed_matrix`, `unreadable` for a file that did not open or parse, or
/// `build_failed`.
pub fn reason_of(why: &str, reading: bool) -> &'static str {
    if !reading {
        return "build_failed";
    }
    if why.contains("has no files") {
        "no_files"
    } else if why.starts_with("transfer syntax") {
        "compressed"
    } else if why.starts_with("pixel data in") {
        "undecodable"
    } else if why.contains("bits allocated")
        || why.contains("samples per pixel")
        || why.starts_with("no Rows")
        || why.starts_with("no Columns")
        || why.starts_with("no Pixel Data")
        || why.starts_with("pixel data holds")
    {
        "unsupported_pixels"
    } else if why.contains("do not share one matrix") {
        "mixed_matrix"
    } else {
        "unreadable"
    }
}

/// Build the pyramids of `stacks` under a working place, one stack at a
/// time, each with `workers` planes at once; a stack that has one is
/// skipped, a stack that fails is counted with why and the rest go on.
/// `go_on` is asked after each stack with the counts so far, and a false
/// stops the build there (a cancel).
pub fn build_many(
    store: &mut Store,
    working: &Path,
    stacks: &[i64],
    workers: usize,
    pack_version: Option<String>,
    go_on: &mut dyn FnMut(&mut Store, &Many) -> bool,
) -> Many {
    let mut out = Many::default();
    let mut seen = std::collections::BTreeSet::new();
    for &stack in stacks {
        if !seen.insert(stack) {
            continue;
        }
        let root = dir(working, stack);
        if matches!(manifest(&root), Ok(Some(_))) {
            out.skipped.push(stack);
        } else {
            match read_volume(store, stack)
                .map_err(|why| reason_of(&why, true))
                .and_then(|v| {
                    build(&v, stack, &root, workers, pack_version.clone())
                        .map_err(|why| reason_of(&why, false))
                }) {
                Ok(m) => {
                    out.bytes += m.bytes_per_level.iter().sum::<u64>();
                    out.built.push(stack);
                }
                Err(why) => {
                    // a half-written pyramid is not one: without its
                    // manifest it is built again next time
                    let _ = std::fs::remove_file(root.join("manifest.json"));
                    out.failed.push((stack, why));
                }
            }
        }
        if !go_on(store, &out) {
            out.stopped = true;
            break;
        }
    }
    out
}

/// How many of `stacks` have their picture under the working place: what a
/// campaign's items need before anyone looks at them (record 45 R3).
pub fn pictures(store: &mut Store, stacks: &[i64]) -> serde_json::Value {
    let mut unique: Vec<i64> = stacks.to_vec();
    unique.sort_unstable();
    unique.dedup();
    let Ok(working) = working_place(store, None) else {
        return serde_json::json!({
            "stacks": unique.len(), "have": 0, "missing": unique.len(), "place": null,
        });
    };
    let root = Path::new(&working.path);
    let have = unique
        .iter()
        .filter(|s| dir(root, **s).join("manifest.json").exists())
        .count();
    serde_json::json!({
        "stacks": unique.len(), "have": have, "missing": unique.len() - have,
        "place": working.name,
    })
}

/// A pyramid command line as the jobs door queues it (record 45 E1): the
/// flags each verb takes and no path, the deployment's packs added where a
/// selection is frozen.
pub(crate) fn located(pack_dir: Option<&Path>, command: Vec<String>) -> Result<Vec<String>, Reply> {
    let verb_owned = command.get(1).cloned();
    let verb = verb_owned.as_deref();
    let takes: &[&str] = match verb {
        Some("build") => &[
            "--stack",
            "--select",
            "--handle",
            "--place",
            "--workers",
            "--pack",
        ],
        Some("list") => &["--place"],
        _ => {
            return Err(Reply::error(
                400,
                "pyramid build (--stack ID | --select selection:NAME@V | --handle ID) or pyramid list",
            ));
        }
    };
    let mut out = vec!["pyramid".to_string(), verb.unwrap_or_default().to_string()];
    let mut it = command.into_iter().skip(2);
    let mut sources = 0;
    let mut select = false;
    while let Some(arg) = it.next() {
        if arg == "--json" && verb == Some("list") {
            out.push(arg);
            continue;
        }
        if !takes.contains(&arg.as_str()) {
            return Err(Reply::error(
                400,
                format!(
                    "pyramid {} takes {}, not {arg}",
                    verb.unwrap_or_default(),
                    takes.join(", ")
                ),
            ));
        }
        let value = it
            .next()
            .ok_or_else(|| Reply::error(400, format!("pyramid {arg} takes a value")))?;
        if matches!(arg.as_str(), "--stack" | "--select" | "--handle") {
            sources += 1;
        }
        select |= arg == "--select";
        out.push(arg);
        out.push(value);
    }
    if verb == Some("build") && sources != 1 {
        return Err(Reply::error(
            400,
            "pyramid build names one of --stack, --select or --handle",
        ));
    }
    if select && let Some(d) = pack_dir {
        out.push("--pack-dir".into());
        out.push(d.display().to_string());
    }
    Ok(out)
}

/// The working place a pyramid is written under: the one named, or the
/// first active working place; refused with the rule's sentence when the
/// deployment binds none.
pub fn working_place(store: &mut Store, name: Option<&str>) -> Result<Place, String> {
    let places = place::list(store).map_err(|e| e.to_string())?;
    let active = places.iter().filter(|p| p.retired_at.is_none());
    if let Some(n) = name {
        return active
            .clone()
            .find(|p| p.name == n)
            .filter(|p| p.role == PlaceRole::Working)
            .cloned()
            .ok_or_else(|| format!("{n} is not an active working place (Wave 5 section 10.2)"));
    }
    active
        .clone()
        .find(|p| p.role == PlaceRole::Working)
        .cloned()
        .ok_or_else(|| {
            "no working place is bound: the pyramid is written under a working place, which an operator adds under Settings or with nils place add (Wave 5 section 10.2)".to_string()
        })
}

/// The window the audit counts one opening in: ten minutes.
const OPEN_WINDOW_SECS: u64 = 600;

/// Who opened which stack when, in this process: the audit's own rows for
/// the window, read once per principal, and every row written since, so
/// the one-row-per-stack rule is kept exactly however many stacks a grid
/// opens (record 45; a window of the newest 200 rows let a grid of more
/// write each stack again).
#[derive(Default)]
struct Opened {
    /// When a principal's rows of the window were read in.
    warmed: std::collections::HashMap<String, u64>,
    /// When (principal, stack) was last written, in seconds.
    at: std::collections::HashMap<(String, i64), u64>,
}

static OPENED: std::sync::LazyLock<std::sync::Mutex<Opened>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Opened::default()));

/// One audit row per stack opened by a person, not per tile: the first
/// request in the window writes the row, the rest in the window do not.
fn note_open(
    registry: &mut nils_registry::Registry,
    caller: &Caller,
    stack: i64,
    through: Option<i64>,
    level: u32,
    purpose: &str,
) -> Result<(), String> {
    use nils_registry::audit::{self, Action, Entry, Filter};
    let now = nils_registry::time::now_secs();
    let since = now.saturating_sub(OPEN_WINDOW_SECS);
    let who = caller.principal.clone();
    let key = (who.clone(), stack);
    let fresh = |o: &Opened| o.at.get(&key).is_some_and(|t| *t >= since);
    let warm = {
        let o = OPENED.lock().map_err(|_| "the open register is poisoned")?;
        if fresh(&o) {
            return Ok(());
        }
        // read the window in once per principal; after that every row this
        // process writes is in the register
        !o.warmed.get(&who).is_some_and(|t| *t >= since)
    };
    if warm {
        let rows = audit::list(
            registry.store(),
            &Filter {
                principal: Some(who.clone()),
                action: Some("instance.open".to_string()),
                since: Some(nils_registry::time::iso_of(since)),
                limit: i64::MAX as usize,
            },
        )
        .map_err(|e| e.to_string())?;
        let mut o = OPENED.lock().map_err(|_| "the open register is poisoned")?;
        for r in rows {
            if let (Some(s), Some(t)) = (
                r.scope["stack"].as_i64(),
                nils_registry::time::secs_of(&r.at),
            ) {
                let e = o.at.entry((who.clone(), s)).or_insert(t);
                *e = (*e).max(t);
            }
        }
        o.warmed.insert(who.clone(), now);
        if fresh(&o) {
            return Ok(());
        }
    }
    audit::record(
        registry,
        &Entry {
            principal: &caller.principal,
            action: Action::InstanceOpen,
            scope: match through {
                Some(c) => serde_json::json!({
                    "stack": stack, "level": level, "purpose": purpose, "campaign": c,
                }),
                None => serde_json::json!({"stack": stack, "level": level, "purpose": purpose}),
            },
            policy: None,
            job_id: None,
            details: None,
        },
    )
    .map_err(|e| e.to_string())?;
    let mut o = OPENED.lock().map_err(|_| "the open register is poisoned")?;
    // what fell out of the window is forgotten now and then
    if o.at.len() > 100_000 {
        o.at.retain(|_, t| *t >= since);
    }
    o.at.insert(key, now);
    Ok(())
}

/// The manifests read by the doors, by path, kept while the file is the
/// same (its modification time and length): a grid reads each stack's
/// manifest once rather than on every tile.
type Cached = std::collections::HashMap<PathBuf, (std::time::SystemTime, u64, Manifest)>;

static MANIFESTS: std::sync::LazyLock<std::sync::Mutex<Cached>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Cached::new()));

/// [`manifest`], through the cache the doors share.
fn manifest_cached(root: &Path) -> Result<Option<Manifest>, String> {
    let p = root.join("manifest.json");
    let Ok(meta) = std::fs::metadata(&p) else {
        return Ok(None);
    };
    let stamp = (meta.modified().map_err(|e| e.to_string())?, meta.len());
    if let Ok(cache) = MANIFESTS.lock()
        && let Some((t, n, m)) = cache.get(&p)
        && (*t, *n) == stamp
    {
        return Ok(Some(m.clone()));
    }
    let m = manifest(root)?;
    if let (Some(m), Ok(mut cache)) = (&m, MANIFESTS.lock()) {
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(p, (stamp.0, stamp.1, m.clone()));
    }
    Ok(m)
}

/// The working place the doors read under, looked up at most once a second:
/// a grid's tiles do not each ask the registry which place it is.
static WORKING: std::sync::LazyLock<std::sync::Mutex<Option<(std::time::Instant, Place)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn working_place_cached(store: &mut Store) -> Result<Place, String> {
    if let Ok(w) = WORKING.lock()
        && let Some((at, p)) = w.as_ref()
        && at.elapsed() < std::time::Duration::from_secs(1)
    {
        return Ok(p.clone());
    }
    let p = working_place(store, None)?;
    if let Ok(mut w) = WORKING.lock() {
        *w = Some((std::time::Instant::now(), p.clone()));
    }
    Ok(p)
}

/// The gated instance door (Wave 5 §12.7): `manifest`, `tiles/{level}/{z}`,
/// `slab/{level}/{z0}-{z1}`, `render/{level}/{z}` under a stack.
/// `through` names the campaign a rater without query:see reads it through
/// (record 48), which the audit row keeps.
pub fn door(
    registry: &mut nils_registry::Registry,
    caller: &Caller,
    stack: i64,
    through: Option<i64>,
    rest: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Result<Reply, Reply> {
    let working = working_place_cached(registry.store()).map_err(|m| Reply::error(409, m))?;
    let root = dir(Path::new(&working.path), stack);
    let m = manifest_cached(&root)
        .map_err(|e| Reply::error(500, e))?
        .ok_or_else(|| {
            Reply::error(
                404,
                format!(
                    "no pyramid for stack {stack}; queue pyramid build --stack {stack} as a job"
                ),
            )
        })?;
    // the disclosure class: pixels are quasi-identifying, so detail quasi
    // opens them; with burned-in annotation they are identifying until the
    // band is held, so detail sensitive opens the tiles and the slab
    if caller.access.detail < Detail::Quasi {
        return Err(Reply::gated(
            403,
            format!("the pixels of stack {stack} are quasi-identifying; detail quasi opens them"),
        ));
    }
    let held = m.annotation.burned_in && caller.access.detail < Detail::Sensitive;
    let level_of = |s: &str| -> Result<u32, Reply> {
        let l: u32 = s
            .parse()
            .map_err(|_| Reply::error(400, "a level is a number from 0"))?;
        if l >= m.levels {
            return Err(Reply::error(
                404,
                format!("level {l} is not in the pyramid; it has {}", m.levels),
            ));
        }
        Ok(l)
    };
    let headers = |extra: Vec<(String, String)>| {
        let mut h = vec![
            ("X-Nils-Codec".to_string(), m.codec.clone()),
            ("X-Nils-Stack".to_string(), stack.to_string()),
            (
                "Cache-Control".to_string(),
                "private, max-age=3600".to_string(),
            ),
        ];
        h.extend(extra);
        h
    };
    match rest {
        ["manifest"] => {
            note_open(registry, caller, stack, through, 0, "manifest")
                .map_err(|e| Reply::error(500, e))?;
            let mut doc = serde_json::to_value(&m).map_err(|e| Reply::error(500, e.to_string()))?;
            doc["place"] = serde_json::json!(working.name);
            doc["held"] = serde_json::json!(held);
            Ok(Reply::ok(doc))
        }
        ["tiles", level, z] => {
            let level = level_of(level)?;
            let z: u32 = z
                .parse()
                .map_err(|_| Reply::error(400, "a plane is a number from 0"))?;
            if held {
                return Err(Reply::gated(
                    403,
                    format!(
                        "stack {stack} carries burned-in annotation; its tiles open at detail sensitive, the render holds the band"
                    ),
                ));
            }
            note_open(registry, caller, stack, through, level, "tiles")
                .map_err(|e| Reply::error(500, e))?;
            let bytes = plane_container(&root, &m, level, z).map_err(|e| Reply::error(404, e))?;
            Ok(Reply::raw(
                CONTENT_TYPE,
                bytes,
                headers(vec![("X-Nils-Planes".to_string(), "1".to_string())]),
            ))
        }
        ["slab", level, range] => {
            let level = level_of(level)?;
            let (z0, z1) = range
                .split_once('-')
                .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
                .ok_or_else(|| Reply::error(400, "a slab is z0-z1, the end exclusive"))?;
            if z1 <= z0 {
                return Err(Reply::error(400, "a slab is z0-z1 with z1 past z0"));
            }
            if z1 - z0 > SLAB_MAX {
                return Err(Reply::error(
                    416,
                    format!("a slab is at most {SLAB_MAX} planes; {} asked", z1 - z0),
                ));
            }
            if held {
                return Err(Reply::gated(
                    403,
                    format!(
                        "stack {stack} carries burned-in annotation; its slab opens at detail sensitive, the render holds the band"
                    ),
                ));
            }
            let nz = m.level_shapes[level as usize].shape[0];
            if z1 > nz {
                return Err(Reply::error(
                    416,
                    format!("the stack has {nz} planes at level {level}"),
                ));
            }
            note_open(registry, caller, stack, through, level, "slab")
                .map_err(|e| Reply::error(500, e))?;
            let bytes =
                slab_container(&root, &m, level, z0, z1).map_err(|e| Reply::error(404, e))?;
            Ok(Reply::raw(
                CONTENT_TYPE,
                bytes,
                headers(vec![("X-Nils-Planes".to_string(), (z1 - z0).to_string())]),
            ))
        }
        ["render", level, z] => {
            let level = level_of(level)?;
            let z: u32 = z
                .parse()
                .map_err(|_| Reply::error(400, "a plane is a number from 0"))?;
            let axis = query
                .get("axis")
                .and_then(|a| a.chars().next())
                .filter(|c| matches!(c, 'x' | 'y' | 'z'))
                .unwrap_or('z');
            let center = query
                .get("c")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(m.window.center);
            let width = query
                .get("w")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(m.window.width);
            note_open(registry, caller, stack, through, level, "render")
                .map_err(|e| Reply::error(500, e))?;
            let (w, h, px) =
                plane_along(&root, &m, level, axis, z).map_err(|e| Reply::error(404, e))?;
            // the band is held on every axis when the annotation is burned in and the caller is below the class
            let jpeg = render_jpeg(w, h, &px, m.slope, m.intercept, center, width, held)
                .map_err(|e| Reply::error(500, e))?;
            Ok(Reply::raw(
                "image/jpeg",
                jpeg,
                headers(vec![("X-Nils-Held".to_string(), held.to_string())]),
            ))
        }
        _ => Err(Reply::error(
            404,
            format!(
                "GET /api/instances/{stack}/{} is not a door; manifest, tiles/{{level}}/{{z}}, slab/{{level}}/{{z0}}-{{z1}} and render/{{level}}/{{z}} are",
                rest.join("/")
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_round_trips_reversibly() {
        let (w, h) = (64u32, 48u32);
        let px: Vec<u16> = (0..w * h).map(|i| ((i * 37) % 65536) as u16).collect();
        let bytes = encode_tile(&px, w, h).unwrap();
        assert!(bytes.len() > 20);
        let (dw, dh, back) = decode_tile(&bytes).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(back, px);
    }

    #[test]
    fn an_edge_tile_smaller_than_the_grid_round_trips() {
        let (w, h) = (13u32, 7u32);
        let px: Vec<u16> = (0..w * h).map(|i| (i * 1000 % 65535) as u16).collect();
        let bytes = encode_tile(&px, w, h).unwrap();
        let (_, _, back) = decode_tile(&bytes).unwrap();
        assert_eq!(back, px);
    }

    /// Record 45: a signed stack with a rescale renders in the modality's
    /// values: stored * slope + intercept, the shift into u16 undone and
    /// the file's Rescale Slope and Intercept applied, in that order.
    #[test]
    fn a_signed_rescaled_stack_renders_in_the_modality_s_values() {
        use dicom_core::VR;
        use nils_dicom::synth::{self, MetaFields, TempDir};
        let dir = TempDir::new("pyramid-signed");
        let (ny, nx) = (16u32, 16u32);
        let raws: [i16; 2] = [-100, 100];
        let mut files = Vec::new();
        for (z, raw) in raws.iter().enumerate() {
            let sop = format!("1.2.3.9.{}", z + 1);
            let us = |tag, v: u16| synth::bytes(tag, VR::US, v.to_le_bytes().to_vec());
            let mut e = synth::minimal_mr("1.2.3", "1.2.3.9", &sop);
            e.push(synth::text(
                tags::INSTANCE_NUMBER,
                VR::IS,
                &(z + 1).to_string(),
            ));
            e.push(synth::text(
                tags::IMAGE_POSITION_PATIENT,
                VR::DS,
                &format!("0\\0\\{}", z * 3),
            ));
            e.push(synth::text(
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                "1\\0\\0\\0\\1\\0",
            ));
            e.push(synth::text(tags::RESCALE_SLOPE, VR::DS, "2"));
            e.push(synth::text(tags::RESCALE_INTERCEPT, VR::DS, "-50"));
            e.push(us(tags::SAMPLES_PER_PIXEL, 1));
            e.push(us(tags::ROWS, ny as u16));
            e.push(us(tags::COLUMNS, nx as u16));
            e.push(us(tags::BITS_ALLOCATED, 16));
            e.push(us(tags::BITS_STORED, 16));
            e.push(us(tags::HIGH_BIT, 15));
            e.push(us(tags::PIXEL_REPRESENTATION, 1));
            let px: Vec<u8> = (0..ny * nx).flat_map(|_| raw.to_le_bytes()).collect();
            e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, px));
            files.push(dir.file(&sop, &synth::part10(&MetaFields::mr(&sop), &e, true)));
        }
        let vol = read_files(&files).unwrap();
        let root = dir.path().join("pyramid");
        let m = build(&vol, 9, &root, 2, None).unwrap();
        assert_eq!(m.slope, 2.0);
        assert_eq!(m.intercept, -50.0 - 32768.0 * 2.0);
        assert!(!m.rescale_varies);
        // the window is in the modality's values: -250 and 150
        assert_eq!(m.window.percentiles, [-250, 150]);
        let (w, h, plane) = decode_plane(&root, &m, 0, 0).unwrap();
        let value = plane[0] as f64 * m.slope + m.intercept;
        assert_eq!(value, -250.0);
        // a window centred on the first plane's value renders it mid-grey,
        // and one centred on the second's renders it black
        let grey = |c: f64| -> f64 {
            let jpeg = render_jpeg(w, h, &plane, m.slope, m.intercept, c, 100.0, false).unwrap();
            let img = image::load_from_memory(&jpeg).unwrap().to_luma8();
            img.pixels().map(|p| p.0[0] as f64).sum::<f64>() / (w * h) as f64
        };
        assert!((grey(-250.0) - 127.5).abs() < 2.0, "{}", grey(-250.0));
        assert!(grey(150.0) < 2.0, "{}", grey(150.0));
        // a manifest from before reads with slope one and its intercept the shift
        let old: Manifest = serde_json::from_value({
            let mut v = serde_json::to_value(&m).unwrap();
            v.as_object_mut().unwrap().remove("slope");
            v["intercept"] = serde_json::json!(-32768);
            v
        })
        .unwrap();
        assert_eq!((old.slope, old.intercept), (1.0, -32768.0));
    }

    #[test]
    fn a_window_of_no_width_renders_as_one_of_width_one() {
        // a window width of 0 (or a negative one) is floored at 1: a value
        // above the centre is white, one below black, never NaN's black
        let px = [100u16, 101, 99, 100];
        let grey = |w: f64| {
            let jpeg = render_jpeg(2, 2, &px, 1.0, 0.0, 100.0, w, false).unwrap();
            image::load_from_memory(&jpeg)
                .unwrap()
                .to_luma8()
                .into_raw()
        };
        for w in [0.0, -5.0] {
            let g = grey(w);
            assert!(g[1] > 200, "{w}: {g:?}");
            assert!(g[2] < 50, "{w}: {g:?}");
            assert_eq!(g, grey(1.0), "{w}");
        }
    }

    #[test]
    fn the_container_carries_offsets_to_every_tile() {
        let c = container(&[vec![1, 2, 3], vec![], vec![9]]);
        assert_eq!(u32::from_le_bytes(c[0..4].try_into().unwrap()), 3);
        let off =
            |i: usize| u32::from_le_bytes(c[4 + 4 * i..8 + 4 * i].try_into().unwrap()) as usize;
        assert_eq!((off(0), off(1), off(2)), (16, 19, 19));
        assert_eq!(&c[off(0)..off(1)], &[1, 2, 3]);
        assert_eq!(&c[off(2)..], &[9]);
    }

    /// The compressed fixtures (tests/fixtures/compressed, written by its
    /// make.sh from planes.txt): synthetic planes of 48 rows by 64 columns
    /// whose pixels make.py computes as [`fixture_value`] does.
    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/compressed")
            .join(format!("{name}.dcm"))
    }

    fn fixtures(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| fixture(n)).collect()
    }

    const FROWS: usize = 48;
    const FCOLS: usize = 64;

    fn fixture_value(kind: &str, x: usize, y: usize, z: usize) -> i64 {
        let wave = ((x * 37 + y * 101 + z * 613) % 4096) as i64;
        match kind {
            "u12" => wave,
            "s16" => wave - 1024,
            "s12" => wave - 2048,
            "u8" => (20 + 2 * x + 2 * y + 5 * z) as i64,
            _ => unreachable!(),
        }
    }

    /// Each stored value against the formula: the raw value, shifted by
    /// 32768 when the volume is signed. Returns the largest difference.
    fn largest_error(vol: &Volume, kind: &str, planes: &[usize]) -> i64 {
        assert_eq!(vol.shape, [planes.len() as u32, FROWS as u32, FCOLS as u32]);
        let mut worst = 0;
        for (k, &z) in planes.iter().enumerate() {
            let plane = vol.plane(k);
            for y in 0..FROWS {
                for x in 0..FCOLS {
                    let raw = plane[y * FCOLS + x] as i64 - vol.intercept;
                    worst = worst.max((raw - fixture_value(kind, x, y, z)).abs());
                }
            }
        }
        worst
    }

    #[test]
    fn a_stack_in_eight_lossless_syntaxes_reads_as_its_native_values() {
        // native explicit little endian, JPEG lossless (process 14 and its
        // first-order prediction), JPEG-LS, JPEG 2000 lossless, RLE,
        // deflated and explicit big endian, one plane each, in one stack
        let names = ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"];
        let vol = read_files(&fixtures(&names)).unwrap();
        assert_eq!(vol.intercept, 0);
        assert_eq!(largest_error(&vol, "u12", &[0, 1, 2, 3, 4, 5, 6, 7]), 0);
        assert_eq!(vol.rescale, (1.0, -1024.0));
        assert!(!vol.lossy);
        assert_eq!(vol.syntaxes.len(), 8, "{:?}", vol.syntaxes);
        let dir = nils_dicom::synth::TempDir::new("pyramid-lossless");
        let m = build(&vol, 1, dir.path(), 2, None).unwrap();
        assert!(!m.lossy);
        assert_eq!(m.source_syntaxes, vol.syntaxes);
        assert_eq!(m.intercept, -1024.0);
    }

    #[test]
    fn signed_compressed_planes_keep_their_sign() {
        // sixteen bits stored: JPEG lossless, JPEG-LS, JPEG 2000, RLE
        let vol = read_files(&fixtures(&["b0", "b1", "b2", "b3"])).unwrap();
        assert_eq!(vol.intercept, 32768);
        assert_eq!(largest_error(&vol, "s16", &[0, 1, 2, 3]), 0);
        // twelve stored in sixteen, where the JPEG codecs hand back the
        // stored bits alone and the native plane has them extended
        let vol = read_files(&fixtures(&["c0", "c1", "c2", "c3"])).unwrap();
        assert_eq!(largest_error(&vol, "s12", &[0, 1, 2, 3]), 0);
        assert!(!vol.lossy);
    }

    #[test]
    fn lossy_planes_read_close_and_mark_the_manifest_lossy() {
        // JPEG baseline, JPEG extended at eight bits, JPEG-LS near-lossless
        // and JPEG 2000 lossy
        let vol = read_files(&fixtures(&["d0", "d1", "d2", "d3"])).unwrap();
        let worst = largest_error(&vol, "u8", &[0, 1, 2, 3]);
        assert!(worst <= 64, "{worst}");
        assert!(vol.lossy);
        let dir = nils_dicom::synth::TempDir::new("pyramid-lossy");
        let m = build(&vol, 4, dir.path(), 2, None).unwrap();
        assert!(m.lossy);
        let text = std::fs::read_to_string(dir.path().join("manifest.json")).unwrap();
        let back: Manifest = serde_json::from_str(&text).unwrap();
        assert!(back.lossy);
        assert_eq!(back.source_syntaxes.len(), 4);
        // a manifest from before reads as not lossy
        let mut old = serde_json::to_value(&m).unwrap();
        old.as_object_mut().unwrap().remove("lossy");
        old.as_object_mut().unwrap().remove("source_syntaxes");
        let old: Manifest = serde_json::from_value(old).unwrap();
        assert!(!old.lossy && old.source_syntaxes.is_empty());
    }

    #[test]
    fn a_file_that_says_it_was_lossy_marks_its_stack_lossy() {
        use dicom_core::VR;
        use nils_dicom::synth::{self, MetaFields, TempDir};
        let dir = TempDir::new("pyramid-said-lossy");
        let sop = "1.2.3.8.1";
        let us = |tag, v: u16| synth::bytes(tag, VR::US, v.to_le_bytes().to_vec());
        let mut e = synth::minimal_mr("1.2.3", "1.2.3.8", sop);
        e.push(us(tags::SAMPLES_PER_PIXEL, 1));
        e.push(us(tags::ROWS, 4));
        e.push(us(tags::COLUMNS, 4));
        e.push(us(tags::BITS_ALLOCATED, 16));
        e.push(us(tags::BITS_STORED, 16));
        e.push(us(tags::HIGH_BIT, 15));
        e.push(us(tags::PIXEL_REPRESENTATION, 0));
        e.push(synth::text(tags::LOSSY_IMAGE_COMPRESSION, VR::CS, "01"));
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, vec![7; 32]));
        let f = dir.file(sop, &synth::part10(&MetaFields::mr(sop), &e, true));
        let vol = read_files(&[f]).unwrap();
        assert!(vol.lossy);
    }

    #[test]
    fn an_undecodable_plane_fails_its_stack_with_the_syntax_and_no_path() {
        let dir = nils_dicom::synth::TempDir::new("pyramid-broken");
        let mut files = Vec::new();
        for name in ["f0", "f1", "f2"] {
            let mut bytes = std::fs::read(fixture(name)).unwrap();
            if name == "f2" {
                // the codestream's start (SOC, SIZ) is gone
                let at = bytes
                    .windows(4)
                    .position(|w| w == [0xFF, 0x4F, 0xFF, 0x51])
                    .expect("a JPEG 2000 codestream");
                bytes[at..at + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
            files.push(dir.file(&format!("{name}.dcm"), &bytes));
        }
        // the two good planes read
        assert!(read_files(&files[..2]).is_ok());
        let why = read_files(&files).err().unwrap();
        assert!(why.contains("1.2.840.10008.1.2.4.90"), "{why}");
        assert!(!why.contains(dir.path().to_str().unwrap()), "{why}");
        assert_eq!(reason_of(&why, true), "undecodable");
    }

    #[test]
    fn twelve_bit_jpeg_extended_is_named_undecodable() {
        // the Rust JPEG decoder reads eight-bit DCT only; a twelve-bit
        // extended plane is counted, not guessed at
        let why = read_files(&fixtures(&["e0"])).err().unwrap();
        assert!(why.contains("1.2.840.10008.1.2.4.51"), "{why}");
        assert_eq!(reason_of(&why, true), "undecodable");
    }

    #[test]
    fn a_syntax_without_a_decoder_is_counted_as_compressed() {
        use dicom_core::VR;
        use nils_dicom::synth::{self, MetaFields, TempDir};
        let dir = TempDir::new("pyramid-no-codec");
        let sop = "1.2.3.7.1";
        let us = |tag, v: u16| synth::bytes(tag, VR::US, v.to_le_bytes().to_vec());
        let mut e = synth::minimal_mr("1.2.3", "1.2.3.7", sop);
        e.push(us(tags::SAMPLES_PER_PIXEL, 1));
        e.push(us(tags::ROWS, 4));
        e.push(us(tags::COLUMNS, 4));
        e.push(us(tags::BITS_ALLOCATED, 16));
        e.push(us(tags::BITS_STORED, 16));
        e.push(us(tags::HIGH_BIT, 15));
        e.push(us(tags::PIXEL_REPRESENTATION, 0));
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OB, vec![0; 32]));
        // JPEG XL, which the registry knows and this build has no codec for
        let meta = MetaFields::with("1.2.840.10008.1.2.4.112", "1.2.840.10008.5.1.4.1.1.4", sop);
        let f = dir.file(sop, &synth::part10(&meta, &e, true));
        let why = read_files(&[f]).err().unwrap();
        assert!(why.contains("1.2.840.10008.1.2.4.112"), "{why}");
        assert_eq!(reason_of(&why, true), "compressed");
    }

    fn multiframe(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multiframe")
            .join(format!("{name}.dcm"))
    }

    /// The largest difference between a volume's planes and the planes of
    /// the multi-frame fixtures' formula they should be, in order.
    fn off_by(vol: &Volume, planes: &[usize]) -> i64 {
        assert_eq!(vol.shape, [planes.len() as u32, FROWS as u32, FCOLS as u32]);
        let mut worst = 0;
        for (z, &p) in planes.iter().enumerate() {
            let plane = vol.plane(z);
            for y in 0..FROWS {
                for x in 0..FCOLS {
                    let got = plane[y * FCOLS + x] as i64 - vol.intercept;
                    worst = worst.max((got - fixture_value("u12", x, y, p)).abs());
                }
            }
        }
        worst
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6)
    }

    #[test]
    fn an_enhanced_object_s_frames_are_its_planes_in_every_syntax() {
        // native, JPEG 2000, JPEG-LS, RLE and JPEG lossless, five frames
        // each, their geometry and rescale in the shared functional groups
        for name in ["g0", "g1", "g2", "g3", "g4"] {
            let vol = read_files(&[multiframe(name)]).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(off_by(&vol, &[0, 1, 2, 3, 4]), 0, "{name}");
            assert_eq!(vol.order, ORDER_POSITION, "{name}");
            assert_eq!(vol.multiframe_files, 1, "{name}");
            assert!(
                close(vol.spacing, [2.0, 0.8, 0.8]),
                "{name}: {:?}",
                vol.spacing
            );
            assert_eq!(vol.rescale, (1.0, -1024.0), "{name}");
            let g = vol.geometry.expect("the frames say where they are");
            assert_eq!(g.orientation, AXIAL, "{name}");
            assert!(close(g.origin, [0.0, 0.0, 0.0]), "{name}");
            assert!(g.frame.parallel && g.frame.evenly_spaced, "{name}");
        }
        let dir = nils_dicom::synth::TempDir::new("pyramid-enhanced");
        let vol = read_files(&[multiframe("g1")]).unwrap();
        let m = build(&vol, 11, dir.path(), 2, None).unwrap();
        assert_eq!(m.shape, [5, 48, 64]);
        assert_eq!(m.order.as_deref(), Some(ORDER_POSITION));
        assert_eq!(m.multiframe_files, 1);
        assert!(m.orientation_known);
        // a manifest from before says neither
        let mut old = serde_json::to_value(&m).unwrap();
        old.as_object_mut().unwrap().remove("order");
        old.as_object_mut().unwrap().remove("multiframe_files");
        let old: Manifest = serde_json::from_value(old).unwrap();
        assert!(old.order.is_none() && old.multiframe_files == 0);
    }

    #[test]
    fn frames_whose_positions_run_out_of_order_are_ordered_along_the_normal() {
        // frame k holds plane [3, 0, 4, 1, 2][k] at its place; the rescale
        // is in each frame's own functional groups
        let vol = read_files(&[multiframe("h0")]).unwrap();
        assert_eq!(off_by(&vol, &[0, 1, 2, 3, 4]), 0);
        assert_eq!(vol.order, ORDER_POSITION);
        assert_eq!(vol.rescale, (1.0, -1024.0));
        assert!(!vol.rescale_varies);
        assert!(close(vol.spacing, [2.0, 0.8, 0.8]));
        assert!(vol.geometry.unwrap().frame.evenly_spaced);
    }

    #[test]
    fn a_classic_multi_frame_object_is_stacked_in_frame_order_and_says_so() {
        // four frames and one position for the file: the frames in order,
        // Spacing Between Slices apart, and no claim of where they are
        let vol = read_files(&[multiframe("i0")]).unwrap();
        assert_eq!(off_by(&vol, &[0, 1, 2, 3]), 0);
        assert_eq!(vol.order, ORDER_FRAMES);
        assert!(vol.geometry.is_none());
        assert!(close(vol.spacing, [3.0, 0.8, 0.8]), "{:?}", vol.spacing);
        assert_eq!(vol.rescale, (1.0, -1024.0));
        let dir = nils_dicom::synth::TempDir::new("pyramid-classic-frames");
        let m = build(&vol, 17, dir.path(), 2, None).unwrap();
        assert_eq!(m.order.as_deref(), Some(ORDER_FRAMES));
        assert!(!m.orientation_known && m.frame.is_none());
    }

    #[test]
    fn one_stack_of_several_multi_frame_files_and_a_single_frame_one() {
        // two enhanced files, one of them JPEG 2000, and a classic plane,
        // handed over in any order
        let files: Vec<PathBuf> = ["j2", "j1", "j0"].iter().map(|n| multiframe(n)).collect();
        let vol = read_files(&files).unwrap();
        assert_eq!(off_by(&vol, &[0, 1, 2, 3, 4, 5, 6]), 0);
        assert_eq!(vol.order, ORDER_POSITION);
        assert_eq!(vol.multiframe_files, 2);
        assert_eq!(vol.syntaxes.len(), 2, "{:?}", vol.syntaxes);
        assert!(close(vol.spacing, [2.0, 0.8, 0.8]));
        assert!(vol.geometry.unwrap().frame.evenly_spaced);
    }

    #[test]
    fn a_file_split_between_stacks_gives_each_stack_its_frames() {
        // frames 1-3 are axial planes 0-2, frames 4-6 sagittal planes 10-12
        let part = |frames: Vec<u32>| StackFile {
            path: multiframe("k0"),
            frames: Some(frames),
        };
        let axial = read_stack(&[part(frame_list("1-3").unwrap())]).unwrap();
        assert_eq!(off_by(&axial, &[0, 1, 2]), 0);
        assert_eq!(axial.geometry.unwrap().orientation, AXIAL);
        let sagittal = read_stack(&[part(frame_list("4-6").unwrap())]).unwrap();
        // along the sagittal normal, which runs against x
        assert_eq!(off_by(&sagittal, &[12, 11, 10]), 0);
        let g = sagittal.geometry.unwrap();
        assert_eq!(plane_of(&g.orientation).0, "sagittal");
        assert!(g.frame.parallel && g.frame.evenly_spaced);
        // the whole file is six planes that are not one volume
        let whole = read_files(&[multiframe("k0")]).unwrap();
        assert_eq!(whole.shape[0], 6);
        assert!(!whole.geometry.unwrap().frame.parallel);
        // a frame the file does not hold fails the stack without a path
        let why = read_stack(&[part(vec![7])]).err().unwrap();
        assert!(!why.contains("multiframe"), "{why}");
        assert_eq!(reason_of(&why, true), "unsupported_pixels");
    }

    #[test]
    fn frame_lists_read_as_the_digest_writes_them() {
        assert_eq!(frame_list("1-4,9,12-13").unwrap(), [1, 2, 3, 4, 9, 12, 13]);
        assert_eq!(frame_list("5").unwrap(), [5]);
        for bad in ["", "0", "4-2", "a-3", "1-"] {
            assert!(frame_list(bad).is_err(), "{bad}");
        }
    }

    /// A copy of a fixture whose every frame is split over two fragments,
    /// with a Basic Offset Table or without one.
    fn fragmented(dir: &nils_dicom::synth::TempDir, name: &str, table: bool) -> PathBuf {
        let mut file = dicom_object::open_file(multiframe(name)).unwrap();
        let seq = match file.element(tags::PIXEL_DATA).unwrap().value() {
            dicom_core::DicomValue::PixelSequence(seq) => seq.clone(),
            _ => panic!("{name} is encapsulated"),
        };
        let (mut fragments, mut offsets) = (Vec::new(), Vec::new());
        let mut at = 0u32;
        for f in seq.fragments() {
            offsets.push(at);
            let half = (f.len() / 4) * 2;
            for piece in [&f[..half], &f[half..]] {
                at += piece.len() as u32 + 8;
                fragments.push(piece.to_vec());
            }
        }
        if !table {
            offsets.clear();
        }
        file.put(dicom_core::DataElement::new(
            tags::PIXEL_DATA,
            dicom_core::VR::OB,
            dicom_core::DicomValue::from(dicom_core::value::PixelFragmentSequence::new(
                offsets, fragments,
            )),
        ));
        let out = dir.path().join(format!(
            "{name}-{}.dcm",
            if table { "table" } else { "bare" }
        ));
        file.write_to_file(&out).unwrap();
        out
    }

    #[test]
    fn frames_spread_over_fragments_are_gathered_by_table_or_by_marker() {
        let dir = nils_dicom::synth::TempDir::new("pyramid-fragments");
        // JPEG-LS by its start of image, JPEG 2000 by its start of codestream
        for name in ["g2", "g1"] {
            for table in [true, false] {
                let f = fragmented(&dir, name, table);
                let vol = read_files(&[f]).unwrap_or_else(|e| panic!("{name} {table}: {e}"));
                assert_eq!(off_by(&vol, &[0, 1, 2, 3, 4]), 0, "{name} {table}");
            }
        }
        // RLE has one fragment a frame by the standard, and more cannot be
        // told apart without a table
        let f = fragmented(&dir, "g3", false);
        let why = read_files(&[f]).err().unwrap();
        assert_eq!(reason_of(&why, true), "undecodable", "{why}");
        assert!(!why.contains(dir.path().to_str().unwrap()), "{why}");
        let f = fragmented(&dir, "g3", true);
        assert!(read_files(&[f]).is_ok());
    }

    #[test]
    fn every_failure_is_counted_by_its_reason() {
        let many = Many {
            failed: (0..30)
                .map(|s| (s, if s % 3 == 0 { "compressed" } else { "no_files" }))
                .collect(),
            ..Many::default()
        };
        let j = many.as_json("scratch");
        assert_eq!(j["failed"], 30);
        assert_eq!(j["failures"].as_array().unwrap().len(), 20);
        assert_eq!(
            j["failures_by_reason"],
            serde_json::json!({"compressed": 10, "no_files": 20})
        );
    }
}
