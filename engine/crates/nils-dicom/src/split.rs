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
    if let Ok(e) = object.element(tags::PIXEL_DATA) {
        let vr = e.vr();
        let kept = match e.value() {
            DicomValue::Primitive(PrimitiveValue::U8(bytes)) => {
                let each = per_frame(bytes.len(), count)?;
                let mut out = Vec::with_capacity(each * frames.len());
                for i in picked(frames.len()) {
                    out.extend_from_slice(&bytes[i * each..(i + 1) * each]);
                }
                DicomValue::Primitive(PrimitiveValue::U8(out.into()))
            }
            DicomValue::Primitive(PrimitiveValue::U16(words)) => {
                let each = per_frame(words.len(), count)?;
                let mut out = Vec::with_capacity(each * frames.len());
                for i in picked(frames.len()) {
                    out.extend_from_slice(&words[i * each..(i + 1) * each]);
                }
                DicomValue::Primitive(PrimitiveValue::U16(out.into()))
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
                let mut at = 0u64;
                for i in picked(frames.len()) {
                    table.push(u32::try_from(at).map_err(|_| "the pixel data is too large")?);
                    for f in &fragments[ranges[i].clone()] {
                        at += f.len() as u64 + 8;
                        out.push(f.clone());
                    }
                }
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
    object.put(DataElement::new(
        tags::NUMBER_OF_FRAMES,
        VR::IS,
        PrimitiveValue::from(frames.len().to_string()),
    ));
    Ok(())
}

/// How many values one frame holds of `total` over `frames`.
fn per_frame(total: usize, frames: u32) -> Result<usize, String> {
    let frames = frames as usize;
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
}
