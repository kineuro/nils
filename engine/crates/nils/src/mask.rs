// SPDX-License-Identifier: AGPL-3.0-only
//! Job type 3, check a result (record 55 H3; the build spec of
//! 2026-10-11, sections 2 to 4): what a mask a pipeline made is, as the
//! check reads it.
//!
//! - **Its grid**, read from its NIfTI header when it is registered: the
//!   dimensions, the voxel-to-world affine (the sform, else the qform, in
//!   RAS millimetres) and the data type. Only the header is read for it.
//! - **How it is tied to a scan**: `same` where its voxels are the scan's
//!   own (the lattices agree within 0.01 mm and 0.001 in direction),
//!   `world` where it is another grid in the scan's frame, `none` where no
//!   scan is known. A mask tied by `none` is never drawn.
//! - **Its parts**, made in its own grid: the connected components of the
//!   labels a set names (26-connected unless the set says 6 or 18), or each
//!   label value of a label map. Per part the voxels, the volume, the
//!   anchor (the voxel nearest the centre of mass, so the view lands inside
//!   a curved lesion), the box, and the order along the grid scan's slice
//!   direction, bottom to top. Nothing is dropped for its size, and a set
//!   of more than 10,000 parts is read whole.
//! - **Its layer** on a scan: a label pyramid with exactly the shape,
//!   levels and planes of the scan's picture pyramid, the mask resampled
//!   onto each of the scan's voxels by world position (nearest), so the
//!   desk draws it with the code that draws the pictures. The images are
//!   never resampled. A coarse voxel takes a label when any of its fine
//!   voxels has one (the most common), so a one-voxel lesion is still seen
//!   at the coarsest level.
//!
//! A scan's voxel (z, y, x) of level 0 sits at `origin + x·dx·row +
//! y·dy·col + z·step` in LPS, from its pyramid's manifest; LPS to RAS
//! flips the first two axes.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::pyramid::{Level, Manifest, TILE};

/// The most parts a set keeps; a set with more is read whole.
pub(crate) const MAX_PARTS: usize = nils_registry::derivative::PARTS_MAX;

/// How near two grids must be to be one (section 2.3): millimetres for a
/// position and a spacing, and the largest difference of a direction's
/// components.
pub(crate) const SAME_MM: f64 = 0.01;
pub(crate) const SAME_DIRECTION: f64 = 0.001;

/// The note a set too large to be lesions keeps.
pub(crate) fn too_many_note() -> String {
    format!(
        "more than {} parts: not a lesion mask",
        group(MAX_PARTS as u64)
    )
}

fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ------------------------------------------------------------ the grid

/// A mask's grid as its header says it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Grid {
    /// `[i, j, k]`: the fastest axis first, as NIfTI keeps it.
    pub dims: [usize; 3],
    /// Voxel `(i, j, k, 1)` to RAS millimetres.
    pub affine: [[f64; 4]; 4],
    pub datatype: String,
    /// Where the affine came from: `sform`, `qform`, or `none` when the
    /// header says nothing of where the voxels are.
    pub form: String,
}

impl Grid {
    /// As the registry keeps it.
    pub(crate) fn doc(&self) -> Value {
        json!({
            "dims": self.dims, "affine": self.affine, "datatype": self.datatype,
            "form": self.form,
        })
    }

    /// From what the registry keeps.
    #[cfg(test)]
    pub(crate) fn from_doc(v: &Value) -> Option<Grid> {
        let dims: [usize; 3] = serde_json::from_value(v["dims"].clone()).ok()?;
        let affine: [[f64; 4]; 4] = serde_json::from_value(v["affine"].clone()).ok()?;
        Some(Grid {
            dims,
            affine,
            datatype: v["datatype"].as_str().unwrap_or("").to_string(),
            form: v["form"].as_str().unwrap_or("sform").to_string(),
        })
    }

    /// Whether the header says where the voxels are.
    pub(crate) fn placed(&self) -> bool {
        self.form != "none"
    }

    /// Voxel `(i, j, k)` to LPS millimetres, as a 3 by 4 matrix.
    fn lps(&self) -> [[f64; 4]; 3] {
        let a = &self.affine;
        [
            [-a[0][0], -a[0][1], -a[0][2], -a[0][3]],
            [-a[1][0], -a[1][1], -a[1][2], -a[1][3]],
            [a[2][0], a[2][1], a[2][2], a[2][3]],
        ]
    }

    /// The lattice of its voxels' centres, in LPS.
    pub(crate) fn lattice(&self) -> Lattice {
        let m = self.lps();
        Lattice {
            origin: [m[0][3], m[1][3], m[2][3]],
            axes: [
                [m[0][0], m[1][0], m[2][0]],
                [m[0][1], m[1][1], m[2][1]],
                [m[0][2], m[1][2], m[2][2]],
            ],
            counts: self.dims,
        }
    }

    /// One voxel's volume in cubic millimetres.
    pub(crate) fn voxel_mm3(&self) -> f64 {
        let l = self.lattice();
        det3(l.axes).abs()
    }
}

// ------------------------------------------------------------ NIfTI

/// What a NIfTI-1 header says, and where its voxels start.
struct Header {
    grid: Grid,
    code: i16,
    offset: usize,
    big: bool,
    slope: f64,
    inter: f64,
}

/// The data types a mask may be stored in, by NIfTI code: the name and
/// the bytes per voxel.
fn datatype(code: i16) -> Option<(&'static str, usize)> {
    Some(match code {
        2 => ("uint8", 1),
        4 => ("int16", 2),
        8 => ("int32", 4),
        16 => ("float32", 4),
        64 => ("float64", 8),
        256 => ("int8", 1),
        512 => ("uint16", 2),
        768 => ("uint32", 4),
        1024 => ("int64", 8),
        1280 => ("uint64", 8),
        _ => return None,
    })
}

/// A file as a reader, through gzip when it is compressed.
fn open(path: &Path) -> Result<Box<dyn Read>, String> {
    let mut f =
        std::fs::File::open(path).map_err(|e| format!("the mask's file is not there: {e}"))?;
    let mut magic = [0u8; 2];
    let n = f.read(&mut magic).map_err(|e| e.to_string())?;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let _ = n;
    if magic == [0x1f, 0x8b] {
        Ok(Box::new(flate2::read::MultiGzDecoder::new(
            std::io::BufReader::new(f),
        )))
    } else {
        Ok(Box::new(std::io::BufReader::new(f)))
    }
}

fn header_of(bytes: &[u8]) -> Result<Header, String> {
    let not = || "not a NIfTI-1 file: a mask is read from its NIfTI header".to_string();
    if bytes.len() < 348 {
        return Err(not());
    }
    let size_le = i32::from_le_bytes(bytes[0..4].try_into().unwrap_or_default());
    let size_be = i32::from_be_bytes(bytes[0..4].try_into().unwrap_or_default());
    let big = match (size_le, size_be) {
        (348, _) => false,
        (_, 348) => true,
        _ => return Err(not()),
    };
    if &bytes[344..347] != b"n+1" && &bytes[344..347] != b"ni1" {
        return Err(not());
    }
    let i16_at = |at: usize| {
        let b: [u8; 2] = bytes[at..at + 2].try_into().unwrap_or_default();
        if big {
            i16::from_be_bytes(b)
        } else {
            i16::from_le_bytes(b)
        }
    };
    let f32_at = |at: usize| {
        let b: [u8; 4] = bytes[at..at + 4].try_into().unwrap_or_default();
        f64::from(if big {
            f32::from_be_bytes(b)
        } else {
            f32::from_le_bytes(b)
        })
    };
    let rank = i16_at(40);
    if !(1..=7).contains(&rank) {
        return Err(not());
    }
    let dim = |i: usize| -> usize {
        if i as i16 <= rank {
            i16_at(40 + 2 * i).max(1) as usize
        } else {
            1
        }
    };
    if (4..=7).any(|i| dim(i) > 1) {
        return Err(format!(
            "the mask holds {} volumes; a mask is one volume",
            (4..=7).map(dim).product::<usize>()
        ));
    }
    let dims = [dim(1), dim(2), dim(3)];
    let code = i16_at(70);
    let (name, _) = datatype(code).ok_or_else(|| {
        format!("the mask's data type (NIfTI code {code}) is not one a mask is stored in")
    })?;
    let pix = [f32_at(80), f32_at(84), f32_at(88)];
    let qfac = if f32_at(76) < 0.0 { -1.0 } else { 1.0 };
    let offset = f32_at(108).max(352.0) as usize;
    let slope = f32_at(112);
    let inter = f32_at(116);
    let (qform, sform) = (i16_at(252), i16_at(254));
    let (affine, form) = if sform > 0 {
        let row = |at: usize| [f32_at(at), f32_at(at + 4), f32_at(at + 8), f32_at(at + 12)];
        (
            [row(280), row(296), row(312), [0.0, 0.0, 0.0, 1.0]],
            "sform",
        )
    } else if qform > 0 {
        let (b, c, d) = (f32_at(256), f32_at(260), f32_at(264));
        let a = (1.0 - (b * b + c * c + d * d)).max(0.0).sqrt();
        let r = [
            [
                a * a + b * b - c * c - d * d,
                2.0 * (b * c - a * d),
                2.0 * (b * d + a * c),
            ],
            [
                2.0 * (b * c + a * d),
                a * a + c * c - b * b - d * d,
                2.0 * (c * d - a * b),
            ],
            [
                2.0 * (b * d - a * c),
                2.0 * (c * d + a * b),
                a * a + d * d - c * c - b * b,
            ],
        ];
        let s = [pix[0], pix[1], pix[2] * qfac];
        let q = [f32_at(268), f32_at(272), f32_at(276)];
        let mut m = [[0.0; 4]; 4];
        for (i, row) in m.iter_mut().take(3).enumerate() {
            for j in 0..3 {
                row[j] = r[i][j] * s[j];
            }
            row[3] = q[i];
        }
        m[3][3] = 1.0;
        (m, "qform")
    } else {
        (
            [
                [pix[0].abs().max(f64::MIN_POSITIVE), 0.0, 0.0, 0.0],
                [0.0, pix[1].abs().max(f64::MIN_POSITIVE), 0.0, 0.0],
                [0.0, 0.0, pix[2].abs().max(f64::MIN_POSITIVE), 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            "none",
        )
    };
    Ok(Header {
        grid: Grid {
            dims,
            affine,
            datatype: name.to_string(),
            form: form.to_string(),
        },
        code,
        offset,
        big,
        slope,
        inter,
    })
}

/// The most bytes a mask's header may declare, its voxels and its offset:
/// a 1024 cubed label map of four bytes a voxel.
pub(crate) const MASK_BYTES_MAX: usize = 4 << 30;

/// How a mask too large to read is refused.
const TOO_LARGE: &str = "the mask's header declares more than";

/// Whether a mask could not be read because its header declares more than
/// a mask holds.
pub(crate) fn too_large(why: &str) -> bool {
    why.starts_with(TOO_LARGE)
}

/// A mask's grid, from its header alone.
pub(crate) fn read_grid(path: &Path) -> Result<Grid, String> {
    let mut r = open(path)?;
    let mut head = vec![0u8; 348];
    r.read_exact(&mut head)
        .map_err(|_| "not a NIfTI-1 file: it is shorter than a header".to_string())?;
    Ok(header_of(&head)?.grid)
}

/// A mask whole: its grid and every voxel's label, `i` fastest. A value
/// is scaled by the header's slope where it has one and rounded; a value
/// of 0 or below is no label.
pub(crate) fn read_labels(path: &Path) -> Result<(Grid, Vec<u32>), String> {
    // the header first, and of the rest no more than it declares: a file
    // that unpacks to more is read no further (the review of E1, item 15)
    let mut r = open(path)?;
    let mut bytes = vec![0u8; 348];
    r.read_exact(&mut bytes)
        .map_err(|_| "not a NIfTI-1 file: it is shorter than a header".to_string())?;
    let h = header_of(&bytes)?;
    let (_, width) = datatype(h.code).unwrap_or(("uint8", 1));
    let n = h
        .grid
        .dims
        .iter()
        .try_fold(1usize, |acc, d| acc.checked_mul(*d))
        .ok_or_else(|| format!("{TOO_LARGE} {} GiB of voxels", MASK_BYTES_MAX >> 30))?;
    let end = n
        .checked_mul(width)
        .and_then(|v| v.checked_add(h.offset))
        .filter(|end| *end <= MASK_BYTES_MAX)
        .ok_or_else(|| {
            format!(
                "{TOO_LARGE} {} GiB of voxels; a mask is read no further",
                MASK_BYTES_MAX >> 30
            )
        })?;
    (&mut r)
        .take(end.saturating_sub(bytes.len()) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("the mask's file: {e}"))?;
    if bytes.len() < end {
        return Err(format!(
            "the mask's file holds {} bytes of voxels; its header says {}",
            bytes.len().saturating_sub(h.offset),
            n * width
        ));
    }
    let data = &bytes[h.offset..end];
    let scaled = h.slope != 0.0 && (h.slope != 1.0 || h.inter != 0.0);
    let label = |v: f64| -> u32 {
        let v = if scaled { v * h.slope + h.inter } else { v };
        let r = v.round();
        if r.is_finite() && r > 0.0 {
            r.min(f64::from(u32::MAX)) as u32
        } else {
            0
        }
    };
    let take = |i: usize| -> [u8; 8] {
        let mut b = [0u8; 8];
        b[..width].copy_from_slice(&data[i * width..(i + 1) * width]);
        if h.big {
            b[..width].reverse();
        }
        b
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let b = take(i);
        let v = match h.code {
            2 => f64::from(b[0]),
            256 => f64::from(b[0] as i8),
            4 => f64::from(i16::from_le_bytes([b[0], b[1]])),
            512 => f64::from(u16::from_le_bytes([b[0], b[1]])),
            8 => f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            768 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            16 => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            64 => f64::from_le_bytes(b),
            1024 => i64::from_le_bytes(b) as f64,
            1280 => u64::from_le_bytes(b) as f64,
            _ => 0.0,
        };
        out.push(label(v));
    }
    Ok((h.grid, out))
}

/// A NIfTI-1 file of u8 or u16 labels with an sform (the tests' masks).
#[cfg(test)]
pub(crate) fn nifti(dims: [usize; 3], affine: [[f64; 4]; 4], labels: &[u32]) -> Vec<u8> {
    let wide = labels.iter().any(|l| *l > 255);
    let mut h = vec![0u8; 352];
    h[0..4].copy_from_slice(&348i32.to_le_bytes());
    let dim: [i16; 8] = [
        3,
        dims[0] as i16,
        dims[1] as i16,
        dims[2] as i16,
        1,
        1,
        1,
        1,
    ];
    for (i, d) in dim.iter().enumerate() {
        h[40 + 2 * i..42 + 2 * i].copy_from_slice(&d.to_le_bytes());
    }
    let (code, bits): (i16, i16) = if wide { (512, 16) } else { (2, 8) };
    h[70..72].copy_from_slice(&code.to_le_bytes());
    h[72..74].copy_from_slice(&bits.to_le_bytes());
    for (i, v) in [1.0f32, 1.0, 1.0, 1.0].iter().enumerate() {
        h[76 + 4 * i..80 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    h[108..112].copy_from_slice(&352f32.to_le_bytes());
    h[254..256].copy_from_slice(&1i16.to_le_bytes());
    for (r, at) in [280usize, 296, 312].iter().enumerate() {
        for c in 0..4 {
            h[at + 4 * c..at + 4 * c + 4].copy_from_slice(&(affine[r][c] as f32).to_le_bytes());
        }
    }
    h[344..348].copy_from_slice(b"n+1\0");
    for l in labels {
        if wide {
            h.extend_from_slice(&(*l as u16).to_le_bytes());
        } else {
            h.push(*l as u8);
        }
    }
    h
}

// ------------------------------------------------------------ lattices

/// The centres of a grid's voxels in LPS: `origin + Σ index[a]·axes[a]`,
/// `counts[a]` along each axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Lattice {
    pub origin: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub counts: [usize; 3],
}

impl Lattice {
    fn point(&self, at: [f64; 3]) -> [f64; 3] {
        let mut p = self.origin;
        for (a, axis) in self.axes.iter().enumerate() {
            for (k, v) in p.iter_mut().enumerate() {
                *v += at[a] * axis[k];
            }
        }
        p
    }
}

/// A scan's lattice at level 0, from its pyramid's manifest: `x` along the
/// rows, `y` down the columns, `z` from plane to plane. Refused where the
/// manifest does not say where its planes are, or they are not parallel
/// and evenly spaced: such a scan gets no overlay in this round.
pub(crate) fn scan_lattice(m: &Manifest) -> Result<Lattice, String> {
    if !m.orientation_known {
        return Err(format!(
            "stack {} does not say where its planes are, so no result is drawn on it",
            m.stack
        ));
    }
    if let Some(f) = m.frame
        && (!f.parallel || !f.evenly_spaced)
    {
        return Err(format!(
            "the planes of stack {} are not parallel and evenly spaced, so no result is drawn on it in this round",
            m.stack
        ));
    }
    let o = &m.orientation;
    let row = [o[0], o[1], o[2]];
    let col = [o[3], o[4], o[5]];
    let [dz, dy, dx] = m.spacing;
    let n = crate::pyramid::normal(o);
    let step = m.step.unwrap_or([n[0] * dz, n[1] * dz, n[2] * dz]);
    Ok(Lattice {
        origin: m.origin,
        axes: [
            [row[0] * dx, row[1] * dx, row[2] * dx],
            [col[0] * dy, col[1] * dy, col[2] * dy],
            step,
        ],
        counts: [
            m.shape[2] as usize,
            m.shape[1] as usize,
            m.shape[0] as usize,
        ],
    })
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let n = norm(v);
    if n > 0.0 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        v
    }
}

fn det3(a: [[f64; 3]; 3]) -> f64 {
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[1][0] * (a[0][1] * a[2][2] - a[0][2] * a[2][1])
        + a[2][0] * (a[0][1] * a[1][2] - a[0][2] * a[1][1])
}

/// Whether two lattices are one: the same voxel centres, whatever order
/// and direction each walks them in (dcm2niix flips the rows and may
/// reverse the planes), within [`SAME_MM`] and [`SAME_DIRECTION`].
pub(crate) fn same_lattice(a: &Lattice, b: &Lattice) -> bool {
    let mut used = [false; 3];
    // per axis of a: the axis of b it walks, and whether backwards
    let mut pairing = [(0usize, false); 3];
    for (i, ax) in a.axes.iter().enumerate() {
        let ua = unit(*ax);
        let mut found = None;
        for (j, bx) in b.axes.iter().enumerate() {
            if used[j] {
                continue;
            }
            let ub = unit(*bx);
            for back in [false, true] {
                let s = if back { -1.0 } else { 1.0 };
                let close = (0..3).all(|k| (ua[k] - s * ub[k]).abs() <= SAME_DIRECTION);
                if close && (norm(*ax) - norm(*bx)).abs() <= SAME_MM && a.counts[i] == b.counts[j] {
                    found = Some((j, back));
                }
            }
        }
        let Some((j, back)) = found else {
            return false;
        };
        used[j] = true;
        pairing[i] = (j, back);
    }
    // a's first voxel is b's voxel at the start or the end of each axis
    let mut at = [0.0; 3];
    for (i, (j, back)) in pairing.iter().enumerate() {
        at[*j] = if *back { (a.counts[i] - 1) as f64 } else { 0.0 };
    }
    let q = b.point(at);
    norm([a.origin[0] - q[0], a.origin[1] - q[1], a.origin[2] - q[2]]) <= SAME_MM
}

/// How a mask is tied to a scan, and the scan: `same` where its lattice is
/// the scan's, `world` where it is another grid in the scan's frame, and
/// `none` where no scan is known, the frames differ, or the header says
/// nothing of where the voxels are.
pub(crate) fn link_of(grid: &Grid, scan: Option<&Lattice>, same_frame: bool) -> &'static str {
    if !grid.placed() || !same_frame {
        return "none";
    }
    match scan {
        Some(l) if same_lattice(&grid.lattice(), l) => "same",
        _ => "world",
    }
}

/// Invert a 3 by 4 affine (a 3 by 3 and a translation).
fn invert(m: [[f64; 4]; 3]) -> Option<[[f64; 4]; 3]> {
    let a = [
        [m[0][0], m[0][1], m[0][2]],
        [m[1][0], m[1][1], m[1][2]],
        [m[2][0], m[2][1], m[2][2]],
    ];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = [
        [
            (a[1][1] * a[2][2] - a[1][2] * a[2][1]) / det,
            (a[0][2] * a[2][1] - a[0][1] * a[2][2]) / det,
            (a[0][1] * a[1][2] - a[0][2] * a[1][1]) / det,
        ],
        [
            (a[1][2] * a[2][0] - a[1][0] * a[2][2]) / det,
            (a[0][0] * a[2][2] - a[0][2] * a[2][0]) / det,
            (a[0][2] * a[1][0] - a[0][0] * a[1][2]) / det,
        ],
        [
            (a[1][0] * a[2][1] - a[1][1] * a[2][0]) / det,
            (a[0][1] * a[2][0] - a[0][0] * a[2][1]) / det,
            (a[0][0] * a[1][1] - a[0][1] * a[1][0]) / det,
        ],
    ];
    let t = [m[0][3], m[1][3], m[2][3]];
    let mut out = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = inv[i][j];
        }
        out[i][3] = -(inv[i][0] * t[0] + inv[i][1] * t[1] + inv[i][2] * t[2]);
    }
    Some(out)
}

/// The map from a scan's voxel `(x, y, z)` to the mask's voxel `(i, j,
/// k)`, both as fractions: the mask's inverse affine of the scan voxel's
/// position.
fn scan_to_mask(grid: &Grid, scan: &Lattice) -> Result<[[f64; 4]; 3], String> {
    let inv = invert(grid.lps()).ok_or("the mask's affine has no inverse")?;
    // scan voxel to LPS
    let s = [
        [
            scan.axes[0][0],
            scan.axes[1][0],
            scan.axes[2][0],
            scan.origin[0],
        ],
        [
            scan.axes[0][1],
            scan.axes[1][1],
            scan.axes[2][1],
            scan.origin[1],
        ],
        [
            scan.axes[0][2],
            scan.axes[1][2],
            scan.axes[2][2],
            scan.origin[2],
        ],
    ];
    let mut out = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            let mut v = 0.0;
            for k in 0..3 {
                v += inv[i][k] * s[k][j];
            }
            if j == 3 {
                v += inv[i][3];
            }
            out[i][j] = v;
        }
    }
    Ok(out)
}

// ------------------------------------------------------------ parts

/// What a set reads a mask as.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SetSpec {
    pub key: String,
    pub words: String,
    /// `components`, `labels` or `whole`.
    pub kind: String,
    /// The label values it reads; none is every value but 0.
    pub labels: Option<Vec<i64>>,
    pub names: BTreeMap<i64, String>,
    pub connectivity: u8,
}

impl SetSpec {
    /// The one set a mask that declares none is read as.
    pub(crate) fn whole() -> SetSpec {
        SetSpec {
            key: nils_pipeline::descriptor::WHOLE.to_string(),
            words: "Whole result".to_string(),
            kind: "whole".to_string(),
            labels: None,
            names: BTreeMap::new(),
            connectivity: 26,
        }
    }

    /// A set a descriptor declares.
    pub(crate) fn of(p: &nils_pipeline::descriptor::PartSet) -> SetSpec {
        SetSpec {
            key: p.key.clone(),
            words: p.words.clone(),
            kind: p.kind.clone(),
            labels: p.labels.clone(),
            names: p.names.clone(),
            connectivity: p.connectivity,
        }
    }

    /// Back from the registry's `part_sets`.
    pub(crate) fn from_doc(v: &Value) -> Option<SetSpec> {
        Some(SetSpec {
            key: v["key"].as_str()?.to_string(),
            words: v["words"].as_str().unwrap_or("").to_string(),
            kind: v["kind"].as_str()?.to_string(),
            labels: v["labels"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_i64).collect()),
            names: v["names"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, n)| Some((k.parse().ok()?, n.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            connectivity: v["connectivity"].as_u64().unwrap_or(26) as u8,
        })
    }

    /// As the registry keeps it, with how many parts it holds and the note
    /// of a set read whole.
    pub(crate) fn doc(&self, count: usize, note: Option<&str>) -> Value {
        let mut v = json!({
            "key": self.key, "words": self.words, "kind": self.kind,
            "labels": match &self.labels {
                Some(l) => json!(l),
                None if self.kind == "whole" => Value::Null,
                None => json!("nonzero"),
            },
            "count": count,
        });
        if !self.names.is_empty() {
            v["names"] = json!(
                self.names
                    .iter()
                    .map(|(k, n)| (k.to_string(), json!(n)))
                    .collect::<serde_json::Map<_, _>>()
            );
        }
        if self.kind == "components" {
            v["connectivity"] = json!(self.connectivity);
        }
        if let Some(n) = note {
            v["note"] = json!(n);
        }
        v
    }

    fn reads(&self, value: u32) -> bool {
        value > 0
            && self
                .labels
                .as_ref()
                .is_none_or(|l| l.contains(&i64::from(value)))
    }
}

/// The parts of one set, or why the set is read whole.
pub(crate) enum Made {
    Parts(Vec<nils_registry::derivative::Part>),
    /// More than [`MAX_PARTS`]: the set is kept whole, with this note.
    Whole(String),
}

/// The neighbours of a voxel at a connectivity: the offsets in `(i, j, k)`.
fn neighbours(connectivity: u8) -> Vec<[i64; 3]> {
    let mut out = Vec::new();
    for dk in -1i64..=1 {
        for dj in -1i64..=1 {
            for di in -1i64..=1 {
                let n = di.abs() + dj.abs() + dk.abs();
                let take = match connectivity {
                    6 => n == 1,
                    18 => n == 1 || n == 2,
                    _ => n >= 1,
                };
                if take {
                    out.push([di, dj, dk]);
                }
            }
        }
    }
    out
}

/// Each voxel's component (from 1, 0 for none) and how many there are;
/// none when there are more than `limit`.
fn components(
    dims: [usize; 3],
    data: &[u32],
    set: &SetSpec,
    limit: usize,
) -> Option<(Vec<u32>, usize)> {
    let [ni, nj, nk] = dims;
    let offsets = neighbours(set.connectivity);
    let mut comp = vec![0u32; data.len()];
    let mut n = 0usize;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..data.len() {
        if comp[start] != 0 || !set.reads(data[start]) {
            continue;
        }
        n += 1;
        if n > limit {
            return None;
        }
        comp[start] = n as u32;
        stack.push(start);
        while let Some(v) = stack.pop() {
            let i = (v % ni) as i64;
            let j = ((v / ni) % nj) as i64;
            let k = (v / (ni * nj)) as i64;
            for o in &offsets {
                let (a, b, c) = (i + o[0], j + o[1], k + o[2]);
                if a < 0 || b < 0 || c < 0 || a >= ni as i64 || b >= nj as i64 || c >= nk as i64 {
                    continue;
                }
                let w = a as usize + ni * (b as usize + nj * c as usize);
                if comp[w] == 0 && set.reads(data[w]) {
                    comp[w] = n as u32;
                    stack.push(w);
                }
            }
        }
    }
    Some((comp, n))
}

/// Each voxel's slot (from 1, 0 for none) by its label value, the values
/// in slot order, or none when there are more than `limit`.
fn by_value(data: &[u32], set: &SetSpec, limit: usize) -> Option<(Vec<u32>, Vec<u32>)> {
    let mut slots: HashMap<u32, u32> = HashMap::new();
    let mut values: Vec<u32> = Vec::new();
    let mut out = vec![0u32; data.len()];
    for (v, slot) in data.iter().zip(out.iter_mut()) {
        if !set.reads(*v) {
            continue;
        }
        let s = match slots.get(v) {
            Some(s) => *s,
            None => {
                if values.len() >= limit {
                    return None;
                }
                values.push(*v);
                let s = values.len() as u32;
                slots.insert(*v, s);
                s
            }
        };
        *slot = s;
    }
    Some((out, values))
}

/// The parts of a set: one per component or per value, each with its
/// voxels, volume, anchor and box (LPS, millimetres), in order along the
/// mask's own slice axis turned up ([`order_normal`]), bottom to top. A
/// component is numbered by that order, from 1; a label keeps its value.
pub(crate) fn make_parts(grid: &Grid, data: &[u32], set: &SetSpec) -> Made {
    make_parts_limited(grid, data, set, MAX_PARTS)
}

fn make_parts_limited(grid: &Grid, data: &[u32], set: &SetSpec, limit: usize) -> Made {
    if set.kind == "whole" {
        return Made::Parts(Vec::new());
    }
    let (slot, count, values) = if set.kind == "components" {
        match components(grid.dims, data, set, limit) {
            Some((c, n)) => (c, n, Vec::new()),
            None => return Made::Whole(too_many_note()),
        }
    } else {
        match by_value(data, set, limit) {
            Some((s, v)) => {
                let n = v.len();
                (s, n, v)
            }
            None => return Made::Whole(too_many_note()),
        }
    };
    let l = grid.lattice();
    let [ni, nj, _] = grid.dims;
    let at = |v: usize| -> [f64; 3] {
        let i = (v % ni) as f64;
        let j = ((v / ni) % nj) as f64;
        let k = (v / (ni * nj)) as f64;
        l.point([i, j, k])
    };
    #[derive(Clone)]
    struct Acc {
        n: u64,
        sum: [f64; 3],
        min: [f64; 3],
        max: [f64; 3],
        best: (f64, [f64; 3]),
    }
    let mut acc = vec![
        Acc {
            n: 0,
            sum: [0.0; 3],
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
            best: (f64::INFINITY, [0.0; 3]),
        };
        count
    ];
    for (v, s) in slot.iter().enumerate() {
        if *s == 0 {
            continue;
        }
        let p = at(v);
        let a = &mut acc[*s as usize - 1];
        a.n += 1;
        for (k, v) in p.iter().enumerate() {
            a.sum[k] += v;
            a.min[k] = a.min[k].min(*v);
            a.max[k] = a.max[k].max(*v);
        }
    }
    // the anchor: the part's voxel nearest its centre of mass
    let centres: Vec<[f64; 3]> = acc
        .iter()
        .map(|a| {
            let n = a.n.max(1) as f64;
            [a.sum[0] / n, a.sum[1] / n, a.sum[2] / n]
        })
        .collect();
    for (v, s) in slot.iter().enumerate() {
        if *s == 0 {
            continue;
        }
        let p = at(v);
        let c = centres[*s as usize - 1];
        let d = norm([p[0] - c[0], p[1] - c[1], p[2] - c[2]]);
        let a = &mut acc[*s as usize - 1];
        if d < a.best.0 {
            a.best = (d, p);
        }
    }
    // a voxel reaches half its extent beyond its centre along each axis
    let half: [f64; 3] = std::array::from_fn(|w| {
        0.5 * (l.axes[0][w].abs() + l.axes[1][w].abs() + l.axes[2][w].abs())
    });
    let mm3 = grid.voxel_mm3();
    let n = order_normal(grid);
    let mut order: Vec<usize> = (0..count).collect();
    let along = |i: usize| {
        let p = acc[i].best.1;
        p[0] * n[0] + p[1] * n[1] + p[2] * n[2]
    };
    order.sort_by(|a, b| {
        along(*a)
            .partial_cmp(&along(*b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(b))
    });
    let method = if set.kind == "components" {
        format!(
            "{}-connected, nils {}",
            set.connectivity,
            env!("CARGO_PKG_VERSION")
        )
    } else {
        format!("labels, nils {}", env!("CARGO_PKG_VERSION"))
    };
    let round = |v: f64| (v * 1000.0).round() / 1000.0;
    let r3 = |p: [f64; 3]| [round(p[0]), round(p[1]), round(p[2])];
    let mut parts = Vec::with_capacity(count);
    for (position, i) in order.iter().enumerate() {
        let a = &acc[*i];
        let label = if set.kind == "components" {
            position as i64 + 1
        } else {
            i64::from(values[*i])
        };
        parts.push(nils_registry::derivative::Part {
            label,
            name: if set.kind == "labels" {
                set.names.get(&label).cloned()
            } else {
                None
            },
            voxels: a.n as i64,
            volume_mm3: round(a.n as f64 * mm3),
            anchor_mm: r3(a.best.1),
            box_mm: [
                r3([a.min[0] - half[0], a.min[1] - half[1], a.min[2] - half[2]]),
                r3([a.max[0] + half[0], a.max[1] + half[1], a.max[2] + half[2]]),
            ],
            position: position as i64,
            method: method.clone(),
            check_status: None,
            check_reason: None,
            check_decision_id: None,
        });
    }
    Made::Parts(parts)
}

/// The direction parts are ordered along (section 3.4, the review of E1,
/// item 4): the mask's own slice axis, turned to point up (superior, LPS),
/// so a part's number depends on the mask's geometry in the world alone:
/// not on whether a scan's picture was built when it was read, and not on
/// the order the file keeps its planes in. An axis with no up or down (a
/// sagittal or coronal mask) is turned toward the positive side of its
/// largest component.
pub(crate) fn order_normal(grid: &Grid) -> [f64; 3] {
    let n = unit(grid.lattice().axes[2]);
    let toward = if n[2].abs() > 1e-6 {
        n[2]
    } else {
        let big = (0..3)
            .max_by(|a, b| {
                n[*a]
                    .abs()
                    .partial_cmp(&n[*b].abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(2);
        n[big]
    };
    if toward < 0.0 {
        [-n[0], -n[1], -n[2]]
    } else {
        n
    }
}

// ------------------------------------------------------------ layers

/// Where a result's layer on a scan lives: beside the scan's levels.
pub(crate) fn layer_dir(working: &Path, stack: i64, sha256: &str, set: &str) -> std::path::PathBuf {
    crate::pyramid::dir(working, stack)
        .join("layers")
        .join(&sha256[..sha256.len().min(16)])
        .join(set)
}

/// What the desk's loader reads first of a layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LayerManifest {
    pub stack: i64,
    pub derivative: i64,
    pub sha256: String,
    pub set: String,
    /// `components`, `labels` or `whole`.
    pub kind: String,
    /// What a voxel holds: `part` (a component's number), `label` (the
    /// label's value) or `mask` (1).
    pub values: String,
    /// `same` or `world`.
    pub link: String,
    pub codec: String,
    pub tile: u32,
    pub levels: u32,
    /// `[nz, ny, nx]` at level 0: the scan's.
    pub shape: [u32; 3],
    pub level_shapes: Vec<Level>,
    pub dtype: String,
    /// How many voxels at level 0 hold a label.
    pub voxels: u64,
    /// The scan's picture the layer was built on, by when it was built:
    /// a picture built again makes the layer stale.
    pub scan_built_at: String,
    pub built_at: String,
    pub wall_seconds: f64,
}

/// Each mask voxel's value in a layer of a set: a component's part
/// number (matched to the parts the registry holds by their anchors), a
/// label's value, or 1 for a whole mask.
fn values_of(
    grid: &Grid,
    data: &[u32],
    set: &SetSpec,
    parts: &[nils_registry::derivative::Part],
) -> Result<(Vec<u16>, &'static str), String> {
    match set.kind.as_str() {
        "components" => {
            let (comp, n) = components(grid.dims, data, set, usize::MAX)
                .ok_or("the components could not be counted")?;
            // a component's number is the part's whose anchor lies in it
            let inv = invert(grid.lps()).ok_or("the mask's affine has no inverse")?;
            let [ni, nj, nk] = grid.dims;
            let mut number = vec![0u16; n + 1];
            for p in parts {
                let a = p.anchor_mm;
                let idx: Vec<i64> = (0..3)
                    .map(|r| {
                        (inv[r][0] * a[0] + inv[r][1] * a[1] + inv[r][2] * a[2] + inv[r][3]).round()
                            as i64
                    })
                    .collect();
                if idx
                    .iter()
                    .zip([ni, nj, nk])
                    .any(|(v, n)| *v < 0 || *v >= n as i64)
                {
                    continue;
                }
                let v = idx[0] as usize + ni * (idx[1] as usize + nj * idx[2] as usize);
                let c = comp[v] as usize;
                if c > 0 && p.label > 0 && p.label <= i64::from(u16::MAX) {
                    number[c] = p.label as u16;
                }
            }
            Ok((comp.iter().map(|c| number[*c as usize]).collect(), "part"))
        }
        "labels" => {
            if let Some(v) = data
                .iter()
                .find(|v| set.reads(**v) && **v > u32::from(u16::MAX))
            {
                return Err(format!(
                    "the label {v} is above {}, which a layer holds",
                    u16::MAX
                ));
            }
            Ok((
                data.iter()
                    .map(|v| if set.reads(*v) { *v as u16 } else { 0 })
                    .collect(),
                "label",
            ))
        }
        _ => Ok((
            data.iter().map(|v| u16::from(set.reads(*v))).collect(),
            "mask",
        )),
    }
}

/// One plane at half resolution in-plane, as the picture's levels halve:
/// a coarse voxel takes the most common label among its fine voxels, and
/// none only where all four have none.
fn halve_labels(plane: &[u16], ny: u32, nx: u32) -> Vec<u16> {
    let (hy, hx) = (ny / 2, nx / 2);
    let (ny, nx) = (ny as usize, nx as usize);
    let mut out = Vec::with_capacity((hy * hx) as usize);
    let mut seen: Vec<u16> = Vec::with_capacity(9);
    for y in 0..hy as usize {
        // the last cell of an odd side takes the row (or column) the
        // picture's halving leaves out, so a lesion on the edge stays at
        // every level (the review of E1, item 14)
        let y1 = if y + 1 == hy as usize { ny } else { 2 * y + 2 };
        for x in 0..hx as usize {
            let x1 = if x + 1 == hx as usize { nx } else { 2 * x + 2 };
            seen.clear();
            for yy in 2 * y..y1 {
                for xx in 2 * x..x1 {
                    seen.push(plane[yy * nx + xx]);
                }
            }
            let mut best = (0u16, 0usize);
            for v in seen.iter().copied() {
                if v == 0 {
                    continue;
                }
                let n = seen.iter().filter(|w| **w == v).count();
                if n > best.1 || (n == best.1 && v < best.0) {
                    best = (v, n);
                }
            }
            out.push(best.0);
        }
    }
    out
}

/// The tiles of one plane, in row-major order of its grid; a tile of no
/// label is encoded once per size.
fn encode_labels(
    plane: &[u16],
    ny: u32,
    nx: u32,
    empty: &std::sync::Mutex<HashMap<(u32, u32), Vec<u8>>>,
) -> Result<Vec<Vec<u8>>, String> {
    let (ty, tx) = (ny.div_ceil(TILE), nx.div_ceil(TILE));
    let mut out = Vec::with_capacity((ty * tx) as usize);
    let mut tile = Vec::with_capacity((TILE * TILE) as usize);
    for j in 0..ty {
        for i in 0..tx {
            let (y0, x0) = (j * TILE, i * TILE);
            let (h, w) = (TILE.min(ny - y0), TILE.min(nx - x0));
            tile.clear();
            for y in y0..y0 + h {
                tile.extend_from_slice(&plane[(y * nx + x0) as usize..(y * nx + x0 + w) as usize]);
            }
            if tile.iter().all(|v| *v == 0) {
                let mut kept = empty.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(t) = kept.get(&(w, h)) {
                    out.push(t.clone());
                    continue;
                }
                let t = crate::pyramid::encode_tile(&tile, w, h)?;
                kept.insert((w, h), t.clone());
                out.push(t);
            } else {
                out.push(crate::pyramid::encode_tile(&tile, w, h)?);
            }
        }
    }
    Ok(out)
}

/// What a layer is built from besides the mask.
pub(crate) struct LayerOf<'a> {
    pub derivative: i64,
    pub sha256: &'a str,
    pub link: &'a str,
    pub set: &'a SetSpec,
    pub parts: &'a [nils_registry::derivative::Part],
}

/// Build a result's layer on a scan under `root`: level 0 resampled from
/// the mask by world position onto every voxel of the scan, each coarser
/// level from the one before as the picture's levels are, the tiles packed
/// one file a level, and the manifest written last.
pub(crate) fn build_layer(
    root: &Path,
    scan: &Manifest,
    grid: &Grid,
    data: &[u32],
    of: &LayerOf<'_>,
    workers: usize,
) -> Result<LayerManifest, String> {
    let started = std::time::Instant::now();
    let lattice = scan_lattice(scan)?;
    let (values, what) = values_of(grid, data, of.set, of.parts)?;
    let to_mask = scan_to_mask(grid, &lattice)?;
    let [nz, ny, nx] = scan.shape;
    let [ni, nj, nk] = grid.dims;
    let workers = workers.max(1);
    // level 0: each scan voxel takes the mask's voxel at its position
    let plane_of = |z: u32| -> Vec<u16> {
        let mut out = vec![0u16; (ny * nx) as usize];
        let m = &to_mask;
        for y in 0..ny {
            let base: [f64; 3] =
                std::array::from_fn(|r| m[r][1] * f64::from(y) + m[r][2] * f64::from(z) + m[r][3]);
            for x in 0..nx {
                let xf = f64::from(x);
                let i = (base[0] + m[0][0] * xf).round();
                let j = (base[1] + m[1][0] * xf).round();
                let k = (base[2] + m[2][0] * xf).round();
                if i < 0.0 || j < 0.0 || k < 0.0 {
                    continue;
                }
                let (i, j, k) = (i as usize, j as usize, k as usize);
                if i >= ni || j >= nj || k >= nk {
                    continue;
                }
                out[(y * nx + x) as usize] = values[i + ni * (j + nj * k)];
            }
        }
        out
    };
    let zs: Vec<u32> = (0..nz).collect();
    let chunk = (zs.len() / workers).max(1);
    let mut current: Vec<Vec<u16>> = std::thread::scope(|s| {
        let handles: Vec<_> = zs
            .chunks(chunk)
            .map(|part| s.spawn(|| part.iter().map(|z| plane_of(*z)).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    if current.len() != nz as usize {
        return Err("a worker building the layer failed".into());
    }
    let voxels = current
        .iter()
        .map(|p| p.iter().filter(|v| **v != 0).count() as u64)
        .sum();
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let empty = std::sync::Mutex::new(HashMap::new());
    let mut level_shapes = Vec::new();
    let (mut ly, mut lx) = (ny, nx);
    for want in &scan.level_shapes {
        if [nz, ly, lx] != want.shape {
            return Err(format!(
                "level {} of the scan's picture is {:?}; the layer's would be {:?}",
                want.level,
                want.shape,
                [nz, ly, lx]
            ));
        }
        let planes = &current;
        let results: Vec<Result<Vec<Vec<Vec<u8>>>, String>> = std::thread::scope(|s| {
            let handles: Vec<_> = planes
                .chunks(chunk)
                .map(|part| {
                    let empty = &empty;
                    s.spawn(move || {
                        part.iter()
                            .map(|p| encode_labels(p, ly, lx, empty))
                            .collect::<Result<Vec<_>, String>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err("a worker panicked".into())))
                .collect()
        });
        let mut tiles: Vec<Vec<Vec<u8>>> = Vec::with_capacity(planes.len());
        for r in results {
            tiles.extend(r?);
        }
        let flat: Vec<&[u8]> = tiles.iter().flatten().map(Vec::as_slice).collect();
        let bytes = crate::tilepack::write_level(
            root,
            want.level,
            [nz, want.tiles[0], want.tiles[1]],
            &flat,
        )?;
        level_shapes.push(Level {
            level: want.level,
            shape: want.shape,
            tiles: want.tiles,
            bytes,
        });
        if (want.level as usize) + 1 < scan.level_shapes.len() {
            current = current.iter().map(|p| halve_labels(p, ly, lx)).collect();
            ly /= 2;
            lx /= 2;
        }
    }
    let manifest = LayerManifest {
        stack: scan.stack,
        derivative: of.derivative,
        sha256: of.sha256.to_string(),
        set: of.set.key.clone(),
        kind: of.set.kind.clone(),
        values: what.to_string(),
        link: of.link.to_string(),
        codec: crate::pyramid::CODEC.to_string(),
        tile: TILE,
        levels: level_shapes.len() as u32,
        shape: scan.shape,
        level_shapes,
        dtype: "uint16".to_string(),
        voxels,
        scan_built_at: scan.built_at.clone(),
        built_at: nils_registry::time::now_iso(),
        wall_seconds: started.elapsed().as_secs_f64(),
    };
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    crate::pyramid::write_manifest(root, &text)?;
    Ok(manifest)
}

/// A layer's manifest, when it is built on the scan's picture as it is
/// now; a layer built on a picture since built again is none.
pub(crate) fn layer_manifest(root: &Path, scan: &Manifest) -> Option<LayerManifest> {
    let text = std::fs::read_to_string(root.join("manifest.json")).ok()?;
    let m: LayerManifest = serde_json::from_str(&text).ok()?;
    (m.scan_built_at == scan.built_at && m.shape == scan.shape).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scan's manifest as the pyramid writes it, from its geometry.
    fn manifest(
        shape: [u32; 3],
        spacing: [f64; 3],
        orientation: [f64; 6],
        origin: [f64; 3],
        step: Option<[f64; 3]>,
    ) -> Manifest {
        let [nz, mut ly, mut lx] = shape;
        let mut levels = Vec::new();
        for level in 0..crate::pyramid::LEVELS {
            levels.push(json!({"level": level, "shape": [nz, ly, lx],
                "tiles": [ly.div_ceil(TILE), lx.div_ceil(TILE)], "bytes": 0}));
            if ly < 2 || lx < 2 {
                break;
            }
            ly /= 2;
            lx /= 2;
        }
        serde_json::from_value(json!({
            "stack": 1, "codec": "htj2k", "tile": TILE, "levels": levels.len(),
            "shape": shape, "spacing": spacing, "dtype": "uint16", "intercept": 0,
            "window": {"percentiles": [0, 1], "center": 0.5, "width": 1.0},
            "bytes_per_level": [], "level_shapes": levels,
            "annotation": {"burned_in": false, "where": "header"},
            "built_at": "2026-10-11T00:00:00Z", "pack_version": null,
            "precompute": {"wall_seconds": 0.0, "workers": 1, "raw_bytes": 0},
            "orientation": orientation, "origin": origin,
            "frame": {"parallel": true, "evenly_spaced": true},
            "orientation_known": true, "step": step,
        }))
        .unwrap()
    }

    /// The affine dcm2niix writes for a scan: the columns as the rows run,
    /// the rows flipped (its j runs up the picture), the planes in order or
    /// reversed; RAS.
    fn dcm2niix(m: &Manifest, reverse: bool) -> [[f64; 4]; 4] {
        let l = scan_lattice(m).unwrap();
        let [nx, ny, nz] = l.counts;
        let mut origin = l.point([0.0, (ny - 1) as f64, 0.0]);
        let mut k = l.axes[2];
        if reverse {
            origin = l.point([0.0, (ny - 1) as f64, (nz - 1) as f64]);
            k = [-k[0], -k[1], -k[2]];
        }
        let j = [-l.axes[1][0], -l.axes[1][1], -l.axes[1][2]];
        let i = l.axes[0];
        let _ = nx;
        let ras = |v: [f64; 3]| [-v[0], -v[1], v[2]];
        let (i, j, k, o) = (ras(i), ras(j), ras(k), ras(origin));
        [
            [i[0], j[0], k[0], o[0]],
            [i[1], j[1], k[1], o[1]],
            [i[2], j[2], k[2], o[2]],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    fn level0(m: &Manifest, grid: &Grid, data: &[u32]) -> Vec<Vec<u16>> {
        let dir = nils_dicom::synth::TempDir::new("mask-layer");
        let set = SetSpec::whole();
        let made = build_layer(
            dir.path(),
            m,
            grid,
            data,
            &LayerOf {
                derivative: 1,
                sha256: "ab",
                link: "same",
                set: &set,
                parts: &[],
            },
            2,
        )
        .unwrap();
        (0..m.shape[0])
            .map(|z| {
                let p =
                    crate::pyramid::decode_plane_in(dir.path(), &made.level_shapes, 0, z).unwrap();
                p.2
            })
            .collect()
    }

    /// Section 16, E1: the world mapping round-trips a volume through
    /// dcm2niix's conventions, axial, sagittal, oblique, rows flipped and
    /// the planes reversed: a mask of the input's own grid is `same` and
    /// lands voxel for voxel.
    #[test]
    fn a_mask_of_the_input_s_own_grid_lands_voxel_for_voxel() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let cases: [(&str, [f64; 6], bool); 5] = [
            ("axial", [1.0, 0.0, 0.0, 0.0, 1.0, 0.0], false),
            (
                "axial, planes reversed",
                [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                true,
            ),
            ("sagittal", [0.0, 1.0, 0.0, 0.0, 0.0, -1.0], false),
            ("oblique", [1.0, 0.0, 0.0, 0.0, s, -s], true),
            (
                "rows reversed in the file",
                [-1.0, 0.0, 0.0, 0.0, -1.0, 0.0],
                false,
            ),
        ];
        let shape = [6u32, 20, 18];
        for (name, orientation, reverse) in cases {
            let m = manifest(
                shape,
                [3.0, 0.5, 0.75],
                orientation,
                [-40.0, 12.5, 30.0],
                None,
            );
            let affine = dcm2niix(&m, reverse);
            let dims = [18usize, 20, 6];
            let grid = Grid {
                dims,
                affine,
                datatype: "uint8".into(),
                form: "sform".into(),
            };
            assert_eq!(
                link_of(&grid, Some(&scan_lattice(&m).unwrap()), true),
                "same",
                "{name}"
            );
            // three marks at known voxels of the file
            let mut data = vec![0u32; dims.iter().product()];
            let marks = [(0usize, 0usize, 0usize), (17, 3, 5), (5, 19, 2)];
            for (i, j, k) in marks {
                data[i + 18 * (j + 20 * k)] = 1;
            }
            let planes = level0(&m, &grid, &data);
            let mut found = Vec::new();
            for (z, p) in planes.iter().enumerate() {
                for (v, l) in p.iter().enumerate() {
                    if *l != 0 {
                        found.push((v % 18, v / 18, z));
                    }
                }
            }
            found.sort_unstable();
            let mut want: Vec<(usize, usize, usize)> = marks
                .iter()
                .map(|(i, j, k)| (*i, 19 - j, if reverse { 5 - k } else { *k }))
                .collect();
            want.sort_unstable();
            assert_eq!(found, want, "{name}");
        }
        // a grid a hundredth of a millimetre off is still the same; a tenth
        // is another grid in the frame
        let m = manifest(
            shape,
            [3.0, 0.5, 0.75],
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            [0.0; 3],
            None,
        );
        let mut affine = dcm2niix(&m, false);
        affine[0][3] += 0.005;
        let mut grid = Grid {
            dims: [18, 20, 6],
            affine,
            datatype: "uint8".into(),
            form: "sform".into(),
        };
        let l = scan_lattice(&m).unwrap();
        assert_eq!(link_of(&grid, Some(&l), true), "same");
        grid.affine[0][3] += 0.1;
        assert_eq!(link_of(&grid, Some(&l), true), "world");
        // another frame, or a header that says nothing of where, is none
        assert_eq!(link_of(&grid, Some(&l), false), "none");
        grid.form = "none".into();
        assert_eq!(link_of(&grid, Some(&l), true), "none");
        let unplaced = Grid {
            form: "sform".into(),
            ..grid.clone()
        };
        assert_eq!(link_of(&unplaced, None, true), "world");
    }

    /// Section 16, E1: a mask on a 1 mm isotropic grid lands within half a
    /// voxel on a 0.5 by 0.5 by 3 mm scan.
    #[test]
    fn a_mask_on_another_grid_lands_within_half_a_voxel() {
        let m = manifest(
            [10, 64, 64],
            [3.0, 0.5, 0.5],
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            [-16.0, -16.0, -15.0],
            None,
        );
        // 1 mm iso, RAS, its first voxel at RAS (16, 16, -15): LPS (-16, -16, -15)
        let dims = [32usize, 32, 30];
        let affine = [
            [-1.0, 0.0, 0.0, 16.0],
            [0.0, -1.0, 0.0, 16.0],
            [0.0, 0.0, 1.0, -15.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let grid = Grid {
            dims,
            affine,
            datatype: "uint8".into(),
            form: "sform".into(),
        };
        assert_eq!(
            link_of(&grid, Some(&scan_lattice(&m).unwrap()), true),
            "world"
        );
        // a block of 6 by 5 by 7 voxels
        let (lo, hi) = ([10usize, 12, 8], [15usize, 16, 14]);
        let mut data = vec![0u32; dims.iter().product()];
        for k in lo[2]..=hi[2] {
            for j in lo[1]..=hi[1] {
                for i in lo[0]..=hi[0] {
                    data[i + 32 * (j + 32 * k)] = 1;
                }
            }
        }
        let gl = grid.lattice();
        let a = gl.point([lo[0] as f64, lo[1] as f64, lo[2] as f64]);
        let b = gl.point([hi[0] as f64, hi[1] as f64, hi[2] as f64]);
        let (min, max): ([f64; 3], [f64; 3]) = (
            std::array::from_fn(|w| a[w].min(b[w])),
            std::array::from_fn(|w| a[w].max(b[w])),
        );
        let planes = level0(&m, &grid, &data);
        let sl = scan_lattice(&m).unwrap();
        let mut inside = 0;
        for (z, p) in planes.iter().enumerate() {
            for (v, l) in p.iter().enumerate() {
                let c = sl.point([(v % 64) as f64, (v / 64) as f64, z as f64]);
                let within =
                    (0..3).all(|w| c[w] >= min[w] - 0.5 - 1e-9 && c[w] <= max[w] + 0.5 + 1e-9);
                let deep = (0..3).all(|w| c[w] > min[w] - 0.5 + 1e-6 && c[w] < max[w] + 0.5 - 1e-6);
                if *l != 0 {
                    assert!(within, "a voxel at {c:?} is drawn outside the block");
                    inside += 1;
                } else {
                    assert!(!deep, "a voxel at {c:?} inside the block is not drawn");
                }
            }
        }
        assert!(inside > 0);
    }

    fn cube(dims: [usize; 3], at: [usize; 3], side: usize, data: &mut [u32], value: u32) {
        for k in at[2]..at[2] + side {
            for j in at[1]..at[1] + side {
                for i in at[0]..at[0] + side {
                    data[i + dims[0] * (j + dims[1] * k)] = value;
                }
            }
        }
    }

    fn iso(dims: [usize; 3]) -> Grid {
        Grid {
            dims,
            affine: [
                [-1.0, 0.0, 0.0, 0.0],
                [0.0, -1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            datatype: "uint8".into(),
            form: "sform".into(),
        }
    }

    fn lesions(connectivity: u8) -> SetSpec {
        SetSpec {
            key: "lesions".into(),
            words: "Lesions".into(),
            kind: "components".into(),
            labels: Some(vec![99]),
            names: BTreeMap::new(),
            connectivity,
        }
    }

    fn parts(made: Made) -> Vec<nils_registry::derivative::Part> {
        match made {
            Made::Parts(p) => p,
            Made::Whole(n) => panic!("read whole: {n}"),
        }
    }

    /// The review of E1, item 15: a mask is read no further than its
    /// header declares: a stream that unpacks past its voxels into
    /// anything (here a tail cut short) reads as the mask, and a header
    /// declaring more than a mask holds is refused before anything is read.
    #[test]
    fn a_mask_is_read_no_further_than_its_header_declares() {
        let dir = nils_dicom::synth::TempDir::new("mask-bounded");
        let affine = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let labels: Vec<u32> = (0..8).map(|v| v % 2).collect();
        let mut bytes = nifti([2, 2, 2], affine, &labels);
        // ten million bytes past the voxels, compressed, the stream cut
        bytes.extend(std::iter::repeat_n(0u8, 10_000_000));
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut e, &bytes).unwrap();
        let mut gz = e.finish().unwrap();
        gz.truncate(gz.len() - 64);
        let p = dir.path().join("long.nii.gz");
        std::fs::write(&p, &gz).unwrap();
        let (g, l) = read_labels(&p).unwrap();
        assert_eq!((g.dims, l), ([2, 2, 2], labels));
        // a header that declares a 30000 cube of four-byte labels
        let mut huge = nifti([2, 2, 2], affine, &[0; 8]);
        for (i, d) in [3i16, 30000, 30000, 30000].iter().enumerate() {
            huge[40 + 2 * i..42 + 2 * i].copy_from_slice(&d.to_le_bytes());
        }
        huge[70..72].copy_from_slice(&768i16.to_le_bytes());
        huge[72..74].copy_from_slice(&32i16.to_le_bytes());
        let p = dir.path().join("huge.nii");
        std::fs::write(&p, &huge).unwrap();
        let why = read_labels(&p).unwrap_err();
        assert!(too_large(&why), "{why}");
    }

    /// The review of E1, item 14: halving a layer keeps the picture's
    /// shape, and a label in the row or column an odd side leaves over is
    /// kept in the last cell, so a one-voxel lesion on the edge is there at
    /// every level; a cell takes its most common label, never nothing
    /// where it holds one.
    #[test]
    fn a_label_on_an_odd_edge_survives_the_halving() {
        let (ny, nx) = (5u32, 7u32);
        let mut plane = vec![0u16; (ny * nx) as usize];
        plane[(4 * nx + 6) as usize] = 9;
        let half = halve_labels(&plane, ny, nx);
        assert_eq!(half.len(), 2 * 3);
        assert_eq!(half[2 * 3 - 1], 9, "{half:?}");
        let quarter = halve_labels(&half, 2, 3);
        assert_eq!(quarter, vec![9]);
        // one label among nothing is that label
        let mut one = vec![0u16; 4];
        one[1] = 3;
        assert_eq!(halve_labels(&one, 2, 2), vec![3]);
    }

    /// The review of E1, item 4: a part's number is the mask's geometry in
    /// the world, bottom to top, whatever order the file keeps its planes
    /// in: the same lesions numbered alike from a file written top down.
    #[test]
    fn parts_are_numbered_bottom_to_top_in_the_world_whatever_the_plane_order() {
        let dims = [20usize, 20, 20];
        let up = iso(dims);
        let mut down = iso(dims);
        // planes written top down: plane 0 is the highest
        down.affine[2] = [0.0, 0.0, -1.0, 19.0];
        let lesions_at = |g_down: bool| {
            let mut data = vec![0u32; dims.iter().product()];
            // a low lesion of 27 voxels and a high one of 1, in the world
            let (low, high) = if g_down { (15, 3) } else { (2, 16) };
            cube(dims, [4, 4, low], 3, &mut data, 99);
            cube(dims, [12, 12, high], 1, &mut data, 99);
            data
        };
        let a = parts(make_parts(&up, &lesions_at(false), &lesions(26)));
        let b = parts(make_parts(&down, &lesions_at(true), &lesions(26)));
        assert_eq!(a.len(), 2);
        assert_eq!(
            a.iter()
                .map(|p| (p.label, p.voxels, p.anchor_mm))
                .collect::<Vec<_>>(),
            b.iter()
                .map(|p| (p.label, p.voxels, p.anchor_mm))
                .collect::<Vec<_>>()
        );
        // bottom to top: the low lesion is part 1
        assert_eq!((a[0].label, a[0].voxels), (1, 27));
        assert!(a[0].anchor_mm[2] < a[1].anchor_mm[2]);
        // a sagittal mask: its slice axis has no up, and is turned to its
        // positive side
        let mut sag = iso(dims);
        sag.affine = [
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let n = order_normal(&sag);
        assert!(n.iter().all(|v| *v >= 0.0), "{n:?}");
    }

    /// Section 16, E1: two cubes touching at a corner are one part at
    /// 26-connectivity and two at 6; a C-shaped part's anchor is inside
    /// it; a one-voxel part is kept; a label map's parts are its values,
    /// named.
    #[test]
    fn parts_join_by_their_connectivity_and_each_anchor_lies_inside() {
        let dims = [20usize, 20, 20];
        let g = iso(dims);
        let mut data = vec![0u32; dims.iter().product()];
        cube(dims, [2, 2, 2], 3, &mut data, 99);
        cube(dims, [5, 5, 5], 3, &mut data, 99);
        // another label the set does not read
        cube(dims, [12, 2, 2], 2, &mut data, 7);
        assert_eq!(parts(make_parts(&g, &data, &lesions(26))).len(), 1);
        let two = parts(make_parts(&g, &data, &lesions(6)));
        assert_eq!(two.len(), 2);
        // bottom to top, numbered so; the volume and the box of each
        assert_eq!((two[0].label, two[0].position), (1, 0));
        assert!(two[0].anchor_mm[2] < two[1].anchor_mm[2]);
        assert_eq!(two[0].voxels, 27);
        assert_eq!(two[0].volume_mm3, 27.0);
        // RAS with i and j flipped is LPS along i, j and k
        assert_eq!(two[0].box_mm, [[1.5, 1.5, 1.5], [4.5, 4.5, 4.5]]);
        assert!(two[0].method.starts_with("6-connected, nils "));

        // a C: its centre of mass is in the gap, its anchor is not
        let mut c = vec![0u32; dims.iter().product()];
        for j in 2..12 {
            for i in 2..5 {
                c[i + 20 * (j + 20 * 5)] = 99;
            }
        }
        for i in 2..12 {
            for j in [2usize, 11] {
                c[i + 20 * (j + 20 * 5)] = 99;
            }
        }
        let p = parts(make_parts(&g, &c, &lesions(26)));
        assert_eq!(p.len(), 1);
        let a = p[0].anchor_mm;
        let (i, j, k) = (a[0] as usize, a[1] as usize, a[2] as usize);
        assert_eq!(
            c[i + 20 * (j + 20 * k)],
            99,
            "the anchor {a:?} lies in the C"
        );

        // one voxel is a part
        let mut one = vec![0u32; dims.iter().product()];
        one[3 + 20 * (4 + 20 * 5)] = 99;
        let p = parts(make_parts(&g, &one, &lesions(26)));
        assert_eq!((p.len(), p[0].voxels), (1, 1));
        assert_eq!(p[0].anchor_mm, [3.0, 4.0, 5.0]);

        // a label map: each value a part, named, the value its label
        let set = SetSpec {
            key: "structures".into(),
            words: "Structures".into(),
            kind: "labels".into(),
            labels: None,
            names: BTreeMap::from([(7, "Left-Thing".to_string())]),
            connectivity: 26,
        };
        let p = parts(make_parts(&g, &data, &set));
        let mut labels: Vec<i64> = p.iter().map(|q| q.label).collect();
        labels.sort_unstable();
        assert_eq!(labels, [7, 99]);
        let seven = p.iter().find(|q| q.label == 7).unwrap();
        assert_eq!(
            (seven.name.as_deref(), seven.voxels),
            (Some("Left-Thing"), 8)
        );
        assert!(seven.method.starts_with("labels, nils "));
    }

    /// Section 16, E1: a set of more parts than the limit is read whole,
    /// with its note; 10,001 single voxels against the 10,000 of a set.
    #[test]
    fn a_set_of_more_than_ten_thousand_parts_is_read_whole() {
        let dims = [202usize, 101, 2];
        let g = iso(dims);
        let mut data = vec![0u32; dims.iter().product()];
        let mut n = 0;
        'fill: for j in (0..101).step_by(2) {
            for i in (0..202).step_by(2) {
                for k in [0usize] {
                    data[i + 202 * (j + 101 * k)] = 99;
                    n += 1;
                    if n == 10_001 {
                        break 'fill;
                    }
                }
            }
        }
        // the rows of every other voxel make 5,151 parts; a second plane of
        // them, offset, makes the rest apart from the first
        let mut k1 = 0;
        if n < 10_001 {
            'more: for j in (1..101).step_by(2) {
                for i in (1..202).step_by(2) {
                    let _ = j;
                    data[i + 202 * (j + 101)] = 0;
                    if k1 > 0 {
                        break 'more;
                    }
                }
            }
        }
        let _ = k1;
        k1 = 0;
        // at 6-connectivity two planes' voxels at the same i and j touch;
        // put the second plane's voxels where the first has none
        for j in (1..101).step_by(2) {
            for i in (1..202).step_by(2) {
                if n == 10_001 {
                    break;
                }
                data[i + 202 * (j + 101)] = 99;
                n += 1;
                k1 += 1;
            }
        }
        assert_eq!(n, 10_001, "{k1}");
        match make_parts(&g, &data, &lesions(6)) {
            Made::Whole(note) => assert_eq!(note, "more than 10,000 parts: not a lesion mask"),
            Made::Parts(p) => panic!("{} parts kept", p.len()),
        }
        // one fewer is kept, every one
        let last = data.iter().rposition(|v| *v == 99).unwrap();
        data[last] = 0;
        assert_eq!(parts(make_parts(&g, &data, &lesions(6))).len(), 10_000);
    }

    /// Section 16, E1: a layer has the shape, levels and plane order of the
    /// scan's picture pyramid, holds each part's number where the part is,
    /// and keeps a one-voxel part at the coarsest level.
    #[test]
    fn a_layer_is_the_shape_of_the_scan_s_pyramid_and_keeps_a_one_voxel_part() {
        let (nz, ny, nx) = (5u32, 300u32, 280u32);
        // a real picture pyramid of a volume
        let vol = crate::pyramid::Volume {
            shape: [nz, ny, nx],
            spacing: [2.0, 0.5, 0.5],
            intercept: 0,
            rescale: (1.0, 0.0),
            rescale_varies: false,
            burned_in: Some(false),
            data: (0..nz * ny * nx).map(|v| (v % 4096) as u16).collect(),
            geometry: Some(crate::pyramid::Geometry {
                orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                origin: [10.0, 20.0, 30.0],
                frame: crate::pyramid::Frame {
                    parallel: true,
                    evenly_spaced: true,
                },
                step: Some([0.0, 0.0, 2.0]),
            }),
            lossy: false,
            syntaxes: Vec::new(),
            order: "position",
            multiframe_files: 0,
        };
        let pictures = nils_dicom::synth::TempDir::new("mask-scan");
        let scan = crate::pyramid::build(&vol, 7, pictures.path(), 2, None).unwrap();
        // a mask on the scan's own grid: one voxel at (x 201, y 33, z 3),
        // and a small block at z 1
        let lattice = scan_lattice(&scan).unwrap();
        let ras = |v: [f64; 3]| [-v[0], -v[1], v[2]];
        let (o, i, j, k) = (
            ras(lattice.origin),
            ras(lattice.axes[0]),
            ras(lattice.axes[1]),
            ras(lattice.axes[2]),
        );
        let grid = Grid {
            dims: [nx as usize, ny as usize, nz as usize],
            affine: [
                [i[0], j[0], k[0], o[0]],
                [i[1], j[1], k[1], o[1]],
                [i[2], j[2], k[2], o[2]],
                [0.0, 0.0, 0.0, 1.0],
            ],
            datatype: "uint8".into(),
            form: "sform".into(),
        };
        assert_eq!(link_of(&grid, Some(&lattice), true), "same");
        let at = |x: usize, y: usize, z: usize| x + nx as usize * (y + ny as usize * z);
        let mut data = vec![0u32; (nz * ny * nx) as usize];
        data[at(201, 33, 3)] = 99;
        for y in 100..104 {
            for x in 50..54 {
                data[at(x, y, 1)] = 99;
            }
        }
        let set = lesions(26);
        let made = parts(make_parts(&grid, &data, &set));
        assert_eq!(made.len(), 2);
        // bottom to top: the block (z 1) is part 1, the voxel (z 3) part 2
        assert_eq!(made[0].voxels, 16);
        assert_eq!(made[1].voxels, 1);
        let root = layer_dir(pictures.path(), 7, &"ab".repeat(32), "lesions");
        let layer = build_layer(
            &root,
            &scan,
            &grid,
            &data,
            &LayerOf {
                derivative: 9,
                sha256: &"ab".repeat(32),
                link: "same",
                set: &set,
                parts: &made,
            },
            3,
        )
        .unwrap();
        assert_eq!(layer.levels, scan.levels);
        assert_eq!(layer.shape, scan.shape);
        for (a, b) in layer.level_shapes.iter().zip(&scan.level_shapes) {
            assert_eq!((a.level, a.shape, a.tiles), (b.level, b.shape, b.tiles));
        }
        assert_eq!(layer.values, "part");
        assert_eq!(layer.voxels, 17);
        let plane = |level: u32, z: u32| {
            crate::pyramid::decode_plane_in(&root, &layer.level_shapes, level, z).unwrap()
        };
        let (w, h, p) = plane(0, 3);
        assert_eq!((w, h), (nx, ny));
        assert_eq!(p[33 * nx as usize + 201], 2);
        assert_eq!(p.iter().filter(|v| **v != 0).count(), 1);
        let (_, _, p) = plane(0, 1);
        assert_eq!(p[101 * nx as usize + 51], 1);
        // the one voxel at the coarsest level, where it falls
        let last = layer.levels - 1;
        let (w, _, p) = plane(last, 3);
        let s = 1usize << last;
        assert_eq!(p[(33 / s) * w as usize + 201 / s], 2);
        // planes with none hold none
        let (_, _, p) = plane(last, 0);
        assert!(p.iter().all(|v| *v == 0));
        // the layer stands for the picture it was built on, and a picture
        // built again makes it stale
        assert!(layer_manifest(&root, &scan).is_some());
        let mut again = scan.clone();
        again.built_at = "2026-10-12T00:00:00Z".into();
        assert!(layer_manifest(&root, &again).is_none());
        // the picture's own levels are untouched beside it
        assert!(crate::pyramid::manifest(pictures.path()).unwrap().is_some());
    }

    /// Section 4.5's costs, measured: a layer of a 512 by 512 by 200 scan,
    /// and the parts of a 256 cubed mask. Run in release:
    /// `cargo test --release -p nils --bin nils mask::tests::costs -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, run in release"]
    fn costs() {
        let (nz, ny, nx) = (200u32, 512u32, 512u32);
        let m = manifest(
            [nz, ny, nx],
            [1.0, 0.5, 0.5],
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            [-128.0, -128.0, -100.0],
            None,
        );
        // a 1 mm mask of 256 cubed over the same field, with a few hundred
        // lesions of a few voxels and one large one
        let dims = [256usize, 256, 256];
        let grid = Grid {
            dims,
            affine: [
                [-1.0, 0.0, 0.0, 128.0],
                [0.0, -1.0, 0.0, 128.0],
                [0.0, 0.0, 1.0, -128.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            datatype: "uint8".into(),
            form: "sform".into(),
        };
        let mut data = vec![0u32; dims.iter().product()];
        let mut seed = 7u64;
        for _ in 0..400 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let at = [
                20 + (seed >> 33) as usize % 210,
                20 + (seed >> 17) as usize % 210,
                40 + (seed >> 45) as usize % 170,
            ];
            cube(dims, at, 3, &mut data, 99);
        }
        cube(dims, [100, 100, 100], 30, &mut data, 99);
        let set = lesions(26);
        let t = std::time::Instant::now();
        let parts = parts(make_parts(&grid, &data, &set));
        let parts_s = t.elapsed().as_secs_f64();
        let dir = nils_dicom::synth::TempDir::new("mask-costs");
        let t = std::time::Instant::now();
        let layer = build_layer(
            dir.path(),
            &m,
            &grid,
            &data,
            &LayerOf {
                derivative: 1,
                sha256: "ab",
                link: "world",
                set: &set,
                parts: &parts,
            },
            std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .min(8),
        )
        .unwrap();
        let layer_s = t.elapsed().as_secs_f64();
        println!(
            "parts of a 256^3 mask: {} parts in {parts_s:.3} s; layer of a 512x512x200 scan: {} levels, {} voxels drawn, in {layer_s:.3} s",
            parts.len(),
            layer.levels,
            layer.voxels
        );
    }

    /// A NIfTI file as the tests write it reads back: its grid from the
    /// header alone, gzipped or not, and every voxel.
    #[test]
    fn a_nifti_mask_reads_back_its_grid_and_labels() {
        let dir = nils_dicom::synth::TempDir::new("mask-nifti");
        let affine = [
            [-0.9, 0.0, 0.0, 90.0],
            [0.0, 1.1, 0.0, -100.0],
            [0.0, 0.0, 1.2, -70.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let labels: Vec<u32> = (0..24).map(|v| if v % 5 == 0 { 99 } else { 0 }).collect();
        let bytes = nifti([4, 3, 2], affine, &labels);
        let plain = dir.path().join("m.nii");
        std::fs::write(&plain, &bytes).unwrap();
        let gz = dir.path().join("m.nii.gz");
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut e, &bytes).unwrap();
        std::fs::write(&gz, e.finish().unwrap()).unwrap();
        for p in [&plain, &gz] {
            let g = read_grid(p).unwrap();
            assert_eq!(g.dims, [4, 3, 2]);
            assert_eq!(g.form, "sform");
            assert!((g.affine[1][1] - 1.1).abs() < 1e-6);
            assert!((g.voxel_mm3() - 0.9 * 1.1 * 1.2).abs() < 1e-5);
            let (g2, l) = read_labels(p).unwrap();
            assert_eq!(g2.dims, g.dims);
            assert_eq!(l, labels);
        }
        let wide: Vec<u32> = (0..24).map(|v| v * 1000).collect();
        std::fs::write(&plain, nifti([4, 3, 2], affine, &wide)).unwrap();
        assert_eq!(read_labels(&plain).unwrap().1, wide);
        std::fs::write(&plain, b"not a mask").unwrap();
        assert!(
            read_grid(&plain)
                .unwrap_err()
                .contains("not a NIfTI-1 file")
        );
        // a doc read back is the grid
        let g = read_grid(&gz).unwrap();
        assert_eq!(Grid::from_doc(&g.doc()), Some(g));
    }
}
