// SPDX-License-Identifier: AGPL-3.0-only

//! The progress line: counters every stage bumps, printed on stderr every ten
//! seconds while the run goes on.

use std::io::{self, IsTerminal, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::report::thousands;

/// How often progress is printed and, in a run that writes, the job's
/// heartbeat is written (§10).
pub const PROGRESS_EVERY: Duration = Duration::from_secs(10);

/// The counters the progress line reads while the stages run.
pub struct Progress {
    pub seen: AtomicU64,
    pub parsed: AtomicU64,
    pub quarantined: AtomicU64,
    pub unchanged: AtomicU64,
    pub ingested: AtomicU64,
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
            parsed: AtomicU64::new(0),
            quarantined: AtomicU64::new(0),
            unchanged: AtomicU64::new(0),
            ingested: AtomicU64::new(0),
            start,
            json,
            dry_run,
            tty: io::stderr().is_terminal(),
            printed: AtomicBool::new(false),
        }
    }

    /// A file the reader took or refused.
    pub fn file(&self, accepted: bool) {
        self.seen.fetch_add(1, Ordering::Relaxed);
        if accepted {
            self.parsed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.quarantined.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A file found as recorded.
    pub fn unchanged(&self) {
        self.seen.fetch_add(1, Ordering::Relaxed);
        self.unchanged.fetch_add(1, Ordering::Relaxed);
    }

    /// Rows the writer filed as ingested.
    pub fn ingested(&self, n: u64) {
        self.ingested.fetch_add(n, Ordering::Relaxed);
    }

    /// The counters as `job.progress` records them.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "seen": self.seen.load(Ordering::Relaxed),
            "parsed": self.parsed.load(Ordering::Relaxed),
            "quarantined": self.quarantined.load(Ordering::Relaxed),
            "unchanged": self.unchanged.load(Ordering::Relaxed),
            "ingested": self.ingested.load(Ordering::Relaxed),
            "elapsed_s": (self.start.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        })
    }

    /// One line on stderr: updating in place on a terminal, one JSON object
    /// with `--json`, a plain line otherwise.
    pub fn print(&self) {
        let seen = self.seen.load(Ordering::Relaxed);
        let parsed = self.parsed.load(Ordering::Relaxed);
        let quarantined = self.quarantined.load(Ordering::Relaxed);
        let unchanged = self.unchanged.load(Ordering::Relaxed);
        let ingested = self.ingested.load(Ordering::Relaxed);
        let elapsed = self.start.elapsed().as_secs_f64();
        let rate = if elapsed > 0.0 {
            seen as f64 / elapsed
        } else {
            0.0
        };
        let mut err = io::stderr().lock();
        let _ = if self.json {
            writeln!(err, "{{\"progress\":{}}}", self.json())
        } else {
            let mut line = format!(
                "{} files, {} parsed, {} quarantined",
                thousands(seen),
                thousands(parsed),
                thousands(quarantined)
            );
            if !self.dry_run {
                line.push_str(&format!(
                    ", {} unchanged, {} ingested",
                    thousands(unchanged),
                    thousands(ingested)
                ));
            }
            line.push_str(&format!(
                ", {} files/s, {elapsed:.0} s",
                thousands(rate.round() as u64)
            ));
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
