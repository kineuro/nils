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
    /// of eight characters or more.
    pub needles: Vec<Vec<u8>>,
}

/// The largest secret file the engine takes: a licence is a few lines.
pub const MAX_BYTES: u64 = 64 * 1024;

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
    let needles = needles(&bytes);
    if needles.is_empty() {
        return Err(format!("the secret {id} is an empty file"));
    }
    Ok(Held {
        id: id.to_string(),
        needles,
    })
}

/// What a secret is looked for by: the whole of it, trimmed, and each line
/// of eight bytes or more, trimmed, longest first.
pub fn needles(bytes: &[u8]) -> Vec<Vec<u8>> {
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
    out.sort_by_key(|n| std::cmp::Reverse(n.len()));
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

/// Whether a file holds any of the secrets, read in pieces that overlap by
/// the longest needle, so a large output is never read whole.
pub fn file_holds(path: &Path, held: &[Held]) -> std::io::Result<bool> {
    let longest = held
        .iter()
        .flat_map(|h| h.needles.iter().map(Vec::len))
        .max()
        .unwrap_or(0);
    if longest == 0 {
        return Ok(false);
    }
    let mut f = std::fs::File::open(path)?;
    let mut carry: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(false);
        }
        carry.extend_from_slice(&buf[..n]);
        if holds(&carry, held) {
            return Ok(true);
        }
        let keep = longest.saturating_sub(1).min(carry.len());
        carry.drain(..carry.len() - keep);
    }
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
}

/// Sweep what a container left: every regular file under `dir` (links are
/// never followed) that holds a secret is removed, except the files named
/// in `rewrite` (a log, `results.json`), which are written again with the
/// secret replaced. Answers what it did, paths relative to `dir`.
pub fn sweep(dir: &Path, held: &[Held], rewrite: &[&Path]) -> std::io::Result<Vec<Swept>> {
    let mut done = Vec::new();
    if held.is_empty() || !dir.is_dir() {
        return Ok(done);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let path = e.path();
            let kind = e.file_type()?;
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if !kind.is_file() || !file_holds(&path, held)? {
                continue;
            }
            let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
            if rewrite.contains(&path.as_path()) {
                redact_file(&path, held)?;
                done.push(Swept::Redacted(rel));
            } else {
                std::fs::remove_file(&path)?;
                done.push(Swept::Removed(rel));
            }
        }
    }
    done.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    Ok(done)
}

/// Write a file again with every secret in it replaced; whether one was.
pub fn redact_file(path: &Path, held: &[Held]) -> std::io::Result<bool> {
    if held.is_empty() || !path.is_file() {
        return Ok(false);
    }
    let bytes = std::fs::read(path)?;
    let (clean, any) = redact(&bytes, held);
    if any {
        let tmp = path.with_extension("redacting");
        std::fs::write(&tmp, &clean)?;
        std::fs::rename(&tmp, path)?;
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
        vec![Held {
            id: "freesurfer_license".into(),
            needles: needles(LICENCE.as_bytes()),
        }]
    }

    #[test]
    fn a_secret_is_looked_for_whole_and_line_by_line() {
        let n = needles(LICENCE.as_bytes());
        // the whole, and the three lines of eight or more; "12345" is too
        // short to look for alone
        assert_eq!(n.len(), 4, "{n:?}");
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
        let done = sweep(&dir, &h, &[&dir.join("log.txt"), &dir.join("results.json")]).unwrap();
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
        assert_eq!(read("lic", &dir.join("ok")).unwrap().needles.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
