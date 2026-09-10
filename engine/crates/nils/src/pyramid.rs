// SPDX-License-Identifier: AGPL-3.0-only
//! The viewing pyramid and the gated instance doors (Wave 5 §12.7), shaped
//! by the viewer study: a browser must never hold a whole scan, so a stack
//! is precomputed at digest as four in-plane levels of 256 by 256 HTJ2K
//! tiles, every slice at every level, written to a `working` place; the
//! doors answer one plane's tiles in one response, a slab of up to 32
//! planes, and a server-rendered plane for the first picture, thin clients
//! and the gated case. The codec is HTJ2K through a pure Rust port of
//! OpenJPH, reversible, in-process.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use dicom_dictionary_std::tags;
use dicom_object::{InMemDicomObject, OpenFileOptions};
use nils_registry::Param;
use nils_registry::schema::Type;
use nils_registry::store::Store;
use openjph_core::codestream::Codestream;
use openjph_core::file::{MemInfile, MemOutfile};
use openjph_core::types::{Point, Size};
use serde::{Deserialize, Serialize};

use crate::serve::{Caller, Reply, Role};
use nils_registry::place::{self, Place, Role as PlaceRole};

pub const TILE: u32 = 256;
pub const LEVELS: u32 = 4;
/// The most planes one slab answer carries.
pub const SLAB_MAX: u32 = 32;
pub const CODEC: &str = "htj2k";
pub const CONTENT_TYPE: &str = "application/x-nils-tiles";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Level {
    pub level: u32,
    /// `[nz, ny, nx]` at this level.
    pub shape: [u32; 3],
    /// `[ty, tx]`: tiles per plane.
    pub tiles: [u32; 2],
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub percentiles: [i64; 2],
    pub center: f64,
    pub width: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub burned_in: bool,
    /// Where the header said it is: `"header"` when the tag named it, else
    /// the bands the render blanks, `"top-and-bottom-eighths"`.
    #[serde(rename = "where")]
    pub place: String,
}

/// What the desk's loader reads first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub stack: i64,
    pub codec: String,
    pub tile: u32,
    pub levels: u32,
    /// `[nz, ny, nx]` at level 0.
    pub shape: [u32; 3],
    /// `[dz, dy, dx]` in millimetres.
    pub spacing: [f64; 3],
    pub dtype: String,
    /// Stored values are `raw + intercept`: a signed volume is shifted by
    /// 32768 into u16, and the viewer shifts it back.
    pub intercept: i64,
    pub window: Window,
    pub bytes_per_level: Vec<u64>,
    pub level_shapes: Vec<Level>,
    pub annotation: Annotation,
    pub built_at: String,
    pub pack_version: Option<String>,
    pub precompute: Precompute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Precompute {
    pub wall_seconds: f64,
    pub workers: usize,
    pub raw_bytes: u64,
}

/// One volume in memory: `nz` planes of `ny` by `nx` u16.
pub struct Volume {
    pub shape: [u32; 3],
    pub spacing: [f64; 3],
    pub intercept: i64,
    pub burned_in: Option<bool>,
    pub data: Vec<u16>,
}

impl Volume {
    fn plane(&self, z: usize) -> &[u16] {
        let n = (self.shape[1] * self.shape[2]) as usize;
        &self.data[z * n..(z + 1) * n]
    }
}

/// The directory a stack's pyramid lives in.
pub fn dir(working: &Path, stack: i64) -> PathBuf {
    working.join("pyramids").join(stack.to_string())
}

/// Read a stack's files from the registry: the instances of the stack and
/// their source files, ordered by position along the stack's normal (the
/// third coordinate of Image Position, then the instance number).
pub fn read_volume(store: &mut Store, stack: i64) -> Result<Volume, String> {
    let sql = format!(
        "SELECT so.root, f.path FROM {} i JOIN {} f ON f.id = i.source_file_id JOIN {} so ON so.id = f.source_id WHERE i.stack_id = {}",
        store.qualified("instance"),
        store.qualified("source_file"),
        store.qualified("source"),
        store.dialect().param(1, Type::Int)
    );
    let rows = store
        .query(&sql, &[Param::Int(stack)])
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Err(format!("stack {stack} has no files the registry can read"));
    }
    let mut files = Vec::with_capacity(rows.len());
    for r in &rows {
        let root = r.text(0).map_err(|e| e.to_string())?;
        let path = r.text(1).map_err(|e| e.to_string())?;
        files.push(Path::new(root).join(path));
    }
    read_files(&files)
}

/// The tags read from a file's header.
struct Slice {
    z: f64,
    instance: i64,
    rows: u32,
    cols: u32,
    signed: bool,
    bits: u16,
    pixels: Vec<u8>,
    spacing: [f64; 2],
    thickness: f64,
    burned_in: Option<bool>,
}

fn f64s(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Option<Vec<f64>> {
    let el = obj.element(tag).ok()?;
    let s = el.to_str().ok()?;
    let v: Vec<f64> = s
        .split('\\')
        .filter_map(|p| p.trim().parse::<f64>().ok())
        .collect();
    (!v.is_empty()).then_some(v)
}

fn int(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Option<i64> {
    obj.element(tag).ok()?.to_int::<i64>().ok()
}

fn text(obj: &InMemDicomObject, tag: dicom_core::Tag) -> Option<String> {
    obj.element(tag)
        .ok()?
        .to_str()
        .ok()
        .map(|s| s.trim().to_string())
}

/// The native transfer syntaxes the pyramid reads: little endian, explicit
/// or implicit; anything compressed is refused with the syntax named.
const NATIVE: [&str; 2] = ["1.2.840.10008.1.2", "1.2.840.10008.1.2.1"];

fn read_slice(path: &Path) -> Result<Slice, String> {
    let file = OpenFileOptions::new()
        .open_file(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let ts = file
        .meta()
        .transfer_syntax()
        .trim_end_matches('\0')
        .to_string();
    if !NATIVE.contains(&ts.as_str()) {
        return Err(format!(
            "transfer syntax {ts} is not native little endian; the pyramid reads uncompressed pixel data only"
        ));
    }
    let obj: &InMemDicomObject = &file;
    let rows = int(obj, tags::ROWS).ok_or("no Rows")? as u32;
    let cols = int(obj, tags::COLUMNS).ok_or("no Columns")? as u32;
    let bits = int(obj, tags::BITS_ALLOCATED).unwrap_or(16) as u16;
    if bits != 16 && bits != 8 {
        return Err(format!("{bits} bits allocated; the pyramid reads 8 and 16"));
    }
    let signed = int(obj, tags::PIXEL_REPRESENTATION).unwrap_or(0) == 1;
    let samples = int(obj, tags::SAMPLES_PER_PIXEL).unwrap_or(1);
    if samples != 1 {
        return Err(format!(
            "{samples} samples per pixel; the pyramid reads one"
        ));
    }
    let pixels = obj
        .element(tags::PIXEL_DATA)
        .map_err(|_| "no Pixel Data".to_string())?
        .to_bytes()
        .map_err(|e| e.to_string())?
        .into_owned();
    let need = (rows * cols) as usize * (bits as usize / 8);
    if pixels.len() < need {
        return Err(format!(
            "pixel data holds {} bytes, the header says {need}",
            pixels.len()
        ));
    }
    let z = f64s(obj, tags::IMAGE_POSITION_PATIENT)
        .and_then(|v| v.get(2).copied())
        .unwrap_or(f64::NAN);
    let instance = int(obj, tags::INSTANCE_NUMBER).unwrap_or(0);
    let spacing = f64s(obj, tags::PIXEL_SPACING)
        .map(|v| [v[0], *v.get(1).unwrap_or(&v[0])])
        .unwrap_or([1.0, 1.0]);
    let thickness = f64s(obj, tags::SPACING_BETWEEN_SLICES)
        .or_else(|| f64s(obj, tags::SLICE_THICKNESS))
        .map(|v| v[0])
        .unwrap_or(1.0);
    let burned_in = text(obj, tags::BURNED_IN_ANNOTATION).map(|s| s.eq_ignore_ascii_case("YES"));
    Ok(Slice {
        z,
        instance,
        rows,
        cols,
        signed,
        bits,
        pixels,
        spacing,
        thickness,
        burned_in,
    })
}

/// The files of one stack into one volume, in order.
pub fn read_files(files: &[PathBuf]) -> Result<Volume, String> {
    let mut slices = Vec::with_capacity(files.len());
    for f in files {
        slices.push(read_slice(f)?);
    }
    let (rows, cols, bits, signed) = (
        slices[0].rows,
        slices[0].cols,
        slices[0].bits,
        slices[0].signed,
    );
    if slices
        .iter()
        .any(|s| s.rows != rows || s.cols != cols || s.bits != bits)
    {
        return Err("the stack's files do not share one matrix".to_string());
    }
    // by position when every file has one, else by instance number
    if slices.iter().all(|s| s.z.is_finite()) {
        slices.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        slices.sort_by_key(|s| s.instance);
    }
    let nz = slices.len() as u32;
    let intercept: i64 = if signed { 32768 } else { 0 };
    let mut data = Vec::with_capacity((nz * rows * cols) as usize);
    for s in &slices {
        let n = (rows * cols) as usize;
        if bits == 8 {
            data.extend(s.pixels[..n].iter().map(|&b| b as u16));
        } else {
            for px in s.pixels[..n * 2].as_chunks::<2>().0 {
                let raw = u16::from_le_bytes(*px);
                data.push(if signed {
                    (raw as i16 as i32 + 32768) as u16
                } else {
                    raw
                });
            }
        }
    }
    let dz = if nz > 1 && slices[0].z.is_finite() && slices[nz as usize - 1].z.is_finite() {
        ((slices[nz as usize - 1].z - slices[0].z) / (nz as f64 - 1.0)).abs()
    } else {
        slices[0].thickness
    };
    let burned_in = slices.iter().find_map(|s| s.burned_in);
    Ok(Volume {
        shape: [nz, rows, cols],
        spacing: [dz, slices[0].spacing[0], slices[0].spacing[1]],
        intercept,
        burned_in,
        data,
    })
}

/// One tile as an HTJ2K codestream, reversible, single component, 16 bits.
pub fn encode_tile(pixels: &[u16], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut cs = Codestream::new();
    cs.access_siz_mut()
        .set_image_extent(Point::new(width, height));
    cs.access_siz_mut().set_num_components(1);
    cs.access_siz_mut()
        .set_comp_info(0, Point::new(1, 1), 16, false);
    cs.access_siz_mut().set_tile_size(Size::new(width, height));
    let decompositions = if width.min(height) >= 32 { 5 } else { 2 };
    cs.access_cod_mut().set_num_decomposition(decompositions);
    cs.access_cod_mut().set_reversible(true);
    cs.access_cod_mut().set_color_transform(false);
    cs.set_planar(0);
    let mut out = MemOutfile::new();
    cs.write_headers(&mut out, &[]).map_err(|e| e.to_string())?;
    let mut line = vec![0i32; width as usize];
    for y in 0..height as usize {
        let row = &pixels[y * width as usize..(y + 1) * width as usize];
        for (l, p) in line.iter_mut().zip(row) {
            *l = *p as i32;
        }
        cs.exchange(&line, 0).map_err(|e| e.to_string())?;
    }
    cs.flush(&mut out).map_err(|e| e.to_string())?;
    Ok(out.get_data().to_vec())
}

/// A tile back to pixels: `(width, height, pixels)`.
pub fn decode_tile(bytes: &[u8]) -> Result<(u32, u32, Vec<u16>), String> {
    let mut infile = MemInfile::new(bytes);
    let mut cs = Codestream::new();
    cs.read_headers(&mut infile).map_err(|e| e.to_string())?;
    let extent = cs.access_siz().get_image_extent();
    let (width, height) = (extent.x, extent.y);
    cs.create(&mut infile).map_err(|e| e.to_string())?;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for _ in 0..height {
        let line = cs.pull(0).ok_or("the codestream ended early")?;
        pixels.extend(line.iter().map(|&v| v.clamp(0, 65535) as u16));
    }
    Ok((width, height, pixels))
}

/// The tile grid of a plane `ny` by `nx`.
fn grid(ny: u32, nx: u32) -> (u32, u32) {
    (ny.div_ceil(TILE), nx.div_ceil(TILE))
}

/// One plane at half resolution in-plane: the 2 by 2 mean.
fn halve(plane: &[u16], ny: u32, nx: u32) -> (Vec<u16>, u32, u32) {
    let (hy, hx) = (ny / 2, nx / 2);
    let mut out = Vec::with_capacity((hy * hx) as usize);
    for y in 0..hy as usize {
        for x in 0..hx as usize {
            let a = plane[(2 * y) * nx as usize + 2 * x] as u32;
            let b = plane[(2 * y) * nx as usize + 2 * x + 1] as u32;
            let c = plane[(2 * y + 1) * nx as usize + 2 * x] as u32;
            let d = plane[(2 * y + 1) * nx as usize + 2 * x + 1] as u32;
            out.push(((a + b + c + d) / 4) as u16);
        }
    }
    (out, hy, hx)
}

/// The tile file name.
fn tile_path(root: &Path, level: u32, z: u32, ty: u32, tx: u32) -> PathBuf {
    root.join(level.to_string())
        .join(z.to_string())
        .join(format!("{ty}_{tx}.j2c"))
}

/// Encode one plane's tiles at one level; returns the bytes written.
fn write_plane(
    root: &Path,
    level: u32,
    z: u32,
    plane: &[u16],
    ny: u32,
    nx: u32,
) -> Result<u64, String> {
    let (ty, tx) = grid(ny, nx);
    let dir = root.join(level.to_string()).join(z.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut bytes = 0u64;
    let mut tile = Vec::with_capacity((TILE * TILE) as usize);
    for j in 0..ty {
        for i in 0..tx {
            let y0 = j * TILE;
            let x0 = i * TILE;
            let h = TILE.min(ny - y0);
            let w = TILE.min(nx - x0);
            tile.clear();
            for y in y0..y0 + h {
                let row = &plane[(y * nx + x0) as usize..(y * nx + x0 + w) as usize];
                tile.extend_from_slice(row);
            }
            let encoded = encode_tile(&tile, w, h)?;
            bytes += encoded.len() as u64;
            std::fs::write(tile_path(root, level, z, j, i), &encoded).map_err(|e| e.to_string())?;
        }
    }
    Ok(bytes)
}

/// The window the viewer opens at: the first and ninety-ninth percentiles
/// of a sample of planes.
fn window(vol: &Volume) -> Window {
    let nz = vol.shape[0] as usize;
    let step = (nz / 16).max(1);
    let mut sample: Vec<u16> = Vec::new();
    for z in (0..nz).step_by(step) {
        let p = vol.plane(z);
        let every = (p.len() / 65536).max(1);
        sample.extend(p.iter().step_by(every));
    }
    if sample.is_empty() {
        return Window {
            percentiles: [0, 0],
            center: 0.0,
            width: 1.0,
        };
    }
    sample.sort_unstable();
    let at = |q: f64| sample[((sample.len() - 1) as f64 * q) as usize] as i64 - vol.intercept;
    let (p1, p99) = (at(0.01), at(0.99));
    Window {
        percentiles: [p1, p99],
        center: (p1 + p99) as f64 / 2.0,
        width: ((p99 - p1) as f64).max(1.0),
    }
}

/// Build the pyramid of a volume under `root`, `workers` planes at a time.
pub fn build(
    vol: &Volume,
    stack: i64,
    root: &Path,
    workers: usize,
    pack_version: Option<String>,
) -> Result<Manifest, String> {
    let started = std::time::Instant::now();
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let [nz, ny, nx] = vol.shape;
    let workers = workers.max(1);
    let mut level_shapes = Vec::new();
    let mut bytes_per_level = Vec::new();
    // every level from the level before it, plane by plane, in parallel over planes
    let mut current: Vec<Vec<u16>> = (0..nz as usize).map(|z| vol.plane(z).to_vec()).collect();
    let (mut ly, mut lx) = (ny, nx);
    for level in 0..LEVELS {
        let (ty, tx) = grid(ly, lx);
        let planes = &current;
        let chunk = (planes.len() / workers).max(1);
        let results: Vec<Result<u64, String>> = std::thread::scope(|s| {
            let mut handles = Vec::new();
            for (c, part) in planes.chunks(chunk).enumerate() {
                let (ly, lx) = (ly, lx);
                handles.push(s.spawn(move || {
                    let mut total = 0u64;
                    for (k, plane) in part.iter().enumerate() {
                        let z = (c * chunk + k) as u32;
                        total += write_plane(root, level, z, plane, ly, lx)?;
                    }
                    Ok::<u64, String>(total)
                }));
            }
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err("a worker panicked".into())))
                .collect()
        });
        let mut bytes = 0u64;
        for r in results {
            bytes += r?;
        }
        level_shapes.push(Level {
            level,
            shape: [nz, ly, lx],
            tiles: [ty, tx],
            bytes,
        });
        bytes_per_level.push(bytes);
        if level + 1 < LEVELS {
            if ly < 2 || lx < 2 {
                break;
            }
            let next: Vec<Vec<u16>> = std::thread::scope(|s| {
                let handles: Vec<_> = current
                    .chunks(chunk)
                    .map(|part| {
                        s.spawn(move || part.iter().map(|p| halve(p, ly, lx).0).collect::<Vec<_>>())
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap_or_default())
                    .collect()
            });
            current = next;
            ly /= 2;
            lx /= 2;
        }
    }
    let annotation = Annotation {
        burned_in: vol.burned_in.unwrap_or(false),
        place: if vol.burned_in.is_some() {
            "header".to_string()
        } else {
            "top-and-bottom-eighths".to_string()
        },
    };
    let manifest = Manifest {
        stack,
        codec: CODEC.to_string(),
        tile: TILE,
        levels: level_shapes.len() as u32,
        shape: [nz, ny, nx],
        spacing: vol.spacing,
        dtype: "uint16".to_string(),
        intercept: -vol.intercept,
        window: window(vol),
        bytes_per_level,
        level_shapes,
        annotation,
        built_at: nils_registry::time::now_iso(),
        pack_version,
        precompute: Precompute {
            wall_seconds: started.elapsed().as_secs_f64(),
            workers,
            raw_bytes: vol.data.len() as u64 * 2,
        },
    };
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(root.join("manifest.json"), text).map_err(|e| e.to_string())?;
    Ok(manifest)
}

/// The manifest of a built pyramid, or none.
pub fn manifest(root: &Path) -> Result<Option<Manifest>, String> {
    let p = root.join("manifest.json");
    if !p.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// One plane's tiles in one container: `[u32 count][u32 offset...][tiles]`,
/// little endian; the offsets are from the start of the container, tiles
/// in row-major order of the plane's grid.
pub fn plane_container(root: &Path, m: &Manifest, level: u32, z: u32) -> Result<Vec<u8>, String> {
    let lv = m
        .level_shapes
        .get(level as usize)
        .ok_or_else(|| format!("level {level} is not in the pyramid; it has {}", m.levels))?;
    if z >= lv.shape[0] {
        return Err(format!("plane {z} is past the stack's {}", lv.shape[0]));
    }
    let [ty, tx] = lv.tiles;
    let mut tiles = Vec::with_capacity((ty * tx) as usize);
    for j in 0..ty {
        for i in 0..tx {
            tiles.push(std::fs::read(tile_path(root, level, z, j, i)).map_err(|e| e.to_string())?);
        }
    }
    Ok(container(&tiles))
}

pub fn container(tiles: &[Vec<u8>]) -> Vec<u8> {
    let head = 4 + 4 * tiles.len();
    let mut out = Vec::with_capacity(head + tiles.iter().map(Vec::len).sum::<usize>());
    out.write_all(&(tiles.len() as u32).to_le_bytes()).ok();
    let mut offset = head as u32;
    for t in tiles {
        out.write_all(&offset.to_le_bytes()).ok();
        offset += t.len() as u32;
    }
    for t in tiles {
        out.extend_from_slice(t);
    }
    out
}

/// A slab: the planes `z0..z1` each as a container, concatenated behind a
/// count and offsets of their own (the same shape one level up).
pub fn slab_container(
    root: &Path,
    m: &Manifest,
    level: u32,
    z0: u32,
    z1: u32,
) -> Result<Vec<u8>, String> {
    let planes: Vec<Vec<u8>> = (z0..z1)
        .map(|z| plane_container(root, m, level, z))
        .collect::<Result<_, _>>()?;
    Ok(container(&planes))
}

/// A whole plane decoded from its tiles.
pub fn decode_plane(
    root: &Path,
    m: &Manifest,
    level: u32,
    z: u32,
) -> Result<(u32, u32, Vec<u16>), String> {
    let lv = m
        .level_shapes
        .get(level as usize)
        .ok_or_else(|| format!("level {level} is not in the pyramid"))?;
    let [_, ny, nx] = lv.shape;
    let [ty, tx] = lv.tiles;
    let mut plane = vec![0u16; (ny * nx) as usize];
    for j in 0..ty {
        for i in 0..tx {
            let bytes =
                std::fs::read(tile_path(root, level, z, j, i)).map_err(|e| e.to_string())?;
            let (w, h, px) = decode_tile(&bytes)?;
            for y in 0..h {
                let src = &px[(y * w) as usize..((y + 1) * w) as usize];
                let dy = j * TILE + y;
                let dx = i * TILE;
                plane[(dy * nx + dx) as usize..(dy * nx + dx + w) as usize].copy_from_slice(src);
            }
        }
    }
    Ok((nx, ny, plane))
}

/// The plane along an axis: `z` is the index along that axis. `y` and `x`
/// planes are assembled from every plane's row or column (the slab door's
/// job in the browser; here for the server render).
pub fn plane_along(
    root: &Path,
    m: &Manifest,
    level: u32,
    axis: char,
    index: u32,
) -> Result<(u32, u32, Vec<u16>), String> {
    let lv = m
        .level_shapes
        .get(level as usize)
        .ok_or_else(|| format!("level {level} is not in the pyramid"))?;
    let [nz, ny, nx] = lv.shape;
    match axis {
        'z' => decode_plane(root, m, level, index.min(nz.saturating_sub(1))),
        'y' => {
            let row = index.min(ny.saturating_sub(1));
            let mut out = Vec::with_capacity((nz * nx) as usize);
            for z in 0..nz {
                let (w, _, px) = decode_plane(root, m, level, z)?;
                out.extend_from_slice(&px[(row * w) as usize..((row + 1) * w) as usize]);
            }
            Ok((nx, nz, out))
        }
        _ => {
            let col = index.min(nx.saturating_sub(1));
            let mut out = Vec::with_capacity((nz * ny) as usize);
            for z in 0..nz {
                let (w, h, px) = decode_plane(root, m, level, z)?;
                for y in 0..h {
                    out.push(px[(y * w + col) as usize]);
                }
            }
            Ok((ny, nz, out))
        }
    }
}

/// The plane with window and level applied, as a JPEG; `blank` blanks the
/// top and bottom eighths, where burned-in annotation is held.
pub fn render_jpeg(
    width: u32,
    height: u32,
    pixels: &[u16],
    intercept: i64,
    center: f64,
    wwidth: f64,
    blank: bool,
) -> Result<Vec<u8>, String> {
    let lo = center - wwidth / 2.0;
    let scale = 255.0 / wwidth.max(1.0);
    let mut gray = Vec::with_capacity(pixels.len());
    let band = height / 8;
    for (n, &p) in pixels.iter().enumerate() {
        let y = n as u32 / width.max(1);
        if blank && (y < band || y >= height - band) {
            gray.push(0u8);
            continue;
        }
        let v = (p as i64 + intercept) as f64;
        gray.push(((v - lo) * scale).clamp(0.0, 255.0) as u8);
    }
    let img =
        image::GrayImage::from_raw(width, height, gray).ok_or("the plane's size does not match")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&img)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// The pyramids a working place holds, by stack.
pub fn built(working: &Path) -> BTreeMap<i64, Manifest> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(working.join("pyramids")) else {
        return out;
    };
    for e in entries.flatten() {
        if let Ok(id) = e.file_name().to_string_lossy().parse::<i64>()
            && let Ok(Some(m)) = manifest(&e.path())
        {
            out.insert(id, m);
        }
    }
    out
}

/// The working place a pyramid is written under: the one named, or the
/// first active working place; refused with the rule's sentence when the
/// deployment binds none.
pub fn working_place(store: &mut Store, name: Option<&str>) -> Result<Place, String> {
    let places = place::list(store).map_err(|e| e.to_string())?;
    let active = places.iter().filter(|p| p.retired_at.is_none());
    if let Some(n) = name {
        return active
            .clone()
            .find(|p| p.name == n)
            .filter(|p| p.role == PlaceRole::Working)
            .cloned()
            .ok_or_else(|| format!("{n} is not an active working place (Wave 5 section 10.2)"));
    }
    active
        .clone()
        .find(|p| p.role == PlaceRole::Working)
        .cloned()
        .ok_or_else(|| {
            "no working place is bound: the pyramid is written under a working place, which an operator adds under Settings or with nils place add (Wave 5 section 10.2)".to_string()
        })
}

/// The window the audit counts one opening in: ten minutes.
const OPEN_WINDOW_SECS: u64 = 600;

/// One audit row per stack opened by a person, not per tile: the first
/// request in the window writes the row, the rest in the window do not.
fn note_open(
    registry: &mut nils_registry::Registry,
    caller: &Caller,
    stack: i64,
    level: u32,
    purpose: &str,
) -> Result<(), String> {
    use nils_registry::audit::{self, Action, Entry, Filter};
    let since = nils_registry::time::iso_of(
        nils_registry::time::now_secs().saturating_sub(OPEN_WINDOW_SECS),
    );
    let recent = audit::list(
        registry.store(),
        &Filter {
            principal: Some(caller.principal.clone()),
            action: Some("instance.open".to_string()),
            since: Some(since),
            limit: 200,
        },
    )
    .map_err(|e| e.to_string())?;
    if recent
        .iter()
        .any(|r| r.scope["stack"].as_i64() == Some(stack))
    {
        return Ok(());
    }
    audit::record(
        registry,
        &Entry {
            principal: &caller.principal,
            action: Action::InstanceOpen,
            scope: serde_json::json!({"stack": stack, "level": level, "purpose": purpose}),
            policy: None,
            job_id: None,
            details: None,
        },
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// The gated instance door (Wave 5 §12.7): `manifest`, `tiles/{level}/{z}`,
/// `slab/{level}/{z0}-{z1}`, `render/{level}/{z}` under a stack.
pub fn door(
    registry: &mut nils_registry::Registry,
    caller: &Caller,
    stack: &str,
    rest: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Result<Reply, Reply> {
    let stack: i64 = stack
        .parse()
        .map_err(|_| Reply::error(404, "a stack is named by its id"))?;
    let working = working_place(registry.store(), None).map_err(|m| Reply::error(409, m))?;
    let root = dir(Path::new(&working.path), stack);
    let m = manifest(&root)
        .map_err(|e| Reply::error(500, e))?
        .ok_or_else(|| {
            Reply::error(
                404,
                format!(
                    "no pyramid for stack {stack}; queue pyramid build --stack {stack} as a job"
                ),
            )
        })?;
    // the disclosure class: pixels are quasi-identifying, so the reviewer role
    // opens them; with burned-in annotation they are identifying until the
    // band is held, so the operator role opens the tiles and the slab
    if !caller.can(Role::Reviewer) {
        return Err(Reply::gated(
            403,
            format!(
                "the pixels of stack {stack} are quasi-identifying; the reviewer role opens them"
            ),
        ));
    }
    let held = m.annotation.burned_in && !caller.can(Role::Operator);
    let level_of = |s: &str| -> Result<u32, Reply> {
        let l: u32 = s
            .parse()
            .map_err(|_| Reply::error(400, "a level is a number from 0"))?;
        if l >= m.levels {
            return Err(Reply::error(
                404,
                format!("level {l} is not in the pyramid; it has {}", m.levels),
            ));
        }
        Ok(l)
    };
    let headers = |extra: Vec<(String, String)>| {
        let mut h = vec![
            ("X-Nils-Codec".to_string(), m.codec.clone()),
            ("X-Nils-Stack".to_string(), stack.to_string()),
            (
                "Cache-Control".to_string(),
                "private, max-age=3600".to_string(),
            ),
        ];
        h.extend(extra);
        h
    };
    match rest {
        ["manifest"] => {
            note_open(registry, caller, stack, 0, "manifest").map_err(|e| Reply::error(500, e))?;
            let mut doc = serde_json::to_value(&m).map_err(|e| Reply::error(500, e.to_string()))?;
            doc["place"] = serde_json::json!(working.name);
            doc["held"] = serde_json::json!(held);
            Ok(Reply::ok(doc))
        }
        ["tiles", level, z] => {
            let level = level_of(level)?;
            let z: u32 = z
                .parse()
                .map_err(|_| Reply::error(400, "a plane is a number from 0"))?;
            if held {
                return Err(Reply::gated(
                    403,
                    format!(
                        "stack {stack} carries burned-in annotation; its tiles open to the operator role, the render holds the band"
                    ),
                ));
            }
            note_open(registry, caller, stack, level, "tiles").map_err(|e| Reply::error(500, e))?;
            let bytes = plane_container(&root, &m, level, z).map_err(|e| Reply::error(404, e))?;
            Ok(Reply::raw(
                CONTENT_TYPE,
                bytes,
                headers(vec![("X-Nils-Planes".to_string(), "1".to_string())]),
            ))
        }
        ["slab", level, range] => {
            let level = level_of(level)?;
            let (z0, z1) = range
                .split_once('-')
                .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
                .ok_or_else(|| Reply::error(400, "a slab is z0-z1, the end exclusive"))?;
            if z1 <= z0 {
                return Err(Reply::error(400, "a slab is z0-z1 with z1 past z0"));
            }
            if z1 - z0 > SLAB_MAX {
                return Err(Reply::error(
                    416,
                    format!("a slab is at most {SLAB_MAX} planes; {} asked", z1 - z0),
                ));
            }
            if held {
                return Err(Reply::gated(
                    403,
                    format!(
                        "stack {stack} carries burned-in annotation; its slab opens to the operator role, the render holds the band"
                    ),
                ));
            }
            let nz = m.level_shapes[level as usize].shape[0];
            if z1 > nz {
                return Err(Reply::error(
                    416,
                    format!("the stack has {nz} planes at level {level}"),
                ));
            }
            note_open(registry, caller, stack, level, "slab").map_err(|e| Reply::error(500, e))?;
            let bytes =
                slab_container(&root, &m, level, z0, z1).map_err(|e| Reply::error(404, e))?;
            Ok(Reply::raw(
                CONTENT_TYPE,
                bytes,
                headers(vec![("X-Nils-Planes".to_string(), (z1 - z0).to_string())]),
            ))
        }
        ["render", level, z] => {
            let level = level_of(level)?;
            let z: u32 = z
                .parse()
                .map_err(|_| Reply::error(400, "a plane is a number from 0"))?;
            let axis = query
                .get("axis")
                .and_then(|a| a.chars().next())
                .filter(|c| matches!(c, 'x' | 'y' | 'z'))
                .unwrap_or('z');
            let center = query
                .get("c")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(m.window.center);
            let width = query
                .get("w")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(m.window.width);
            note_open(registry, caller, stack, level, "render")
                .map_err(|e| Reply::error(500, e))?;
            let (w, h, px) =
                plane_along(&root, &m, level, axis, z).map_err(|e| Reply::error(404, e))?;
            // the band is held on every axis when the annotation is burned in and the caller is below the class
            let jpeg = render_jpeg(w, h, &px, m.intercept, center, width, held)
                .map_err(|e| Reply::error(500, e))?;
            Ok(Reply::raw(
                "image/jpeg",
                jpeg,
                headers(vec![("X-Nils-Held".to_string(), held.to_string())]),
            ))
        }
        _ => Err(Reply::error(
            404,
            format!(
                "GET /api/instances/{stack}/{} is not a door; manifest, tiles/{{level}}/{{z}}, slab/{{level}}/{{z0}}-{{z1}} and render/{{level}}/{{z}} are",
                rest.join("/")
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_round_trips_reversibly() {
        let (w, h) = (64u32, 48u32);
        let px: Vec<u16> = (0..w * h).map(|i| ((i * 37) % 65536) as u16).collect();
        let bytes = encode_tile(&px, w, h).unwrap();
        assert!(bytes.len() > 20);
        let (dw, dh, back) = decode_tile(&bytes).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(back, px);
    }

    #[test]
    fn an_edge_tile_smaller_than_the_grid_round_trips() {
        let (w, h) = (13u32, 7u32);
        let px: Vec<u16> = (0..w * h).map(|i| (i * 1000 % 65535) as u16).collect();
        let bytes = encode_tile(&px, w, h).unwrap();
        let (_, _, back) = decode_tile(&bytes).unwrap();
        assert_eq!(back, px);
    }

    #[test]
    fn the_container_carries_offsets_to_every_tile() {
        let c = container(&[vec![1, 2, 3], vec![], vec![9]]);
        assert_eq!(u32::from_le_bytes(c[0..4].try_into().unwrap()), 3);
        let off =
            |i: usize| u32::from_le_bytes(c[4 + 4 * i..8 + 4 * i].try_into().unwrap()) as usize;
        assert_eq!((off(0), off(1), off(2)), (16, 19, 19));
        assert_eq!(&c[off(0)..off(1)], &[1, 2, 3]);
        assert_eq!(&c[off(2)..], &[9]);
    }
}
