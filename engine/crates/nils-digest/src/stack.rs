// SPDX-License-Identifier: AGPL-3.0-only

//! Stack signatures (`docs/specs/wave1-parse-and-digest.md`, §8): v0's
//! fingerprint of an instance's stack membership, computed from the file alone,
//! and the orientation class it carries.
//!
//! The signature is fourteen values of the file in a fixed order, each in the
//! normal form of §8 (a float rounded to its decimals, an integer, a text, or
//! the orientation class), a null as the empty string; the stack key is the
//! unkeyed BLAKE2b-8 of their canonical string. Two instances of a series
//! with the same key share a stack, and stacks of a series that only the
//! echo number told apart are one ([`Echo`], [`one_echo`]).

use std::borrow::Cow;
use std::collections::HashSet;
use std::hash::Hash;

use blake2::digest::consts::U8;
use blake2::{Blake2b, Digest};
use nils_dicom::{Extracted, Level, Value};

use crate::batch::canonical_value;

/// A confidence strictly below this counts an `orientation_oblique`
/// diagnostic. A plane at exactly this confidence is not oblique, which is
/// what the name says and what every other threshold in the engine does.
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

    /// The class a name of [`Class::name`] names.
    pub fn of_name(name: &str) -> Option<Class> {
        match name {
            "Axial" => Some(Class::Axial),
            "Coronal" => Some(Class::Coronal),
            "Sagittal" => Some(Class::Sagittal),
            _ => None,
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

/// The signature of one instance: the key of the stack it belongs to, what
/// its echo adds to that key, and its orientation.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    /// Sixteen hex characters: BLAKE2b-8 of the canonical string.
    pub key: String,
    pub echo: Echo,
    pub orientation: Orientation,
}

impl Signature {
    pub fn of(x: &Extracted) -> Signature {
        let orientation = orientation(iop(x));
        let v = |column: &str| x.value(Level::Stack, column);
        Signature {
            key: key_of(&canonical_of(v, orientation.class)),
            echo: Echo::of(v, orientation.class),
            orientation,
        }
    }
}

/// What the echo adds to a stack's signature (wave 7a). A file cannot say
/// what its EchoNumbers (0018,0086) counts, and not every vendor writes an
/// echo there: one writes the frame of a cine, another the turn of a slice.
/// Its series can say, once every file of it is in ([`one_echo`]), and this
/// is what that takes of each stack: the key of its signature without the
/// echo number and the echo time, and those two in their normal form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Echo {
    /// The key of the other twelve values and the orientation class, with
    /// the Dixon part a Philips frame names, which the key carries where a
    /// file's frames disagree on it.
    pub rest: String,
    /// The echo number as read; empty where the file writes none.
    pub number: String,
    /// The echo time rounded as the key rounds it, or none where that is
    /// zero or the file writes none: a zero echo time is not evidence of a
    /// short one (record 35, S4), and states no echo time at all.
    pub time: Option<String>,
}

impl Echo {
    /// The echo of anything that has the stack-level columns, as the
    /// signature is computed: a file, a group of an enhanced object's frames,
    /// or a stack row read back.
    pub fn of<'a>(v: impl Fn(&str) -> Option<&'a Value>, class: Class) -> Echo {
        let mut rest = canonical_of(
            |column| match column {
                "echo_numbers" | "echo_time" => None,
                _ => v(column),
            },
            class,
        );
        rest.push('|');
        rest.push_str(dixon_part_of(v(FRAME_IMAGE_TYPE)).unwrap_or_default());
        let time = rounded(v("echo_time"), 2);
        Echo {
            rest: key_of(&rest),
            number: as_read(v("echo_numbers")).into_owned(),
            time: (!time.is_empty() && time != "0.00").then(|| time.into_owned()),
        }
    }
}

/// Whether stacks of one series that agree on all of their signature but the
/// echo (one [`Echo::rest`]) are one echo the echo number split: two or more
/// echo numbers, and never two echo times. A real multi-echo acquisition
/// states an echo time for each echo, and keeps a stack per echo; a series
/// that states one echo time, or none, for all of its echo numbers is
/// counting something else with them, and its stacks are one.
pub fn one_echo<N: Eq + Hash, T: Eq + Hash>(
    echoes: impl IntoIterator<Item = (N, Option<T>)>,
) -> bool {
    let mut numbers = HashSet::new();
    let mut times = HashSet::new();
    for (number, time) in echoes {
        numbers.insert(number);
        times.extend(time);
    }
    numbers.len() > 1 && times.len() < 2
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
    canonical_of(|column| x.value(Level::Stack, column), class)
}

/// The canonical string of anything that has the stack-level columns: a file,
/// or one group of the frames of an enhanced object (record 37, S8). A frame
/// contributes exactly what an instance contributes.
fn canonical_of<'a>(v: impl Fn(&str) -> Option<&'a Value>, class: Class) -> String {
    let values: [Cow<'a, str>; 14] = [
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
        push_escaped(&mut out, value);
    }
    out
}

/// A value onto a canonical string, a `|` or a `\` escaped with a backslash.
fn push_escaped(out: &mut String, value: &str) {
    for c in value.chars() {
        if c == '|' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
}

use nils_dicom::frames::{FRAME_IMAGE_TYPE, dixon_part_of};

fn iop(x: &Extracted) -> Option<&str> {
    text_of(x.value(Level::Stack, "image_orientation_patient"))
}

fn text_of(v: Option<&Value>) -> Option<&str> {
    match v {
        Some(Value::Text(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// One stack a file holds: its signature and which of the file's frames are
/// in it (record 37, S8). A classic instance holds one, and names no frames.
#[derive(Debug, Clone, PartialEq)]
pub struct FileStack {
    pub signature: Signature,
    /// The frames of the file in this stack, counting from one, as ascending
    /// inclusive ranges. Empty when every frame of the file is in this one
    /// stack, which is every file but a split enhanced object.
    pub ranges: Vec<(u32, u32)>,
    /// How many frames of the file are in this stack; one for a classic
    /// instance.
    pub frames: u32,
    /// The stack-level values of the frames in this stack, in catalogue
    /// order, when they are a group's and not the file's own. The stack row
    /// is written from these, so a second stack says what its frames said.
    pub values: Option<Vec<Option<Value>>>,
}

impl FileStack {
    /// The frames as a list a person can read: `1-4,9,12-20`; empty when the
    /// stack holds the whole file.
    pub fn list(&self) -> String {
        let mut out = String::new();
        for (i, (a, b)) in self.ranges.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            if a == b {
                out.push_str(&a.to_string());
            } else {
                out.push_str(&format!("{a}-{b}"));
            }
        }
        out
    }

    /// The first frame of the file in this stack, counting from one.
    pub fn first_frame(&self) -> u32 {
        self.ranges.first().map(|(a, _)| *a).unwrap_or(1)
    }
}

/// The stacks one file holds (record 37, S8): one, as before, unless the
/// frames of an enhanced multi-frame object state more than one, in which
/// case each group of frames that agrees on the fourteen values is a stack of
/// its own. Where the groups disagree on the Dixon part (W, F, IP or OP) the
/// ImageType Philips writes per frame names, the part is a fifteenth value,
/// and each part is a stack.
/// The first is the stack of the file's first frame, the one an instance is
/// filed under.
pub fn stacks_of(x: &Extracted) -> Vec<FileStack> {
    if x.frames.groups.len() < 2 {
        return vec![FileStack {
            signature: Signature::of(x),
            ranges: Vec::new(),
            frames: x.frames.count.max(1),
            values: None,
        }];
    }
    // A Philips enhanced Dixon object writes its parts (W, F, IP, OP) only in
    // the ImageType of each frame's (2005,140F) item, which is not one of the
    // fourteen values, so its parts would round back to one signature. Only
    // when the file's groups disagree on the Dixon part that ImageType names
    // is the part a fifteenth value, so every other file, classic or enhanced
    // (one whose frames differ there as magnitude and phase do included),
    // keeps the key it always had.
    let part = |g: &nils_dicom::FrameGroup| dixon_part_of(g.value(FRAME_IMAGE_TYPE));
    let first = part(&x.frames.groups[0]);
    let parts = x.frames.groups[1..].iter().any(|g| part(g) != first);
    let mut out: Vec<FileStack> = Vec::new();
    for g in &x.frames.groups {
        let orientation = orientation(text_of(g.value("image_orientation_patient")));
        let v = |c: &str| g.value(c);
        let mut canonical = canonical_of(v, orientation.class);
        if parts {
            canonical.push('|');
            canonical.push_str(part(g).unwrap_or_default());
        }
        let signature = Signature {
            key: key_of(&canonical),
            echo: Echo::of(v, orientation.class),
            orientation,
        };
        match out.iter_mut().find(|s| s.signature.key == signature.key) {
            Some(s) => {
                s.ranges.extend(g.ranges.iter().copied());
                s.frames += g.count;
            }
            None => out.push(FileStack {
                signature,
                ranges: g.ranges.clone(),
                frames: g.count,
                values: Some(g.values.clone()),
            }),
        }
    }
    // groups that differ in the raw values may still round to one signature:
    // then the file is one stack again, and names no frames
    if out.len() == 1 {
        out[0].ranges.clear();
        out[0].values = None;
        return out;
    }
    for s in &mut out {
        s.ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(s.ranges.len());
        for (a, b) in s.ranges.drain(..) {
            match merged.last_mut() {
                Some(last) if last.1 + 1 >= a => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        s.ranges = merged;
    }
    out
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

    /// Stacks the echo number split are one where no two of them state
    /// different echo times, and a zero or absent time states none.
    #[test]
    fn an_echo_number_is_an_echo_only_where_the_echo_time_moves_with_it() {
        let stated = |t: &'static str| Some(t);
        // a cine: 32 echo numbers, the time stated on two of them
        let cine: Vec<(String, Option<&str>)> = (1..=32)
            .map(|n| (n.to_string(), if n <= 2 { stated("11.60") } else { None }))
            .collect();
        assert!(one_echo(cine.iter().map(|(n, t)| (n.as_str(), *t))));
        // slices taking turns under one echo time, and no time at all
        assert!(one_echo([("1", stated("25.50")), ("2", stated("25.50"))]));
        assert!(one_echo([("1", None::<&str>), ("2", None)]));
        // a dual echo; three echoes, two of them stated; and a stated time
        // beside one the file left out
        assert!(!one_echo([("1", stated("10.00")), ("2", stated("80.00"))]));
        assert!(!one_echo([
            ("1", stated("10.00")),
            ("2", stated("20.00")),
            ("3", None)
        ]));
        assert!(one_echo([("1", stated("10.00")), ("2", None)]));
        // one echo number is nothing to fold
        assert!(!one_echo([("1", stated("10.00")), ("1", None)]));
        assert!(!one_echo(std::iter::empty::<(&str, Option<&str>)>()));
    }

    /// The echo of a file: the key of its signature without the echo number
    /// and the echo time, and those two as the key reads them.
    #[test]
    fn the_echo_of_a_file() {
        use dicom_core::VR;
        use dicom_dictionary_std::tags;
        use nils_dicom::synth::{MetaFields, TempDir, minimal_mr, part10, text};

        let dir = TempDir::new("stack-echo");
        let file = |name: &str, te: &str, en: &str, tr: &str| {
            let mut elems = minimal_mr("1.2.3", "1.2.3.4", name);
            elems.push(text(tags::ECHO_TIME, VR::DS, te));
            elems.push(text(tags::ECHO_NUMBERS, VR::IS, en));
            elems.push(text(tags::REPETITION_TIME, VR::DS, tr));
            elems.push(text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M"));
            let path = dir.file(name, &part10(&MetaFields::mr(name), &elems, true));
            Signature::of(&nils_dicom::extract(&path).unwrap())
        };
        let first = file("1.2.3.4.1", "11.604", "1", "40.9");
        let fifth = file("1.2.3.4.5", "0", "5", "40.9");
        let other = file("1.2.3.4.6", "-0.001", "5", "150");
        assert_ne!(first.key, fifth.key);
        assert_eq!(first.echo.rest, fifth.echo.rest);
        assert_ne!(first.echo.rest, other.echo.rest);
        assert_eq!(first.echo.number, "1");
        assert_eq!(first.echo.time.as_deref(), Some("11.60"));
        assert_eq!(fifth.echo.number, "5");
        assert_eq!(fifth.echo.time, None);
        assert_eq!(other.echo.time, None);
        // the rest is the key of the signature with both left empty
        assert_eq!(
            first.echo.rest,
            key_of("||||40.9||||||||Axial|ORIGINAL\\\\PRIMARY\\\\M|")
        );
    }

    /// An enhanced MR object: the top-level ImageType of a Philips Dixon
    /// object, which names no part, and one per-frame item per frame, each
    /// with its orientation and, where given, a private per-frame sequence
    /// (`private`) whose first item holds an ImageType.
    fn enhanced(private: dicom_core::Tag, frames: &[(&str, Option<&str>)]) -> Extracted {
        use dicom_core::VR;
        use dicom_dictionary_std::tags;
        use nils_dicom::synth::{TempDir, enhanced_meta, enhanced_mr, fg_orientation, seq, text};

        let per_frame: Vec<Vec<nils_dicom::synth::Elem>> = frames
            .iter()
            .map(|(iop, image_type)| {
                let mut groups = vec![fg_orientation(iop)];
                if let Some(t) = image_type {
                    groups.push(seq(private, vec![vec![text(tags::IMAGE_TYPE, VR::CS, t)]]));
                }
                groups
            })
            .collect();
        let mut elems = enhanced_mr("1.2.3", "1.2.3.4", "1.2.3.4.5", Vec::new(), per_frame);
        elems.push(text(
            tags::IMAGE_TYPE,
            VR::CS,
            "DERIVED\\PRIMARY\\DIXON\\NONE",
        ));
        let dir = TempDir::new("stack-enhanced");
        let path = dir.file(
            "a.dcm",
            &nils_dicom::synth::part10(&enhanced_meta("1.2.3.4.5"), &elems, true),
        );
        nils_dicom::extract(&path).unwrap()
    }

    const PHILIPS: dicom_core::Tag = dicom_core::Tag(0x2005, 0x140F);
    const SIEMENS: dicom_core::Tag = dicom_core::Tag(0x0021, 0x1201);
    const AXIAL: &str = "1\\0\\0\\0\\1\\0";
    const SAGITTAL: &str = "0\\1\\0\\0\\0\\-1";
    /// The fourteen values of an axial and a sagittal frame of [`enhanced`],
    /// as every build before the Philips per-frame ImageType wrote them.
    const AXIAL_14: &str = "||||||||||||Axial|DERIVED\\\\PRIMARY\\\\DIXON\\\\NONE";
    const SAGITTAL_14: &str = "||||||||||||Sagittal|DERIVED\\\\PRIMARY\\\\DIXON\\\\NONE";

    /// The keys a classic image and an enhanced object whose groups agree on
    /// the Philips per-frame ImageType are given are the keys of the fourteen
    /// values, byte for byte as before.
    #[test]
    fn keys_are_unchanged_where_the_frame_image_type_does_not_disagree() {
        use dicom_core::VR;
        use dicom_dictionary_std::tags;
        use nils_dicom::synth::{MetaFields, TempDir, minimal_mr, part10, text};

        // a classic image: the pinned key of `the_canonical_string_of_a_file`
        let dir = TempDir::new("stack-classic");
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
        assert_eq!(x.value(Level::Stack, FRAME_IMAGE_TYPE), None);
        let stacks = stacks_of(&x);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].signature.key, "e77101de3f76b1de");

        // an enhanced object whose frames all name the same part: one stack,
        // the key of its fourteen values
        let x = enhanced(
            PHILIPS,
            &[(AXIAL, Some("DERIVED\\PRIMARY\\W\\W\\DERIVED")); 4],
        );
        assert_eq!(
            x.value(Level::Stack, FRAME_IMAGE_TYPE),
            Some(&Value::Text("DERIVED\\PRIMARY\\W\\W\\DERIVED".into()))
        );
        let stacks = stacks_of(&x);
        assert_eq!(stacks.len(), 1);
        assert!(stacks[0].ranges.is_empty());
        assert_eq!(canonical(&x), AXIAL_14);
        assert_eq!(stacks[0].signature.key, key_of(AXIAL_14));

        // two orientations, one part: two stacks, each the key of its
        // fourteen values, as record 37 S8 split them
        let x = enhanced(
            PHILIPS,
            &[
                (AXIAL, Some("DERIVED\\PRIMARY\\W\\W\\DERIVED")),
                (AXIAL, Some("DERIVED\\PRIMARY\\W\\W\\DERIVED")),
                (SAGITTAL, Some("DERIVED\\PRIMARY\\W\\W\\DERIVED")),
                (SAGITTAL, Some("DERIVED\\PRIMARY\\W\\W\\DERIVED")),
            ],
        );
        let keys: Vec<String> = stacks_of(&x).into_iter().map(|s| s.signature.key).collect();
        assert_eq!(keys, [key_of(AXIAL_14), key_of(SAGITTAL_14)]);

        // two orientations and no private ImageType at all: the same
        let x = enhanced(
            PHILIPS,
            &[
                (AXIAL, None),
                (AXIAL, None),
                (SAGITTAL, None),
                (SAGITTAL, None),
            ],
        );
        let keys: Vec<String> = stacks_of(&x).into_iter().map(|s| s.signature.key).collect();
        assert_eq!(keys, [key_of(AXIAL_14), key_of(SAGITTAL_14)]);
    }

    /// A Philips enhanced Dixon object whose frames name four parts in the
    /// ImageType of their (2005,140F) items is four stacks, each the key of
    /// its fourteen values and its part.
    #[test]
    fn the_parts_of_a_philips_dixon_object_are_stacks_of_their_own() {
        let parts = ["W", "F", "IP", "OP"];
        let types: Vec<String> = parts
            .iter()
            .map(|p| format!("DERIVED\\PRIMARY\\{p}\\{p}\\DERIVED"))
            .collect();
        // interleaved: W, F, IP, OP, W, F, IP, OP
        let frames: Vec<(&str, Option<&str>)> = (0..8)
            .map(|i| (AXIAL, Some(types[i % 4].as_str())))
            .collect();
        let x = enhanced(PHILIPS, &frames);
        assert_eq!(x.frames.groups.len(), 4);
        let stacks = stacks_of(&x);
        assert_eq!(stacks.len(), 4);
        for (i, (s, p)) in stacks.iter().zip(parts).enumerate() {
            let want = format!("{AXIAL_14}|{p}");
            assert_eq!(s.signature.key, key_of(&want), "{p}");
            let n = i as u32 + 1;
            assert_eq!(s.ranges, [(n, n), (n + 4, n + 4)], "{p}");
            assert_eq!(s.frames, 2);
            let values = s.values.as_ref().expect("a part's own values");
            let column = nils_dicom::catalogue::fields_of(Level::Stack)
                .position(|(_, f)| f.column == FRAME_IMAGE_TYPE)
                .unwrap();
            assert_eq!(
                values[column],
                Some(Value::Text(format!("DERIVED\\PRIMARY\\{p}\\{p}\\DERIVED")))
            );
        }
        // the first stack is the first frame's, which the file is filed under
        assert_eq!(stacks[0].first_frame(), 1);
        // and a part's echo is never another part's, so no fold joins them
        let rests: HashSet<&str> = stacks
            .iter()
            .map(|s| s.signature.echo.rest.as_str())
            .collect();
        assert_eq!(rests.len(), 4);
    }

    /// A Philips enhanced object whose frames differ in their (2005,140F)
    /// ImageType only as magnitude and phase do (a QMap's M_SE and PHASE MAP)
    /// names no Dixon part: one stack, with the key of its fourteen values,
    /// as before; and beside a second orientation, the two stacks it always
    /// had, with their old keys.
    #[test]
    fn magnitude_and_phase_frames_keep_their_key() {
        let m = "ORIGINAL\\PRIMARY\\M_SE\\M\\SE";
        let p = "ORIGINAL\\PRIMARY\\PHASE MAP\\P\\SE";
        let x = enhanced(
            PHILIPS,
            &[
                (AXIAL, Some(m)),
                (AXIAL, Some(p)),
                (AXIAL, Some(m)),
                (AXIAL, Some(p)),
            ],
        );
        let stacks = stacks_of(&x);
        assert_eq!(stacks.len(), 1);
        assert!(stacks[0].ranges.is_empty());
        assert_eq!(stacks[0].frames, 4);
        assert_eq!(stacks[0].signature.key, key_of(AXIAL_14));

        let x = enhanced(
            PHILIPS,
            &[
                (AXIAL, Some(m)),
                (AXIAL, Some(p)),
                (SAGITTAL, Some(m)),
                (SAGITTAL, Some(p)),
            ],
        );
        let stacks = stacks_of(&x);
        let keys: Vec<&str> = stacks.iter().map(|s| s.signature.key.as_str()).collect();
        assert_eq!(keys, [key_of(AXIAL_14), key_of(SAGITTAL_14)]);
        assert_eq!(stacks[0].ranges, [(1, 2)]);
        assert_eq!(stacks[1].ranges, [(3, 4)]);
    }

    /// A Siemens enhanced object's (0021,1201) items carry an ImageType too,
    /// and it is never read as the Philips one: the frames that differ only
    /// there are one stack, with the key they always had.
    #[test]
    fn a_siemens_enhanced_object_is_not_split_by_its_private_image_type() {
        let x = enhanced(
            SIEMENS,
            &[
                (AXIAL, Some("ORIGINAL\\PRIMARY\\M\\NORM")),
                (AXIAL, Some("ORIGINAL\\PRIMARY\\P\\NORM")),
                (AXIAL, Some("ORIGINAL\\PRIMARY\\M\\NORM")),
                (AXIAL, Some("ORIGINAL\\PRIMARY\\P\\NORM")),
            ],
        );
        assert_eq!(x.value(Level::Stack, FRAME_IMAGE_TYPE), None);
        assert!(!x.frames.split());
        let stacks = stacks_of(&x);
        assert_eq!(stacks.len(), 1);
        assert!(stacks[0].ranges.is_empty());
        assert_eq!(stacks[0].signature.key, key_of(AXIAL_14));
    }
}
