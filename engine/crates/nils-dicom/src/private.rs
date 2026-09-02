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
