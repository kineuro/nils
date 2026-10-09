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
//! size (512 on the long side at most) as frames, in the pyramid's plane
//! order. Every picture is an 8-bit grey baseline JPEG windowed with the
//! window the pyramid opens at; the header holds the offsets.
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
/// preview is built again.
pub const FORMAT: u32 = 1;
const MAGIC: &[u8; 8] = b"NILSPV01";
/// The middle planes' long side, in pixels.
pub const MIDDLE: u32 = 256;
/// The frames' long side at most, in pixels.
pub const FRAME_MAX: u32 = 512;
/// The JPEG quality of every picture.
pub const QUALITY: u8 = 85;
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
    pub quality: u8,
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

/// One of a stack's files with what the digest is made of.
#[derive(Debug, Clone)]
pub struct Source {
    pub file: StackFile,
    pub size: i64,
    pub mtime_ns: i64,
}

/// A stack's files, as the pyramid reads them, with their sizes and times.
pub fn sources(store: &mut Store, stack: i64) -> Result<Vec<Source>, String> {
    let p = store.dialect().param(1, Type::Int);
    let whole = format!(
        "SELECT so.root, f.path, f.size, f.mtime_ns FROM {i} i JOIN {f} f ON f.id = i.source_file_id \
         JOIN {so} so ON so.id = f.source_id WHERE i.stack_id = {p} \
         AND NOT EXISTS (SELECT 1 FROM {fr} fr WHERE fr.instance_id = i.id)",
        i = store.qualified("instance"),
        f = store.qualified("source_file"),
        so = store.qualified("source"),
        fr = store.qualified("instance_frame"),
    );
    let framed = format!(
        "SELECT so.root, f.path, f.size, f.mtime_ns, fr.frames FROM {fr} fr JOIN {i} i ON i.id = fr.instance_id \
         JOIN {f} f ON f.id = i.source_file_id JOIN {so} so ON so.id = f.source_id \
         WHERE fr.stack_id = {p}",
        i = store.qualified("instance"),
        f = store.qualified("source_file"),
        so = store.qualified("source"),
        fr = store.qualified("instance_frame"),
    );
    let e = |e: nils_registry::store::Error| e.to_string();
    let mut out = Vec::new();
    for r in store.query(&whole, &[Param::Int(stack)]).map_err(e)? {
        out.push(Source {
            file: StackFile::whole(Path::new(r.text(0).map_err(e)?).join(r.text(1).map_err(e)?)),
            size: r.int(2).map_err(e)?,
            mtime_ns: r.int(3).map_err(e)?,
        });
    }
    for r in store.query(&framed, &[Param::Int(stack)]).map_err(e)? {
        out.push(Source {
            file: StackFile {
                path: Path::new(r.text(0).map_err(e)?).join(r.text(1).map_err(e)?),
                frames: Some(pyramid::frame_list(r.text(4).map_err(e)?)?),
            },
            size: r.int(2).map_err(e)?,
            mtime_ns: r.int(3).map_err(e)?,
        });
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
    h.update(format!("nils-preview {FORMAT} {MIDDLE} {FRAME_MAX} {QUALITY} {stack}\n").as_bytes());
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

/// A picture of grey bytes as a baseline JPEG.
fn jpeg(px: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(px.len() / 6 + 1024);
    let (w16, h16) = (
        u16::try_from(w).map_err(|_| "a picture wider than 65535")?,
        u16::try_from(h).map_err(|_| "a picture taller than 65535")?,
    );
    jpeg_encoder::Encoder::new(&mut out, QUALITY)
        .encode(px, w16, h16, jpeg_encoder::ColorType::Luma)
        .map_err(|e| e.to_string())?;
    Ok(out)
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
}

fn draw(
    planes: &[Vec<u8>],
    [nz, ny, nx]: [u32; 3],
    spacing: [f64; 3],
    head_up: bool,
    workers: usize,
) -> Result<Drawn, String> {
    let [dz, dy, dx] = spacing.map(|d| if d.is_finite() && d > 0.0 { d } else { 1.0 });
    // every plane at display size, its pixels' own proportions kept (a
    // viewer scales by the spacing the header says)
    let long = nx.max(ny);
    let (fw, fh) = if long > FRAME_MAX {
        let k = FRAME_MAX as f64 / long as f64;
        (
            ((nx as f64 * k).round() as u32).max(1),
            ((ny as f64 * k).round() as u32).max(1),
        )
    } else {
        (nx, ny)
    };
    let workers = workers.clamp(1, 64);
    let chunk = planes.len().div_ceil(workers).max(1);
    let parts: Vec<Result<Vec<Vec<u8>>, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = planes
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|p| jpeg(&resized(p.clone(), nx, ny, fw, fh)?, fw, fh))
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
    let n = (ny * nx) as usize;
    let planes: Vec<Vec<u8>> = vol
        .data
        .chunks(n)
        .map(|p| p.iter().map(|v| lut[*v as usize]).collect())
        .collect();
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
    let drawn = draw(&planes, vol.shape, vol.spacing, head_up, workers)?;
    let mut h = header(false);
    h.build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let file = assemble(h, drawn)?;
    bytes += file.len() as u64;
    if burned_in {
        let band = (ny / 8) as usize;
        let held: Vec<Vec<u8>> = planes
            .into_iter()
            .map(|mut p| {
                for y in (0..band).chain(ny as usize - band..ny as usize) {
                    p[y * nx as usize..(y + 1) * nx as usize].fill(0);
                }
                p
            })
            .collect();
        let drawn = draw(&held, vol.shape, vol.spacing, head_up, workers)?;
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
        return Err(format!("a preview of format {}", header.format));
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
        let (header, start, mut head) = header_of(&file, 64 << 10)?;
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

/// The stacks whose preview a door is making now, so many requests for one
/// stack make it once.
static MAKING: std::sync::LazyLock<(Mutex<BTreeSet<i64>>, std::sync::Condvar)> =
    std::sync::LazyLock::new(|| (Mutex::new(BTreeSet::new()), std::sync::Condvar::new()));

/// Make a stack's preview in this request, never through the queue: the
/// one who asks waits for one stack's decode, once; a second request for
/// it waits for the first.
fn make_now(store: &mut Store, working: &Path, stack: i64) -> Result<(), (String, bool)> {
    let (lock, cv) = &*MAKING;
    {
        // one maker a stack: a second asker waits for the first, then
        // finds the preview current (or makes it, where the first failed)
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

/// The stacks queued to be made in the background, and whether a maker
/// runs.
static WARMING: std::sync::LazyLock<Mutex<(BTreeSet<i64>, bool)>> =
    std::sync::LazyLock::new(|| Mutex::new((BTreeSet::new(), false)));

/// Make the previews of `stacks` on a thread of this process, one after
/// another, after the answer has gone: a page of scans listed before their
/// sort made pictures has them when it is listed again. Never the job
/// queue.
pub fn warm(home: &nils_registry::home::Home, working: &Path, stacks: &[i64]) {
    if stacks.is_empty() {
        return;
    }
    let mut w = WARMING.lock().unwrap_or_else(|e| e.into_inner());
    w.0.extend(stacks.iter().copied());
    if w.1 {
        return;
    }
    w.1 = true;
    drop(w);
    let home = home.clone();
    let working = working.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("nils-preview-warm".into())
        .spawn(move || {
            let mut registry = home.open().ok();
            loop {
                let next = {
                    let mut w = WARMING.lock().unwrap_or_else(|e| e.into_inner());
                    let next = w.0.pop_first();
                    if next.is_none() {
                        w.1 = false;
                    }
                    next
                };
                let Some(stack) = next else { break };
                if let Some(r) = registry.as_mut() {
                    let _ = make_now(r.store(), &working, stack);
                }
            }
        });
}

// ---------------------------------------------------------------------------
// The doors.

/// `GET /api/instances/{stack}/preview` and `.../preview/planes`, opened
/// as every picture is (Wave 5 §12.7): detail quasi, through a campaign for
/// a rater without query:see, one audit row a stack in the window, and the
/// band held below detail sensitive where the stack carries burned-in
/// annotation.
pub fn door(
    registry: &mut nils_registry::Registry,
    caller: &Caller,
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
    let mut open = opened(&plain).map_err(|e| Reply::error(500, e))?;
    if open.is_none() {
        if !stack_exists(registry.store(), stack)? {
            return Err(Reply::error(404, format!("no stack {stack}")));
        }
        make_now(registry.store(), root, stack).map_err(|(why, reading)| {
            let reason = pyramid::reason_of(&why, reading);
            let mut r = Reply::error(
                422,
                format!("the preview of stack {stack} could not be made ({reason})"),
            );
            r.body["stack"] = serde_json::json!(stack);
            r.body["reason"] = serde_json::json!(reason);
            r
        })?;
        open = opened(&plain).map_err(|e| Reply::error(500, e))?;
    }
    let mut open =
        open.ok_or_else(|| Reply::error(500, "the preview was made and is not there"))?;
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
    fn the_digest_changes_with_the_files_and_the_frames() {
        let s = |p: &str, size, t, frames: Option<Vec<u32>>| Source {
            file: StackFile {
                path: PathBuf::from(p),
                frames,
            },
            size,
            mtime_ns: t,
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
