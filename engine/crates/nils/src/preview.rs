// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55 H2 (E1): a stack's preview, made when the stack is sorted, so
//! no picture on the Data page waits for a job queue or a 3D volume. One
//! file per stack under the working place, beside the pyramids:
//!
//! ```text
//! previews/<stack mod 1000, three digits>/<stack>.preview
//! previews/<...>/<stack>.held.preview   (burned-in annotation only)
//! ```
//!
//! A file is `NILSPV01`, a little endian u32 with the header's length, the
//! header as JSON, then the pictures back to back: the three middle planes
//! (the stack's own plane, `axial`, and the two across it through its
//! middle, `coronal` across its rows and `sagittal` across its columns, all
//! named relative to the stack) at about 256 pixels on the long side in
//! true millimetre proportions, then every plane of the stack at display
//! size as frames, in the pyramid's plane order. Every picture is an 8-bit
//! grey baseline JPEG windowed with the window the pyramid opens at; the
//! header holds the offsets.
//!
//! The frames keep to a budget a stack (format 2): 512 pixels on the long
//! side at most, 384 for a stack of more than [`MANY_PLANES`] planes, and
//! the quality the highest whose frames come to about [`FRAMES_BUDGET`]
//! bytes in all (a frame of [`FRAME_BYTES_MIN`] at least), measured on a
//! few of the stack's own planes before the rest are encoded.
//!
//! A preview is keyed by the stack's content digest (its files' paths,
//! sizes and modification times, and the frames it takes of each): built
//! again only when that changes, so building is idempotent and resumable.
//! The pixels are decoded once a stack. A stack whose files carry
//! burned-in annotation gets a second file with the top and bottom eighths
//! of its own plane blank, which a caller below detail sensitive is
//! served, as the render door does.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use blake2::digest::consts::U16;
use blake2::{Blake2b, Digest};
use nils_registry::Param;
use nils_registry::schema::Type;
use nils_registry::store::Store;
use serde::{Deserialize, Serialize};

use crate::grants::Detail;
use crate::pyramid::{self, StackFile, Volume, Window};
use crate::serve::{Caller, Reply};

/// The preview's own format: a change makes every digest new, so every
/// preview is built again, and a file of another format is read as no
/// preview at all. Format 2 keeps the frames to a budget a stack.
pub const FORMAT: u32 = 2;
const MAGIC: &[u8; 8] = b"NILSPV01";
/// The middle planes' long side, in pixels.
pub const MIDDLE: u32 = 256;
/// The frames' long side at most, in pixels.
pub const FRAME_MAX: u32 = 512;
/// And for a stack of more than [`MANY_PLANES`] planes.
pub const FRAME_MAX_MANY: u32 = 384;
pub const MANY_PLANES: u32 = 200;
/// What a stack's frames come to in all, about: a 448-plane stack is a
/// few megabytes, not twenty.
pub const FRAMES_BUDGET: u64 = 6 << 20;
/// A frame's share of the budget is never below this, so a stack of many
/// planes keeps a picture worth reading.
pub const FRAME_BYTES_MIN: u64 = 12 << 10;
/// The JPEG quality of the middle planes, and of the frames at most.
pub const QUALITY: u8 = 85;
/// The frames' quality at least.
pub const QUALITY_MIN: u8 = 50;
pub const CODEC: &str = "jpeg";
/// The frames door's container.
pub const FRAMES_TYPE: &str = "application/x-nils-frames";
/// The middle plane names, relative to the stack.
pub const PLANES: [&str; 3] = ["axial", "coronal", "sagittal"];
/// How long a cached answer stands when the request names the digest.
pub const IMMUTABLE: &str = "private, max-age=31536000, immutable";
/// And when it does not: the browser asks again with the ETag.
pub const REVALIDATE: &str = "private, no-cache";

/// One picture in the file: where it is after the header, and its size.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Blob {
    pub offset: u64,
    pub length: u64,
    pub width: u32,
    pub height: u32,
}

/// Every plane at display size, back to back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Frames {
    pub count: u32,
    pub width: u32,
    pub height: u32,
    /// `count + 1` offsets after the header: frame `z` is
    /// `offsets[z]..offsets[z + 1]`.
    pub offsets: Vec<u64>,
}

/// What a preview file says of itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub format: u32,
    pub stack: i64,
    /// The stack's content digest the preview was made from.
    pub digest: String,
    pub codec: String,
    /// The middle planes' quality.
    pub quality: u8,
    /// The frames' quality, chosen to keep to the budget.
    #[serde(default)]
    pub frame_quality: u8,
    /// Planes, rows, columns, as the pyramid's.
    pub shape: [u32; 3],
    /// Millimetres between planes, rows and columns.
    pub spacing: [f64; 3],
    /// The window the pictures are drawn with, in the modality's values.
    pub window: Window,
    pub slope: f64,
    pub intercept: f64,
    pub orientation: [f64; 6],
    pub orientation_known: bool,
    /// The patient plane nearest the stack's own.
    pub plane: String,
    pub oblique: bool,
    /// The planes across the stack are drawn with the head up.
    pub head_up: bool,
    pub burned_in: bool,
    /// The top and bottom eighths of the stack's own plane are blank.
    pub held: bool,
    pub middle: BTreeMap<String, Blob>,
    pub frames: Frames,
    pub built_at: String,
    pub build_ms: f64,
}

/// Where a stack's preview lives under a working place.
pub fn path(working: &Path, stack: i64, held: bool) -> PathBuf {
    working
        .join("previews")
        .join(format!("{:03}", stack.rem_euclid(1000)))
        .join(format!(
            "{stack}{}.preview",
            if held { ".held" } else { "" }
        ))
}

/// One of a stack's files with what the digest is made of, and where the
/// registry says it lies (for the middle plane, found with no decode).
#[derive(Debug, Clone)]
pub struct Source {
    pub file: StackFile,
    pub size: i64,
    pub mtime_ns: i64,
    pub instance: Option<i64>,
    pub position: Option<[f64; 3]>,
    /// How many frames the file holds, as its header says.
    pub frames_in_file: Option<i64>,
    /// Its rows and columns, as its header says.
    pub matrix: Option<(u32, u32)>,
}

impl Source {
    /// How many of the stack's planes the file gives.
    fn planes(&self) -> u32 {
        match &self.file.frames {
            Some(f) => f.len() as u32,
            None => self.frames_in_file.filter(|n| *n > 1).unwrap_or(1) as u32,
        }
    }
}

/// Numbers written as DICOM writes them, `a\b\c`.
fn numbers(text: Option<&str>) -> Vec<f64> {
    text.map(|t| {
        t.split('\\')
            .filter_map(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .collect()
    })
    .unwrap_or_default()
}

/// A stack's files, as the pyramid reads them, with their sizes and times.
pub fn sources(store: &mut Store, stack: i64) -> Result<Vec<Source>, String> {
    let p = store.dialect().param(1, Type::Int);
    let whole = format!(
        "SELECT so.root, f.path, f.size, f.mtime_ns, i.instance_number, i.image_position_patient, \
         i.number_of_frames, i.rows, i.columns FROM {i} i JOIN {f} f ON f.id = i.source_file_id \
         JOIN {so} so ON so.id = f.source_id WHERE i.stack_id = {p} \
         AND NOT EXISTS (SELECT 1 FROM {fr} fr WHERE fr.instance_id = i.id)",
        i = store.qualified("instance"),
        f = store.qualified("source_file"),
        so = store.qualified("source"),
        fr = store.qualified("instance_frame"),
    );
    let framed = format!(
        "SELECT so.root, f.path, f.size, f.mtime_ns, i.instance_number, i.image_position_patient, \
         i.number_of_frames, i.rows, i.columns, fr.frames FROM {fr} fr JOIN {i} i ON i.id = fr.instance_id \
         JOIN {f} f ON f.id = i.source_file_id JOIN {so} so ON so.id = f.source_id \
         WHERE fr.stack_id = {p}",
        i = store.qualified("instance"),
        f = store.qualified("source_file"),
        so = store.qualified("source"),
        fr = store.qualified("instance_frame"),
    );
    let e = |e: nils_registry::store::Error| e.to_string();
    let mut out = Vec::new();
    for (sql, framed) in [(&whole, false), (&framed, true)] {
        for r in store.query(sql, &[Param::Int(stack)]).map_err(e)? {
            let path = Path::new(r.text(0).map_err(e)?).join(r.text(1).map_err(e)?);
            let frames = if framed {
                Some(pyramid::frame_list(r.text(9).map_err(e)?)?)
            } else {
                None
            };
            let at = numbers(r.opt_text(5).map_err(e)?);
            out.push(Source {
                file: StackFile { path, frames },
                size: r.int(2).map_err(e)?,
                mtime_ns: r.int(3).map_err(e)?,
                instance: r.opt_int(4).map_err(e)?,
                position: (at.len() >= 3).then(|| [at[0], at[1], at[2]]),
                frames_in_file: r.opt_int(6).map_err(e)?,
                matrix: match (r.opt_int(7).map_err(e)?, r.opt_int(8).map_err(e)?) {
                    (Some(rows), Some(cols)) if rows > 0 && cols > 0 => {
                        Some((rows as u32, cols as u32))
                    }
                    _ => None,
                },
            });
        }
    }
    if out.is_empty() {
        return Err(format!("stack {stack} has no files the registry can read"));
    }
    out.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    Ok(out)
}

/// The stack's content digest: its files' paths, sizes, times and frames,
/// and the preview's format, as 32 hex characters.
pub fn digest(stack: i64, files: &[Source]) -> String {
    let mut h = Blake2b::<U16>::new();
    h.update(
        format!(
            "nils-preview {FORMAT} {MIDDLE} {FRAME_MAX} {FRAME_MAX_MANY} {MANY_PLANES} {FRAMES_BUDGET} {FRAME_BYTES_MIN} {QUALITY} {QUALITY_MIN} {stack}\n"
        )
        .as_bytes(),
    );
    for s in files {
        h.update(s.file.path.as_os_str().as_encoded_bytes());
        h.update(format!("\0{}\0{}\0", s.size, s.mtime_ns).as_bytes());
        if let Some(f) = &s.file.frames {
            for n in f {
                h.update(n.to_le_bytes());
            }
        }
        h.update(b"\n");
    }
    hex::encode(h.finalize())
}

/// A picture of grey bytes as a baseline JPEG at the middle planes'
/// quality.
fn jpeg(px: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    jpeg_at(px, w, h, QUALITY)
}

/// A picture of grey bytes as a baseline JPEG, with Huffman tables made
/// for it (a tenth smaller, decoded as fast).
fn jpeg_at(px: &[u8], w: u32, h: u32, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(px.len() / 6 + 1024);
    let (w16, h16) = (
        u16::try_from(w).map_err(|_| "a picture wider than 65535")?,
        u16::try_from(h).map_err(|_| "a picture taller than 65535")?,
    );
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality);
    enc.set_optimized_huffman_tables(true);
    enc.encode(px, w16, h16, jpeg_encoder::ColorType::Luma)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// The frames' size for a stack of `nz` planes of `ny` by `nx`: the long
/// side at most [`FRAME_MAX`], [`FRAME_MAX_MANY`] for a stack of many
/// planes, the pixels' own proportions kept (a viewer scales by the
/// spacing the header says).
pub fn frame_size(nz: u32, ny: u32, nx: u32) -> (u32, u32) {
    let most = if nz > MANY_PLANES {
        FRAME_MAX_MANY
    } else {
        FRAME_MAX
    };
    let long = nx.max(ny);
    if long > most {
        let k = most as f64 / long as f64;
        (
            ((nx as f64 * k).round() as u32).max(1),
            ((ny as f64 * k).round() as u32).max(1),
        )
    } else {
        (nx, ny)
    }
}

/// What one frame of a stack of `nz` planes may come to, about.
pub fn frame_budget(nz: u32) -> u64 {
    (FRAMES_BUDGET / u64::from(nz.max(1))).max(FRAME_BYTES_MIN)
}

/// The frames' quality: the highest, in steps of five, whose frames of a
/// few sample planes come to the budget on average. `sample` are those
/// planes at frame size.
fn frame_quality(sample: &[Vec<u8>], fw: u32, fh: u32, budget: u64) -> Result<u8, String> {
    if sample.is_empty() {
        return Ok(QUALITY);
    }
    let mean = |q: u8| -> Result<u64, String> {
        let mut n = 0u64;
        for p in sample {
            n += jpeg_at(p, fw, fh, q)?.len() as u64;
        }
        Ok(n / sample.len() as u64)
    };
    if mean(QUALITY)? <= budget {
        return Ok(QUALITY);
    }
    // the size falls with the quality: halve the steps between
    let (mut lo, mut hi) = (QUALITY_MIN / 5, QUALITY / 5 - 1);
    if mean(lo * 5)? > budget {
        return Ok(QUALITY_MIN);
    }
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if mean(mid * 5)? <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Ok(lo * 5)
}

/// Grey bytes resized to `w` by `h`.
fn resized(px: Vec<u8>, sw: u32, sh: u32, w: u32, h: u32) -> Result<Vec<u8>, String> {
    if (sw, sh) == (w, h) {
        return Ok(px);
    }
    let img = image::GrayImage::from_raw(sw, sh, px).ok_or("a plane does not match its size")?;
    Ok(image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle).into_raw())
}

/// The size a picture of `w_mm` by `h_mm` takes with `long` pixels on its
/// longer side, each side at least 8.
fn fit(w_mm: f64, h_mm: f64, long: u32) -> (u32, u32) {
    let ok = |v: f64| v.is_finite() && v > 0.0;
    let (w_mm, h_mm) = if ok(w_mm) && ok(h_mm) {
        (w_mm, h_mm)
    } else {
        (1.0, 1.0)
    };
    let long = long as f64;
    let (w, h) = if w_mm >= h_mm {
        (long, long * h_mm / w_mm)
    } else {
        (long * w_mm / h_mm, long)
    };
    ((w.round() as u32).max(8), (h.round() as u32).max(8))
}

/// The pictures of one set of grey planes: the three middle planes and
/// every frame, each a JPEG.
struct Drawn {
    middle: Vec<(&'static str, Vec<u8>, u32, u32)>,
    frames: Vec<Vec<u8>>,
    fw: u32,
    fh: u32,
    quality: u8,
}

/// The planes a frame quality is measured on: the middle and the two
/// quarters.
fn sample_planes(nz: u32) -> Vec<usize> {
    let mut zs: Vec<usize> = [nz / 2, nz / 4, nz * 3 / 4]
        .into_iter()
        .map(|z| z as usize)
        .collect();
    zs.sort_unstable();
    zs.dedup();
    zs
}

fn draw(
    planes: &[Vec<u8>],
    [nz, ny, nx]: [u32; 3],
    spacing: [f64; 3],
    head_up: bool,
    workers: usize,
    quality: Option<u8>,
) -> Result<Drawn, String> {
    let [dz, dy, dx] = spacing.map(|d| if d.is_finite() && d > 0.0 { d } else { 1.0 });
    let (fw, fh) = frame_size(nz, ny, nx);
    let quality = match quality {
        Some(q) => q,
        None => {
            let sample: Vec<Vec<u8>> = sample_planes(nz)
                .into_iter()
                .map(|z| resized(planes[z].clone(), nx, ny, fw, fh))
                .collect::<Result<_, _>>()?;
            frame_quality(&sample, fw, fh, frame_budget(nz))?
        }
    };
    let workers = workers.clamp(1, 64);
    let chunk = planes.len().div_ceil(workers).max(1);
    let parts: Vec<Result<Vec<Vec<u8>>, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = planes
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|p| jpeg_at(&resized(p.clone(), nx, ny, fw, fh)?, fw, fh, quality))
                        .collect::<Result<Vec<_>, String>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("an encoder panicked".into()))
            })
            .collect()
    });
    let mut frames = Vec::with_capacity(planes.len());
    for p in parts {
        frames.extend(p?);
    }
    let mut middle = Vec::new();
    let mid = &planes[(nz / 2) as usize];
    let (w, h) = fit(nx as f64 * dx, ny as f64 * dy, MIDDLE);
    middle.push((
        "axial",
        jpeg(&resized(mid.clone(), nx, ny, w, h)?, w, h)?,
        w,
        h,
    ));
    if nz > 1 {
        let (row, col) = (ny / 2, nx / 2);
        let mut across_rows = Vec::with_capacity((nz * nx) as usize);
        let mut across_cols = Vec::with_capacity((nz * ny) as usize);
        for p in planes {
            across_rows.extend_from_slice(&p[(row * nx) as usize..((row + 1) * nx) as usize]);
            across_cols.extend((0..ny).map(|y| p[(y * nx + col) as usize]));
        }
        for (name, mut px, width, w_mm) in [
            ("coronal", across_rows, nx, nx as f64 * dx),
            ("sagittal", across_cols, ny, ny as f64 * dy),
        ] {
            if head_up {
                // the first plane is the lowest: drawn at the bottom
                let w = width as usize;
                let rows: Vec<Vec<u8>> = px.chunks(w).rev().map(<[u8]>::to_vec).collect();
                px = rows.concat();
            }
            let (w, h) = fit(w_mm, nz as f64 * dz, MIDDLE);
            middle.push((name, jpeg(&resized(px, width, nz, w, h)?, w, h)?, w, h));
        }
    }
    Ok(Drawn {
        middle,
        frames,
        fw,
        fh,
        quality,
    })
}

/// The bytes of a preview file.
fn assemble(mut header: Header, drawn: Drawn) -> Result<Vec<u8>, String> {
    let mut at = 0u64;
    let mut body: Vec<u8> = Vec::new();
    for (name, bytes, w, h) in drawn.middle {
        header.middle.insert(
            name.to_string(),
            Blob {
                offset: at,
                length: bytes.len() as u64,
                width: w,
                height: h,
            },
        );
        at += bytes.len() as u64;
        body.extend_from_slice(&bytes);
    }
    let mut offsets = Vec::with_capacity(drawn.frames.len() + 1);
    for f in &drawn.frames {
        offsets.push(at);
        at += f.len() as u64;
    }
    offsets.push(at);
    header.frame_quality = drawn.quality;
    header.frames = Frames {
        count: drawn.frames.len() as u32,
        width: drawn.fw,
        height: drawn.fh,
        offsets,
    };
    for f in drawn.frames {
        body.extend_from_slice(&f);
    }
    let text = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(12 + text.len() + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(&text);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Write a file whole or not at all: a part file of its own, renamed.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("a preview has no directory")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    static PART: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = PART.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let part = path.with_extension(format!("part.{}.{n}", std::process::id()));
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&part, path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        e.to_string()
    })
}

/// What making a stack's preview did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Made {
    Built {
        bytes: u64,
    },
    /// The preview there was made from the same files.
    Current,
}

/// A volume's planes as grey bytes through `window`, with the slope and
/// intercept that make a stored value the modality's.
fn grey(vol: &Volume, window: &Window) -> (Vec<Vec<u8>>, f64, f64) {
    // stored * slope + intercept is the modality's value: the shift of a
    // signed volume undone, then the file's rescale
    let slope = vol.rescale.0;
    let intercept = vol.rescale.1 - vol.intercept as f64 * vol.rescale.0;
    let wwidth = window.width.max(1.0);
    let lo = window.center - wwidth / 2.0;
    let scale = 255.0 / wwidth;
    let (k, offset) = (slope * scale, (intercept - lo) * scale);
    // one look-up for every stored value the volume can hold
    let lut: Vec<u8> = (0..=u16::MAX as u32)
        .map(|v| (v as f64 * k + offset).clamp(0.0, 255.0) as u8)
        .collect();
    let n = (vol.shape[1] * vol.shape[2]) as usize;
    let planes = vol
        .data
        .chunks(n.max(1))
        .map(|p| p.iter().map(|v| lut[*v as usize]).collect())
        .collect();
    (planes, slope, intercept)
}

/// The top and bottom eighths of a plane blank: the band burned-in
/// annotation is written in.
fn hold_band(p: &mut [u8], ny: u32, nx: u32) {
    let band = (ny / 8) as usize;
    for y in (0..band).chain(ny as usize - band..ny as usize) {
        p[y * nx as usize..(y + 1) * nx as usize].fill(0);
    }
}

/// Make the preview of a volume read from `files`: one decode, the window,
/// the grey planes, then every picture; a second file with the band held
/// where the stack carries burned-in annotation.
pub fn write(
    vol: &Volume,
    stack: i64,
    digest: &str,
    working: &Path,
    workers: usize,
) -> Result<u64, String> {
    let started = Instant::now();
    let [nz, ny, nx] = vol.shape;
    if nz == 0 || ny == 0 || nx == 0 {
        return Err("the stack holds no plane".into());
    }
    let window = pyramid::window(vol);
    let (planes, slope, intercept) = grey(vol, &window);
    let orientation = vol.geometry.map_or(pyramid::AXIAL, |g| g.orientation);
    let (plane, oblique) = pyramid::plane_of(&orientation);
    let head_up = pyramid::normal(&orientation)[2] > 0.5;
    let burned_in = vol.burned_in.unwrap_or(false);
    let header = |held: bool| Header {
        format: FORMAT,
        stack,
        digest: digest.to_string(),
        codec: CODEC.to_string(),
        quality: QUALITY,
        frame_quality: 0,
        shape: vol.shape,
        spacing: vol.spacing,
        window: window.clone(),
        slope,
        intercept,
        orientation,
        orientation_known: vol.geometry.is_some(),
        plane: plane.to_string(),
        oblique,
        head_up,
        burned_in,
        held,
        middle: BTreeMap::new(),
        frames: Frames {
            count: 0,
            width: 0,
            height: 0,
            offsets: Vec::new(),
        },
        built_at: nils_registry::time::now_iso(),
        build_ms: 0.0,
    };
    let mut bytes = 0u64;
    let drawn = draw(&planes, vol.shape, vol.spacing, head_up, workers, None)?;
    let quality = drawn.quality;
    let mut h = header(false);
    h.build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let file = assemble(h, drawn)?;
    bytes += file.len() as u64;
    if burned_in {
        let held: Vec<Vec<u8>> = planes
            .into_iter()
            .map(|mut p| {
                hold_band(&mut p, ny, nx);
                p
            })
            .collect();
        let drawn = draw(
            &held,
            vol.shape,
            vol.spacing,
            head_up,
            workers,
            Some(quality),
        )?;
        let mut h = header(true);
        h.build_ms = started.elapsed().as_secs_f64() * 1000.0;
        let held_file = assemble(h, drawn)?;
        bytes += held_file.len() as u64;
        write_whole(&path(working, stack, true), &held_file)?;
    } else {
        let _ = std::fs::remove_file(path(working, stack, true));
    }
    // the plain file last: its digest says the set is whole
    write_whole(&path(working, stack, false), &file)?;
    stills().pop(&stack);
    forget(&path(working, stack, false));
    forget(&path(working, stack, true));
    Ok(bytes)
}

/// The digest a stack's preview file was made from, read from its header.
pub fn digest_on_disk(working: &Path, stack: i64) -> Option<String> {
    read_header(&path(working, stack, false))
        .ok()
        .map(|(h, _)| h.digest)
}

/// What a file of another format is said to be: read as no preview, so
/// it is made again.
const OLD_FORMAT: &str = "a preview of format ";

/// A file's header and where its pictures begin.
fn read_header(p: &Path) -> Result<(Header, u64), String> {
    let f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let (h, start, _) = header_of(&f, 4096)?;
    Ok((h, start))
}

/// Read a header from an open file, `first` bytes at once; answers the
/// header, where the pictures begin, and the bytes read.
fn header_of(f: &std::fs::File, first: usize) -> Result<(Header, u64, Vec<u8>), String> {
    let mut buf = vec![0u8; first];
    let mut got = 0;
    while got < buf.len() {
        match f.read_at(&mut buf[got..], got as u64) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    buf.truncate(got);
    if buf.len() < 12 || &buf[..8] != MAGIC {
        return Err("not a preview file".into());
    }
    let hlen = u32::from_le_bytes(buf[8..12].try_into().expect("four bytes")) as usize;
    if buf.len() < 12 + hlen {
        let mut more = vec![0u8; 12 + hlen - buf.len()];
        f.read_exact_at(&mut more, buf.len() as u64)
            .map_err(|e| e.to_string())?;
        buf.extend_from_slice(&more);
    }
    let header: Header = serde_json::from_slice(&buf[12..12 + hlen]).map_err(|e| e.to_string())?;
    if header.format != FORMAT {
        return Err(format!("{OLD_FORMAT}{}", header.format));
    }
    Ok((header, (12 + hlen) as u64, buf))
}

/// Make a stack's preview unless the one there was made from the same
/// files (or with `force`, always).
pub fn make(
    store: &mut Store,
    working: &Path,
    stack: i64,
    force: bool,
    workers: usize,
) -> Result<Made, (String, bool)> {
    let files = sources(store, stack).map_err(|w| (w, true))?;
    let digest = digest(stack, &files);
    if !force && digest_on_disk(working, stack).as_deref() == Some(digest.as_str()) {
        return Ok(Made::Current);
    }
    let list: Vec<StackFile> = files.into_iter().map(|s| s.file).collect();
    let vol = pyramid::read_stack(&list).map_err(|w| (w, true))?;
    let bytes = write(&vol, stack, &digest, working, workers).map_err(|w| (w, false))?;
    Ok(Made::Built { bytes })
}

/// What a build over many stacks did.
#[derive(Debug, Default)]
pub struct Many {
    pub built: Vec<i64>,
    pub current: Vec<i64>,
    /// A stack and the class of why ([`pyramid::reason_of`]), never the
    /// reader's words, which can name a file's path.
    pub failed: Vec<(i64, &'static str)>,
    pub bytes: u64,
    pub pruned: usize,
    pub stopped: bool,
    pub seconds: f64,
}

impl Many {
    pub fn as_json(&self, place: &str) -> serde_json::Value {
        serde_json::json!({
            "place": place,
            "stacks": self.built.len() + self.current.len() + self.failed.len(),
            "built": self.built.len(),
            "current": self.current.len(),
            "failed": self.failed.len(),
            "bytes": self.bytes,
            "pruned": self.pruned,
            "stopped": self.stopped,
            "seconds": (self.seconds * 1000.0).round() / 1000.0,
            "failures": self.failed.iter().take(20).map(|(s, r)| serde_json::json!({"stack": s, "reason": r})).collect::<Vec<_>>(),
            "failures_by_reason": self.failed.iter().fold(BTreeMap::<&str, usize>::new(), |mut by, (_, r)| {
                *by.entry(r).or_default() += 1;
                by
            }),
        })
    }
}

/// Make the previews of `stacks`, one stack at a time, each with `workers`
/// encoders; `go_on` is asked after each with the counts so far, and a
/// false stops there (what was made stays made).
pub fn make_many(
    store: &mut Store,
    working: &Path,
    stacks: &[i64],
    force: bool,
    workers: usize,
    go_on: &mut dyn FnMut(&mut Store, &Many) -> bool,
) -> Many {
    let started = Instant::now();
    let mut out = Many::default();
    let mut seen = BTreeSet::new();
    for &stack in stacks {
        if !seen.insert(stack) {
            continue;
        }
        match make(store, working, stack, force, workers) {
            Ok(Made::Built { bytes }) => {
                out.bytes += bytes;
                out.built.push(stack);
            }
            Ok(Made::Current) => out.current.push(stack),
            Err((why, reading)) => out.failed.push((stack, pyramid::reason_of(&why, reading))),
        }
        out.seconds = started.elapsed().as_secs_f64();
        if !go_on(store, &out) {
            out.stopped = true;
            break;
        }
    }
    out.seconds = started.elapsed().as_secs_f64();
    out
}

/// Remove the previews of stacks the registry no longer holds; answers how
/// many files went.
pub fn prune(working: &Path, keep: &BTreeSet<i64>) -> usize {
    let mut gone = 0;
    let Ok(shards) = std::fs::read_dir(working.join("previews")) else {
        return 0;
    };
    for shard in shards.flatten() {
        let Ok(files) = std::fs::read_dir(shard.path()) else {
            continue;
        };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            let Some(id) = name.split('.').next().and_then(|s| s.parse::<i64>().ok()) else {
                continue;
            };
            // a part file left by a build that died is litter too
            if (!keep.contains(&id) || name.contains(".part."))
                && std::fs::remove_file(f.path()).is_ok()
            {
                gone += 1;
            }
        }
    }
    gone
}

/// Every stack of the registry, by id.
pub fn every_stack(store: &mut Store) -> Result<Vec<i64>, String> {
    let sql = format!("SELECT id FROM {} ORDER BY id", store.qualified("stack"));
    store
        .query(&sql, &[])
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| r.int(0).map_err(|e| e.to_string()))
        .collect()
}

/// The stacks a dataset's digests created first, as its scans are.
pub fn dataset_stacks(
    store: &mut Store,
    dataset: &nils_registry::place::Place,
) -> Result<Vec<i64>, String> {
    let ids = crate::sources::source_ids(store, dataset).map_err(|e| e.to_string())?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let list = ids
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT st.id FROM {} st JOIN {} b ON b.id = st.first_batch_id \
         WHERE b.source_id IN ({list}) ORDER BY st.id",
        store.qualified("stack"),
        store.qualified("ingest_batch"),
    );
    store
        .query(&sql, &[])
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| r.int(0).map_err(|e| e.to_string()))
        .collect()
}

/// The stacks a classify job judged: what its preview step makes.
pub fn classified_by(store: &mut Store, job: i64) -> Result<Vec<i64>, String> {
    let sql = format!(
        "SELECT stack_id FROM {} WHERE job_id = {} ORDER BY stack_id",
        store.qualified("classification"),
        store.dialect().param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(job)])
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| r.int(0).map_err(|e| e.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Reading: the open files the doors keep.

/// A preview file held open: its header, its first bytes (the header and
/// the three middle planes), and the file for the frames.
pub struct Open {
    pub header: Header,
    /// Where the pictures begin in the file.
    start: u64,
    /// The file's first bytes: the header and the middle planes.
    head: Vec<u8>,
    file: std::fs::File,
    stamp: (SystemTime, u64),
    checked: Mutex<Instant>,
}

impl Open {
    fn read(p: &Path) -> Result<Option<Open>, String> {
        let file = match std::fs::File::open(p) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        let meta = file.metadata().map_err(|e| e.to_string())?;
        let stamp = (meta.modified().map_err(|e| e.to_string())?, meta.len());
        // the header and the middle planes are in the first 64 KiB of a
        // preview nearly always: one read
        let (header, start, mut head) = match header_of(&file, 64 << 10) {
            Ok(h) => h,
            Err(e) if e.starts_with(OLD_FORMAT) => return Ok(None),
            Err(e) => return Err(e),
        };
        let end = start
            + header
                .middle
                .values()
                .map(|b| b.offset + b.length)
                .max()
                .unwrap_or(0);
        if (head.len() as u64) < end {
            let mut more = vec![0u8; (end - head.len() as u64) as usize];
            file.read_exact_at(&mut more, head.len() as u64)
                .map_err(|e| e.to_string())?;
            head.extend_from_slice(&more);
        }
        head.truncate(end as usize);
        Ok(Some(Open {
            header,
            start,
            head,
            file,
            stamp,
            checked: Mutex::new(Instant::now()),
        }))
    }

    /// One middle plane's JPEG.
    pub fn middle(&self, name: &str) -> Option<(&[u8], &Blob)> {
        let b = self.header.middle.get(name)?;
        let at = (self.start + b.offset) as usize;
        Some((self.head.get(at..at + b.length as usize)?, b))
    }

    /// Frames `from..to` as one container: u32 `from`, u32 count, u32
    /// width, u32 height, `count + 1` u32 offsets from the start of the
    /// body, then the JPEGs back to back. One read of the file.
    pub fn frames(&self, from: u32, to: u32) -> Result<Vec<u8>, String> {
        let f = &self.header.frames;
        let (a, b) = (f.offsets[from as usize], f.offsets[to as usize]);
        let count = to - from;
        let lead = 16 + 4 * (count as usize + 1);
        let mut out = vec![0u8; lead + (b - a) as usize];
        out[0..4].copy_from_slice(&from.to_le_bytes());
        out[4..8].copy_from_slice(&count.to_le_bytes());
        out[8..12].copy_from_slice(&f.width.to_le_bytes());
        out[12..16].copy_from_slice(&f.height.to_le_bytes());
        for (i, z) in (from..=to).enumerate() {
            let at = lead as u64 + f.offsets[z as usize] - a;
            out[16 + 4 * i..20 + 4 * i].copy_from_slice(&(at as u32).to_le_bytes());
        }
        self.file
            .read_exact_at(&mut out[lead..], self.start + a)
            .map_err(|e| e.to_string())?;
        Ok(out)
    }
}

/// How many preview files the doors keep open.
pub const OPEN_FILES: usize = 512;
/// How long an open file is trusted before its stamp is looked at again.
const RECHECK: Duration = Duration::from_secs(2);

static OPEN: std::sync::LazyLock<Mutex<lru::LruCache<PathBuf, Arc<Open>>>> =
    std::sync::LazyLock::new(|| {
        Mutex::new(lru::LruCache::new(
            std::num::NonZeroUsize::new(OPEN_FILES).expect("not zero"),
        ))
    });

fn open_cache() -> std::sync::MutexGuard<'static, lru::LruCache<PathBuf, Arc<Open>>> {
    OPEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Drop a file from the open files: it was written again.
fn forget(p: &Path) {
    open_cache().pop(p);
}

/// How many preview files are open, for the health door.
pub fn open_count() -> usize {
    open_cache().len()
}

/// A preview file, from the open files or opened now; none where there is
/// no such file. The cache's lock is not held while a file opens.
pub fn opened(p: &Path) -> Result<Option<Arc<Open>>, String> {
    let held = open_cache().get(p).cloned();
    if let Some(o) = held {
        let mut checked = o.checked.lock().unwrap_or_else(|e| e.into_inner());
        if checked.elapsed() < RECHECK {
            return Ok(Some(Arc::clone(&o)));
        }
        let same = std::fs::metadata(p)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())))
            .is_some_and(|s| s == o.stamp);
        if same {
            *checked = Instant::now();
            return Ok(Some(Arc::clone(&o)));
        }
    }
    match Open::read(p)? {
        Some(o) => {
            let o = Arc::new(o);
            open_cache().put(p.to_path_buf(), Arc::clone(&o));
            Ok(Some(o))
        }
        None => {
            open_cache().pop(p);
            Ok(None)
        }
    }
}

/// Many previews at once, `READS_AT_ONCE` opening together: a page of
/// scans on slow storage pays one open's wait, not fifty.
pub fn opened_many(paths: &[PathBuf]) -> Vec<Option<Arc<Open>>> {
    let at_once = pyramid::READS_AT_ONCE.max(1);
    let mut out: Vec<Option<Arc<Open>>> = vec![None; paths.len()];
    std::thread::scope(|s| {
        let chunk = paths.len().div_ceil(at_once).max(1);
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|p| opened(p).ok().flatten())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut i = 0;
        for h in handles {
            for o in h.join().unwrap_or_default() {
                out[i] = o;
                i += 1;
            }
        }
    });
    out
}

/// A middle plane as a data URL.
pub fn data_url(jpeg: &[u8]) -> String {
    format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    )
}

// ---------------------------------------------------------------------------
// Made now: a door asked for a preview no sort made.
//
// The first picture is v0's: the middle plane's one file decoded in the
// request, drawn and answered at once; the whole preview is made after, on
// a thread of the engine, never in the request and never through the job
// queue. A short range of frames is decoded from its own files the same
// way; a long one waits for the preview.

/// Where a stack's middle plane is, found from the registry with no decode,
/// and the stack's digest.
#[derive(Debug, Clone)]
pub struct Plan {
    pub stack: i64,
    pub digest: String,
    /// The file (and the frame of it) the middle plane is in.
    pub middle: StackFile,
    /// Every plane's file in the pyramid's order, where each file is one
    /// plane and the registry says where each lies: what a range of frames
    /// is decoded from before the preview is made.
    pub order: Option<Vec<PathBuf>>,
    pub planes: u32,
    /// Millimetres between planes, from the positions.
    pub dz: Option<f64>,
    /// The middle file's rows and columns, as the registry says.
    pub matrix: Option<(u32, u32)>,
}

/// The stack's orientation, as the registry keeps it.
fn orientation_of(store: &mut Store, stack: i64) -> Option<[f64; 6]> {
    let sql = format!(
        "SELECT image_orientation_patient FROM {} WHERE id = {}",
        store.qualified("stack"),
        store.dialect().param(1, Type::Int)
    );
    let row = store.query_opt(&sql, &[Param::Int(stack)]).ok()??;
    let v = numbers(row.opt_text(0).ok()?);
    (v.len() >= 6).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
}

/// The plan of a stack: its files in the order the pyramid puts its planes
/// in (along the normal where every file says where it lies, else by the
/// instance number, then the path), and the middle one.
pub fn plan(store: &mut Store, stack: i64) -> Result<Plan, (String, bool)> {
    let mut files = sources(store, stack).map_err(|w| (w, true))?;
    let digest = digest(stack, &files);
    let placed = files.iter().all(|f| f.position.is_some());
    let normal = orientation_of(store, stack).map(|o| pyramid::normal(&o));
    // where each file lies along the stack: the normal, or the coordinate
    // the positions spread along most
    let along: Option<[f64; 3]> = if placed {
        normal.or_else(|| {
            let spread = |k: usize| {
                let v = files.iter().map(|f| f.position.expect("placed")[k]);
                v.clone().fold(f64::MIN, f64::max) - v.fold(f64::MAX, f64::min)
            };
            let k = (0..3)
                .max_by(|a, b| spread(*a).total_cmp(&spread(*b)))
                .unwrap_or(2);
            let mut n = [0.0; 3];
            n[k] = 1.0;
            Some(n)
        })
    } else {
        None
    };
    let key = |f: &Source| -> f64 {
        match (along, f.position) {
            (Some(n), Some(p)) => p[0] * n[0] + p[1] * n[1] + p[2] * n[2],
            _ => f.instance.map_or(f64::INFINITY, |i| i as f64),
        }
    };
    // stable: files at one place keep the paths' order
    files.sort_by(|a, b| key(a).total_cmp(&key(b)));
    let planes: Vec<(usize, Option<u32>)> = files
        .iter()
        .enumerate()
        .flat_map(|(i, f)| -> Vec<(usize, Option<u32>)> {
            match &f.file.frames {
                Some(list) => list.iter().map(|n| (i, Some(*n))).collect(),
                None if f.planes() > 1 => (1..=f.planes()).map(|n| (i, Some(n))).collect(),
                None => vec![(i, None)],
            }
        })
        .collect();
    let (file, frame) = planes[planes.len() / 2];
    let middle = StackFile {
        path: files[file].file.path.clone(),
        frames: frame.map(|n| vec![n]),
    };
    let single = planes.len() == files.len() && planes.iter().all(|(_, f)| f.is_none());
    let order = (single && placed).then(|| files.iter().map(|f| f.file.path.clone()).collect());
    let n = planes.len();
    let dz = (single && placed && n > 1)
        .then(|| (key(&files[n - 1]) - key(&files[0])).abs() / (n as f64 - 1.0))
        .filter(|d| d.is_finite() && *d > 1e-3);
    Ok(Plan {
        stack,
        digest,
        matrix: files[file].matrix,
        middle,
        order,
        planes: n as u32,
        dz,
    })
}

/// A stack's middle plane, decoded alone: its first picture while no
/// preview is made, and what the preview's header will say of it.
#[derive(Debug)]
pub struct Still {
    pub digest: String,
    /// Planes, rows, columns: the planes counted by the registry.
    pub shape: [u32; 3],
    pub spacing: [f64; 3],
    /// The window of the one plane, in the modality's values.
    pub window: Window,
    pub slope: f64,
    pub intercept: f64,
    pub orientation: [f64; 6],
    pub orientation_known: bool,
    pub plane: String,
    pub oblique: bool,
    pub head_up: bool,
    pub burned_in: bool,
    pub width: u32,
    pub height: u32,
    pub jpeg: Vec<u8>,
    /// The picture with the band blank, where the stack carries burned-in
    /// annotation.
    pub held_jpeg: Option<Vec<u8>>,
    pub decode_ms: f64,
}

/// The middle plane of a plan, decoded and drawn: one file read.
pub fn still_of(plan: &Plan) -> Result<Still, (String, bool)> {
    let started = Instant::now();
    let vol = pyramid::read_stack_at(std::slice::from_ref(&plan.middle), Some(MIDDLE))
        .map_err(|w| (w, true))?;
    let [_, ny, nx] = vol.shape;
    if ny == 0 || nx == 0 {
        return Err(("the stack's middle plane holds no pixels".into(), false));
    }
    let window = pyramid::window(&vol);
    let (planes, slope, intercept) = grey(&vol, &window);
    let p = planes.into_iter().next().ok_or((
        "the stack's middle plane holds no pixels".to_string(),
        false,
    ))?;
    let [_, dy, dx] = vol
        .spacing
        .map(|d| if d.is_finite() && d > 0.0 { d } else { 1.0 });
    let (w, h) = fit(nx as f64 * dx, ny as f64 * dy, MIDDLE);
    let draw = |px: Vec<u8>| -> Result<Vec<u8>, (String, bool)> {
        jpeg(&resized(px, nx, ny, w, h).map_err(|e| (e, false))?, w, h).map_err(|e| (e, false))
    };
    let burned_in = vol.burned_in.unwrap_or(false);
    let held_jpeg = if burned_in {
        let mut held = p.clone();
        hold_band(&mut held, ny, nx);
        Some(draw(held)?)
    } else {
        None
    };
    let jpeg = draw(p)?;
    let orientation = vol.geometry.map_or(pyramid::AXIAL, |g| g.orientation);
    let (plane, oblique) = pyramid::plane_of(&orientation);
    // the plane as stored, though it was decoded at a lower resolution
    let (rows, cols) = plan.matrix.unwrap_or((ny, nx));
    let (dy, dx) = (dy * ny as f64 / rows as f64, dx * nx as f64 / cols as f64);
    Ok(Still {
        digest: plan.digest.clone(),
        shape: [plan.planes, rows, cols],
        spacing: [plan.dz.unwrap_or(vol.spacing[0]), dy, dx],
        window,
        slope,
        intercept,
        orientation,
        orientation_known: vol.geometry.is_some(),
        plane: plane.to_string(),
        oblique,
        head_up: pyramid::normal(&orientation)[2] > 0.5,
        burned_in,
        width: w,
        height: h,
        jpeg,
        held_jpeg,
        decode_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

impl Still {
    /// The picture a caller is served: the band held unless `sensitive`.
    pub fn picture(&self, sensitive: bool) -> (&[u8], bool) {
        match &self.held_jpeg {
            Some(h) if !sensitive => (h, true),
            _ => (&self.jpeg, false),
        }
    }
}

/// How many stills the engine keeps, a few kilobytes each.
pub const STILLS_KEPT: usize = 2048;

static STILLS: std::sync::LazyLock<Mutex<lru::LruCache<i64, Arc<Still>>>> =
    std::sync::LazyLock::new(|| {
        Mutex::new(lru::LruCache::new(
            std::num::NonZeroUsize::new(STILLS_KEPT).expect("not zero"),
        ))
    });

fn stills() -> std::sync::MutexGuard<'static, lru::LruCache<i64, Arc<Still>>> {
    STILLS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The still kept for a plan, when it was made from the same files.
fn kept_still(plan: &Plan) -> Option<Arc<Still>> {
    stills()
        .get(&plan.stack)
        .filter(|s| s.digest == plan.digest)
        .cloned()
}

/// A stack's still: kept, or decoded now.
pub fn still(store: &mut Store, stack: i64) -> Result<(Plan, Arc<Still>), (String, bool)> {
    let plan = plan(store, stack)?;
    if let Some(s) = kept_still(&plan) {
        return Ok((plan, s));
    }
    let s = Arc::new(still_of(&plan)?);
    stills().put(stack, Arc::clone(&s));
    Ok((plan, s))
}

/// The stills of many stacks for a page, as many as are decoded within
/// `budget`: those kept at once, the rest decoded on threads of the engine
/// a few at a time. A still not ready in time is not waited for; its
/// thread goes on, and keeps it for the page asked next.
pub fn stills_within(
    store: &mut Store,
    stacks: &[i64],
    budget: Duration,
) -> std::collections::HashMap<i64, Arc<Still>> {
    let deadline = Instant::now() + budget;
    let mut out = std::collections::HashMap::new();
    let mut todo = std::collections::VecDeque::new();
    for &stack in stacks {
        let Ok(plan) = plan(store, stack) else {
            continue;
        };
        match kept_still(&plan) {
            Some(s) => {
                out.insert(stack, s);
            }
            None => todo.push_back(plan),
        }
    }
    if todo.is_empty() {
        return out;
    }
    type Done = (Mutex<Vec<(i64, Arc<Still>)>>, std::sync::Condvar);
    let todo = Arc::new(Mutex::new(todo));
    let done: Arc<Done> = Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new()));
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8)
        .min(todo.lock().map_or(1, |t| t.len()));
    for _ in 0..threads {
        let (todo, done) = (Arc::clone(&todo), Arc::clone(&done));
        let _ = std::thread::Builder::new()
            .name("nils-still".into())
            .spawn(move || {
                loop {
                    let next = todo.lock().ok().and_then(|mut t| t.pop_front());
                    let Some(plan) = next else { break };
                    if let Ok(s) = still_of(&plan) {
                        let s = Arc::new(s);
                        stills().put(plan.stack, Arc::clone(&s));
                        if let Ok(mut d) = done.0.lock() {
                            d.push((plan.stack, s));
                        }
                    }
                    done.1.notify_all();
                }
            });
    }
    let mut got = done.0.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let empty = todo.lock().map_or(true, |t| t.is_empty());
        if left.is_zero() || (empty && got.len() + out.len() >= stacks.len()) {
            break;
        }
        got = done
            .1
            .wait_timeout(got, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    for (stack, s) in got.drain(..) {
        out.insert(stack, s);
    }
    out
}

/// Frames `from..to` decoded now from their own files, before the preview
/// is made: drawn as the preview's will be, through the still's window,
/// in one container as [`Open::frames`] answers.
pub fn frames_now(
    plan: &Plan,
    still: &Still,
    from: u32,
    to: u32,
    held: bool,
) -> Result<Vec<u8>, (String, bool)> {
    let order = plan.order.as_ref().ok_or((
        "the stack's planes are not one file each".to_string(),
        false,
    ))?;
    let files: Vec<StackFile> = order[from as usize..to as usize]
        .iter()
        .map(StackFile::whole)
        .collect();
    let (fw0, fh0) = frame_size(plan.planes, still.shape[1], still.shape[2]);
    // decoded at a resolution near the frames' (these frames stand until
    // the preview's own replace them); a stack whose files mix syntaxes is
    // read whole where one at a lower resolution does not match the others
    let vol = pyramid::read_stack_at(&files, Some(fw0.max(fh0) * 3 / 4))
        .or_else(|_| pyramid::read_stack(&files))
        .map_err(|w| (w, true))?;
    let [nz, ny, nx] = vol.shape;
    if nz != to - from {
        return Err(("the planes read are not the planes asked".into(), false));
    }
    let (mut planes, _, _) = grey(&vol, &still.window);
    if held {
        for p in &mut planes {
            hold_band(p, ny, nx);
        }
    }
    let (fw, fh) = (fw0, fh0);
    let sized: Vec<Vec<u8>> = planes
        .into_iter()
        .map(|p| resized(p, nx, ny, fw, fh))
        .collect::<Result<_, _>>()
        .map_err(|e| (e, false))?;
    let mid = sized.len() / 2;
    let quality = frame_quality(&sized[mid..=mid], fw, fh, frame_budget(plan.planes))
        .map_err(|e| (e, false))?;
    let frames: Vec<Vec<u8>> = sized
        .iter()
        .map(|p| jpeg_at(p, fw, fh, quality))
        .collect::<Result<_, _>>()
        .map_err(|e| (e, false))?;
    Ok(container(from, fw, fh, &frames))
}

/// Frames in the planes door's container.
fn container(from: u32, fw: u32, fh: u32, frames: &[Vec<u8>]) -> Vec<u8> {
    let count = frames.len();
    let lead = 16 + 4 * (count + 1);
    let mut out = Vec::with_capacity(lead + frames.iter().map(Vec::len).sum::<usize>());
    for v in [from, count as u32, fw, fh] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut at = lead;
    out.extend_from_slice(&(at as u32).to_le_bytes());
    for f in frames {
        at += f.len();
        out.extend_from_slice(&(at as u32).to_le_bytes());
    }
    for f in frames {
        out.extend_from_slice(f);
    }
    out
}

/// The stacks whose preview a door is making now, so many requests for one
/// stack make it once.
static MAKING: std::sync::LazyLock<(Mutex<BTreeSet<i64>>, std::sync::Condvar)> =
    std::sync::LazyLock::new(|| (Mutex::new(BTreeSet::new()), std::sync::Condvar::new()));

/// Make a stack's preview on the engine's thread: one maker a stack; a
/// second asker waits for the first, then finds the preview current (or
/// makes it, where the first failed).
fn make_now(store: &mut Store, working: &Path, stack: i64) -> Result<(), (String, bool)> {
    let (lock, cv) = &*MAKING;
    {
        let mut making = lock.lock().unwrap_or_else(|e| e.into_inner());
        while making.contains(&stack) {
            making = cv.wait(making).unwrap_or_else(|e| e.into_inner());
        }
        making.insert(stack);
    }
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    let made = make(store, working, stack, false, workers).map(|_| ());
    let mut making = lock.lock().unwrap_or_else(|e| e.into_inner());
    making.remove(&stack);
    cv.notify_all();
    made
}

/// The stacks queued to be made on the engine's thread: a stack a reader
/// opened first, then a page's, and whether a maker runs; the stacks whose
/// making failed, with the class of why, so a door that waits stops.
#[derive(Default)]
struct Warming {
    first: std::collections::VecDeque<i64>,
    rest: BTreeSet<i64>,
    running: bool,
    failed: std::collections::HashMap<i64, &'static str>,
    /// Counts the previews made or failed, so a waiter knows to look.
    ended: u64,
}

static WARMING: std::sync::LazyLock<(Mutex<Warming>, std::sync::Condvar)> =
    std::sync::LazyLock::new(|| (Mutex::new(Warming::default()), std::sync::Condvar::new()));

fn warming() -> std::sync::MutexGuard<'static, Warming> {
    WARMING.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// Make the previews of `stacks` on a thread of this process, one after
/// another, after the answer has gone: a page of scans listed before their
/// sort made pictures has them when it is listed again. Never the job
/// queue.
pub fn warm(home: &nils_registry::home::Home, working: &Path, stacks: &[i64]) {
    if stacks.is_empty() {
        return;
    }
    let mut w = warming();
    for s in stacks {
        if !w.first.contains(s) {
            w.rest.insert(*s);
        }
    }
    start(home, working, w);
}

/// Make a stack's preview before any other queued: a reader opened it.
pub fn warm_first(home: &nils_registry::home::Home, working: &Path, stack: i64) {
    let mut w = warming();
    w.rest.remove(&stack);
    w.failed.remove(&stack);
    if !w.first.contains(&stack) {
        w.first.push_back(stack);
    }
    start(home, working, w);
}

fn start(
    home: &nils_registry::home::Home,
    working: &Path,
    mut w: std::sync::MutexGuard<'static, Warming>,
) {
    if w.running {
        return;
    }
    w.running = true;
    drop(w);
    let home = home.clone();
    let working = working.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("nils-preview-warm".into())
        .spawn(move || {
            let mut registry = home.open().ok();
            loop {
                let next = {
                    let mut w = warming();
                    let next = w.first.pop_front().or_else(|| w.rest.pop_first());
                    if next.is_none() {
                        w.running = false;
                    }
                    next
                };
                let Some(stack) = next else { break };
                let made = match registry.as_mut() {
                    Some(r) => make_now(r.store(), &working, stack),
                    None => Err(("the registry could not be opened".to_string(), false)),
                };
                let mut w = warming();
                match made {
                    Ok(()) => {
                        w.failed.remove(&stack);
                    }
                    Err((why, reading)) => {
                        w.failed.insert(stack, pyramid::reason_of(&why, reading));
                    }
                }
                w.ended += 1;
                WARMING.1.notify_all();
            }
        });
    if spawned.is_err() {
        warming().running = false;
    }
}

/// How long a door holds a request for a preview being made.
pub const HOLD: Duration = Duration::from_secs(8);
/// How many requests may be held at once: each holds a request handler.
pub const HOLDS_AT_ONCE: usize = 2;
/// How soon a reader asks again, in milliseconds, when it was not held.
pub const RETRY_AFTER_MS: u64 = 250;
/// The longest range of frames decoded from the files in the request.
pub const FRAMES_NOW_MAX: u32 = 32;

static HOLDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A place among the requests held, given back when dropped.
pub struct Held;

impl Drop for Held {
    fn drop(&mut self) {
        HOLDING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A place among the requests held, when one is free.
pub fn hold() -> Option<Held> {
    let n = HOLDING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if n >= HOLDS_AT_ONCE {
        HOLDING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        return None;
    }
    Some(Held)
}

/// Wait up to `limit` for a stack's preview at `p`: the file, or the class
/// of why its making failed, or none when the time ran out.
fn wait_made(p: &Path, stack: i64, limit: Duration) -> Result<Option<Arc<Open>>, &'static str> {
    let until = Instant::now() + limit;
    loop {
        if let Ok(Some(o)) = opened(p) {
            return Ok(Some(o));
        }
        let w = warming();
        if let Some(why) = w.failed.get(&stack) {
            return Err(why);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        let seen = w.ended;
        let (w, _) = WARMING
            .1
            .wait_timeout_while(w, left.min(Duration::from_millis(250)), |w| w.ended == seen)
            .unwrap_or_else(|e| e.into_inner());
        drop(w);
    }
}

/// What a door does for a stack with no preview: an answer now, or the
/// preview, made while the request was held.
enum Now {
    Answer(Reply),
    Made(Arc<Open>),
}

/// The answer while the preview is being made, to ask again soon.
fn not_yet(stack: i64) -> Reply {
    let mut r = Reply::error(
        503,
        format!(
            "the preview of stack {stack} is being made; ask again in {RETRY_AFTER_MS} milliseconds"
        ),
    );
    r.body["stack"] = serde_json::json!(stack);
    r.body["building"] = serde_json::json!(true);
    r.body["retry_after"] = serde_json::json!(1);
    r.body["retry_after_ms"] = serde_json::json!(RETRY_AFTER_MS);
    r.headers.push(("Retry-After".to_string(), "1".to_string()));
    r.headers
        .push(("Cache-Control".to_string(), "no-store".to_string()));
    r
}

fn unmade(stack: i64, reason: &str) -> Reply {
    let mut r = Reply::error(
        422,
        format!("the preview of stack {stack} could not be made ({reason})"),
    );
    r.body["stack"] = serde_json::json!(stack);
    r.body["reason"] = serde_json::json!(reason);
    r
}

/// A stack with no preview: its making queued first, and the still, a short
/// range of frames or a held request answered.
#[allow(clippy::too_many_arguments)]
fn now(
    registry: &mut nils_registry::Registry,
    caller: &Caller,
    home: &nils_registry::home::Home,
    root: &Path,
    place: &str,
    stack: i64,
    through: Option<i64>,
    planes: bool,
    query: &std::collections::HashMap<String, String>,
) -> Result<Now, Reply> {
    // the first picture before the whole preview, which is made after it,
    // before any other queued; a stack whose middle plane cannot be read is
    // asked again with the whole
    let made = still(registry.store(), stack);
    warm_first(home, root, stack);
    let (plan, still) =
        made.map_err(|(why, reading)| unmade(stack, pyramid::reason_of(&why, reading)))?;
    let sensitive = caller.access.detail >= Detail::Sensitive;
    let held = still.burned_in && !sensitive;
    pyramid::note_open(registry, caller, stack, through, 0, "preview")
        .map_err(|e| Reply::error(500, e))?;
    let headers = || {
        vec![
            ("Cache-Control".to_string(), "no-store".to_string()),
            ("X-Nils-Stack".to_string(), stack.to_string()),
            ("X-Nils-Held".to_string(), held.to_string()),
            ("X-Nils-Preview".to_string(), still.digest.clone()),
            ("X-Nils-Partial".to_string(), "true".to_string()),
        ]
    };
    let plain = path(root, stack, false);
    let held_on = || -> Result<Now, Reply> {
        let Some(_slot) = hold() else {
            return Ok(Now::Answer(not_yet(stack)));
        };
        match wait_made(&plain, stack, HOLD) {
            Ok(Some(o)) => Ok(Now::Made(o)),
            Ok(None) => Ok(Now::Answer(not_yet(stack))),
            Err(reason) => Err(unmade(stack, reason)),
        }
    };
    if planes {
        let count = still.shape[0];
        let num = |k: &str, d: u32| -> Result<u32, Reply> {
            match query.get(k).filter(|v| !v.is_empty()) {
                Some(v) => v
                    .parse::<u32>()
                    .map_err(|_| Reply::error(400, format!("{k} is a plane number from 0"))),
                None => Ok(d),
            }
        };
        let (from, to) = (num("from", 0)?, num("to", count)?);
        if from >= to || to > count {
            return Err(Reply::error(
                416,
                format!(
                    "the stack has {count} planes; from..to is from below to, to at most {count}"
                ),
            ));
        }
        if to - from <= FRAMES_NOW_MAX && plan.order.is_some() {
            let bytes = frames_now(&plan, &still, from, to, held)
                .map_err(|(why, reading)| unmade(stack, pyramid::reason_of(&why, reading)))?;
            let mut hd = headers();
            hd.push(("X-Nils-Planes".to_string(), (to - from).to_string()));
            return Ok(Now::Answer(Reply::raw(FRAMES_TYPE, bytes, hd)));
        }
        return held_on();
    }
    if let Some(p) = query.get("plane").filter(|p| !p.is_empty()) {
        if p == "axial" {
            let (jpeg, _) = still.picture(sensitive);
            return Ok(Now::Answer(Reply::raw(
                "image/jpeg",
                jpeg.to_vec(),
                headers(),
            )));
        }
        return held_on();
    }
    let (jpeg, _) = still.picture(sensitive);
    let [nz, ny, nx] = still.shape;
    let (fw, fh) = frame_size(nz, ny, nx);
    let v = &still.digest;
    let doc = serde_json::json!({
        "stack": stack,
        "digest": v,
        "format": FORMAT,
        "codec": CODEC,
        "partial": true,
        "retry_after_ms": RETRY_AFTER_MS,
        "decode_ms": (still.decode_ms * 10.0).round() / 10.0,
        "shape": still.shape,
        "spacing": still.spacing,
        "window": still.window,
        "slope": still.slope,
        "intercept": still.intercept,
        "orientation": still.orientation,
        "orientation_known": still.orientation_known,
        "plane": still.plane,
        "oblique": still.oblique,
        "head_up": still.head_up,
        "burned_in": still.burned_in,
        "held": held,
        "built_at": null,
        "place": place,
        "middle": {
            "axial": {"width": still.width, "height": still.height, "bytes": jpeg.len(), "data": data_url(jpeg)},
        },
        "frames": {
            "count": nz, "width": fw, "height": fh, "bytes": null,
            "url": format!("/api/instances/{stack}/preview/planes?from=0&to={nz}&v={v}"),
        },
    });
    let mut r = Reply::ok(doc);
    r.headers = headers();
    Ok(Now::Answer(r))
}

// ---------------------------------------------------------------------------
// The doors.

/// `GET /api/instances/{stack}/preview` and `.../preview/planes`, opened
/// as every picture is (Wave 5 §12.7): detail quasi, through a campaign for
/// a rater without query:see, one audit row a stack in the window, and the
/// band held below detail sensitive where the stack carries burned-in
/// annotation.
#[allow(clippy::too_many_arguments)]
pub fn door(
    registry: &mut nils_registry::Registry,
    caller: &Caller,
    home: &nils_registry::home::Home,
    stack: i64,
    through: Option<i64>,
    rest: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Result<Reply, Reply> {
    let what = match rest {
        ["preview"] => None,
        ["preview", "planes"] => Some(()),
        _ => {
            return Err(Reply::error(
                404,
                format!(
                    "GET /api/instances/{stack}/{} is not a door; preview and preview/planes are",
                    rest.join("/")
                ),
            ));
        }
    };
    let working =
        pyramid::working_place_cached(registry.store()).map_err(|m| Reply::error(409, m))?;
    if caller.access.detail < Detail::Quasi {
        return Err(Reply::gated(
            403,
            format!("the pixels of stack {stack} are quasi-identifying; detail quasi opens them"),
        ));
    }
    let root = Path::new(&working.path);
    let plain = path(root, stack, false);
    let mut open = match opened(&plain).map_err(|e| Reply::error(500, e))? {
        Some(o) => o,
        None => {
            if !stack_exists(registry.store(), stack)? {
                return Err(Reply::error(404, format!("no stack {stack}")));
            }
            // the first picture now from one file, the preview made after
            match now(
                registry,
                caller,
                home,
                root,
                &working.name,
                stack,
                through,
                what.is_some(),
                query,
            )? {
                Now::Answer(r) => return Ok(r),
                Now::Made(o) => o,
            }
        }
    };
    let held = open.header.burned_in && caller.access.detail < Detail::Sensitive;
    if held {
        open = opened(&path(root, stack, true))
            .map_err(|e| Reply::error(500, e))?
            .ok_or_else(|| Reply::error(500, "the held preview is not there"))?;
    }
    let h = &open.header;
    pyramid::note_open(registry, caller, stack, through, 0, "preview")
        .map_err(|e| Reply::error(500, e))?;
    let named = query
        .get("v")
        .is_some_and(|v| !v.is_empty() && h.digest.starts_with(v.as_str()));
    let headers = |etag: String| {
        vec![
            ("ETag".to_string(), format!("\"{etag}\"")),
            (
                "Cache-Control".to_string(),
                if named { IMMUTABLE } else { REVALIDATE }.to_string(),
            ),
            ("X-Nils-Stack".to_string(), stack.to_string()),
            ("X-Nils-Held".to_string(), held.to_string()),
            ("X-Nils-Preview".to_string(), h.digest.clone()),
        ]
    };
    let tag = format!("{}{}", h.digest, if held { "-held" } else { "" });
    if what.is_some() {
        let count = h.frames.count;
        let num = |k: &str, d: u32| -> Result<u32, Reply> {
            match query.get(k).filter(|v| !v.is_empty()) {
                Some(v) => v
                    .parse::<u32>()
                    .map_err(|_| Reply::error(400, format!("{k} is a plane number from 0"))),
                None => Ok(d),
            }
        };
        let (from, to) = (num("from", 0)?, num("to", count)?);
        if from >= to || to > count {
            return Err(Reply::error(
                416,
                format!(
                    "the stack has {count} planes; from..to is from below to, to at most {count}"
                ),
            ));
        }
        let bytes = open.frames(from, to).map_err(|e| Reply::error(500, e))?;
        let mut hd = headers(format!("{tag}-f{from}-{to}"));
        hd.push(("X-Nils-Planes".to_string(), (to - from).to_string()));
        return Ok(Reply::raw(FRAMES_TYPE, bytes, hd));
    }
    if let Some(plane) = query.get("plane").filter(|p| !p.is_empty()) {
        let Some((jpeg, _)) = open.middle(plane) else {
            return Err(Reply::error(
                404,
                format!(
                    "plane is axial, coronal or sagittal, of a stack of more than one plane; not {plane}"
                ),
            ));
        };
        return Ok(Reply::raw(
            "image/jpeg",
            jpeg.to_vec(),
            headers(format!("{tag}-{plane}")),
        ));
    }
    let mut middle = serde_json::Map::new();
    for name in PLANES {
        if let Some((jpeg, b)) = open.middle(name) {
            middle.insert(
                name.to_string(),
                serde_json::json!({"width": b.width, "height": b.height, "bytes": b.length, "data": data_url(jpeg)}),
            );
        }
    }
    let f = &h.frames;
    let v = &h.digest;
    let doc = serde_json::json!({
        "stack": stack,
        "digest": v,
        "format": h.format,
        "codec": h.codec,
        "partial": false,
        "shape": h.shape,
        "spacing": h.spacing,
        "window": h.window,
        "slope": h.slope,
        "intercept": h.intercept,
        "orientation": h.orientation,
        "orientation_known": h.orientation_known,
        "plane": h.plane,
        "oblique": h.oblique,
        "head_up": h.head_up,
        "burned_in": h.burned_in,
        "held": held,
        "built_at": h.built_at,
        "place": working.name,
        "middle": middle,
        "frames": {
            "count": f.count, "width": f.width, "height": f.height,
            "quality": h.frame_quality,
            "bytes": f.offsets.last().copied().unwrap_or(0) - f.offsets.first().copied().unwrap_or(0),
            "url": format!("/api/instances/{stack}/preview/planes?from=0&to={}&v={v}", f.count),
        },
    });
    let mut r = Reply::ok(doc);
    r.headers = headers(format!("{tag}-doc"));
    Ok(r)
}

fn stack_exists(store: &mut Store, stack: i64) -> Result<bool, Reply> {
    let sql = format!(
        "SELECT 1 FROM {} WHERE id = {}",
        store.qualified("stack"),
        store.dialect().param(1, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(stack)])
        .map(|r| r.is_some())
        .map_err(|e| Reply::error(500, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(nz: u32, ny: u32, nx: u32, burned_in: bool) -> Volume {
        let mut data = Vec::with_capacity((nz * ny * nx) as usize);
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    data.push(((x * 7 + y * 3 + z * 101) % 4096) as u16);
                }
            }
        }
        Volume {
            shape: [nz, ny, nx],
            spacing: [2.0, 0.5, 0.5],
            intercept: 0,
            rescale: (1.0, 0.0),
            rescale_varies: false,
            burned_in: Some(burned_in),
            data,
            geometry: None,
            lossy: false,
            syntaxes: Vec::new(),
            order: pyramid::ORDER_POSITION,
            multiframe_files: 0,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let p = base.join(format!("nils-preview-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_preview_holds_three_middle_planes_and_every_frame() {
        let work = scratch("shape");
        let vol = volume(20, 600, 300, false);
        write(&vol, 7, "d1", &work, 4).unwrap();
        let p = path(&work, 7, false);
        assert!(p.ends_with("previews/007/7.preview"), "{}", p.display());
        let o = opened(&p).unwrap().unwrap();
        assert_eq!(o.header.digest, "d1");
        assert_eq!(o.header.frames.count, 20);
        // the long side to 512, proportions kept
        assert_eq!((o.header.frames.width, o.header.frames.height), (256, 512));
        for name in PLANES {
            let (jpeg, b) = o.middle(name).unwrap();
            assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "{name}");
            assert!(b.width.max(b.height) == MIDDLE, "{name}: {b:?}");
        }
        // the axial middle: 300 x 0.5 by 600 x 0.5 millimetres
        assert_eq!(o.middle("axial").unwrap().1.width, 128);
        let body = o.frames(3, 6).unwrap();
        let u = |at: usize| u32::from_le_bytes(body[at..at + 4].try_into().unwrap());
        assert_eq!((u(0), u(4), u(8), u(12)), (3, 3, 256, 512));
        let offs: Vec<usize> = (0..4).map(|i| u(16 + 4 * i) as usize).collect();
        assert_eq!(offs[0], 16 + 16);
        assert_eq!(*offs.last().unwrap(), body.len());
        for w in offs.windows(2) {
            assert_eq!(&body[w[0]..w[0] + 2], &[0xFF, 0xD8]);
            assert_eq!(&body[w[1] - 2..w[1]], &[0xFF, 0xD9]);
        }
        assert!(!path(&work, 7, true).exists());
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn burned_in_annotation_gets_a_held_file_with_the_band_blank() {
        let work = scratch("held");
        let vol = volume(4, 64, 64, true);
        write(&vol, 1001, "d2", &work, 2).unwrap();
        let held = opened(&path(&work, 1001, true)).unwrap().unwrap();
        assert!(held.header.held && held.header.burned_in);
        let plain = opened(&path(&work, 1001, false)).unwrap().unwrap();
        assert!(!plain.header.held && plain.header.burned_in);
        assert!(path(&work, 1001, true).ends_with("previews/001/1001.held.preview"));
        // the held frame's top rows decode to black
        let f = held.frames(0, 1).unwrap();
        let img = image::load_from_memory(&f[16 + 8..]).unwrap().into_luma8();
        assert!(img.rows().take(6).flatten().all(|p| p.0[0] < 8));
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn a_single_plane_has_its_own_middle_alone() {
        let work = scratch("single");
        write(&volume(1, 32, 48, false), 3, "d3", &work, 1).unwrap();
        let o = opened(&path(&work, 3, false)).unwrap().unwrap();
        assert!(o.middle("axial").is_some());
        assert!(o.middle("coronal").is_none());
        assert_eq!(o.header.frames.count, 1);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn frames_keep_to_a_budget_a_stack() {
        // a stack of many planes is drawn smaller, its proportions kept
        assert_eq!(frame_size(448, 640, 640), (384, 384));
        assert_eq!(frame_size(200, 640, 640), (512, 512));
        assert_eq!(frame_size(88, 1024, 800), (400, 512));
        assert_eq!(frame_size(23, 256, 256), (256, 256));
        // a frame's share of the budget, never below the least
        assert_eq!(frame_budget(448), (6 << 20) / 448);
        assert_eq!(frame_budget(4000), FRAME_BYTES_MIN);
        assert_eq!(frame_budget(10), (6 << 20) / 10);
        // the quality falls until the sample planes fit the budget
        let vol = volume(3, 384, 384, false);
        let (planes, _, _) = grey(&vol, &pyramid::window(&vol));
        let q = frame_quality(&planes, 384, 384, 1).unwrap();
        assert_eq!(q, QUALITY_MIN);
        let q = frame_quality(&planes, 384, 384, u64::MAX).unwrap();
        assert_eq!(q, QUALITY);
        let full = jpeg_at(&planes[1], 384, 384, QUALITY).unwrap().len() as u64;
        let q = frame_quality(&planes, 384, 384, full * 2 / 3).unwrap();
        assert!(q > QUALITY_MIN && q < QUALITY && q.is_multiple_of(5), "{q}");
        // and the file says which it took
        let work = scratch("budget");
        write(&volume(3, 64, 64, false), 9, "d9", &work, 1).unwrap();
        let o = opened(&path(&work, 9, false)).unwrap().unwrap();
        assert_eq!(o.header.format, FORMAT);
        assert!(
            o.header.frame_quality >= QUALITY_MIN,
            "{}",
            o.header.frame_quality
        );
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn a_preview_of_another_format_reads_as_none_and_is_made_again() {
        let work = scratch("format");
        write(&volume(3, 32, 32, false), 11, "d11", &work, 1).unwrap();
        let p = path(&work, 11, false);
        let bytes = std::fs::read(&p).unwrap();
        let key = format!("\"format\":{FORMAT}").into_bytes();
        let at = bytes.windows(key.len()).position(|w| w == key).unwrap();
        let mut old = bytes.clone();
        old[at + 9] = b'1';
        std::fs::write(&p, &old).unwrap();
        forget(&p);
        assert!(opened(&p).unwrap().is_none());
        assert!(digest_on_disk(&work, 11).is_none());
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn a_container_of_frames_is_the_planes_doors() {
        let frames = vec![vec![1u8, 2, 3], vec![4u8; 5]];
        let c = container(7, 32, 16, &frames);
        let u = |at: usize| u32::from_le_bytes(c[at..at + 4].try_into().unwrap());
        assert_eq!((u(0), u(4), u(8), u(12)), (7, 2, 32, 16));
        assert_eq!((u(16), u(20), u(24)), (28, 31, 36));
        assert_eq!(c.len(), 36);
        assert_eq!(&c[28..31], &[1, 2, 3]);
    }

    #[test]
    fn the_digest_changes_with_the_files_and_the_frames() {
        let s = |p: &str, size, t, frames: Option<Vec<u32>>| Source {
            file: StackFile {
                path: PathBuf::from(p),
                frames,
            },
            size,
            mtime_ns: t,
            instance: None,
            position: None,
            frames_in_file: None,
            matrix: None,
        };
        let a = digest(1, &[s("/a", 10, 1, None)]);
        assert_eq!(a.len(), 32);
        assert_eq!(a, digest(1, &[s("/a", 10, 1, None)]));
        assert_ne!(a, digest(2, &[s("/a", 10, 1, None)]));
        assert_ne!(a, digest(1, &[s("/a", 11, 1, None)]));
        assert_ne!(a, digest(1, &[s("/a", 10, 2, None)]));
        assert_ne!(a, digest(1, &[s("/a", 10, 1, Some(vec![1]))]));
    }
}
