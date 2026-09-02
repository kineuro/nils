// SPDX-License-Identifier: AGPL-3.0-only

//! `nils digest --dry-run`: the walker feeds a pool of parsers, every file is
//! read and extracted, nothing is written, the report is printed. The bounds
//! are those of §9.1: at most 16,384 paths between the walker and the parsers.

use std::fmt;
use std::io::{self, IsTerminal, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select};

use crate::knobs::Settings;
use crate::report::{Counts, Report, Setup, thousands};
use crate::rss::peak_rss;
use crate::walk::{WalkEvent, walk};

/// How many paths may wait between the walker and the parsers.
pub const WALK_BOUND: usize = 16_384;

/// How often progress is printed.
pub const PROGRESS_EVERY: Duration = Duration::from_secs(10);

/// Why a run could not start or finish.
#[derive(Debug)]
pub enum DigestError {
    /// The root could not be listed.
    Root { path: String, error: io::Error },
}

impl fmt::Display for DigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DigestError::Root { path, error } => write!(f, "cannot list {path}: {error}"),
        }
    }
}

impl std::error::Error for DigestError {}

/// The counters the progress line reads while the parsers run.
struct Progress {
    seen: AtomicU64,
    parsed: AtomicU64,
    quarantined: AtomicU64,
    start: Instant,
    json: bool,
    tty: bool,
    printed: std::sync::atomic::AtomicBool,
}

impl Progress {
    fn new(start: Instant, json: bool) -> Progress {
        Progress {
            seen: AtomicU64::new(0),
            parsed: AtomicU64::new(0),
            quarantined: AtomicU64::new(0),
            start,
            json,
            tty: io::stderr().is_terminal(),
            printed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn file(&self, accepted: bool) {
        self.seen.fetch_add(1, Ordering::Relaxed);
        if accepted {
            self.parsed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.quarantined.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// One line on stderr: updating in place on a terminal, one JSON object
    /// with `--json`, a plain line otherwise.
    fn print(&self) {
        let seen = self.seen.load(Ordering::Relaxed);
        let parsed = self.parsed.load(Ordering::Relaxed);
        let quarantined = self.quarantined.load(Ordering::Relaxed);
        let elapsed = self.start.elapsed().as_secs_f64();
        let rate = if elapsed > 0.0 {
            seen as f64 / elapsed
        } else {
            0.0
        };
        let mut err = io::stderr().lock();
        let _ = if self.json {
            writeln!(
                err,
                "{{\"progress\":{{\"seen\":{seen},\"parsed\":{parsed},\"quarantined\":{quarantined},\"files_per_s\":{rate:.0},\"elapsed_s\":{elapsed:.1}}}}}"
            )
        } else if self.tty {
            write!(
                err,
                "\r\x1b[2K{} files, {} parsed, {} quarantined, {} files/s, {:.0} s",
                thousands(seen),
                thousands(parsed),
                thousands(quarantined),
                thousands(rate.round() as u64),
                elapsed
            )
        } else {
            writeln!(
                err,
                "{} files, {} parsed, {} quarantined, {} files/s, {:.0} s",
                thousands(seen),
                thousands(parsed),
                thousands(quarantined),
                thousands(rate.round() as u64),
                elapsed
            )
        };
        let _ = err.flush();
        self.printed.store(true, Ordering::Relaxed);
    }

    /// End the updating line, if one was printed.
    fn finish(&self) {
        if self.tty && !self.json && self.printed.load(Ordering::Relaxed) {
            let _ = writeln!(io::stderr());
        }
    }
}

/// Walk and parse `settings.root`, write nothing, return the report.
pub fn dry_run(settings: &Settings) -> Result<Report, DigestError> {
    let start = Instant::now();
    let root = settings.root.clone();
    // The root's failure is the job's, before a thread is started.
    if let Err(error) = std::fs::read_dir(&root) {
        return Err(DigestError::Root {
            path: root.display().to_string(),
            error,
        });
    }
    let progress = Progress::new(start, settings.json);
    let (tx, rx) = crossbeam_channel::bounded::<WalkEvent>(WALK_BOUND);
    let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(0);

    let (walked, counts) = std::thread::scope(|s| {
        let walker = {
            let filter = settings.filter.clone();
            let root = root.clone();
            let threads = settings.walk_threads;
            s.spawn(move || {
                let result = walk(&root, threads, &filter, &tx);
                drop(tx);
                result
            })
        };
        let workers: Vec<_> = (0..settings.workers.max(1))
            .map(|_| {
                let rx = rx.clone();
                let done_tx: Sender<()> = done_tx.clone();
                let progress = &progress;
                s.spawn(move || {
                    let counts = parse_all(&rx, progress);
                    drop(done_tx);
                    counts
                })
            })
            .collect();
        drop(done_tx);
        drop(rx);

        let ticker = crossbeam_channel::tick(PROGRESS_EVERY);
        loop {
            select! {
                recv(ticker) -> _ => progress.print(),
                recv(done_rx) -> _ => break,
            }
        }
        progress.finish();

        let walked = walker.join().expect("walker thread");
        let mut counts = Counts::default();
        for w in workers {
            counts.merge(w.join().expect("parser thread"));
        }
        (walked, counts)
    });
    if let Err(error) = walked {
        return Err(DigestError::Root {
            path: root.display().to_string(),
            error,
        });
    }

    let setup = Setup {
        name: settings.name.clone(),
        root: root.display().to_string(),
        dry_run: true,
        files: settings.filter.to_string(),
        workers: settings.workers.max(1),
        walk_threads: settings.walk_threads.max(1),
    };
    Ok(Report::new(
        setup,
        &counts,
        start.elapsed().as_secs_f64(),
        peak_rss(),
    ))
}

/// One parser thread: every event until the walker is done.
fn parse_all(rx: &Receiver<WalkEvent>, progress: &Progress) -> Counts {
    let mut counts = Counts::default();
    for event in rx {
        match event {
            WalkEvent::File(path) => {
                let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                match nils_dicom::extract(&path) {
                    Ok(x) => {
                        counts.accepted(&x, bytes);
                        progress.file(true);
                    }
                    Err(r) => {
                        counts.refused(&r, bytes);
                        progress.file(false);
                    }
                }
            }
            WalkEvent::Skipped { reason, .. } => counts.skipped(reason),
            WalkEvent::WalkError { error, .. } => counts.walk_error(&error),
        }
    }
    counts
}
