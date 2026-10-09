// SPDX-License-Identifier: AGPL-3.0-only
//! A pyramid level packed into one file (record 55 H2, slice E6).
//!
//! A pyramid used to keep every tile in a file of its own, so a 160-plane
//! stack of 512 by 512 was about 1,100 files, and a coronal or sagittal
//! render, which needs a strip of every plane, opened one file per plane:
//! 1.3 to 1.7 s on storage that charges for each open (a network share, an
//! old pool). Now each level is one file, `<level>.tiles`, beside the
//! manifest:
//!
//! ```text
//! 0   magic "NILSLVL1"            8 bytes
//! 8   version (1)                 u32
//! 12  planes, tile rows, columns  3 x u32
//! 24  where the tiles start       u64
//! 32  the index: per tile, in the order plane, tile row, tile column,
//!     its offset from the file's start (u64) and its length (u32)
//! ... the tiles, in the index's order, one after another
//! ```
//!
//! little endian. The tiles of one plane, and of a run of planes, are one
//! piece of the file, so a slab of 32 planes is one read. A level file is
//! written to a part file and renamed into place, so it is either whole or
//! not there. The engine keeps the level files it has opened, with their
//! indexes, and reads with positioned reads; it checks the file is the one
//! it opened (its length, time and inode) on every lookup, so a rebuilt or
//! repacked level is opened again.
//!
//! A level without its packed file is read from the old layout,
//! `<level>/<plane>/<row>_<column>.j2c`, so pyramids built before still
//! open; `nils pyramid pack` converts them.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

pub const MAGIC: &[u8; 8] = b"NILSLVL1";
pub const VERSION: u32 = 1;
const HEAD: usize = 32;
const ENTRY: usize = 12;

/// The packed file of a level.
pub fn level_path(root: &Path, level: u32) -> PathBuf {
    root.join(format!("{level}.tiles"))
}

/// A tile's file in the layout before packing.
pub fn loose_path(root: &Path, level: u32, z: u32, ty: u32, tx: u32) -> PathBuf {
    root.join(level.to_string())
        .join(z.to_string())
        .join(format!("{ty}_{tx}.j2c"))
}

/// A level's shape in tiles: planes, tile rows, tile columns.
pub type Grid = [u32; 3];

fn count(grid: Grid) -> usize {
    grid[0] as usize * grid[1] as usize * grid[2] as usize
}

/// Write a level's tiles (in the order plane, row, column) as one packed
/// file: to a part file of its own, flushed to the storage, then renamed
/// over `<level>.tiles`. Returns the tiles' bytes, without the index.
pub fn write_level(root: &Path, level: u32, grid: Grid, tiles: &[&[u8]]) -> Result<u64, String> {
    let n = count(grid);
    if tiles.len() != n {
        return Err(format!(
            "level {level} has {} tiles, and its grid {grid:?} holds {n}",
            tiles.len()
        ));
    }
    let start = (HEAD + ENTRY * n) as u64;
    let mut head = Vec::with_capacity(HEAD + ENTRY * n);
    head.extend_from_slice(MAGIC);
    head.extend_from_slice(&VERSION.to_le_bytes());
    for g in grid {
        head.extend_from_slice(&g.to_le_bytes());
    }
    head.extend_from_slice(&start.to_le_bytes());
    let mut at = start;
    for t in tiles {
        let len = u32::try_from(t.len()).map_err(|_| "a tile is over 4 GB".to_string())?;
        head.extend_from_slice(&at.to_le_bytes());
        head.extend_from_slice(&len.to_le_bytes());
        at += t.len() as u64;
    }
    let path = level_path(root, level);
    // a part of its own per writer: two builds of one stack at once must
    // not write one part file together
    static PART: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let k = PART.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let part = root.join(format!("{level}.tiles.part.{}.{k}", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let f = File::create(&part)?;
        let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
        w.write_all(&head)?;
        for t in tiles {
            w.write_all(t)?;
        }
        let f = w.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&part, &path)?;
        // the rename itself is durable once the folder is
        if let Ok(d) = File::open(root) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(format!("level {level}'s packed file was not written: {e}"));
    }
    Ok(at - start)
}

/// Part files a writer left behind when it stopped half way.
pub fn remove_parts(root: &Path) -> usize {
    let mut n = 0;
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().contains(".tiles.part.")
                && std::fs::remove_file(e.path()).is_ok()
            {
                n += 1;
            }
        }
    }
    n
}

/// An open level file and its index.
pub struct Packed {
    file: File,
    pub grid: Grid,
    index: Vec<(u64, u32)>,
    stamp: Stamp,
}

/// What a level file is: its length, its modification time and its inode.
type Stamp = (u64, Option<std::time::SystemTime>, u64);

fn stamp_of(meta: &std::fs::Metadata) -> Stamp {
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(meta);
    #[cfg(not(unix))]
    let ino = 0;
    (meta.len(), meta.modified().ok(), ino)
}

/// Read exactly `buf.len()` bytes at `offset`, without moving a cursor:
/// many readers share one open file.
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < buf.len() {
            let n = std::os::windows::fs::FileExt::seek_read(
                file,
                &mut buf[done..],
                offset + done as u64,
            )?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]))
}

impl Packed {
    /// Open a level file and read its index, checking that every tile lies
    /// inside the file.
    pub fn open(path: &Path) -> std::io::Result<Packed> {
        let file = File::open(path)?;
        let meta = file.metadata()?;
        let bad = |why: &str| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a packed level: {why}", path.display()),
            )
        };
        let mut head = [0u8; HEAD];
        read_at(&file, &mut head, 0).map_err(|_| bad("it is shorter than its header"))?;
        if &head[..8] != MAGIC {
            return Err(bad("no magic"));
        }
        if u32_at(&head, 8) != VERSION {
            return Err(bad("a version this engine does not read"));
        }
        let grid = [u32_at(&head, 12), u32_at(&head, 16), u32_at(&head, 20)];
        let n = count(grid);
        let start = u64_at(&head, 24);
        if start != (HEAD + ENTRY * n) as u64 || start > meta.len() {
            return Err(bad("its index does not fit"));
        }
        let mut raw = vec![0u8; ENTRY * n];
        read_at(&file, &mut raw, HEAD as u64).map_err(|_| bad("its index is cut short"))?;
        let mut index = Vec::with_capacity(n);
        for i in 0..n {
            let (off, len) = (u64_at(&raw, i * ENTRY), u32_at(&raw, i * ENTRY + 8));
            if off < start || off + len as u64 > meta.len() {
                return Err(bad("a tile lies outside the file"));
            }
            index.push((off, len));
        }
        Ok(Packed {
            file,
            grid,
            index,
            stamp: stamp_of(&meta),
        })
    }

    /// The tiles of planes `z0..z1`, in the order plane, row, column, read
    /// as one piece of the file.
    pub fn read_planes(&self, z0: u32, z1: u32) -> std::io::Result<Vec<Vec<u8>>> {
        let per = self.grid[1] as usize * self.grid[2] as usize;
        let (a, b) = (z0 as usize * per, (z1 as usize * per).min(self.index.len()));
        if a >= b {
            return Ok(Vec::new());
        }
        let first = self.index[a].0;
        let (lo, ll) = self.index[b - 1];
        let mut buf = vec![0u8; (lo + ll as u64 - first) as usize];
        read_at(&self.file, &mut buf, first)?;
        let mut out = Vec::with_capacity(b - a);
        for &(off, len) in &self.index[a..b] {
            let at = (off - first) as usize;
            let tile = buf.get(at..at + len as usize).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the packed level's tiles are not in its index's order",
                )
            })?;
            out.push(tile.to_vec());
        }
        Ok(out)
    }

    /// Every tile of the level.
    pub fn read_all(&self) -> std::io::Result<Vec<Vec<u8>>> {
        self.read_planes(0, self.grid[0])
    }
}

/// How many level files the engine keeps open: each holds one descriptor
/// and an index of twelve bytes a tile.
pub const KEPT_OPEN: usize = 128;

struct Kept {
    files: HashMap<PathBuf, (Arc<Packed>, u64)>,
    tick: u64,
}

static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(|| {
    Mutex::new(Kept {
        files: HashMap::new(),
        tick: 0,
    })
});

fn kept() -> std::sync::MutexGuard<'static, Kept> {
    KEPT.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The packed file of a level, from the files the engine keeps open while
/// it is the same file, or None where the level is not packed (the layout
/// before). A file that cannot be opened or read is an error.
pub fn level(root: &Path, level: u32) -> std::io::Result<Option<Arc<Packed>>> {
    let path = level_path(root, level);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            kept().files.remove(&path);
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let stamp = stamp_of(&meta);
    {
        let mut k = kept();
        k.tick += 1;
        let tick = k.tick;
        if let Some((p, used)) = k.files.get_mut(&path)
            && p.stamp == stamp
        {
            *used = tick;
            return Ok(Some(Arc::clone(p)));
        }
    }
    let p = Arc::new(Packed::open(&path)?);
    let mut k = kept();
    if k.files.len() >= KEPT_OPEN
        && let Some(oldest) = k
            .files
            .iter()
            .min_by_key(|(_, (_, used))| *used)
            .map(|(path, _)| path.clone())
    {
        k.files.remove(&oldest);
    }
    let tick = k.tick;
    k.files.insert(path, (Arc::clone(&p), tick));
    Ok(Some(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_reads_back_tile_by_tile_and_as_runs_of_planes() {
        let dir = nils_dicom::synth::TempDir::new("tilepack-round-trip");
        let root = dir.path();
        let tiles: Vec<Vec<u8>> = (0..3 * 2 * 2).map(|i| vec![i as u8; 10 + i * 7]).collect();
        let refs: Vec<&[u8]> = tiles.iter().map(Vec::as_slice).collect();
        let bytes = write_level(root, 0, [3, 2, 2], &refs).unwrap();
        assert_eq!(bytes, tiles.iter().map(|t| t.len() as u64).sum::<u64>());
        let p = level(root, 0).unwrap().unwrap();
        assert_eq!(p.grid, [3, 2, 2]);
        assert_eq!(p.read_all().unwrap(), tiles);
        assert_eq!(p.read_planes(1, 2).unwrap(), tiles[4..8].to_vec());
        assert_eq!(p.read_planes(1, 3).unwrap(), tiles[4..].to_vec());
        // no part file is left, and a level not packed is None
        let names: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["0.tiles".to_string()]);
        assert!(level(root, 1).unwrap().is_none());
    }

    #[test]
    fn a_level_written_again_is_opened_again() {
        let dir = nils_dicom::synth::TempDir::new("tilepack-again");
        let root = dir.path();
        write_level(root, 0, [1, 1, 1], &[b"first"]).unwrap();
        let a = level(root, 0).unwrap().unwrap();
        assert_eq!(a.read_all().unwrap(), vec![b"first".to_vec()]);
        write_level(root, 0, [1, 1, 2], &[b"second", b"third!"]).unwrap();
        let b = level(root, 0).unwrap().unwrap();
        assert_eq!(b.grid, [1, 1, 2]);
        assert_eq!(
            b.read_all().unwrap(),
            vec![b"second".to_vec(), b"third!".to_vec()]
        );
    }

    #[test]
    fn a_cut_or_foreign_file_is_refused_not_misread() {
        let dir = nils_dicom::synth::TempDir::new("tilepack-bad");
        let root = dir.path();
        write_level(root, 0, [2, 1, 1], &[b"aaaa", b"bbbb"]).unwrap();
        let path = level_path(root, 0);
        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() - 2]).unwrap();
        assert!(level(root, 0).is_err());
        std::fs::write(&path, b"not a pyramid level at all, just words").unwrap();
        assert!(level(root, 0).is_err());
        assert!(write_level(root, 1, [2, 1, 1], &[b"one"]).is_err());
        assert!(!level_path(root, 1).exists());
    }
}
