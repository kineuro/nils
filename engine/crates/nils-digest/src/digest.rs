// SPDX-License-Identifier: AGPL-3.0-only

//! `nils digest`: the walker feeds the resume check, the resume check feeds a
//! pool of parsers, the parsers feed the writer (§9.1). With `--dry-run`
//! there is no registry: every file is read, nothing is written, the report
//! is printed. The bounds are those of §9.1: at most 16,384 paths between the
//! walker and the resume check, 1,024 tasks before the parsers, two batches
//! per parser before the writer.
//!
//! A stop (§10) travels through one [`Cancel`] token: asked once, the walker
//! lets its queue go, the resume check and the parsers finish the task in
//! hand, and the writer commits every batch the parsers still send, so that
//! everything read is written and the report is the registry's. Asked twice,
//! the writer abandons the batch in flight at its next checkpoint and lets
//! the rest go; those files are read again next time. Either way the batch
//! and the job end as `cancelled`, and nothing is marked gone.

use std::fmt;
use std::io;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, select};
use nils_registry::dialect::Conflict;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::{now_iso, now_secs, secs_of};
use nils_registry::{HomeError, Registry};

use crate::batch::{Batch, Batcher, Item, ParsedFile, RowHashes, Task};
use crate::cancel::{Cancel, Cancelled, Scripted, process_alive};
use crate::knobs::Settings;
use crate::progress::{PROGRESS_EVERY, Progress};
use crate::report::{Counts, Report, Setup, Written};
use crate::resume::{self, Records};
use crate::rss::peak_rss;
use crate::rule::Rule;
use crate::stack::Signature;
use crate::walk::{Filter, WalkEvent, walk};
use crate::writer::{self, QUARANTINE_KIND, Writer};

/// How many paths may wait between the walker and the resume check.
pub const WALK_BOUND: usize = 16_384;

/// How many tasks may wait before the parsers.
pub const TASK_BOUND: usize = 1_024;

/// How many closed batches may wait for the writer: two per worker, but no
/// more than this. Beyond a handful the queue only holds memory (a batch of
/// 2,000 parsed rows is tens of megabytes), and the parsers block on a full
/// queue at no cost to throughput, since a full queue means the writer is
/// the wall either way.
pub const BATCH_BOUND: usize = 16;

/// A running job whose heartbeat is younger than this holds the registry;
/// an older one is taken over as failed (§10). A job of this host whose
/// process is gone is taken over at once.
pub const FRESH_SECS: u64 = 60;

/// Why a run could not start or finish.
#[derive(Debug)]
pub enum DigestError {
    /// The root could not be listed.
    Root {
        path: String,
        error: io::Error,
    },
    /// The registry refused something.
    Registry(HomeError),
    /// Another job holds the registry.
    Busy {
        job_id: i64,
        since: String,
    },
    Message(String),
}

impl fmt::Display for DigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DigestError::Root { path, error } => write!(f, "cannot list {path}: {error}"),
            DigestError::Registry(e) => write!(f, "{e}"),
            DigestError::Busy { job_id, since } => write!(
                f,
                "job {job_id} is running (last heartbeat {since}); wait for it, or for its takeover {FRESH_SECS} s after its last heartbeat"
            ),
            DigestError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for DigestError {}

impl From<HomeError> for DigestError {
    fn from(e: HomeError) -> DigestError {
        DigestError::Registry(e)
    }
}

impl From<nils_registry::Error> for DigestError {
    fn from(e: nils_registry::Error) -> DigestError {
        DigestError::Registry(HomeError::Store(e))
    }
}

/// Walk and parse `settings.root`, write nothing, return the report.
pub fn dry_run(settings: &Settings) -> Result<Report, DigestError> {
    run(settings, None, &Cancel::new())
}

/// Walk, parse and write `settings.root` into the registry as one batch of
/// one job; return the report, which the batch also records.
pub fn digest(settings: &Settings, registry: &mut Registry) -> Result<Report, DigestError> {
    run(settings, Some(registry), &Cancel::new())
}

/// [`dry_run`] without a registry, [`digest`] with one, and either of them
/// asked to stop through `cancel` (§10): the report then says how it ended.
pub fn digest_with(
    settings: &Settings,
    registry: Option<&mut Registry>,
    cancel: &Cancel,
) -> Result<Report, DigestError> {
    run(settings, registry, cancel)
}

/// The rows a run holds while it goes on.
struct Run {
    job_id: i64,
    source_id: i64,
    batch_id: i64,
}

fn run(
    settings: &Settings,
    mut registry: Option<&mut Registry>,
    cancel: &Cancel,
) -> Result<Report, DigestError> {
    let start = Instant::now();
    // The root's failure is the job's, before anything is recorded.
    if let Err(error) = std::fs::read_dir(&settings.root) {
        return Err(DigestError::Root {
            path: settings.root.display().to_string(),
            error,
        });
    }
    let run = match registry.as_deref_mut() {
        Some(reg) => Some(start_job(reg, settings)?),
        None => None,
    };
    let result = execute(
        settings,
        registry.as_deref_mut(),
        run.as_ref(),
        start,
        cancel,
    );
    if let (Err(e), Some(reg), Some(r)) = (&result, registry, run.as_ref()) {
        mark_failed(reg, r, &e.to_string());
    }
    result
}

/// The host as the job records it.
pub fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// A column read back as text on either backend.
fn text_of(store: &Store, table_name: &str, column: &str) -> String {
    let t = table(table_name);
    let c = t
        .column(column)
        .unwrap_or_else(|| panic!("{table_name}.{column} is not a column"));
    store.dialect().text_of(c)
}

/// Refuse a fresh running job, take over a stale one or one of this host
/// whose process is gone, then record this job, its source and its batch in
/// one transaction (§10).
fn start_job(registry: &mut Registry, settings: &Settings) -> Result<Run, DigestError> {
    registry.refresh_meta()?;
    let now = now_iso();
    let host = hostname();
    let pid = i64::from(std::process::id());
    let root = settings.root.display().to_string();
    let root_canonical = std::fs::canonicalize(&settings.root)
        .map_err(|error| DigestError::Root {
            path: root.clone(),
            error,
        })?
        .display()
        .to_string();
    let backend = registry.store().backend().name();
    let bulk = registry.store().bulk_path().map(|b| b.name());
    let store = registry.store();
    let job_t = table("job");
    let sql = format!(
        "SELECT id, {}, {}, pid, host FROM {} WHERE state = 'running'",
        text_of(store, "job", "heartbeat_at"),
        text_of(store, "job", "started_at"),
        store.qualified("job")
    );
    for j in store.query(&sql, &[])? {
        let id = j.int(0)?;
        let last = j
            .opt_text(1)?
            .or(j.opt_text(2)?)
            .unwrap_or_default()
            .to_string();
        let its_pid = j.opt_int(3)?;
        let its_host = j.opt_text(4)?.unwrap_or_default();
        // A job of this host whose process is gone left no one to beat its
        // heart: it is over, however fresh the last beat.
        let gone = its_host == host && its_pid.is_some_and(|p| process_alive(p) == Some(false));
        let fresh = secs_of(&last).is_some_and(|s| now_secs().saturating_sub(s) < FRESH_SECS);
        if fresh && !gone {
            return Err(DigestError::Busy {
                job_id: id,
                since: last,
            });
        }
        let error = match its_pid {
            Some(p) if gone => format!("process {p} is gone; no heartbeat since {last}"),
            _ => format!("stale: no heartbeat since {last}"),
        };
        store.update_by_id(
            job_t,
            &[
                ("state", Param::from("failed")),
                ("finished_at", Param::from(now.as_str())),
                ("error", Param::from(error)),
            ],
            "id",
            id,
        )?;
        fail_batches(store, &now, "job_id", id)?;
    }
    store.begin()?;
    let result = (|| -> Result<Run, DigestError> {
        let job = store.insert(
            &Insert::new(
                job_t,
                &[
                    "kind",
                    "name",
                    "args",
                    "state",
                    "pid",
                    "host",
                    "started_at",
                    "heartbeat_at",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from("digest"),
                Param::from(settings.name.as_str()),
                Param::from(settings.config().to_string()),
                Param::from("running"),
                Param::Int(pid),
                Param::from(host.as_str()),
                Param::from(now.as_str()),
                Param::from(now.as_str()),
            ]],
        )?;
        let job_id = job.first().ok_or_else(no_id)?.int(0)?;
        let source_t = table("source");
        let inserted = store.insert(
            &Insert::new(source_t, &["root", "root_canonical", "first_seen_at"])
                .on_conflict(Conflict::Nothing(&["root_canonical"]))
                .returning(&["id"]),
            &[vec![
                Param::from(root.as_str()),
                Param::from(root_canonical.as_str()),
                Param::from(now.as_str()),
            ]],
        )?;
        let source_id = match inserted.first() {
            Some(r) => r.int(0)?,
            None => {
                let sql = format!(
                    "SELECT id FROM {} WHERE root_canonical = {}",
                    store.qualified("source"),
                    store.dialect().param(1, Type::Text)
                );
                store
                    .query_opt(&sql, &[Param::from(root_canonical.as_str())])?
                    .ok_or_else(no_id)?
                    .int(0)?
            }
        };
        let mut config = settings.config();
        config["backend"] = backend.into();
        config["bulk_path"] = bulk.into();
        let batch = store.insert(
            &Insert::new(
                table("ingest_batch"),
                &[
                    "source_id",
                    "job_id",
                    "name",
                    "config",
                    "started_at",
                    "state",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::Int(source_id),
                Param::Int(job_id),
                Param::from(settings.name.as_str()),
                Param::from(config.to_string()),
                Param::from(now.as_str()),
                Param::from("running"),
            ]],
        )?;
        let batch_id = batch.first().ok_or_else(no_id)?.int(0)?;
        Ok(Run {
            job_id,
            source_id,
            batch_id,
        })
    })();
    match result {
        Ok(run) => {
            store.commit()?;
            Ok(run)
        }
        Err(e) => {
            let _ = store.rollback();
            Err(e)
        }
    }
}

fn no_id() -> DigestError {
    DigestError::Message("the store returned no id for a new row".into())
}

/// The running batches with `by = id` end as failed, and `reparse_from` is
/// set to the last second each recorded: the run that follows reads those
/// files again, since a crash between the registry's commit and the linkage
/// store's loses the identity rows of that transaction (§9.3).
fn fail_batches(
    store: &mut Store,
    now: &str,
    by: &str,
    id: i64,
) -> Result<u64, nils_registry::Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {batch} SET state = 'failed', finished_at = {}, \
         reparse_from = (SELECT MAX(f.seen_at) FROM {files} AS f WHERE f.batch_id = ingest_batch.id) \
         WHERE {by} = {} AND state = 'running'",
        d.param(1, Type::Timestamp),
        d.param(2, Type::Int),
        batch = store.qualified("ingest_batch"),
        files = store.qualified("source_file"),
    );
    store.execute(&sql, &[Param::from(now), Param::Int(id)])
}

/// Best effort: the job and the batch end as failed, with the error text.
fn mark_failed(registry: &mut Registry, run: &Run, error: &str) {
    let now = now_iso();
    let store = registry.store();
    let _ = store.rollback();
    let _ = store.update_by_id(
        table("job"),
        &[
            ("state", Param::from("failed")),
            ("finished_at", Param::from(now.as_str())),
            ("error", Param::from(error)),
        ],
        "id",
        run.job_id,
    );
    let _ = fail_batches(store, &now, "id", run.batch_id);
}

/// The pipeline, then the batch's record.
fn execute(
    settings: &Settings,
    mut registry: Option<&mut Registry>,
    run: Option<&Run>,
    start: Instant,
    cancel: &Cancel,
) -> Result<Report, DigestError> {
    let root = settings.root.clone();
    let dry = registry.is_none();
    let workers = settings.workers.max(1);
    let progress = Progress::new(start, settings.json, dry);
    let script = Scripted::from_env();

    let records = match (registry.as_deref(), run) {
        (Some(reg), Some(r)) => Some(Records::new(reg.open_reader()?, r.source_id)?),
        _ => None,
    };
    let mut writer = match (registry.as_deref_mut(), run) {
        (Some(reg), Some(r)) => Some(
            Writer::new(
                reg,
                &settings.identity,
                r.source_id,
                r.batch_id,
                Some(r.job_id),
            )?
            .cancelled_by(cancel.clone(), script),
        ),
        _ => None,
    };

    let (walk_tx, walk_rx) = crossbeam_channel::bounded::<WalkEvent>(WALK_BOUND);
    let (task_tx, task_rx) = crossbeam_channel::bounded::<Task>(TASK_BOUND);
    let (batch_tx, batch_rx) = crossbeam_channel::bounded::<Batch>((2 * workers).min(BATCH_BOUND));
    let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(0);

    let (walked, resumed, mut counts, wrote) = std::thread::scope(|s| {
        let walker = {
            let filter = settings.filter.clone();
            let root = root.clone();
            let threads = settings.walk_threads.max(1);
            s.spawn(move || {
                let result = walk(&root, threads, &filter, &walk_tx, cancel);
                drop(walk_tx);
                result
            })
        };
        let resumer = {
            let stage = resume::Stage {
                root: &root,
                records,
                retry_quarantine: settings.retry_quarantine,
                restart: settings.restart,
                rows: settings.batch_rows,
            };
            let tx = batch_tx.clone();
            let progress = &progress;
            s.spawn(move || {
                let result = resume::run(stage, &walk_rx, &task_tx, &tx, progress, cancel);
                drop(task_tx);
                drop(tx);
                result
            })
        };
        let parsers: Vec<_> = (0..workers)
            .map(|_| {
                let rx = task_rx.clone();
                let tx = batch_tx.clone();
                let rows = settings.batch_rows;
                let rule = &settings.identity;
                let progress = &progress;
                s.spawn(move || {
                    let counts = parse_all(&rx, &tx, rows, rule, progress, cancel);
                    drop(tx);
                    counts
                })
            })
            .collect();
        drop(task_rx);
        drop(batch_tx);
        let consumer = {
            let progress = &progress;
            let writer = writer.as_mut();
            s.spawn(move || {
                let result = match writer {
                    Some(w) => writer::run(w, &batch_rx, progress),
                    None => {
                        // a dry run has no commits; a scripted stop counts
                        // the batches it lets go instead
                        let mut drained = 0;
                        for _ in batch_rx.iter() {
                            drained += 1;
                            if let Some(s) = script {
                                s.after_commit(drained, cancel);
                            }
                        }
                        Ok(())
                    }
                };
                drop(done_tx);
                result
            })
        };

        let ticker = crossbeam_channel::tick(PROGRESS_EVERY);
        loop {
            select! {
                recv(ticker) -> _ => progress.print(),
                recv(done_rx) -> _ => break,
            }
        }
        progress.finish();

        let walked = walker.join().expect("walker thread");
        let resumed = resumer.join().expect("resume thread");
        let mut counts = Counts::default();
        for p in parsers {
            counts.merge(p.join().expect("parser thread"));
        }
        let wrote = consumer.join().expect("writer thread");
        (walked, resumed, counts, wrote)
    });

    let (written, own) = match writer.take() {
        Some(mut w) => (
            Some(std::mem::take(&mut w.written)),
            Some(std::mem::take(&mut w.counts)),
        ),
        None => (None, None),
    };
    // the writer's borrow of the registry ends here
    drop(writer);
    if let Err(error) = walked {
        return Err(DigestError::Root {
            path: root.display().to_string(),
            error,
        });
    }
    counts.merge(resumed?);
    if let Some(own) = own {
        counts.merge(own);
    }
    wrote?;
    let cancelled = if cancel.abort() {
        Some(Cancelled::Aborted)
    } else if cancel.stop() {
        Some(Cancelled::Stopped)
    } else {
        None
    };

    let setup = Setup {
        name: settings.name.clone(),
        root: root.display().to_string(),
        dry_run: dry,
        files: settings.filter.to_string(),
        workers,
        walk_threads: settings.walk_threads.max(1),
    };
    let elapsed = start.elapsed().as_secs_f64();
    match (registry, run, written) {
        (Some(reg), Some(r), Some(written)) => finish(
            reg, r, settings, setup, &counts, written, elapsed, cancelled,
        ),
        _ => {
            let mut report = Report::new(setup, &counts, elapsed, peak_rss());
            report.cancelled = cancelled;
            Ok(report)
        }
    }
}

/// The run's last transaction: the files no longer under the root marked
/// gone (§5.2), the batch's counts and epoch, the job done, or both of them
/// cancelled when the run was asked to stop.
#[allow(clippy::too_many_arguments)]
fn finish(
    registry: &mut Registry,
    run: &Run,
    settings: &Settings,
    setup: Setup,
    counts: &Counts,
    mut written: Written,
    elapsed: f64,
    cancelled: Option<Cancelled>,
) -> Result<Report, DigestError> {
    let now = now_iso();
    if written.writes == 0 {
        written.epoch = registry.meta().epoch;
    }
    let state = if cancelled.is_some() {
        "cancelled"
    } else {
        "done"
    };
    let store = registry.store();
    store.begin()?;
    let result = (|| -> Result<Report, DigestError> {
        // Only a complete walk of everything says what is gone: no directory
        // unlisted, no file left out by the filter, no stop before the end.
        if cancelled.is_none() && counts.walk_errors == 0 && matches!(settings.filter, Filter::All)
        {
            let d = store.dialect();
            let sql = format!(
                "UPDATE {} SET status = 'gone', batch_id = {}, seen_at = {} WHERE source_id = {} AND batch_id <> {} AND status <> 'gone'",
                store.qualified("source_file"),
                d.param(1, Type::Int),
                d.param(2, Type::Timestamp),
                d.param(3, Type::Int),
                d.param(4, Type::Int)
            );
            written.gone = store.execute(
                &sql,
                &[
                    Param::Int(run.batch_id),
                    Param::from(now.as_str()),
                    Param::Int(run.source_id),
                    Param::Int(run.batch_id),
                ],
            )?;
        }
        let mut report = Report::new(setup, counts, elapsed, peak_rss());
        report.written = Some(written.clone());
        report.cancelled = cancelled;
        // one review item per class the run quarantined into (§5.3): the
        // count is the evidence, the paths stay in the quarantine list
        let items: Vec<Vec<Param>> = report
            .quarantine
            .iter()
            .filter(|c| c.count > 0)
            .map(|c| {
                let reference = serde_json::json!({ "batch_id": run.batch_id, "class": c.class });
                let evidence = serde_json::json!({ "count": c.count });
                vec![
                    Param::from(QUARANTINE_KIND),
                    Param::from("batch"),
                    Param::from(reference.to_string()),
                    Param::from(evidence.to_string()),
                    Param::from("open"),
                    Param::from(now.as_str()),
                ]
            })
            .collect();
        if !items.is_empty() {
            store.insert(
                &Insert::new(
                    table("review_item"),
                    &["kind", "scope", "ref", "evidence", "status", "created_at"],
                ),
                &items,
            )?;
        }
        let report_json = serde_json::to_string(&report).unwrap_or_default();
        store.update_by_id(
            table("ingest_batch"),
            &[
                ("finished_at", Param::from(now.as_str())),
                ("state", Param::from(state)),
                ("counts", Param::from(report_json)),
                ("epoch_after", Param::Int(written.epoch)),
            ],
            "id",
            run.batch_id,
        )?;
        store.update_by_id(
            table("job"),
            &[
                ("state", Param::from(state)),
                ("finished_at", Param::from(now.as_str())),
                ("heartbeat_at", Param::from(now.as_str())),
                (
                    "progress",
                    Param::from(serde_json::to_string(&written).unwrap_or_default()),
                ),
            ],
            "id",
            run.job_id,
        )?;
        Ok(report)
    })();
    match result {
        Ok(report) => {
            store.commit()?;
            Ok(report)
        }
        Err(e) => {
            let _ = store.rollback();
            Err(e)
        }
    }
}

/// One parser thread: every task until the resume check is done or a stop is
/// asked, the identity rule applied to every file read, the items batched
/// for the writer; the batch in hand is sent whole either way, so that what
/// was read is written.
fn parse_all(
    rx: &Receiver<Task>,
    tx: &Sender<Batch>,
    rows: usize,
    rule: &Rule,
    progress: &Progress,
    cancel: &Cancel,
) -> Counts {
    let mut counts = Counts::default();
    let mut batcher = Batcher::new(rows);
    for task in rx {
        if cancel.stop() {
            break;
        }
        let item = match task {
            Task::Parse {
                path,
                rel,
                dir,
                size,
                mtime_ns,
                prior,
            } => match nils_dicom::extract_with(&path, rule.fields()) {
                Ok(mut x) => {
                    let ident = rule.apply(&mut x, &rel);
                    let signature = Signature::of(&x);
                    counts.accepted(
                        &x,
                        rule.id_type_of(&ident),
                        &ident.value,
                        &signature.key,
                        size,
                    );
                    progress.file(true);
                    let hashes = RowHashes::of(&x);
                    Item::Parsed(Box::new(ParsedFile {
                        extracted: x,
                        ident,
                        signature,
                        path: rel,
                        dir,
                        size,
                        mtime_ns,
                        hashes,
                        prior,
                    }))
                }
                Err(refusal) => {
                    counts.refused(&refusal, size);
                    progress.file(false);
                    Item::Refused {
                        path: rel,
                        dir,
                        size,
                        mtime_ns,
                        refusal,
                    }
                }
            },
            Task::Skipped {
                rel,
                dir,
                size,
                mtime_ns,
                reason,
            } => {
                counts.skipped(reason);
                Item::Skipped {
                    path: rel,
                    dir,
                    size,
                    mtime_ns,
                    reason,
                }
            }
            Task::WalkError { error } => {
                counts.walk_error(&error);
                Item::WalkError { error }
            }
        };
        if let Some(batch) = batcher.push(item)
            && tx.send(batch).is_err()
        {
            break;
        }
    }
    if let Some(batch) = batcher.take() {
        let _ = tx.send(batch);
    }
    counts
}
