// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55 H2, round 5 (A23): the logs a person reads at the doors. A
//! job's output, what its verb printed on both streams, is kept beside the
//! registry in `logs/jobs/<id>.log`, written by the worker that ran it and
//! cut past [`CAP`]; a pipeline run's units keep their `log.txt` in the
//! run's scratch place, as they always did. `GET /api/jobs/{id}/log` and
//! `GET /api/pipeline-runs/{id}/units/{unit}/log` read either as a slice of
//! bytes: the last [`TAIL`] by default, or from a byte a reader already has,
//! so a page that follows a running job asks only for what is new.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Where the jobs' logs live under the registry home.
pub(crate) const JOBS_DIR: &str = "logs/jobs";
/// The most a job's log keeps; past it, one line says it was cut.
pub(crate) const CAP: u64 = 32 * 1024 * 1024;
/// What a door answers when asked for no byte in particular: the end.
pub(crate) const TAIL: u64 = 64 * 1024;
/// The most one answer carries.
pub(crate) const MAX: u64 = 1024 * 1024;

/// A job's log file under the registry home.
pub(crate) fn job_path(home: &Path, id: i64) -> PathBuf {
    home.join(JOBS_DIR).join(format!("{id}.log"))
}

/// A job's log as its worker writes it: the lines of both streams as they
/// come, up to [`CAP`], then one line saying the rest was not kept. A log
/// that cannot be made leaves the job running as before, with no log.
pub(crate) struct Writer {
    file: Option<File>,
    written: u64,
    cut: bool,
}

impl Writer {
    pub(crate) fn open(home: &Path, id: i64) -> Writer {
        let path = job_path(home, id);
        let file = path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .ok()
            .and_then(|_| File::create(&path).ok());
        Writer {
            file,
            written: 0,
            cut: false,
        }
    }

    /// One line the verb printed, its end of line its own.
    pub(crate) fn line(&mut self, line: &[u8]) {
        let Some(f) = self.file.as_mut() else {
            return;
        };
        if self.cut {
            return;
        }
        let len = line.len() as u64 + u64::from(!line.ends_with(b"\n"));
        if self.written + len > CAP {
            self.cut = true;
            let _ = writeln!(
                f,
                "nils: this log was cut at {} MiB; the job ran on, and what it printed past here was not kept",
                CAP / (1024 * 1024)
            );
            return;
        }
        let _ = f.write_all(line);
        if !line.ends_with(b"\n") {
            let _ = f.write_all(b"\n");
        }
        self.written += len;
    }
}

/// Where a reader asked to start and how much it takes, from the query.
pub(crate) fn asked(
    query: &std::collections::HashMap<String, String>,
) -> Result<(Option<u64>, u64), String> {
    let num = |k: &str| -> Result<Option<u64>, String> {
        query
            .get(k)
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| format!("{k} is a count of bytes, not {v}"))
            })
            .transpose()
    };
    let from = num("from")?;
    let limit = num("limit")?.unwrap_or(TAIL).clamp(1, MAX);
    Ok((from, limit))
}

/// A slice of a log: `from` to `to` of its `size` bytes. With no `from`,
/// the last `limit` bytes from the start of a line; a slice that stops short
/// of the end stops after a whole line where it holds one, so the next
/// slice starts at a line.
pub(crate) fn read(path: &Path, from: Option<u64>, limit: u64) -> std::io::Result<Value> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    let tail = from.is_none();
    let start = from.unwrap_or(size.saturating_sub(limit)).min(size);
    let end = (start + limit).min(size);
    let mut buf = vec![0u8; (end - start) as usize];
    f.seek(SeekFrom::Start(start))?;
    f.read_exact(&mut buf)?;
    let mut from = start;
    if tail
        && start > 0
        && let Some(at) = buf.iter().position(|b| *b == b'\n')
        && at + 1 < buf.len()
    {
        buf.drain(..=at);
        from = start + at as u64 + 1;
    }
    let mut to = from + buf.len() as u64;
    if to < size
        && let Some(at) = buf.iter().rposition(|b| *b == b'\n')
    {
        buf.truncate(at + 1);
        to = from + buf.len() as u64;
    }
    Ok(json!({
        "size": size,
        "from": from,
        "to": to,
        "text": String::from_utf8_lossy(&buf),
        "earlier": from > 0,
    }))
}

/// `nils jobs prune`: the logs of jobs the registry no longer holds go with
/// them. Answers how many went.
pub(crate) fn prune(home: &Path, kept: &mut dyn FnMut(i64) -> bool) -> std::io::Result<usize> {
    let dir = home.join(JOBS_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut gone = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".log"))
            .and_then(|n| n.parse::<i64>().ok())
        else {
            continue;
        };
        if !kept(id) {
            std::fs::remove_file(e.path())?;
            gone += 1;
        }
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nils-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_tail_starts_at_a_line_and_a_page_ends_after_one() {
        let dir = temp("tail");
        let path = dir.join("1.log");
        let text: String = (0..100).map(|i| format!("line {i:03}\n")).collect();
        std::fs::write(&path, &text).unwrap();
        // 9 bytes a line: the last 40 bytes start mid-line, so the slice
        // starts at the next line
        let doc = read(&path, None, 40).unwrap();
        assert_eq!(doc["size"], 900);
        assert_eq!(doc["to"], 900);
        assert_eq!(doc["from"], 864);
        assert_eq!(doc["text"], "line 096\nline 097\nline 098\nline 099\n");
        assert_eq!(doc["earlier"], true);
        // from a byte: a page that stops short ends after a whole line
        let doc = read(&path, Some(0), 20).unwrap();
        assert_eq!(doc["text"], "line 000\nline 001\n");
        assert_eq!(doc["to"], 18);
        assert_eq!(doc["earlier"], false);
        let doc = read(&path, Some(18), 9).unwrap();
        assert_eq!(doc["text"], "line 002\n");
        // past the end: nothing, and where the end is
        let doc = read(&path, Some(5000), 10).unwrap();
        assert_eq!(doc["from"], 900);
        assert_eq!(doc["text"], "");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_log_is_cut_past_its_cap_with_a_line_that_says_so() {
        let dir = temp("cap");
        let mut w = Writer::open(&dir, 7);
        w.written = CAP - 10;
        w.line(b"short\n");
        w.line(b"this line goes over the cap");
        w.line(b"and this one is never kept\n");
        let text = std::fs::read_to_string(job_path(&dir, 7)).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.starts_with("short\n"));
        assert!(text.contains("this log was cut at 32 MiB"), "{text}");
        let mut kept = |id: i64| id != 7;
        assert_eq!(prune(&dir, &mut kept).unwrap(), 1);
        assert!(!job_path(&dir, 7).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn asked_reads_from_and_limit_and_refuses_words() {
        let mut q = std::collections::HashMap::new();
        assert_eq!(asked(&q).unwrap(), (None, TAIL));
        q.insert("from".to_string(), "12".to_string());
        q.insert("limit".to_string(), "99999999".to_string());
        assert_eq!(asked(&q).unwrap(), (Some(12), MAX));
        q.insert("from".to_string(), "end".to_string());
        assert!(asked(&q).unwrap_err().contains("from is a count of bytes"));
    }
}
