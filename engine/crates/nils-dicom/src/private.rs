// SPDX-License-Identifier: AGPL-3.0-only

//! Private elements: the creator-aware lookup, and the six diffusion values v0
//! read from Siemens, GE and Philips headers
//! (`docs/specs/wave1-parse-and-digest.md`, §6.2).
//!
//! A private element lives in a block that its creator string reserves:
//! `(0019,0010) = "SIEMENS MR HEADER"` puts the block's elements at
//! `(0019,10xx)`. v0 read the fixed slot `10`; NILS finds the block by its
//! creator and falls back to the fixed slot only when the group declares no
//! creator at all, so a file whose blocks are shifted is read right rather than
//! read wrong.
//!
//! In an implicit VR file a private element has no VR on disk and the parser
//! keeps its bytes; the getters here decode those bytes the way the creator's
//! dictionary says (IS and CS text, SS and FL binary), where v0's `int(str(b))`
//! gave up.

use dicom_core::header::Header;
use dicom_core::{DicomValue, PrimitiveValue, Tag, VR};
use dicom_object::InMemDicomObject;
use dicom_object::mem::InMemElement;

use crate::charset::Charset;
use crate::csa;
use crate::value::{Conversion, Converter, Value, convert_primitive, parse_int};

/// Find the private element `elem` of the block that `creator` reserves in
/// `group`.
pub fn private_element<'a>(
    obj: &'a InMemDicomObject,
    group: u16,
    creator: &str,
    elem: u8,
) -> Option<&'a InMemElement> {
    let mut declared_any = false;
    for e in obj.iter() {
        let tag = e.tag();
        if tag.group() != group {
            continue;
        }
        let slot = tag.element();
        if !(0x0010..=0x00FF).contains(&slot) {
            continue;
        }
        declared_any = true;
        if creator_matches(e, creator) {
            return obj.get(Tag(group, (slot << 8) | u16::from(elem)));
        }
    }
    if declared_any {
        None
    } else {
        obj.get(Tag(group, 0x1000 | u16::from(elem)))
    }
}

fn creator_matches(e: &InMemElement, creator: &str) -> bool {
    let DicomValue::Primitive(p) = e.value() else {
        return false;
    };
    let text = match p {
        PrimitiveValue::U8(bytes) => bytes.iter().map(|&b| b as char).collect::<String>(),
        other => other.to_str().into_owned(),
    };
    text.trim_matches([' ', '\0'])
        .eq_ignore_ascii_case(creator.trim())
}

/// The primitive of an element, with the bytes of an untyped (UN) element read
/// as the text they would be under `vr`.
fn primitive_as(e: &InMemElement, vr: VR) -> Option<PrimitiveValue> {
    let DicomValue::Primitive(p) = e.value() else {
        return None;
    };
    match (p, vr) {
        (PrimitiveValue::U8(bytes), VR::SS) if bytes.len() >= 2 => {
            let v = i16::from_le_bytes([bytes[0], bytes[1]]);
            Some(PrimitiveValue::from(v))
        }
        (PrimitiveValue::U8(bytes), VR::FL) if bytes.len() >= 4 => {
            let v = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Some(PrimitiveValue::from(v))
        }
        (PrimitiveValue::U8(bytes), _) => {
            let text: String = bytes.iter().map(|&b| b as char).collect();
            let parts: Vec<String> = text
                .split('\\')
                .map(|s| s.trim_matches([' ', '\0']).to_string())
                .collect();
            Some(PrimitiveValue::Strs(parts.into_iter().collect()))
        }
        _ => Some(p.clone()),
    }
}

/// One private element a pack asks the digest to read
/// (`docs/specs/wave4a-engine-completes.md`, §5.2).
///
/// Addressed by the creator that reserves the block and the offset inside
/// it, never by the block's position, because the block moves from file to
/// file. The VR is the dictionary's when the pack knows it, which is how the
/// bytes of an implicit VR file are read as the number they are rather than
/// as the text they are not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingest {
    pub creator: String,
    pub group: u16,
    /// The offset within the block: the low byte of the element.
    pub element: u8,
    /// The value representation the dictionary gives it, when known.
    pub vr: Option<String>,
}

impl Ingest {
    /// How the registry keys the value: `0019xx0C SIEMENS MR HEADER`. The
    /// creator is kept as the pack spelled it; a lookup folds case.
    pub fn address(&self) -> String {
        address(self.group, self.element, &self.creator)
    }
}

/// The key a private element is stored under, by group, offset and creator.
pub fn address(group: u16, element: u8, creator: &str) -> String {
    format!("{group:04X}xx{element:02X} {}", creator.trim())
}

/// The longest value read in: a parameter is short, and a header blob that
/// happens to be addressed by a pack is not a parameter.
const INGEST_MAX: usize = 256;

/// Read every element of an ingest list from a data set, as text, aligned
/// with the list: `None` where the file has no such element, where the
/// value is longer than a parameter, or where it is not printable.
pub fn read_ingest(
    obj: &InMemDicomObject,
    list: &[Ingest],
    charset: &Charset,
) -> Vec<Option<String>> {
    list.iter()
        .map(|i| {
            private_element(obj, i.group, &i.creator, i.element)
                .and_then(|e| ingest_text(e, i.vr.as_deref(), charset))
        })
        .collect()
}

/// The text of one element, decoded the way its VR says.
///
/// An explicit VR file says what the element is and the parser has decoded
/// it. In an implicit VR file the element is bytes, and the dictionary's VR
/// decides whether those bytes are a little-endian number or a string; with
/// no VR at all they are read as a string, which is right for `IS`, `DS`,
/// `LO`, `SH` and `CS`, the shapes a parameter usually has.
fn ingest_text(e: &InMemElement, vr: Option<&str>, charset: &Charset) -> Option<String> {
    let DicomValue::Primitive(p) = e.value() else {
        return None;
    };
    let text = match p {
        PrimitiveValue::U8(bytes) if e.vr() == VR::UN || e.vr() == VR::OB => {
            if bytes.len() > INGEST_MAX {
                return None;
            }
            match vr {
                Some("SS") => numbers(bytes, 2, |b| i16::from_le_bytes([b[0], b[1]]).to_string()),
                Some("US") => numbers(bytes, 2, |b| u16::from_le_bytes([b[0], b[1]]).to_string()),
                Some("SL") => numbers(bytes, 4, |b| {
                    i32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_string()
                }),
                Some("UL") => numbers(bytes, 4, |b| {
                    u32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_string()
                }),
                Some("FL") => numbers(bytes, 4, |b| {
                    format!("{}", f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                }),
                Some("FD") => numbers(bytes, 8, |b| {
                    format!(
                        "{}",
                        f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
                    )
                }),
                // The bytes as the string they are, decoded under the
                // character set the file declared, the way the typed
                // getters above read them.
                _ => primitive_as(e, VR::LO).and_then(|p| {
                    match convert_primitive(Converter::Text, &p, VR::LO, charset).value {
                        Some(Value::Text(t)) => Some(t),
                        _ => None,
                    }
                }),
            }
        }
        _ => {
            if p.calculate_byte_len() > INGEST_MAX {
                return None;
            }
            Some(p.to_str().into_owned())
        }
    };
    let text = text?;
    let text = text.trim_matches([' ', '\0']).to_string();
    if text.is_empty() || !text.chars().all(|c| !c.is_control()) {
        return None;
    }
    Some(text)
}

/// Fixed-width little-endian numbers, joined the way DICOM joins values.
fn numbers(bytes: &[u8], width: usize, one: impl Fn(&[u8]) -> String) -> Option<String> {
    if bytes.len() < width || !bytes.len().is_multiple_of(width) {
        return None;
    }
    Some(bytes.chunks(width).map(one).collect::<Vec<_>>().join("\\"))
}

/// The six diffusion values, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dwi {
    SiemensBValue,
    SiemensDirectionality,
    SiemensPeDirPositive,
    GeBValue,
    GeNDirections,
    PhilipsBValue,
}

impl Dwi {
    pub fn tag_text(self) -> &'static str {
        match self {
            Dwi::SiemensBValue => "(0019,xx0C) SIEMENS MR HEADER",
            Dwi::SiemensDirectionality => "(0019,xx0D) SIEMENS MR HEADER",
            Dwi::SiemensPeDirPositive => {
                "(0029,xx10) SIEMENS CSA HEADER, SV10 PhaseEncodingDirectionPositive"
            }
            Dwi::GeBValue => "(0043,xx39) GEMS_PARM_01, first value",
            Dwi::GeNDirections => "(0043,xx30) GEMS_PARM_01",
            Dwi::PhilipsBValue => "(2001,xx03) Philips Imaging DD 001, sentinel above 1e37 is null",
        }
    }

    /// Read the value from the data set.
    pub fn read(self, obj: &InMemDicomObject, charset: &Charset) -> Conversion {
        match self {
            Dwi::SiemensBValue => private_element(obj, 0x0019, "SIEMENS MR HEADER", 0x0C)
                .and_then(|e| primitive_as(e, VR::IS))
                .map(|p| convert_primitive(Converter::Int, &p, VR::IS, charset))
                .unwrap_or_default(),
            Dwi::SiemensDirectionality => private_element(obj, 0x0019, "SIEMENS MR HEADER", 0x0D)
                .and_then(|e| primitive_as(e, VR::CS))
                .map(|p| convert_primitive(Converter::Text, &p, VR::CS, charset))
                .unwrap_or_default(),
            Dwi::SiemensPeDirPositive => {
                let Some(e) = private_element(obj, 0x0029, "SIEMENS CSA HEADER", 0x10) else {
                    return Conversion::default();
                };
                let DicomValue::Primitive(PrimitiveValue::U8(bytes)) = e.value() else {
                    return Conversion::default();
                };
                match csa::first_value(bytes, "PhaseEncodingDirectionPositive") {
                    Some(text) => match parse_int(&text) {
                        Some(i) => Conversion {
                            value: Some(Value::Int(i)),
                            ..Default::default()
                        },
                        None => Conversion {
                            invalid: Some(text),
                            ..Default::default()
                        },
                    },
                    None => Conversion::default(),
                }
            }
            Dwi::GeBValue => {
                let Some(p) = private_element(obj, 0x0043, "GEMS_PARM_01", 0x39)
                    .and_then(|e| primitive_as(e, VR::IS))
                else {
                    return Conversion::default();
                };
                let first = match &p {
                    PrimitiveValue::Strs(parts) => parts.first().map(|s| s.trim().to_string()),
                    PrimitiveValue::Str(s) => Some(s.trim().to_string()),
                    other if other.multiplicity() >= 1 => {
                        other.to_str().split('\\').next().map(str::to_string)
                    }
                    _ => None,
                };
                match first {
                    None => Conversion::default(),
                    Some(s) if s.is_empty() => Conversion::default(),
                    Some(s) => match parse_int(&s) {
                        Some(i) => Conversion {
                            value: Some(Value::Int(i)),
                            ..Default::default()
                        },
                        None => Conversion {
                            invalid: Some(s),
                            ..Default::default()
                        },
                    },
                }
            }
            Dwi::GeNDirections => private_element(obj, 0x0043, "GEMS_PARM_01", 0x30)
                .and_then(|e| primitive_as(e, VR::SS))
                .map(|p| convert_primitive(Converter::Int, &p, VR::SS, charset))
                .unwrap_or_default(),
            Dwi::PhilipsBValue => {
                let Some(p) = private_element(obj, 0x2001, "Philips Imaging DD 001", 0x03)
                    .and_then(|e| primitive_as(e, VR::FL))
                else {
                    return Conversion::default();
                };
                let c = convert_primitive(Converter::Double, &p, VR::FL, charset);
                match c.value {
                    Some(Value::Double(d)) if d > 1e37 => Conversion::default(),
                    _ => c,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::{DataElement, Tag};

    fn object(elems: Vec<DataElement<InMemDicomObject>>) -> InMemDicomObject {
        let mut ds = InMemDicomObject::new_empty();
        for e in elems {
            ds.put(e);
        }
        ds
    }

    fn text(g: u16, e: u16, vr: VR, v: &str) -> DataElement<InMemDicomObject> {
        DataElement::new(Tag(g, e), vr, PrimitiveValue::from(v))
    }

    fn raw(g: u16, e: u16, bytes: Vec<u8>) -> DataElement<InMemDicomObject> {
        DataElement::new(Tag(g, e), VR::UN, PrimitiveValue::U8(bytes.into()))
    }

    fn ingest(creator: &str, group: u16, element: u8, vr: Option<&str>) -> Ingest {
        Ingest {
            creator: creator.into(),
            group,
            element,
            vr: vr.map(str::to_string),
        }
    }

    #[test]
    fn an_ingested_element_is_found_by_its_creator_wherever_the_block_sits() {
        // The whole point of addressing by creator: the same vendor lands at
        // a different offset from file to file.
        let list = [ingest("SIEMENS MR HEADER", 0x0019, 0x0C, Some("IS"))];
        let shifted = object(vec![
            text(0x0019, 0x0010, VR::LO, "SOMEBODY ELSE"),
            text(0x0019, 0x0011, VR::LO, "SIEMENS MR HEADER"),
            text(0x0019, 0x100C, VR::IS, "0"),
            text(0x0019, 0x110C, VR::IS, "1000"),
        ]);
        assert_eq!(
            read_ingest(&shifted, &list, &Charset::resolve(None)),
            vec![Some("1000".to_string())]
        );
        let absent = object(vec![text(0x0019, 0x0010, VR::LO, "SOMEBODY ELSE")]);
        assert_eq!(
            read_ingest(&absent, &list, &Charset::resolve(None)),
            vec![None]
        );
    }

    #[test]
    fn the_bytes_of_an_implicit_file_are_read_the_way_the_dictionary_says() {
        // With no VR on disk the parser keeps bytes; the dictionary's VR is
        // what turns them back into the number they are.
        let obj = object(vec![
            text(0x0043, 0x0010, VR::LO, "GEMS_PARM_01"),
            raw(0x0043, 0x1030, vec![0x0A, 0x00]),
            raw(0x0043, 0x1031, 0.5f32.to_le_bytes().to_vec()),
            raw(0x0043, 0x1032, b"12\\34 ".to_vec()),
        ]);
        let list = [
            ingest("GEMS_PARM_01", 0x0043, 0x30, Some("SS")),
            ingest("GEMS_PARM_01", 0x0043, 0x31, Some("FL")),
            ingest("GEMS_PARM_01", 0x0043, 0x32, None),
        ];
        assert_eq!(
            read_ingest(&obj, &list, &Charset::resolve(None)),
            vec![
                Some("10".to_string()),
                Some("0.5".to_string()),
                Some("12\\34".to_string())
            ]
        );
    }

    #[test]
    fn a_blob_and_a_binary_value_are_not_ingested() {
        // A parameter is short and printable; a CSA header addressed by
        // mistake is neither, and it must not become a series column.
        let obj = object(vec![
            text(0x0029, 0x0010, VR::LO, "SIEMENS CSA HEADER"),
            raw(0x0029, 0x1010, vec![0u8; 4096]),
            raw(0x0029, 0x1011, vec![0x01, 0x02, 0x03]),
        ]);
        let list = [
            ingest("SIEMENS CSA HEADER", 0x0029, 0x10, None),
            ingest("SIEMENS CSA HEADER", 0x0029, 0x11, None),
        ];
        assert_eq!(
            read_ingest(&obj, &list, &Charset::resolve(None)),
            vec![None, None]
        );
    }

    #[test]
    fn an_address_names_the_offset_and_the_creator_and_never_the_slot() {
        assert_eq!(
            ingest("SIEMENS MR HEADER", 0x0019, 0x0C, None).address(),
            "0019xx0C SIEMENS MR HEADER"
        );
    }
}
