// SPDX-License-Identifier: AGPL-3.0-only

//! The progress line: counters every stage bumps, printed on stderr every
//! ten seconds while the run goes on, and written to the job at every
//! heartbeat.

use std::io::{self, IsTerminal, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use nils_digest::report::{human_bytes, thousands};

pub use nils_digest::progress::PROGRESS_EVERY;

/// The counters the progress line reads while the stages run.
pub struct Progress {
    pub seen: AtomicU64,
    pub written: AtomicU64,
    pub unchanged: AtomicU64,
    pub held: AtomicU64,
    pub refused: AtomicU64,
    /// The size of every file seen.
    pub bytes: AtomicU64,
    start: Instant,
    json: bool,
    dry_run: bool,
    tty: bool,
    printed: AtomicBool,
}

impl Progress {
    pub fn new(start: Instant, json: bool, dry_run: bool) -> Progress {
        Progress {
            seen: AtomicU64::new(0),
            written: AtomicU64::new(0),
            unchanged: AtomicU64::new(0),
            held: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            start,
            json,
            dry_run,
            tty: io::stderr().is_terminal(),
            printed: AtomicBool::new(false),
        }
    }

    /// One file seen, under one of the four outcomes.
    pub fn file(&self, counter: &AtomicU64, bytes: u64) {
        self.seen.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn rate(&self) -> f64 {
        let elapsed = self.start.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.seen.load(Ordering::Relaxed) as f64 / elapsed
        } else {
            0.0
        }
    }

    /// The counters as `job.progress` records them.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "seen": self.seen.load(Ordering::Relaxed),
            "written": self.written.load(Ordering::Relaxed),
            "unchanged": self.unchanged.load(Ordering::Relaxed),
            "held": self.held.load(Ordering::Relaxed),
            "refused": self.refused.load(Ordering::Relaxed),
            "bytes": self.bytes.load(Ordering::Relaxed),
            "files_per_s": self.rate().round(),
            "elapsed_s": (self.start.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        })
    }

    /// One line on stderr: updating in place on a terminal, one JSON object
    /// with `--json`, a plain line otherwise.
    pub fn print(&self) {
        let mut err = io::stderr().lock();
        let _ = if self.json {
            writeln!(err, "{{\"progress\":{}}}", self.json())
        } else {
            let line = format!(
                "{} files, {} {}, {} unchanged, {} held, {} refused, {}, {} files/s, {:.0} s",
                thousands(self.seen.load(Ordering::Relaxed)),
                thousands(self.written.load(Ordering::Relaxed)),
                if self.dry_run {
                    "would write"
                } else {
                    "written"
                },
                thousands(self.unchanged.load(Ordering::Relaxed)),
                thousands(self.held.load(Ordering::Relaxed)),
                thousands(self.refused.load(Ordering::Relaxed)),
                human_bytes(self.bytes.load(Ordering::Relaxed)),
                thousands(self.rate().round() as u64),
                self.start.elapsed().as_secs_f64(),
            );
            if self.tty {
                write!(err, "\r\x1b[2K{line}")
            } else {
                writeln!(err, "{line}")
            }
        };
        let _ = err.flush();
        self.printed.store(true, Ordering::Relaxed);
    }

    /// End the updating line, if one was printed.
    pub fn finish(&self) {
        if self.tty && !self.json && self.printed.load(Ordering::Relaxed) {
            let _ = writeln!(io::stderr());
        }
    }
}
