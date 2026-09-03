// SPDX-License-Identifier: AGPL-3.0-only

//! Synthetic DICOM files for tests: a small writer that lays out a data set
//! as explicit or implicit VR little endian, with or without the Part 10
//! meta group and preamble, so that every crate can make the files it tests
//! against without a corpus. Nothing here is used by the digest.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use dicom_core::{Tag, VR};
use dicom_dictionary_std::tags;

/// One element to write.
#[derive(Debug, Clone)]
pub struct Elem {
    pub tag: Tag,
    pub vr: VR,
    pub body: Body,
}

/// The value of an element.
#[derive(Debug, Clone)]
pub enum Body {
    /// A string in the default repertoire (ISO 8859-1, one byte per
    /// character), padded to even length on write (NUL for UI, space
    /// otherwise). Other encodings go in as [`Body::Bytes`].
    Text(String),
    /// Raw bytes, padded with NUL to even length.
    Bytes(Vec<u8>),
    /// The items of a sequence, the sequence and its items with a length.
    Items(Vec<Vec<Elem>>),
    /// The items of a sequence of undefined length, each item of undefined
    /// length too, closed by delimiters.
    UndefinedItems(Vec<Vec<Elem>>),
}

/// A string element. Not for binary VRs; see [`num`].
pub fn text(tag: Tag, vr: VR, value: &str) -> Elem {
    assert!(!is_binary(vr), "{vr:?} is a binary VR, use num or bytes");
    Elem {
        tag,
        vr,
        body: Body::Text(value.to_string()),
    }
}

/// An element with raw bytes.
pub fn bytes(tag: Tag, vr: VR, value: Vec<u8>) -> Elem {
    Elem {
        tag,
        vr,
        body: Body::Bytes(value),
    }
}

/// A numeric element with a binary VR (US, SS, UL, SL, FL, FD).
pub fn num(tag: Tag, vr: VR, value: f64) -> Elem {
    let b = match vr {
        VR::US => (value as u16).to_le_bytes().to_vec(),
        VR::SS => (value as i16).to_le_bytes().to_vec(),
        VR::UL => (value as u32).to_le_bytes().to_vec(),
        VR::SL => (value as i32).to_le_bytes().to_vec(),
        VR::FL => (value as f32).to_le_bytes().to_vec(),
        VR::FD => value.to_le_bytes().to_vec(),
        other => panic!("{other:?} is not a numeric binary VR"),
    };
    bytes(tag, vr, b)
}

/// An unsigned short.
pub fn us(tag: Tag, value: u16) -> Elem {
    num(tag, VR::US, f64::from(value))
}

/// A sequence.
pub fn seq(tag: Tag, items: Vec<Vec<Elem>>) -> Elem {
    Elem {
        tag,
        vr: VR::SQ,
        body: Body::Items(items),
    }
}

/// A sequence written the way a scanner may write it and the way the
/// standard allows: the sequence and every item of undefined length, closed
/// by an item delimiter and a sequence delimiter (PS3.5 §7.5). Files in the
/// archive carry `ProcedureCodeSequence` in exactly this form.
pub fn seq_undefined(tag: Tag, items: Vec<Vec<Elem>>) -> Elem {
    Elem {
        tag,
        vr: VR::SQ,
        body: Body::UndefinedItems(items),
    }
}

fn is_binary(vr: VR) -> bool {
    matches!(
        vr,
        VR::US
            | VR::SS
            | VR::UL
            | VR::SL
            | VR::FL
            | VR::FD
            | VR::OB
            | VR::OW
            | VR::OF
            | VR::OD
            | VR::OL
            | VR::OV
            | VR::SV
            | VR::UV
            | VR::UN
            | VR::AT
    )
}

/// VRs whose explicit header carries a four-byte length.
fn long_header(vr: VR) -> bool {
    matches!(
        vr,
        VR::OB
            | VR::OD
            | VR::OF
            | VR::OL
            | VR::OV
            | VR::OW
            | VR::SQ
            | VR::SV
            | VR::UC
            | VR::UN
            | VR::UR
            | VR::UT
            | VR::UV
    )
}

fn padded(body: &Body, vr: VR) -> Vec<u8> {
    match body {
        Body::Text(s) => {
            // the default repertoire: one byte per character, ISO 8859-1
            let mut v: Vec<u8> = s
                .chars()
                .map(|c| {
                    u8::try_from(u32::from(c))
                        .unwrap_or_else(|_| panic!("{c:?} is not ISO 8859-1; use bytes"))
                })
                .collect();
            if v.len() % 2 == 1 {
                v.push(if vr == VR::UI { 0 } else { b' ' });
            }
            v
        }
        Body::Bytes(b) => {
            let mut v = b.clone();
            if v.len() % 2 == 1 {
                v.push(0);
            }
            v
        }
        Body::Items(_) | Body::UndefinedItems(_) => unreachable!(),
    }
}

fn encode_elem(out: &mut Vec<u8>, e: &Elem, explicit: bool) {
    let value: Vec<u8> = match &e.body {
        Body::Items(items) => {
            let mut v = Vec::new();
            for item in items {
                let body = encode_dataset(item, explicit);
                v.extend_from_slice(&0xFFFEu16.to_le_bytes());
                v.extend_from_slice(&0xE000u16.to_le_bytes());
                v.extend_from_slice(&(body.len() as u32).to_le_bytes());
                v.extend_from_slice(&body);
            }
            v
        }
        Body::UndefinedItems(items) => {
            // the item of undefined length ends at its delimiter, and the
            // sequence at its own; the header below writes 0xFFFFFFFF
            let mut v = Vec::new();
            for item in items {
                v.extend_from_slice(&0xFFFEu16.to_le_bytes());
                v.extend_from_slice(&0xE000u16.to_le_bytes());
                v.extend_from_slice(&u32::MAX.to_le_bytes());
                v.extend_from_slice(&encode_dataset(item, explicit));
                v.extend_from_slice(&0xFFFEu16.to_le_bytes());
                v.extend_from_slice(&0xE00Du16.to_le_bytes());
                v.extend_from_slice(&0u32.to_le_bytes());
            }
            v.extend_from_slice(&0xFFFEu16.to_le_bytes());
            v.extend_from_slice(&0xE0DDu16.to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v
        }
        other => padded(other, e.vr),
    };
    let undefined = matches!(e.body, Body::UndefinedItems(_));
    let declared = if undefined {
        u32::MAX
    } else {
        value.len() as u32
    };
    out.extend_from_slice(&e.tag.group().to_le_bytes());
    out.extend_from_slice(&e.tag.element().to_le_bytes());
    if explicit {
        out.extend_from_slice(e.vr.to_string().as_bytes());
        if long_header(e.vr) {
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&declared.to_le_bytes());
        } else {
            out.extend_from_slice(&(value.len() as u16).to_le_bytes());
        }
    } else {
        out.extend_from_slice(&declared.to_le_bytes());
    }
    out.extend_from_slice(&value);
}

/// Encode a data set, elements sorted by tag.
pub fn encode_dataset(elems: &[Elem], explicit: bool) -> Vec<u8> {
    let mut sorted: Vec<&Elem> = elems.iter().collect();
    sorted.sort_by_key(|e| e.tag);
    let mut out = Vec::new();
    for e in sorted {
        encode_elem(&mut out, e, explicit);
    }
    out
}

/// The fields of the meta group.
#[derive(Debug, Clone)]
pub struct MetaFields {
    pub transfer_syntax: String,
    pub sop_class: Option<String>,
    pub sop_instance: Option<String>,
    pub implementation_class: Option<String>,
    pub implementation_version: Option<String>,
}

/// The implementation class UID the synthetic files claim.
pub const IMPLEMENTATION_CLASS: &str = "1.2.826.0.1.3680043.10.1234.1";
/// Their implementation version name.
pub const IMPLEMENTATION_VERSION: &str = "NILS_SYNTH";

impl MetaFields {
    /// Explicit VR little endian, MR Image Storage.
    pub fn mr(sop_instance: &str) -> Self {
        Self::with(
            crate::read::EXPLICIT_VR_LE,
            "1.2.840.10008.5.1.4.1.1.4",
            sop_instance,
        )
    }

    /// Explicit VR little endian, CT Image Storage.
    pub fn ct(sop_instance: &str) -> Self {
        Self::with(
            crate::read::EXPLICIT_VR_LE,
            "1.2.840.10008.5.1.4.1.1.2",
            sop_instance,
        )
    }

    /// Explicit VR little endian, PET Image Storage.
    pub fn pet(sop_instance: &str) -> Self {
        Self::with(
            crate::read::EXPLICIT_VR_LE,
            "1.2.840.10008.5.1.4.1.1.128",
            sop_instance,
        )
    }

    pub fn with(transfer_syntax: &str, sop_class: &str, sop_instance: &str) -> Self {
        MetaFields {
            transfer_syntax: transfer_syntax.to_string(),
            sop_class: Some(sop_class.to_string()),
            sop_instance: Some(sop_instance.to_string()),
            implementation_class: Some(IMPLEMENTATION_CLASS.to_string()),
            implementation_version: Some(IMPLEMENTATION_VERSION.to_string()),
        }
    }

    fn elems(&self) -> Vec<Elem> {
        let mut v = vec![
            bytes(Tag(0x0002, 0x0001), VR::OB, vec![0, 1]),
            text(Tag(0x0002, 0x0010), VR::UI, &self.transfer_syntax),
        ];
        if let Some(s) = &self.sop_class {
            v.push(text(Tag(0x0002, 0x0002), VR::UI, s));
        }
        if let Some(s) = &self.sop_instance {
            v.push(text(Tag(0x0002, 0x0003), VR::UI, s));
        }
        if let Some(s) = &self.implementation_class {
            v.push(text(Tag(0x0002, 0x0012), VR::UI, s));
        }
        if let Some(s) = &self.implementation_version {
            v.push(text(Tag(0x0002, 0x0013), VR::SH, s));
        }
        v
    }
}

/// A Part 10 file: the preamble (when asked), `DICM`, the meta group, the
/// data set in the meta's transfer syntax (explicit VR little endian unless
/// the syntax is implicit).
pub fn part10(meta: &MetaFields, dataset: &[Elem], preamble: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if preamble {
        out.extend(std::iter::repeat_n(0u8, 128));
    }
    out.extend_from_slice(b"DICM");
    let group = encode_dataset(&meta.elems(), true);
    encode_elem(
        &mut out,
        &num(Tag(0x0002, 0x0000), VR::UL, group.len() as f64),
        true,
    );
    out.extend_from_slice(&group);
    let explicit = meta.transfer_syntax != crate::read::IMPLICIT_VR_LE;
    out.extend_from_slice(&encode_dataset(dataset, explicit));
    out
}

/// A bare data set with no meta group.
pub fn bare(dataset: &[Elem], explicit: bool) -> Vec<u8> {
    encode_dataset(dataset, explicit)
}

fn minimal(study: &str, series: &str, sop: &str, sop_class: &str, modality: &str) -> Vec<Elem> {
    vec![
        text(tags::SOP_CLASS_UID, VR::UI, sop_class),
        text(tags::SOP_INSTANCE_UID, VR::UI, sop),
        text(tags::STUDY_INSTANCE_UID, VR::UI, study),
        text(tags::SERIES_INSTANCE_UID, VR::UI, series),
        text(tags::MODALITY, VR::CS, modality),
    ]
}

/// The five elements an accepted MR file needs.
pub fn minimal_mr(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    minimal(study, series, sop, "1.2.840.10008.5.1.4.1.1.4", "MR")
}

/// The five elements an accepted CT file needs.
pub fn minimal_ct(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    minimal(study, series, sop, "1.2.840.10008.5.1.4.1.1.2", "CT")
}

/// The five elements an accepted PET file needs.
pub fn minimal_pet(study: &str, series: &str, sop: &str) -> Vec<Elem> {
    minimal(study, series, sop, "1.2.840.10008.5.1.4.1.1.128", "PT")
}

/// A directory under the system's temporary directory, removed on drop.
#[derive(Debug)]
pub struct TempDir(PathBuf);

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("nils-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Write a file under the directory and return its path.
    pub fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, content).expect("write file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::{Form, read};

    #[test]
    fn part10_round_trips_through_the_reader() {
        let dir = TempDir::new("synth");
        let elems = minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        for preamble in [true, false] {
            let path = dir.file(
                "a.dcm",
                &part10(&MetaFields::mr("1.2.3.4"), &elems, preamble),
            );
            let h = read(&path).unwrap();
            assert_eq!(h.form, Form::Part10);
            let meta = h.meta.unwrap();
            assert_eq!(meta.transfer_syntax(), crate::read::EXPLICIT_VR_LE);
            assert_eq!(meta.media_storage_sop_instance_uid(), "1.2.3.4");
            assert_eq!(
                meta.implementation_version_name(),
                Some(IMPLEMENTATION_VERSION)
            );
            let s = h
                .dataset
                .get(tags::STUDY_INSTANCE_UID)
                .unwrap()
                .value()
                .to_str()
                .unwrap();
            assert_eq!(s.trim_end_matches('\0'), "1.2.3");
        }
    }

    #[test]
    fn bare_forms_round_trip() {
        let dir = TempDir::new("synth-bare");
        let mut elems = minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4");
        elems.push(us(tags::ROWS, 512));
        elems.push(seq(
            tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
            vec![vec![seq(
                tags::MR_ECHO_SEQUENCE,
                vec![vec![num(tags::EFFECTIVE_ECHO_TIME, VR::FD, 30.0)]],
            )]],
        ));
        for (explicit, form) in [(false, Form::BareImplicit), (true, Form::BareExplicit)] {
            let path = dir.file("bare", &bare(&elems, explicit));
            let h = read(&path).unwrap();
            assert_eq!(h.form, form);
            assert!(h.meta.is_none());
            let rows = h
                .dataset
                .get(tags::ROWS)
                .unwrap()
                .value()
                .to_int::<u16>()
                .unwrap();
            assert_eq!(rows, 512);
            assert!(
                h.dataset
                    .get(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE)
                    .is_some()
            );
        }
    }

    #[test]
    fn implicit_part10_is_written_implicit() {
        let dir = TempDir::new("synth-implicit");
        let elems = minimal_ct("1.2.3", "1.2.3.1", "1.2.3.4");
        let meta = MetaFields::with(
            crate::read::IMPLICIT_VR_LE,
            "1.2.840.10008.5.1.4.1.1.2",
            "1.2.3.4",
        );
        let path = dir.file("a.dcm", &part10(&meta, &elems, true));
        let h = read(&path).unwrap();
        assert_eq!(h.transfer_syntax(), crate::read::IMPLICIT_VR_LE);
        assert_eq!(
            h.dataset
                .get(tags::MODALITY)
                .unwrap()
                .value()
                .to_str()
                .unwrap()
                .trim(),
            "CT"
        );
    }
}
