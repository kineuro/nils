// SPDX-License-Identifier: AGPL-3.0-only

//! Secret inputs (record 49 R3): a file the site keeps, such as the lab's
//! FreeSurfer licence, that a pipeline declares under `x-nils.secrets`.
//!
//! The engine reads the file when a run starts and mounts it read-only into
//! that pipeline's containers alone. It is never copied: not into an input,
//! an output, a log, the run's record or its results. A container can still
//! print what it was given, so after each container the engine sweeps what
//! it left: a log is written again with every occurrence replaced by
//! `[secret <id>]`, `results.json` likewise, and any other file that holds
//! the secret is removed and refused. What the run records is the secret's
//! id, never its path or its bytes.

use std::io::Read;
use std::path::{Path, PathBuf};

/// A secret the engine holds for one run.
#[derive(Debug, Clone)]
pub struct Held {
    pub id: String,
    /// What is looked for: the whole file, trimmed, and each of its lines
    /// of eight characters or more, and each of those as base64 would
    /// spell it at any of its three alignments.
    pub needles: Vec<Vec<u8>>,
}

impl Held {
    /// A secret held by its id, looked for by the needles of its bytes.
    pub fn new(id: &str, bytes: &[u8]) -> Held {
        Held {
            id: id.to_string(),
            needles: needles(bytes),
        }
    }
}

/// The largest secret file the engine takes: a licence is a few lines.
pub const MAX_BYTES: u64 = 64 * 1024;

/// How deep the sweep reads into archives inside archives: a gzip stream
/// in a tar in a gzip stream is three.
const MAX_DEPTH: usize = 4;

/// Read a secret for a run: a regular file, no larger than [`MAX_BYTES`],
/// and not empty. The error names the id and the cure, never the bytes.
pub fn read(id: &str, path: &Path) -> Result<Held, String> {
    let meta = std::fs::metadata(path).map_err(|e| {
        format!(
            "the secret {id} is set to a file that cannot be read ({e}); nils pipeline secret set {id} --file <path>"
        )
    })?;
    if !meta.is_file() {
        return Err(format!("the secret {id} is set to something not a file"));
    }
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "the secret {id} is {} bytes, more than a secret file's {MAX_BYTES}",
            meta.len()
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| format!("the secret {id} cannot be read: {e}"))?;
    let held = Held::new(id, &bytes);
    if held.needles.is_empty() {
        return Err(format!("the secret {id} is an empty file"));
    }
    Ok(held)
}

/// The plain needles of a secret: the whole of it, trimmed, and each line
/// of eight bytes or more, trimmed, longest first.
fn plain_needles(bytes: &[u8]) -> Vec<Vec<u8>> {
    let trim = |b: &[u8]| -> Vec<u8> {
        let start = b.iter().position(|c| !c.is_ascii_whitespace());
        let end = b.iter().rposition(|c| !c.is_ascii_whitespace());
        match (start, end) {
            (Some(s), Some(e)) => b[s..=e].to_vec(),
            _ => Vec::new(),
        }
    };
    let mut out: Vec<Vec<u8>> = Vec::new();
    let whole = trim(bytes);
    if !whole.is_empty() {
        out.push(whole);
    }
    for line in bytes.split(|c| *c == b'\n') {
        let l = trim(line);
        if l.len() >= 8 && !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

/// What a secret is looked for by: its plain needles, and the part of each
/// that base64 spells the same wherever in a longer text it falls (at each
/// of the three alignments, less the characters its neighbours share),
/// longest first.
pub fn needles(bytes: &[u8]) -> Vec<Vec<u8>> {
    let plain = plain_needles(bytes);
    let mut out = plain.clone();
    for n in &plain {
        for pad in 0..3usize {
            let mut b = vec![0u8; pad];
            b.extend_from_slice(n);
            let enc = encode_base64(&b);
            // the characters that read the padding, and those that read
            // what follows the needle, differ with the neighbours
            let head = (pad * 4).div_ceil(3);
            let tail_bytes = b.len() % 3;
            let full = (b.len() / 3) * 4;
            let end = if tail_bytes == 0 {
                full
            } else {
                full.min(enc.len())
            };
            if end > head + 8 {
                let inner = enc[head..end].to_vec();
                if !out.contains(&inner) {
                    out.push(inner);
                }
            }
        }
    }
    out.sort_by_key(|n| std::cmp::Reverse(n.len()));
    out
}

/// Standard base64, padded.
pub fn encode_base64(bytes: &[u8]) -> Vec<u8> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(A[(n >> 18) as usize & 63]);
        out.push(A[(n >> 12) as usize & 63]);
        out.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(if c.len() > 2 {
            A[n as usize & 63]
        } else {
            b'='
        });
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Whether some bytes hold any of the secrets.
pub fn holds(bytes: &[u8], held: &[Held]) -> bool {
    held.iter()
        .any(|h| h.needles.iter().any(|n| find(bytes, n).is_some()))
}

/// What reading a file for the secrets found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Clean,
    Holds,
    /// It could not be read inside: an archive the sweep does not open, a
    /// broken gzip stream, or nesting past [`MAX_DEPTH`].
    Unscannable(String),
}

/// A compressed or archived form the sweep cannot read inside, by its
/// first bytes.
fn closed_form(head: &[u8]) -> Option<&'static str> {
    let starts = |m: &[u8]| head.starts_with(m);
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        Some("a zip archive")
    } else if starts(b"BZh") {
        Some("a bzip2 stream")
    } else if starts(b"\xFD7zXZ\x00") {
        Some("an xz stream")
    } else if starts(b"\x28\xB5\x2F\xFD") {
        Some("a zstd stream")
    } else if starts(b"7z\xBC\xAF\x27\x1C") {
        Some("a 7z archive")
    } else if starts(b"\x04\x22\x4D\x18") {
        Some("an lz4 stream")
    } else {
        None
    }
}

/// Read up to `n` bytes, fewer only at the end.
fn read_head(r: &mut dyn Read, n: usize) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(n);
    r.take(n as u64).read_to_end(&mut head)?;
    Ok(head)
}

/// Scan a stream for the secrets, opening a gzip stream and the members of
/// a tar, each as deep as [`MAX_DEPTH`]; `tick` is called for each piece
/// read, so a long scan can say it is alive.
fn scan(
    r: &mut dyn Read,
    held: &[Held],
    longest: usize,
    depth: usize,
    tick: &mut dyn FnMut(),
) -> std::io::Result<Found> {
    let head = read_head(r, 512)?;
    if head.starts_with(&[0x1f, 0x8b]) {
        if depth >= MAX_DEPTH {
            return Ok(Found::Unscannable("gzip streams nested too deep".into()));
        }
        let chained = std::io::Cursor::new(head).chain(r);
        let mut gz = flate2::read::MultiGzDecoder::new(chained);
        return match scan(&mut gz, held, longest, depth + 1, tick) {
            Ok(f) => Ok(f),
            Err(e)
                if e.kind() == std::io::ErrorKind::InvalidInput
                    || e.kind() == std::io::ErrorKind::InvalidData
                    || e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                Ok(Found::Unscannable(format!(
                    "a gzip stream that does not read: {e}"
                )))
            }
            Err(e) => Err(e),
        };
    }
    if let Some(form) = closed_form(&head) {
        return Ok(Found::Unscannable(format!(
            "{form}, which the sweep does not open"
        )));
    }
    if head.len() == 512 && &head[257..262] == b"ustar" {
        if depth >= MAX_DEPTH {
            return Ok(Found::Unscannable("archives nested too deep".into()));
        }
        return scan_tar(head, r, held, longest, depth + 1, tick);
    }
    // plain bytes, in pieces that overlap by the longest needle
    if holds(&head, held) {
        return Ok(Found::Holds);
    }
    let mut carry = head;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let keep = longest.saturating_sub(1).min(carry.len());
        carry.drain(..carry.len() - keep);
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(Found::Clean);
        }
        tick();
        carry.extend_from_slice(&buf[..n]);
        if holds(&carry, held) {
            return Ok(Found::Holds);
        }
    }
}

/// The members of a tar, each scanned as a stream of its own; the headers
/// are scanned too, since a member's name may spell the secret.
fn scan_tar(
    first: Vec<u8>,
    r: &mut dyn Read,
    held: &[Held],
    longest: usize,
    depth: usize,
    tick: &mut dyn FnMut(),
) -> std::io::Result<Found> {
    let mut header = first;
    loop {
        if header.len() < 512 || header.iter().all(|b| *b == 0) {
            // the end, or two blocks of zeros: whatever follows is read
            // as plain bytes
            let mut rest = std::io::Cursor::new(header).chain(r);
            return scan(&mut rest, held, longest, MAX_DEPTH, tick);
        }
        if holds(&header, held) {
            return Ok(Found::Holds);
        }
        let size_field = String::from_utf8_lossy(&header[124..136]).to_string();
        let Ok(size) =
            u64::from_str_radix(size_field.trim_matches(|c: char| c == '\0' || c == ' '), 8)
        else {
            return Ok(Found::Unscannable(
                "a tar whose header does not read".into(),
            ));
        };
        let mut member = r.take(size);
        match scan(&mut member, held, longest, depth, tick)? {
            Found::Clean => {}
            other => return Ok(other),
        }
        std::io::copy(&mut member, &mut std::io::sink())?;
        let pad = (512 - size % 512) % 512;
        std::io::copy(&mut r.take(pad), &mut std::io::sink())?;
        header = read_head(r, 512)?;
    }
}

/// Open a file never through a link.
fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    o.open(path)
}

/// Whether a file holds any of the secrets, read in pieces that overlap by
/// the longest needle, so a large output is never read whole, and inside
/// its gzip streams and tar members.
pub fn file_holds(path: &Path, held: &[Held]) -> std::io::Result<bool> {
    Ok(file_found(path, held, &mut || {})? != Found::Clean)
}

/// What a file holds of the secrets; see [`file_holds`].
pub fn file_found(path: &Path, held: &[Held], tick: &mut dyn FnMut()) -> std::io::Result<Found> {
    let longest = held
        .iter()
        .flat_map(|h| h.needles.iter().map(Vec::len))
        .max()
        .unwrap_or(0);
    if longest == 0 {
        return Ok(Found::Clean);
    }
    let mut f = open_nofollow(path)?;
    scan(&mut f, held, longest, 0, tick)
}

/// Some bytes with every occurrence of a secret replaced by `[secret <id>]`;
/// whether any was.
pub fn redact(bytes: &[u8], held: &[Held]) -> (Vec<u8>, bool) {
    let mut out = bytes.to_vec();
    let mut any = false;
    for h in held {
        let mark = format!("[secret {}]", h.id).into_bytes();
        for n in &h.needles {
            // look on after each mark, so a secret the mark itself spells
            // is never replaced forever
            let mut from = 0;
            while let Some(at) = find(&out[from..], n).map(|i| i + from) {
                out.splice(at..at + n.len(), mark.iter().copied());
                from = at + mark.len();
                any = true;
            }
        }
    }
    (out, any)
}

/// What a sweep did to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Swept {
    /// Written again with the secret replaced.
    Redacted(PathBuf),
    /// Removed: an output that held the secret.
    Removed(PathBuf),
    /// Removed: an output the sweep could not read inside, and why.
    Unscannable(PathBuf, String),
}

/// What a sweep did, and what it could not read at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    pub done: Vec<Swept>,
    /// Folders and files it could not read even after making them its own
    /// again, relative to the folder swept: the unit they belong to is not
    /// vouched for.
    pub unreadable: Vec<PathBuf>,
}

/// Give a folder or file that a container shut back to its owner, never
/// through a link.
#[cfg(unix)]
fn reopen(path: &Path, dir: bool) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_symlink() => std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if dir { 0o700 } else { 0o600 }),
        )
        .is_ok(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn reopen(_path: &Path, _dir: bool) -> bool {
    false
}

/// Sweep what a container left: every regular file under `dir` (links are
/// never followed) that holds a secret, or that is an archive the sweep
/// cannot read inside, is removed, except the files named in `rewrite` (a
/// log, `results.json`), which are written again with the secret replaced,
/// through `scratch`, a folder the container never saw. A folder or file
/// it cannot read, even after giving it back to its owner, is named in
/// [`Sweep::unreadable`] and the rest is swept. `tick` is called as it
/// reads.
pub fn sweep(
    dir: &Path,
    held: &[Held],
    rewrite: &[&Path],
    scratch: &Path,
    tick: &mut dyn FnMut(),
) -> std::io::Result<Sweep> {
    let mut out = Sweep::default();
    if held.is_empty() {
        return Ok(out);
    }
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => {}
        _ => return Ok(out),
    }
    let rel = |p: &Path| p.strip_prefix(dir).unwrap_or(p).to_path_buf();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        tick();
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) if reopen(&d, true) => match std::fs::read_dir(&d) {
                Ok(e) => e,
                Err(_) => {
                    out.unreadable.push(rel(&d));
                    continue;
                }
            },
            Err(_) => {
                out.unreadable.push(rel(&d));
                continue;
            }
        };
        for e in entries {
            let Ok(e) = e else {
                out.unreadable.push(rel(&d));
                continue;
            };
            let path = e.path();
            let Ok(kind) = e.file_type() else {
                out.unreadable.push(rel(&path));
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let found = match file_found(&path, held, tick) {
                Ok(f) => f,
                Err(_) if reopen(&path, false) => match file_found(&path, held, tick) {
                    Ok(f) => f,
                    Err(_) => {
                        out.unreadable.push(rel(&path));
                        continue;
                    }
                },
                Err(_) => {
                    out.unreadable.push(rel(&path));
                    continue;
                }
            };
            match found {
                Found::Clean => {}
                Found::Holds if rewrite.contains(&path.as_path()) => {
                    redact_file(&path, held, scratch)?;
                    out.done.push(Swept::Redacted(rel(&path)));
                }
                Found::Holds => {
                    std::fs::remove_file(&path)?;
                    out.done.push(Swept::Removed(rel(&path)));
                }
                Found::Unscannable(why) => {
                    std::fs::remove_file(&path)?;
                    out.done.push(Swept::Unscannable(rel(&path), why));
                }
            }
        }
    }
    out.done
        .sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    out.unreadable.sort();
    out.unreadable.dedup();
    Ok(out)
}

/// Write a file again with every secret in it replaced; whether one was.
/// The file is read never through a link, and the new copy is written
/// first in `scratch`, a folder the container never saw, created new and
/// never through a link, then renamed over it.
pub fn redact_file(path: &Path, held: &[Held], scratch: &Path) -> std::io::Result<bool> {
    if held.is_empty() {
        return Ok(false);
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => {}
        _ => return Ok(false),
    }
    let mut bytes = Vec::new();
    open_nofollow(path)?.read_to_end(&mut bytes)?;
    let (clean, any) = redact(&bytes, held);
    if any {
        std::fs::create_dir_all(scratch)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let tmp = scratch.join(format!(".{name}.redacting-{}-{nonce}", std::process::id()));
        crate::files::write_new(&tmp, &clean)?;
        if std::fs::rename(&tmp, path).is_err() {
            // another filesystem: the file goes, and the copy is written
            // new where it was, never through a link
            let _ = std::fs::remove_file(&tmp);
            std::fs::remove_file(path)?;
            crate::files::write_new(path, &clean)?;
        }
    }
    Ok(any)
}

/// Replace every secret in the strings of a JSON value; whether one was.
pub fn redact_json(v: &mut serde_json::Value, held: &[Held]) -> bool {
    match v {
        serde_json::Value::String(s) => {
            let (clean, any) = redact(s.as_bytes(), held);
            if any {
                *s = String::from_utf8_lossy(&clean).into_owned();
            }
            any
        }
        // every string is redacted, so none is skipped once one was
        serde_json::Value::Array(items) => {
            let mut any = false;
            for i in items.iter_mut() {
                any |= redact_json(i, held);
            }
            any
        }
        serde_json::Value::Object(map) => {
            let mut any = false;
            for i in map.values_mut() {
                any |= redact_json(i, held);
            }
            any
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LICENCE: &str = "someone@lab.example\n12345\n *CxYz0123abcdEF\n FSaBcDeFgHiJk\n";

    fn held() -> Vec<Held> {
        vec![Held::new("freesurfer_license", LICENCE.as_bytes())]
    }

    #[test]
    fn a_secret_is_looked_for_whole_and_line_by_line() {
        let n = plain_needles(LICENCE.as_bytes());
        // the whole, and the three lines of eight or more; "12345" is too
        // short to look for alone
        assert_eq!(n.len(), 4, "{n:?}");
        // and each as base64 spells it, at three alignments
        assert!(needles(LICENCE.as_bytes()).len() > 4);
        assert!(n.iter().all(|x| x.as_slice() != b"12345"));
        let h = held();
        assert!(holds(b"licence: *CxYz0123abcdEF ok", &h));
        assert!(!holds(b"nothing here 12345", &h));
        let (clean, any) = redact(b"a someone@lab.example b FSaBcDeFgHiJk", &h);
        assert!(any);
        assert_eq!(
            String::from_utf8(clean).unwrap(),
            "a [secret freesurfer_license] b [secret freesurfer_license]"
        );
        let mut v =
            serde_json::json!({"units": [{"error": "read *CxYz0123abcdEF", "metrics": {"n": 3}}]});
        assert!(redact_json(&mut v, &h));
        assert!(!v.to_string().contains("CxYz"), "{v}");
    }

    #[test]
    fn a_sweep_rewrites_the_log_and_results_and_removes_an_output_that_holds_it() {
        let dir = std::env::temp_dir().join(format!("nils-secret-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub-1")).unwrap();
        let h = held();
        std::fs::write(dir.join("log.txt"), format!("start\n{LICENCE}end\n")).unwrap();
        std::fs::write(dir.join("results.json"), "{\"e\": \"FSaBcDeFgHiJk\"}").unwrap();
        // a large output with the secret across the edge of a read
        let mut big = vec![b'x'; (1 << 20) - 5];
        big.extend_from_slice(b"*CxYz0123abcdEF");
        std::fs::write(dir.join("sub-1/leak.bin"), &big).unwrap();
        std::fs::write(dir.join("sub-1/clean.nii"), vec![0u8; 3000]).unwrap();
        let scratch = dir.with_extension("nils");
        let done = sweep(
            &dir,
            &h,
            &[&dir.join("log.txt"), &dir.join("results.json")],
            &scratch,
            &mut || {},
        )
        .unwrap()
        .done;
        assert_eq!(done.len(), 3, "{done:?}");
        assert!(!dir.join("sub-1/leak.bin").exists());
        assert!(dir.join("sub-1/clean.nii").exists());
        for f in ["log.txt", "results.json"] {
            let t = std::fs::read(dir.join(f)).unwrap();
            assert!(!holds(&t, &h), "{f}");
            assert!(String::from_utf8_lossy(&t).contains("[secret freesurfer_license]"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A container may plant a link where the rewrite of a file would write
    /// its copy; the rewrite never follows it, and writes where the
    /// container never saw.
    #[cfg(unix)]
    #[test]
    fn a_rewrite_never_follows_a_link_the_container_planted() {
        let dir = std::env::temp_dir().join(format!("nils-secret-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let victim = dir.join("cached.sif");
        std::fs::write(&victim, b"the image").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("out/results.redacting")).unwrap();
        let results = dir.join("out/results.json");
        std::fs::write(&results, "{\"e\": \"FSaBcDeFgHiJk\"}").unwrap();
        let h = held();
        let _ = sweep(
            &dir.join("out"),
            &h,
            &[&results],
            &dir.join("out.nils"),
            &mut || {},
        );
        assert_eq!(std::fs::read(&victim).unwrap(), b"the image");
        assert!(!holds(&std::fs::read(&results).unwrap(), &h));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A secret inside a gzip stream (a .gz, a .tar.gz, an .mgz), inside a
    /// gzip member of a tar, or written as base64 is found; an archive the
    /// sweep cannot read inside is refused.
    #[test]
    fn a_secret_compressed_archived_or_encoded_is_found() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("nils-secret-deep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let gz = |bytes: &[u8]| {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            e.write_all(bytes).unwrap();
            e.finish().unwrap()
        };
        let tar = |name: &str, bytes: &[u8]| {
            let mut h = vec![0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(b"0000644");
            let size = format!("{:011o}", bytes.len());
            h[124..135].copy_from_slice(size.as_bytes());
            h[156] = b'0';
            h[257..262].copy_from_slice(b"ustar");
            h[148..156].copy_from_slice(b"        ");
            let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            let mut out = h;
            out.extend_from_slice(bytes);
            out.resize(out.len().div_ceil(512) * 512, 0);
            out.extend(vec![0u8; 1024]);
            out
        };
        let leak = b"licence *CxYz0123abcdEF here".to_vec();
        std::fs::write(dir.join("a.nii.gz"), gz(&leak)).unwrap();
        std::fs::write(dir.join("b.mgz"), gz(&leak)).unwrap();
        std::fs::write(dir.join("c.tar.gz"), gz(&tar("s/mri/x.mgz", &gz(&leak)))).unwrap();
        for (i, pre) in ["", "x", "xy", "xyz"].iter().enumerate() {
            let b64 = encode_base64(format!("{pre}FSaBcDeFgHiJk yy").as_bytes());
            std::fs::write(dir.join(format!("d{i}.txt")), &b64).unwrap();
        }
        std::fs::write(dir.join("e.zip"), b"PK\x03\x04 not read inside").unwrap();
        std::fs::write(dir.join("f.nii.gz"), gz(&vec![0u8; 4096])).unwrap();
        let mut ticks = 0;
        let done = sweep(&dir, &held(), &[], &dir.with_extension("nils"), &mut || {
            ticks += 1
        })
        .unwrap();
        assert!(ticks > 0);
        for f in [
            "a.nii.gz", "b.mgz", "c.tar.gz", "d0.txt", "d1.txt", "d2.txt", "d3.txt", "e.zip",
        ] {
            assert!(!dir.join(f).exists(), "{f}: {done:?}");
        }
        assert!(dir.join("f.nii.gz").exists(), "{done:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder the container made unreadable does not stop the sweep: what
    /// can be read is swept, and the rest is named.
    #[cfg(unix)]
    #[test]
    fn a_folder_made_unreadable_does_not_stop_the_sweep() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nils-secret-shut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("z")).unwrap();
        std::fs::write(dir.join("a/leak.txt"), b"*CxYz0123abcdEF").unwrap();
        std::fs::write(dir.join("z/leak.txt"), b"*CxYz0123abcdEF").unwrap();
        std::fs::set_permissions(dir.join("a"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = sweep(&dir, &held(), &[], &dir.with_extension("nils"), &mut || {});
        let _ = std::fs::set_permissions(dir.join("a"), std::fs::Permissions::from_mode(0o700));
        assert!(r.is_ok(), "{r:?}");
        // the engine gave the folder back to itself and swept it too
        assert!(!dir.join("a/leak.txt").exists());
        assert!(!dir.join("z/leak.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_secret_that_is_not_a_small_file_is_refused_by_its_id() {
        let dir = std::env::temp_dir().join(format!("nils-secret-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let e = read("lic", &dir.join("missing")).unwrap_err();
        assert!(e.contains("nils pipeline secret set lic"), "{e}");
        assert!(read("lic", &dir).unwrap_err().contains("not a file"));
        std::fs::write(dir.join("empty"), "  \n").unwrap();
        assert!(
            read("lic", &dir.join("empty"))
                .unwrap_err()
                .contains("empty")
        );
        std::fs::write(dir.join("ok"), LICENCE).unwrap();
        assert!(read("lic", &dir.join("ok")).unwrap().needles.len() > 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
