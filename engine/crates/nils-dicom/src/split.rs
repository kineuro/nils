// SPDX-License-Identifier: AGPL-3.0-only

//! Some frames of an enhanced multi-frame object kept, the rest let go
//! (#463).
//!
//! The digest makes one stack of each run of frames an enhanced object holds
//! that carry the same stack-level values ([`crate::frames`]), and writes
//! which frames are whose as ranges (`1-4,9,12-20`). A reader of one such
//! stack reads its frames; a release writes it, and a release writes files.
//! So the object is written with that stack's frames alone: its per-frame
//! functional groups, its pixel data (native, a frame's bytes each, or
//! encapsulated, a frame's fragments each) and its frame count. Everything
//! else stays as it is; what the object is called is the writer's to say,
//! since the object it was is no longer the one written.

use std::ops::Range;

use dicom_core::value::{DataSetSequence, PixelFragmentSequence, PrimitiveValue};
use dicom_core::{DataElement, DicomValue, VR};
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;

/// The frames a range list names, counting from one, as the digest writes
/// them: `1-4,9,12-20`.
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

/// The run-length syntax, whose fragments never start with a marker.
const RLE: &str = "1.2.840.10008.1.2.5";

/// Which fragments of an encapsulated object are which frame: one each when
/// there are as many as frames, else by the Basic Offset Table, else by
/// where a codestream starts (a JPEG or JPEG-LS start of image, a JPEG
/// 2000 start of codestream). A frame is a range of fragment indices. The
/// words of a refusal name no file.
pub fn fragment_ranges(
    fragments: &[Vec<u8>],
    table: &[u32],
    frames: usize,
    ts: &str,
) -> Result<Vec<Range<usize>>, String> {
    if frames <= 1 {
        return Ok(std::iter::once(0..fragments.len()).collect());
    }
    if fragments.len() == frames {
        return Ok((0..frames).map(|i| i..i + 1).collect());
    }
    let apart = || {
        format!(
            "{} fragments could not be told apart into {frames} frames",
            fragments.len()
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

/// Keep `frames` (counting from one, in the order given) of the object and
/// let the others go: the per-frame functional groups, the pixel data and
/// NumberOfFrames. `ts` is the object's transfer syntax. An object with no
/// pixel data keeps its groups alone. A frame the object does not hold,
/// pixel data that does not divide into its frames, or fragments that
/// cannot be told apart, is refused in words that name no file.
pub fn keep_frames(object: &mut InMemDicomObject, frames: &[u32], ts: &str) -> Result<(), String> {
    let count: u32 = object
        .element(tags::NUMBER_OF_FRAMES)
        .ok()
        .and_then(|e| e.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
        .ok_or("the object states no NumberOfFrames")?;
    if frames.is_empty() || frames.iter().any(|&f| f == 0 || f > count) {
        return Err(format!(
            "the frames kept are not frames of an object of {count}"
        ));
    }
    let picked =
        |n: usize| -> Vec<usize> { frames.iter().map(|&f| (f - 1) as usize).take(n).collect() };

    // the per-frame functional groups, one item a frame
    if let Ok(e) = object.element(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE) {
        let DicomValue::Sequence(seq) = e.value() else {
            return Err("the per-frame functional groups are not a sequence".into());
        };
        let items = seq.items();
        if items.len() != count as usize {
            return Err(format!(
                "{} per-frame functional groups for {count} frames",
                items.len()
            ));
        }
        let kept: Vec<InMemDicomObject> = picked(frames.len())
            .into_iter()
            .map(|i| items[i].clone())
            .collect();
        object.put(DataElement::new(
            tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
            VR::SQ,
            DicomValue::from(DataSetSequence::from(kept)),
        ));
    }

    // the pixel data, a frame's worth each
    let geometry = Geometry::of(object);
    let mut one_fragment_frames: Option<Vec<(u64, u64)>> = None;
    if let Ok(e) = object.element(tags::PIXEL_DATA) {
        let vr = e.vr();
        let kept = match e.value() {
            DicomValue::Primitive(PrimitiveValue::U8(bytes)) => {
                let out = split_bytes(bytes, geometry, count, &picked(frames.len()), true)?;
                // a value of an odd length is padded to an even one
                let mut out = out;
                if out.len() % 2 == 1 {
                    out.push(0);
                }
                DicomValue::Primitive(PrimitiveValue::U8(out.into()))
            }
            DicomValue::Primitive(PrimitiveValue::U16(words)) => {
                // 8-bit and 1-bit data stored as OW arrive as words, but
                // its frames are bytes (or bits): split them as the OB
                // path does, then pack the kept bytes into words again
                let stated_bits = object
                    .element(tags::BITS_ALLOCATED)
                    .ok()
                    .and_then(|e| e.to_int::<u32>().ok());
                if matches!(stated_bits, Some(1 | 8)) {
                    if geometry.is_none() {
                        return Err("pixel data of 8-bit or 1-bit samples stored as words \
                             cannot be split without rows, columns and bits allocated"
                            .into());
                    }
                    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
                    let mut out =
                        split_bytes(&bytes, geometry, count, &picked(frames.len()), false)?;
                    if out.len() % 2 == 1 {
                        out.push(0);
                    }
                    let packed: Vec<u16> = out
                        .chunks(2)
                        .map(|p| u16::from_le_bytes([p[0], p[1]]))
                        .collect();
                    DicomValue::Primitive(PrimitiveValue::U16(packed.into()))
                } else {
                    let each = match geometry.filter(|g| g.bits_allocated == 16) {
                        Some(g) => g.pixels(),
                        None => per_frame(words.len(), count)?,
                    };
                    fits(words.len(), each, count)?;
                    let mut out = Vec::with_capacity(each * frames.len());
                    for i in picked(frames.len()) {
                        out.extend_from_slice(&words[i * each..(i + 1) * each]);
                    }
                    DicomValue::Primitive(PrimitiveValue::U16(out.into()))
                }
            }
            DicomValue::Primitive(PrimitiveValue::Empty) => {
                DicomValue::Primitive(PrimitiveValue::Empty)
            }
            DicomValue::Primitive(_) => {
                return Err("pixel data of a kind whose frames cannot be told apart".into());
            }
            DicomValue::PixelSequence(seq) => {
                let fragments = seq.fragments();
                let ranges = fragment_ranges(fragments, seq.offset_table(), count as usize, ts)?;
                let mut out: Vec<Vec<u8>> = Vec::new();
                let mut table: Vec<u32> = Vec::new();
                let mut extended: Option<Vec<(u64, u64)>> = Some(Vec::new());
                let mut at = 0u64;
                for i in picked(frames.len()) {
                    table.push(u32::try_from(at).map_err(|_| "the pixel data is too large")?);
                    let range = ranges[i].clone();
                    if range.len() == 1 {
                        if let Some(x) = extended.as_mut() {
                            x.push((at, fragments[range.start].len() as u64));
                        }
                    } else {
                        extended = None;
                    }
                    for f in &fragments[range] {
                        at += f.len() as u64 + 8;
                        out.push(f.clone());
                    }
                }
                one_fragment_frames = extended;
                // a table is written where the object had one
                if seq.offset_table().is_empty() {
                    table.clear();
                }
                DicomValue::PixelSequence(PixelFragmentSequence::new(table, out))
            }
            DicomValue::Sequence(_) => return Err("the pixel data is a sequence".into()),
        };
        object.put(DataElement::new(tags::PIXEL_DATA, vr, kept));
    }
    // the Extended Offset Table and its lengths name every frame of the
    // object: written again for the frames kept, one fragment each, else
    // let go, the Basic Offset Table or the fragments' own starts telling
    // a reader where a frame begins
    if object.element(tags::EXTENDED_OFFSET_TABLE).is_ok()
        || object.element(tags::EXTENDED_OFFSET_TABLE_LENGTHS).is_ok()
    {
        match one_fragment_frames {
            Some(x) if !x.is_empty() => {
                let (offsets, lengths): (Vec<u64>, Vec<u64>) = x.into_iter().unzip();
                object.put(DataElement::new(
                    tags::EXTENDED_OFFSET_TABLE,
                    VR::OV,
                    PrimitiveValue::U64(offsets.into()),
                ));
                object.put(DataElement::new(
                    tags::EXTENDED_OFFSET_TABLE_LENGTHS,
                    VR::OV,
                    PrimitiveValue::U64(lengths.into()),
                ));
            }
            _ => {
                object.remove_element(tags::EXTENDED_OFFSET_TABLE);
                object.remove_element(tags::EXTENDED_OFFSET_TABLE_LENGTHS);
            }
        }
    }
    object.put(DataElement::new(
        tags::NUMBER_OF_FRAMES,
        VR::IS,
        PrimitiveValue::from(frames.len().to_string()),
    ));
    Ok(())
}

/// What one frame of native pixel data is, from the object's own words:
/// rows, columns, samples per pixel and bits allocated.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    rows: usize,
    columns: usize,
    samples: usize,
    bits_allocated: usize,
}

impl Geometry {
    fn of(object: &InMemDicomObject) -> Option<Geometry> {
        let n = |tag| -> Option<usize> {
            object
                .element(tag)
                .ok()?
                .to_int::<u32>()
                .ok()
                .map(|v| v as usize)
        };
        let g = Geometry {
            rows: n(tags::ROWS)?,
            columns: n(tags::COLUMNS)?,
            samples: n(tags::SAMPLES_PER_PIXEL).unwrap_or(1),
            bits_allocated: n(tags::BITS_ALLOCATED)?,
        };
        (g.rows > 0 && g.columns > 0 && g.samples > 0 && g.bits_allocated > 0).then_some(g)
    }

    /// The samples of one frame.
    fn pixels(&self) -> usize {
        self.rows * self.columns * self.samples
    }

    /// The bytes of one frame, bits allocated a whole number of bytes.
    fn bytes(&self) -> usize {
        self.pixels() * self.bits_allocated.div_ceil(8)
    }
}

/// The bytes of the `picked` frames (zero-based) of native pixel data of
/// bytes: whole frames of the object's geometry, or, for 1-bit data, bits
/// packed end to end and packed again from the first bit. `bare_ok` lets an
/// object without geometry be split by its length (the OB path).
fn split_bytes(
    bytes: &[u8],
    geometry: Option<Geometry>,
    count: u32,
    picked: &[usize],
    bare_ok: bool,
) -> Result<Vec<u8>, String> {
    match geometry.filter(|g| g.bits_allocated == 1) {
        // 1-bit data packs its frames end to end in bits, a frame not
        // starting on a byte
        Some(g) => {
            let each = g.pixels();
            if bytes.len() * 8 < each * count as usize {
                return Err(format!(
                    "1-bit pixel data of {} bytes is short of {count} frames",
                    bytes.len()
                ));
            }
            let mut out = vec![0u8; (each * picked.len()).div_ceil(8)];
            let mut to = 0usize;
            for &i in picked {
                for b in i * each..(i + 1) * each {
                    if bytes[b / 8] >> (b % 8) & 1 == 1 {
                        out[to / 8] |= 1 << (to % 8);
                    }
                    to += 1;
                }
            }
            Ok(out)
        }
        None => {
            let each = match geometry {
                Some(g) => g.bytes(),
                None if bare_ok => per_frame(bytes.len(), count)?,
                None => return Err("the frame geometry is missing".into()),
            };
            fits(bytes.len(), each, count)?;
            let mut out = Vec::with_capacity(each * picked.len() + 1);
            for &i in picked {
                out.extend_from_slice(&bytes[i * each..(i + 1) * each]);
            }
            Ok(out)
        }
    }
}

/// Whether `total` values hold `frames` frames of `each`: exactly, or with
/// the one byte that pads a value of an odd length to an even one.
fn fits(total: usize, each: usize, frames: u32) -> Result<(), String> {
    let need = each * frames as usize;
    if total == need || total == need + 1 {
        return Ok(());
    }
    Err(format!(
        "pixel data of {total} values does not hold {frames} frames of {each}"
    ))
}

/// How many values one frame holds of `total` over `frames`.
fn per_frame(total: usize, frames: u32) -> Result<usize, String> {
    let frames = frames as usize;
    // a value of an odd length carries one byte of padding
    if frames > 0 && !total.is_multiple_of(frames) && (total - 1).is_multiple_of(frames) {
        return Ok((total - 1) / frames);
    }
    if frames == 0 || !total.is_multiple_of(frames) {
        return Err(format!(
            "pixel data of {total} values does not divide into {frames} frames"
        ));
    }
    Ok(total / frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    fn object(
        frames: usize,
        pixels: Option<DicomValue<InMemDicomObject, Vec<u8>>>,
    ) -> InMemDicomObject {
        let per_frame: Vec<Vec<synth::Elem>> = (0..frames)
            .map(|i| {
                vec![synth::fg_orientation(if i < frames / 2 {
                    "1\\0\\0\\0\\1\\0"
                } else {
                    "0\\1\\0\\0\\0\\-1"
                })]
            })
            .collect();
        let elems = synth::enhanced_mr("1.2.3", "1.2.3.1", "1.2.3.1.1", Vec::new(), per_frame);
        let dir = synth::TempDir::new("split");
        let path = dir.file(
            "f.dcm",
            &synth::part10(&synth::enhanced_meta("1.2.3.1.1"), &elems, true),
        );
        let mut o = dicom_object::open_file(&path)
            .expect("the fixture reads")
            .into_inner();
        if let Some(p) = pixels {
            o.put(DataElement::new(tags::PIXEL_DATA, VR::OW, p));
        }
        o
    }

    fn orientations(o: &InMemDicomObject) -> Vec<String> {
        let DicomValue::Sequence(seq) = o
            .element(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
            .unwrap()
            .value()
            .clone()
        else {
            panic!("a sequence");
        };
        seq.items()
            .iter()
            .map(|item| {
                let DicomValue::Sequence(po) = item
                    .element(tags::PLANE_ORIENTATION_SEQUENCE)
                    .unwrap()
                    .value()
                    .clone()
                else {
                    panic!("a sequence");
                };
                po.items()[0]
                    .element(tags::IMAGE_ORIENTATION_PATIENT)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn frame_lists_read_as_the_digest_writes_them() {
        assert_eq!(frame_list("1-4,9,12-13").unwrap(), [1, 2, 3, 4, 9, 12, 13]);
        assert_eq!(frame_list("5").unwrap(), [5]);
        for bad in ["", "0", "4-2", "a-3", "1-"] {
            assert!(frame_list(bad).is_err(), "{bad}");
        }
    }

    /// The frames of the second stack of a file of two keep their groups
    /// and their pixels, and the count says how many.
    #[test]
    fn a_stack_s_frames_keep_their_groups_and_their_pixels() {
        // four frames of two by two 16-bit pixels, each frame its number
        let words: Vec<u16> = (1..=4u16).flat_map(|f| [f; 4]).collect();
        let mut o = object(
            4,
            Some(DicomValue::Primitive(PrimitiveValue::U16(words.into()))),
        );
        keep_frames(&mut o, &[3, 4], "1.2.840.10008.1.2.1").unwrap();
        assert_eq!(
            o.element(tags::NUMBER_OF_FRAMES).unwrap().to_str().unwrap(),
            "2"
        );
        assert_eq!(orientations(&o), ["0\\1\\0\\0\\0\\-1", "0\\1\\0\\0\\0\\-1"]);
        let DicomValue::Primitive(PrimitiveValue::U16(kept)) =
            o.element(tags::PIXEL_DATA).unwrap().value().clone()
        else {
            panic!("16-bit pixels");
        };
        assert_eq!(kept.to_vec(), [3, 3, 3, 3, 4, 4, 4, 4]);
    }

    /// An encapsulated object keeps each kept frame's fragments, with a
    /// table where it had one; a frame it does not hold is refused.
    #[test]
    fn an_encapsulated_stack_keeps_its_fragments() {
        let fragments: Vec<Vec<u8>> = (1..=4u8).map(|f| vec![0xFF, 0xD8, f, f]).collect();
        let table = vec![0u32, 12, 24, 36];
        let seq = PixelFragmentSequence::new(table, fragments);
        let mut o = object(4, Some(DicomValue::PixelSequence(seq)));
        keep_frames(&mut o, &[1, 2], "1.2.840.10008.1.2.4.50").unwrap();
        let DicomValue::PixelSequence(kept) = o.element(tags::PIXEL_DATA).unwrap().value().clone()
        else {
            panic!("encapsulated");
        };
        assert_eq!(
            kept.fragments(),
            [vec![0xFF, 0xD8, 1, 1], vec![0xFF, 0xD8, 2, 2]]
        );
        assert_eq!(kept.offset_table(), [0, 12]);
        assert_eq!(orientations(&o).len(), 2);
        let mut again = object(4, None);
        assert!(keep_frames(&mut again, &[5], "1.2.840.10008.1.2.1").is_err());
        assert!(keep_frames(&mut again, &[], "1.2.840.10008.1.2.1").is_err());
        // no pixels: the groups and the count alone
        keep_frames(&mut again, &[1], "1.2.840.10008.1.2.1").unwrap();
        assert_eq!(orientations(&again), ["1\\0\\0\\0\\1\\0"]);
    }

    fn with_geometry(o: &mut InMemDicomObject, rows: u16, columns: u16, bits: u16) {
        for (tag, v) in [
            (tags::ROWS, rows),
            (tags::COLUMNS, columns),
            (tags::BITS_ALLOCATED, bits),
            (tags::SAMPLES_PER_PIXEL, 1),
        ] {
            o.put(DataElement::new(tag, VR::US, PrimitiveValue::from(v)));
        }
    }

    fn bytes_of(o: &InMemDicomObject) -> Vec<u8> {
        o.element(tags::PIXEL_DATA)
            .unwrap()
            .to_bytes()
            .unwrap()
            .to_vec()
    }

    /// 8-bit native data of an odd length carries a pad byte, which is no
    /// frame's; what is kept is padded again to an even length.
    #[test]
    fn eight_bit_data_with_its_pad_byte_keeps_its_frames() {
        // three frames of one row of three, each frame its number, a pad
        let mut bytes: Vec<u8> = (1..=3u8).flat_map(|f| [f; 3]).collect();
        bytes.push(0);
        let mut o = object(
            3,
            Some(DicomValue::Primitive(PrimitiveValue::U8(bytes.into()))),
        );
        with_geometry(&mut o, 1, 3, 8);
        keep_frames(&mut o, &[2], "1.2.840.10008.1.2.1").unwrap();
        assert_eq!(bytes_of(&o), [2, 2, 2, 0]);
        // without the object's geometry the pad is told apart too
        let mut bytes: Vec<u8> = (1..=3u8).flat_map(|f| [f; 3]).collect();
        bytes.push(0);
        let mut bare = object(
            3,
            Some(DicomValue::Primitive(PrimitiveValue::U8(bytes.into()))),
        );
        keep_frames(&mut bare, &[3], "1.2.840.10008.1.2.1").unwrap();
        assert_eq!(bytes_of(&bare), [3, 3, 3, 0]);
    }

    /// 1-bit data packs its frames end to end in bits; the frames kept are
    /// packed again from the first bit.
    #[test]
    fn one_bit_data_keeps_its_frames_bit_by_bit() {
        // three frames of two by two: 1111, 0000, 1010 from the first bit
        let bits = [1u8, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 0];
        let mut packed = vec![0u8; 2];
        for (i, b) in bits.iter().enumerate() {
            packed[i / 8] |= b << (i % 8);
        }
        let mut o = object(
            3,
            Some(DicomValue::Primitive(PrimitiveValue::U8(packed.into()))),
        );
        with_geometry(&mut o, 2, 2, 1);
        keep_frames(&mut o, &[1, 3], "1.2.840.10008.1.2.1").unwrap();
        // 1111 then 1010: 0b0101_1111, padded to two bytes
        assert_eq!(bytes_of(&o), [0b0101_1111, 0]);
    }

    fn words_of(bytes: &[u8]) -> Vec<u16> {
        bytes
            .chunks(2)
            .map(|p| u16::from_le_bytes([p[0], *p.get(1).unwrap_or(&0)]))
            .collect()
    }

    fn ow(bytes: &[u8]) -> Option<DicomValue<InMemDicomObject, Vec<u8>>> {
        Some(DicomValue::Primitive(PrimitiveValue::U16(
            words_of(bytes).into(),
        )))
    }

    /// 8-bit data stored as OW arrives as words; its frames are still
    /// bytes, odd sizes included.
    #[test]
    fn eight_bit_data_stored_as_words_splits_by_bytes() {
        // two frames of three bytes: three words, no pad rule
        let mut o = object(2, ow(&[1, 1, 1, 2, 2, 2]));
        with_geometry(&mut o, 1, 3, 8);
        keep_frames(&mut o, &[2], "1.2.840.10008.1.2.1").unwrap();
        assert_eq!(bytes_of(&o), [2, 2, 2, 0]);
        // three frames of three bytes and a pad byte
        let mut o = object(3, ow(&[1, 1, 1, 2, 2, 2, 3, 3, 3, 0]));
        with_geometry(&mut o, 1, 3, 8);
        keep_frames(&mut o, &[3, 1], "1.2.840.10008.1.2.1").unwrap();
        assert_eq!(bytes_of(&o), [3, 3, 3, 1, 1, 1]);
        // a length that is not the geometry's is refused
        let mut o = object(2, ow(&[1, 1, 1, 2, 2, 2, 9, 9]));
        with_geometry(&mut o, 1, 3, 8);
        assert!(keep_frames(&mut o, &[1], "1.2.840.10008.1.2.1").is_err());
    }

    /// 1-bit data stored as OW is split by bits, as the OB path does.
    #[test]
    fn one_bit_data_stored_as_words_splits_by_bits() {
        // three frames of three by three: 27 bits, four bytes, two words
        let mut bits = [0u8; 27];
        bits[..9].fill(1);
        bits[18..].fill(1);
        let mut packed = vec![0u8; 4];
        for (i, b) in bits.iter().enumerate() {
            packed[i / 8] |= b << (i % 8);
        }
        let mut o = object(3, ow(&packed));
        with_geometry(&mut o, 3, 3, 1);
        keep_frames(&mut o, &[2, 3], "1.2.840.10008.1.2.1").unwrap();
        // 000000000 then 111111111: bits 9..18 set
        assert_eq!(bytes_of(&o), [0, 0b1111_1110, 0b11, 0]);
    }

    /// Words of 8-bit data with no geometry to split them by are refused.
    #[test]
    fn eight_bit_words_without_geometry_are_refused() {
        let mut o = object(2, ow(&[1, 1, 1, 2, 2, 2]));
        o.put(DataElement::new(
            tags::BITS_ALLOCATED,
            VR::US,
            PrimitiveValue::from(8u16),
        ));
        let err = keep_frames(&mut o, &[1], "1.2.840.10008.1.2.1").unwrap_err();
        assert!(err.contains("cannot be split"), "{err}");
    }

    /// The Extended Offset Table and its lengths are written again for the
    /// frames kept, one fragment each, and let go where a frame is more.
    #[test]
    fn the_extended_offset_table_follows_the_frames_kept() {
        let fragments: Vec<Vec<u8>> = (1..=4u8).map(|f| vec![0xFF, 0xD8, f, f, f, f]).collect();
        let seq = PixelFragmentSequence::new(Vec::<u32>::new(), fragments);
        let mut o = object(4, Some(DicomValue::PixelSequence(seq)));
        let eot = |o: &mut InMemDicomObject| {
            o.put(DataElement::new(
                tags::EXTENDED_OFFSET_TABLE,
                VR::OV,
                PrimitiveValue::U64(vec![0u64, 14, 28, 42].into()),
            ));
            o.put(DataElement::new(
                tags::EXTENDED_OFFSET_TABLE_LENGTHS,
                VR::OV,
                PrimitiveValue::U64(vec![6u64; 4].into()),
            ));
        };
        eot(&mut o);
        keep_frames(&mut o, &[2, 4], "1.2.840.10008.1.2.4.50").unwrap();
        let u64s = |o: &InMemDicomObject, tag| -> Vec<u64> {
            match o.element(tag).unwrap().value() {
                DicomValue::Primitive(PrimitiveValue::U64(v)) => v.to_vec(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(u64s(&o, tags::EXTENDED_OFFSET_TABLE), [0, 14]);
        assert_eq!(u64s(&o, tags::EXTENDED_OFFSET_TABLE_LENGTHS), [6, 6]);
        // native data has no fragments: the tables go
        let words: Vec<u16> = (1..=4u16).flat_map(|f| [f; 4]).collect();
        let mut native = object(
            4,
            Some(DicomValue::Primitive(PrimitiveValue::U16(words.into()))),
        );
        eot(&mut native);
        keep_frames(&mut native, &[1], "1.2.840.10008.1.2.1").unwrap();
        assert!(native.element(tags::EXTENDED_OFFSET_TABLE).is_err());
        assert!(native.element(tags::EXTENDED_OFFSET_TABLE_LENGTHS).is_err());
    }
}
