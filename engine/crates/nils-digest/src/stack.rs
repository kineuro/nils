// SPDX-License-Identifier: AGPL-3.0-only

//! Stack signatures (`docs/specs/wave1-parse-and-digest.md`, §8): v0's
//! fingerprint of an instance's stack membership, computed from the file alone,
//! and the orientation class it carries.
//!
//! The signature is fourteen values of the file in a fixed order, each in the
//! normal form of §8 (a float rounded to its decimals, an integer, a text, or
//! the orientation class), a null as the empty string; the stack key is the
//! unkeyed BLAKE2b-8 of their canonical string. Two instances of a series
//! with the same key share a stack.

use std::borrow::Cow;

use blake2::digest::consts::U8;
use blake2::{Blake2b, Digest};
use nils_dicom::{Extracted, Level, Value};

use crate::batch::canonical_value;

/// A confidence under this counts an `orientation_oblique` diagnostic.
pub const OBLIQUE_BELOW: f64 = 0.9;

/// The class of an image plane, by the dominant axis of its normal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Axial,
    Coronal,
    Sagittal,
}

impl Class {
    /// The name as v0 wrote it and the `orientation` column holds it.
    pub fn name(self) -> &'static str {
        match self {
            Class::Axial => "Axial",
            Class::Coronal => "Coronal",
            Class::Sagittal => "Sagittal",
        }
    }
}

/// The orientation of an image plane: its class and how well the normal
/// aligns with that axis (1.0 is exact; 0.5 stands for unknown).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orientation {
    pub class: Class,
    pub confidence: f64,
}

impl Orientation {
    /// What a missing, short or degenerate ImageOrientationPatient gives.
    pub const UNKNOWN: Orientation = Orientation {
        class: Class::Axial,
        confidence: 0.5,
    };

    /// True when the plane is known and far enough from every axis to be
    /// worth a look. The largest component of a unit normal is at least
    /// 1/√3, so a confidence of 0.5 only ever means unknown, and an unknown
    /// plane is not oblique.
    pub fn oblique(&self) -> bool {
        self.confidence < OBLIQUE_BELOW && *self != Orientation::UNKNOWN
    }
}

/// The signature of one instance: the key of the stack it belongs to and its
/// orientation.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    /// Sixteen hex characters: BLAKE2b-8 of the canonical string.
    pub key: String,
    pub orientation: Orientation,
}

impl Signature {
    pub fn of(x: &Extracted) -> Signature {
        let orientation = orientation(iop(x));
        Signature {
            key: key_of(&canonical_with(x, orientation.class)),
            orientation,
        }
    }
}

/// The canonical string of a file's signature: the fourteen values of §8 in
/// its order, joined by `|`, a null as the empty string, a `|` or a `\` in a
/// value escaped with a backslash.
pub fn canonical(x: &Extracted) -> String {
    canonical_with(x, orientation(iop(x)).class)
}

/// The stack key of a canonical string.
pub fn key_of(canonical: &str) -> String {
    hex::encode(Blake2b::<U8>::digest(canonical.as_bytes()))
}

fn canonical_with(x: &Extracted, class: Class) -> String {
    let v = |column: &str| x.value(Level::Stack, column);
    let values: [Cow<'_, str>; 14] = [
        rounded(v("echo_time"), 2),
        rounded(v("inversion_time"), 1),
        as_read(v("echo_numbers")),
        as_read(v("echo_train_length")),
        rounded(v("repetition_time"), 1),
        rounded(v("flip_angle"), 1),
        as_read(v("receive_coil_name")),
        as_read(v("xray_exposure")),
        rounded(v("kvp"), 0),
        rounded(v("tube_current"), 0),
        as_read(v("pet_bed_index")),
        as_read(v("pet_frame_type")),
        Cow::Borrowed(class.name()),
        as_read(v("image_type")),
    ];
    let mut out = String::new();
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push('|');
        }
        for c in value.chars() {
            if c == '|' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

fn iop(x: &Extracted) -> Option<&str> {
    match x.value(Level::Stack, "image_orientation_patient") {
        Some(Value::Text(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// A number rounded to `decimals` the way Python's `round` does it: half to
/// even on the exact binary value, which `{:.n}` also does; a zero keeps no
/// sign, as `-0.0 == 0.0`. A null is the empty string.
fn rounded(v: Option<&Value>, decimals: usize) -> Cow<'_, str> {
    let d = match v {
        None => return Cow::Borrowed(""),
        Some(Value::Double(d)) => *d,
        Some(Value::Int(i)) => *i as f64,
        Some(other) => return canonical_value(other),
    };
    let s = format!("{d:.decimals$}");
    match s.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|b| b == b'0' || b == b'.') => Cow::Owned(rest.to_string()),
        _ => Cow::Owned(s),
    }
}

/// A value as read: an integer, a text, a double in its shortest form.
fn as_read(v: Option<&Value>) -> Cow<'_, str> {
    match v {
        None => Cow::Borrowed(""),
        Some(v) => canonical_value(v),
    }
}

/// v0's `compute_orientation`: the normal of the image plane by the cross
/// product of the row and column cosines, the confidence its largest absolute
/// component, the class the dominant axis (X Sagittal, Y Coronal, Z Axial,
/// ties in that order). Missing, short, unparsable or degenerate cosines give
/// [`Orientation::UNKNOWN`].
pub fn orientation(iop: Option<&str>) -> Orientation {
    let Some(iop) = iop else {
        return Orientation::UNKNOWN;
    };
    if iop.is_empty() {
        return Orientation::UNKNOWN;
    }
    let cleaned: String = iop
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '\'' | '"'))
        .collect();
    let parts: Vec<&str> = cleaned.trim().split('\\').collect();
    if parts.len() < 6 {
        return Orientation::UNKNOWN;
    }
    let mut c = [0f64; 6];
    for (slot, part) in c.iter_mut().zip(&parts) {
        match part.trim().parse::<f64>() {
            Ok(d) => *slot = d,
            Err(_) => return Orientation::UNKNOWN,
        }
    }
    let [rx, ry, rz, cx, cy, cz] = c;
    let nx = ry * cz - rz * cy;
    let ny = rz * cx - rx * cz;
    let nz = rx * cy - ry * cx;
    let magnitude = (nx * nx + ny * ny + nz * nz).sqrt();
    if magnitude < 1e-10 {
        return Orientation::UNKNOWN;
    }
    let abs_nx = nx.abs() / magnitude;
    let abs_ny = ny.abs() / magnitude;
    let abs_nz = nz.abs() / magnitude;
    let confidence = abs_nx.max(abs_ny).max(abs_nz).clamp(0.0, 1.0);
    let class = if abs_nx >= abs_ny && abs_nx >= abs_nz {
        Class::Sagittal
    } else if abs_ny >= abs_nx && abs_ny >= abs_nz {
        Class::Coronal
    } else {
        Class::Axial
    };
    Orientation { class, confidence }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(d: f64, decimals: usize) -> String {
        rounded(Some(&Value::Double(d)), decimals).into_owned()
    }

    #[test]
    fn rounding_is_pythons_round() {
        // The tie cases the spec pins (§8): half to even on the exact binary
        // value, so 2.675 (which is below the tie in binary) rounds down.
        assert_eq!(r(2.125, 2), "2.12");
        assert_eq!(r(2.675, 2), "2.67");
        assert_eq!(r(2.5, 0), "2");
        assert_eq!(r(0.5, 0), "0");
        assert_eq!(r(1.5, 0), "2");
        assert_eq!(r(3.5, 0), "4");
        assert_eq!(r(0.125, 2), "0.12");
        assert_eq!(r(0.375, 2), "0.38");
        assert_eq!(r(1.005, 2), "1.00");
        assert_eq!(r(2.45, 1), "2.5");
        assert_eq!(r(2.55, 1), "2.5");
        assert_eq!(r(0.25, 1), "0.2");
        assert_eq!(r(0.35, 1), "0.3");
        assert_eq!(r(99.95, 1), "100.0");
        assert_eq!(r(120.0, 0), "120");
        assert_eq!(r(4.0, 2), "4.00");
        assert_eq!(r(-0.001, 2), "0.00");
        assert_eq!(r(-0.4, 0), "0");
        assert_eq!(r(-0.6, 0), "-1");
        assert_eq!(rounded(Some(&Value::Int(3)), 1), "3.0");
        assert_eq!(rounded(None, 1), "");
    }

    #[test]
    fn values_as_read_and_nulls() {
        assert_eq!(as_read(None), "");
        assert_eq!(as_read(Some(&Value::Int(7))), "7");
        assert_eq!(as_read(Some(&Value::Double(0.1))), "0.1");
        assert_eq!(as_read(Some(&Value::Double(100.0))), "100.0");
        assert_eq!(
            as_read(Some(&Value::Text("ORIGINAL\\PRIMARY".into()))),
            "ORIGINAL\\PRIMARY"
        );
    }

    #[test]
    fn orientation_is_v0s() {
        let o = orientation(Some("1\\0\\0\\0\\1\\0"));
        assert_eq!(o.class, Class::Axial);
        assert_eq!(o.confidence, 1.0);
        let o = orientation(Some("0\\1\\0\\0\\0\\-1"));
        assert_eq!(o.class, Class::Sagittal);
        assert_eq!(o.confidence, 1.0);
        let o = orientation(Some("1\\0\\0\\0\\0\\-1"));
        assert_eq!(o.class, Class::Coronal);
        assert_eq!(o.confidence, 1.0);
        // A tilted axial plane: Z still dominates, the confidence drops.
        let o = orientation(Some("1\\0\\0\\0\\0.95\\-0.3122"));
        assert_eq!(o.class, Class::Axial);
        assert!(o.confidence < 1.0 && o.confidence > OBLIQUE_BELOW);
        assert!(!o.oblique());
        // Forty-five degrees between Y and Z: the tie goes to Coronal.
        let o = orientation(Some("1\\0\\0\\0\\0.70710678\\0.70710678"));
        assert_eq!(o.class, Class::Coronal);
        assert!(o.oblique());
        assert!((o.confidence - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        // Brackets, quotes and spaces are stripped, as v0 did.
        let o = orientation(Some(
            "['1', '0', '0', '0', '1', '0']"
                .replace(", ", "\\")
                .as_str(),
        ));
        assert_eq!(o.class, Class::Axial);
        let o = orientation(Some(" 1\\ 0\\0\\0\\1\\0 "));
        assert_eq!(o.class, Class::Axial);
        assert_eq!(o.confidence, 1.0);
        // The unknowns: missing, empty, short, garbage, parallel vectors.
        for iop in [
            None,
            Some(""),
            Some("1\\0\\0"),
            Some("a\\b\\c\\d\\e\\f"),
            Some("1\\0\\0\\1\\0\\0"),
        ] {
            assert_eq!(orientation(iop), Orientation::UNKNOWN, "{iop:?}");
        }
        assert!(!Orientation::UNKNOWN.oblique());
    }

    #[test]
    fn the_key_is_blake2b_8_of_the_canonical_string() {
        let key = key_of("10.00||||500.0|90.0|||||||Axial|ORIGINAL\\\\PRIMARY");
        assert_eq!(key.len(), 16);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(
            key,
            key_of("10.00||||500.0|90.0|||||||Axial|ORIGINAL\\\\PRIMARY")
        );
        assert_ne!(
            key,
            key_of("10.00||||500.0|90.0|||||||Axial|ORIGINAL\\\\PRIMARY|")
        );
        // Pinned against Python's `hashlib.blake2b(digest_size=8)`: later
        // waves refer to a stack by this key.
        assert_eq!(key_of(""), "e4a6a0577479b2b4");
        assert_eq!(
            key_of("10.00||1||500.0|90.0|||||||Sagittal|ORIGINAL\\\\PRIMARY\\\\M"),
            "e77101de3f76b1de"
        );
    }

    #[test]
    fn the_canonical_string_of_a_file() {
        use dicom_core::VR;
        use dicom_dictionary_std::tags;
        use nils_dicom::synth::{MetaFields, TempDir, minimal_mr, part10, text};

        let dir = TempDir::new("stack-canonical");
        let mut elems = minimal_mr("1.2.3", "1.2.3.4", "1.2.3.4.5");
        elems.push(text(tags::ECHO_TIME, VR::DS, "10"));
        elems.push(text(tags::REPETITION_TIME, VR::DS, "499.96"));
        elems.push(text(tags::FLIP_ANGLE, VR::DS, "90"));
        elems.push(text(tags::ECHO_NUMBERS, VR::IS, "1"));
        elems.push(text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M"));
        elems.push(text(
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            "0\\1\\0\\0\\0\\-1",
        ));
        let path = dir.file("a.dcm", &part10(&MetaFields::mr("1.2.3.4.5"), &elems, true));
        let x = nils_dicom::extract(&path).unwrap();
        assert_eq!(
            canonical(&x),
            "10.00||1||500.0|90.0|||||||Sagittal|ORIGINAL\\\\PRIMARY\\\\M"
        );
        let s = Signature::of(&x);
        assert_eq!(s.key, "e77101de3f76b1de");
        assert_eq!(s.orientation.class, Class::Sagittal);
        assert_eq!(s.orientation.confidence, 1.0);

        // A CT file has null MR values: they do not tell its stacks apart.
        let path = dir.file(
            "b.dcm",
            &part10(
                &MetaFields::ct("1.2.3.4.6"),
                &nils_dicom::synth::minimal_ct("1.2.3", "1.2.3.4", "1.2.3.4.6"),
                true,
            ),
        );
        let x = nils_dicom::extract(&path).unwrap();
        assert_eq!(canonical(&x), "||||||||||||Axial|");
        assert_eq!(Signature::of(&x).orientation, Orientation::UNKNOWN);
    }
}
