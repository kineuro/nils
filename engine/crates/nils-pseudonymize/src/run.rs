// SPDX-License-Identifier: AGPL-3.0-only

//! `nils pseudonymize`: the walker feeds the resume stage, the resume stage
//! feeds a pool of workers, each worker asks the one registry thread who
//! its file is about and writes the file, and the registry thread records a
//! row per file. The bounds are the digest's (§9.1). A stop (§10) travels
//! through one [`Cancel`] token: the walker lets its queue go, the workers
//! finish the file in hand, the registry thread records what was written,
//! and the batch and the job end as `cancelled`; a written file stays
//! written, and the next run finds it unchanged.
//!
//! The registry thread is where the key lives: it resolves identities for
//! the workers in batches through the digest's [`Resolver`], one linkage
//! query per identifier the cache does not hold, answers each worker the
//! code, or the keyed lookup and the sealed identifier of a file to hold,
//! and writes `pseudonym_file` in transactions of `batch_rows`. An
//! identifier never leaves the process and is never written in the clear.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, select};
use dicom_core::Tag;
use nils_digest::cancel::{Cancel, Cancelled};
use nils_digest::digest::{TASK_BOUND, WALK_BOUND};
use nils_digest::resolve::{Found, Make, ResolveError, Resolver, Who, collision_message};
use nils_digest::rule::Ident;
use nils_digest::walk::{Filter, mtime_ns_of, walk};
use nils_digest::writer::COLLISION_KIND;
use nils_registry::dialect::Conflict;
use nils_registry::job;
use nils_registry::review;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::now_iso;
use nils_registry::{HomeError, Registry};

use crate::layout::{self, Facts};
use crate::progress::{PROGRESS_EVERY, Progress};
use crate::report::{Files, Report, Subjects};
use crate::resume::{self, Prior, Records, Task, relative, state};
use crate::rewrite::{self, COPY_BUF, Scrub};
use crate::settings::{Settings, Unmapped};

/// How many items may wait for the registry thread.
pub const ITEM_BOUND: usize = 4_096;

/// How many resolve requests the registry thread takes in one go.
const ASK_BATCH: usize = 512;

/// The reason a file that could be read could not be written.
pub const UNWRITABLE: &str = "unwritable";

/// Why a run could not start or finish.
#[derive(Debug)]
pub enum PseudonymizeError {
    /// The originals could not be listed.
    Root {
        path: String,
        error: io::Error,
    },
    Registry(HomeError),
    /// Another job holds the registry.
    Busy {
        job_id: i64,
        since: String,
    },
    Message(String),
}

impl fmt::Display for PseudonymizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PseudonymizeError::Root { path, error } => write!(f, "cannot list {path}: {error}"),
            PseudonymizeError::Registry(e) => write!(f, "{e}"),
            PseudonymizeError::Busy { job_id, since } => write!(
                f,
                "job {job_id} is running (last heartbeat {since}); wait for it, or for its takeover {} s after its last heartbeat",
                nils_digest::digest::FRESH_SECS
            ),
            PseudonymizeError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PseudonymizeError {}

impl From<HomeError> for PseudonymizeError {
    fn from(e: HomeError) -> PseudonymizeError {
        PseudonymizeError::Registry(e)
    }
}

impl From<nils_registry::Error> for PseudonymizeError {
    fn from(e: nils_registry::Error) -> PseudonymizeError {
        PseudonymizeError::Registry(HomeError::Store(e))
    }
}

/// Run the dataset's originals into its pseudonymised tree as one batch of
/// one job; return the report, which the batch also records.
pub fn pseudonymize(
    settings: &Settings,
    registry: &mut Registry,
) -> Result<Report, PseudonymizeError> {
    pseudonymize_with(settings, registry, &Cancel::new())
}

/// [`pseudonymize`], asked to stop through `cancel` (§10): the report then
/// says how it ended. With `settings.dry_run` nothing is written and no
/// job or batch is recorded, and the report says what a run would do.
pub fn pseudonymize_with(
    settings: &Settings,
    registry: &mut Registry,
    cancel: &Cancel,
) -> Result<Report, PseudonymizeError> {
    let start = Instant::now();
    if let Err(error) = std::fs::read_dir(&settings.originals) {
        return Err(PseudonymizeError::Root {
            path: settings.originals.display().to_string(),
            error,
        });
    }
    if !settings.dry_run {
        std::fs::create_dir_all(&settings.anon).map_err(|error| PseudonymizeError::Root {
            path: settings.anon.display().to_string(),
            error,
        })?;
    }
    let run = if settings.dry_run {
        None
    } else {
        Some(start_job(registry, settings)?)
    };
    let result = execute(settings, registry, run.as_ref(), start, cancel);
    if let (Err(e), Some(r)) = (&result, run.as_ref()) {
        mark_failed(registry, r, &e.to_string());
    }
    result
}

/// The rows a run holds while it goes on.
struct Run {
    job_id: i64,
    batch_id: i64,
}

/// Claim the registry through the one job model (Wave 4a §9.1), then the
/// source row of the pseudonymised tree and the batch, of kind
/// `pseudonymize`, in one transaction. The source is the tree the digest
/// after this reads, never the originals: the registry never points at an
/// identified file.
fn start_job(registry: &mut Registry, settings: &Settings) -> Result<Run, PseudonymizeError> {
    registry.refresh_meta()?;
    let now = now_iso();
    let root = settings.anon.display().to_string();
    let root_canonical = std::fs::canonicalize(&settings.anon)
        .map_err(|error| PseudonymizeError::Root {
            path: root.clone(),
            error,
        })?
        .display()
        .to_string();
    let store = registry.store();
    let job_id = match job::claim(
        store,
        &job::Claim {
            kind: "pseudonymize",
            name: settings.name.as_str(),
            args: settings.config(),
        },
    ) {
        Ok(id) => id,
        Err(job::Error::Busy { job_id, since, .. }) => {
            return Err(PseudonymizeError::Busy { job_id, since });
        }
        Err(job::Error::Store(e)) => return Err(e.into()),
        Err(job::Error::Message(m)) => return Err(PseudonymizeError::Message(m)),
    };
    store.begin()?;
    let result = (|| -> Result<Run, PseudonymizeError> {
        let inserted = store.insert(
            &Insert::new(
                table("source"),
                &["root", "root_canonical", "first_seen_at"],
            )
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
                    "kind",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::Int(source_id),
                Param::Int(job_id),
                Param::from(settings.name.as_str()),
                Param::from(settings.config().to_string()),
                Param::from(now.as_str()),
                Param::from("running"),
                Param::from("pseudonymize"),
            ]],
        )?;
        let batch_id = batch.first().ok_or_else(no_id)?.int(0)?;
        Ok(Run { job_id, batch_id })
    })();
    match result {
        Ok(run) => {
            store.commit()?;
            Ok(run)
        }
        Err(e) => {
            let _ = store.rollback();
            let _ = job::finish(store, job_id, job::State::Failed, Some(&e.to_string()));
            Err(e)
        }
    }
}

fn no_id() -> PseudonymizeError {
    PseudonymizeError::Message("the store returned no id for a new row".into())
}

/// Best effort: the job and the batch end as failed, with the error text.
fn mark_failed(registry: &mut Registry, run: &Run, error: &str) {
    let now = now_iso();
    let store = registry.store();
    let _ = store.rollback();
    let _ = job::finish(store, run.job_id, job::State::Failed, Some(error));
    let _ = store.update_by_id(
        table("ingest_batch"),
        &[
            ("state", Param::from("failed")),
            ("finished_at", Param::from(now.as_str())),
        ],
        "id",
        run.batch_id,
    );
}

/// What a worker asks the registry thread.
struct Ask {
    ident: Ident,
    make: Make,
    reply: Sender<Answer>,
}

/// What the registry thread answers.
enum Answer {
    Found {
        id: i64,
        code: String,
        created: bool,
        provisional: bool,
    },
    /// No subject holds the identifier and none was made: the file is
    /// held with these, or would be coded in a dry run.
    Unknown {
        lookup: Vec<u8>,
        sealed: Vec<u8>,
        id_type: String,
    },
    Failed(String),
}

/// One thing the registry thread records.
pub enum Item {
    Written {
        rel: String,
        dir: String,
        size: u64,
        mtime: i64,
        out_path: String,
        out_size: u64,
        digest: String,
        subject: i64,
        created: bool,
        provisional: bool,
    },
    Held {
        rel: String,
        dir: String,
        size: u64,
        mtime: i64,
        shape: String,
        lookup: Vec<u8>,
        sealed: Vec<u8>,
        id_type: String,
    },
    Refused {
        rel: String,
        dir: String,
        size: u64,
        mtime: i64,
        class: String,
    },
    /// An earlier run's row whose file and output stand: touched.
    Unchanged {
        id: i64,
    },
    StillHeld {
        id: i64,
        shape: Option<String>,
    },
    StillRefused {
        id: i64,
    },
    /// A dry run's file that would be written, coded from an identifier no
    /// subject holds.
    WouldCode {
        lookup: Vec<u8>,
    },
    Skipped,
    WalkError {
        error: String,
    },
}

/// What one worker tallies; merged into the report at the end.
#[derive(Default)]
struct Tally {
    /// Tag to how many times it was removed.
    removed: BTreeMap<String, u64>,
    private_removed: u64,
    refused_by: BTreeMap<String, u64>,
    /// The registry thread could not answer: the run is over.
    error: Option<String>,
}

impl Tally {
    fn applied(&mut self, applied: &nils_release::scrub::Applied) {
        for ((what, action), n) in &applied.changes {
            if *action != "removed" {
                continue;
            }
            *self.removed.entry(what.clone()).or_insert(0) += *n as u64;
            if what.starts_with("private ") {
                self.private_removed += *n as u64;
            }
        }
    }

    fn merge(&mut self, other: Tally) {
        for (k, n) in other.removed {
            *self.removed.entry(k).or_insert(0) += n;
        }
        self.private_removed += other.private_removed;
        for (k, n) in other.refused_by {
            *self.refused_by.entry(k).or_insert(0) += n;
        }
        if self.error.is_none() {
            self.error = other.error;
        }
    }
}

/// What every worker shares, read only.
struct Ctx<'a> {
    settings: &'a Settings,
    scrub: Scrub<'a>,
    progress: &'a Progress,
    cancel: &'a Cancel,
    /// The outputs claimed this run, so two files landing on one place
    /// take a counter each.
    claimed: Mutex<HashSet<String>>,
    /// The directories made this run: `create_dir_all` once per directory.
    dirs: Mutex<HashSet<PathBuf>>,
}

/// The pipeline, then the batch's record.
fn execute(
    settings: &Settings,
    registry: &mut Registry,
    run: Option<&Run>,
    start: Instant,
    cancel: &Cancel,
) -> Result<Report, PseudonymizeError> {
    let workers = settings.workers.max(1);
    let progress = Progress::new(start, settings.json, settings.dry_run);
    let records = Some(Records::new(registry.open_reader()?, settings.place_id)?);
    let resolver = Resolver::new(
        registry,
        &settings.identity,
        run.map(|r| r.batch_id).unwrap_or(0),
    )?;
    let held_rows = if settings.held {
        Some(held_rows(registry.store(), settings.place_id)?)
    } else {
        None
    };
    let ctx = Ctx {
        settings,
        scrub: Scrub::new(
            &settings.private,
            &settings.tags.keep,
            &settings.tags.remove,
        ),
        progress: &progress,
        cancel,
        claimed: Mutex::new(HashSet::new()),
        dirs: Mutex::new(HashSet::new()),
    };

    let (walk_tx, walk_rx) = crossbeam_channel::bounded(WALK_BOUND);
    let (task_tx, task_rx) = crossbeam_channel::bounded::<Task>(TASK_BOUND);
    let (item_tx, item_rx) = crossbeam_channel::bounded::<Item>(ITEM_BOUND);
    let (ask_tx, ask_rx) = crossbeam_channel::bounded::<Ask>(workers * 2);
    let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(0);

    let mut recorder = Recorder::new(registry, resolver, settings, run, &progress, cancel);
    let (walked, resumed, mut tally) = std::thread::scope(|s| {
        let source = {
            let root = settings.originals.clone();
            let threads = settings.walk_threads.max(1);
            let held_rows = held_rows;
            let tasks = task_tx.clone();
            let items = item_tx.clone();
            s.spawn(move || {
                let result = match held_rows {
                    Some(rows) => {
                        held_source(&root, rows, &tasks, &items, cancel);
                        Ok(())
                    }
                    None => walk(&root, threads, &Filter::All, &walk_tx, cancel),
                };
                drop(walk_tx);
                drop(tasks);
                drop(items);
                result
            })
        };
        let resumer = {
            let root = settings.originals.clone();
            let tasks = task_tx;
            let items = item_tx.clone();
            let progress = &progress;
            s.spawn(move || {
                let result =
                    resume::run(&root, records, &walk_rx, &tasks, &items, progress, cancel);
                drop(tasks);
                drop(items);
                result
            })
        };
        let pool: Vec<_> = (0..workers)
            .map(|_| {
                let rx = task_rx.clone();
                let asks = ask_tx.clone();
                let items = item_tx.clone();
                let ctx = &ctx;
                s.spawn(move || {
                    let tally = worker(ctx, &rx, &asks, &items);
                    drop(asks);
                    drop(items);
                    tally
                })
            })
            .collect();
        drop(task_rx);
        drop(ask_tx);
        drop(item_tx);
        let recording = {
            let recorder = &mut recorder;
            s.spawn(move || {
                let result = recorder.run(&ask_rx, &item_rx);
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

        let walked = source.join().expect("source thread");
        let resumed = resumer.join().expect("resume thread");
        let mut tally = Tally::default();
        for w in pool {
            tally.merge(w.join().expect("worker thread"));
        }
        let recorded = recording.join().expect("registry thread");
        if let Err(e) = recorded
            && tally.error.is_none()
        {
            tally.error = Some(e.to_string());
        }
        (walked, resumed, tally)
    });
    if let Err(error) = walked {
        return Err(PseudonymizeError::Root {
            path: settings.originals.display().to_string(),
            error,
        });
    }
    resumed?;
    if let Some(error) = tally.error.take() {
        return Err(PseudonymizeError::Message(error));
    }
    let cancelled = if cancel.abort() {
        Some(Cancelled::Aborted)
    } else if cancel.stop() {
        Some(Cancelled::Stopped)
    } else {
        None
    };
    let elapsed = start.elapsed().as_secs_f64();
    recorder.finish(tally, elapsed, cancelled)
}

/// The held rows a map released or a person coded anyway (record 26 §4):
/// what `--held` reads, and nothing else.
fn held_rows(store: &mut Store, place_id: i64) -> Result<Vec<(i64, String, bool)>, HomeError> {
    let d = store.dialect();
    let t = table("pseudonym_file");
    let released = d.text_of(t.column("released_at").expect("released_at"));
    let sql = format!(
        "SELECT id, path, code_anyway FROM {} WHERE place_id = {} AND state = 'held' \
         AND ({released} IS NOT NULL OR code_anyway = 1) ORDER BY id",
        store.qualified("pseudonym_file"),
        d.param(1, Type::Int)
    );
    let rows = store.query(&sql, &[Param::Int(place_id)])?;
    rows.iter()
        .map(|r| Ok((r.int(0)?, r.text(1)?.to_string(), r.int(2)? != 0)))
        .collect::<Result<_, nils_registry::Error>>()
        .map_err(HomeError::Store)
}

/// The source of a `--held` run: each held row's file, stat'ed, as a task
/// with its record beside it; a file that is gone is refused.
fn held_source(
    root: &Path,
    rows: Vec<(i64, String, bool)>,
    tasks: &Sender<Task>,
    items: &Sender<Item>,
    cancel: &Cancel,
) {
    for (id, rel, code_anyway) in rows {
        if cancel.stop() {
            break;
        }
        let path = root.join(&rel);
        let (_, dir) = relative(Path::new(""), Path::new(&rel));
        let sent = match std::fs::metadata(&path) {
            Ok(m) => tasks
                .send(Task::Read {
                    path,
                    rel,
                    size: m.len(),
                    mtime: mtime_ns_of(&m),
                    prior: Some(Prior {
                        id,
                        out_path: None,
                        changed: false,
                        code_anyway,
                    }),
                })
                .is_err(),
            Err(_) => items
                .send(Item::Refused {
                    rel,
                    dir,
                    size: 0,
                    mtime: 0,
                    class: "unreadable".to_string(),
                })
                .is_err(),
        };
        if sent {
            break;
        }
    }
}

/// One worker: every task until the stage before is done or a stop is
/// asked; the file read and framed, its identity asked of the registry
/// thread, the plan applied, the file written, and a row sent to be
/// recorded.
fn worker(ctx: &Ctx<'_>, rx: &Receiver<Task>, asks: &Sender<Ask>, items: &Sender<Item>) -> Tally {
    let mut tally = Tally::default();
    let mut buf = vec![0u8; COPY_BUF];
    let (reply_tx, reply_rx) = crossbeam_channel::bounded::<Answer>(1);
    let progress = ctx.progress;
    let dry = ctx.settings.dry_run;
    for task in rx {
        if ctx.cancel.stop() {
            break;
        }
        let (path, rel, size, mtime, prior) = match task {
            Task::Check {
                path,
                rel,
                size,
                mtime,
                id,
                out_path,
                out_size,
            } => {
                let out = ctx.settings.anon.join(&out_path);
                match std::fs::metadata(&out) {
                    Ok(m) if m.len() as i64 == out_size => {
                        progress.file(&progress.unchanged, size);
                        if items.send(Item::Unchanged { id }).is_err() {
                            break;
                        }
                        continue;
                    }
                    // the output is gone or not what was recorded: written
                    // again, over its own place
                    _ => (
                        path,
                        rel,
                        size,
                        mtime,
                        Some(Prior {
                            id,
                            out_path: Some(out_path),
                            changed: false,
                            code_anyway: false,
                        }),
                    ),
                }
            }
            Task::Read {
                path,
                rel,
                size,
                mtime,
                prior,
            } => (path, rel, size, mtime, prior),
        };
        let (_, dir) = relative(Path::new(""), Path::new(&rel));
        let prepared = match rewrite::prepare(&path, &rel, &ctx.settings.identity) {
            Ok(p) => p,
            Err(refusal) => {
                *tally
                    .refused_by
                    .entry(refusal.class.name().to_string())
                    .or_insert(0) += 1;
                progress.file(&progress.refused, size);
                if items
                    .send(Item::Refused {
                        rel,
                        dir,
                        size,
                        mtime,
                        class: refusal.class.name().to_string(),
                    })
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        // who the file is about: made when the dataset codes unmapped
        // identifiers or a person asked for this one; never in a dry run
        let anyway = prior.as_ref().is_some_and(|p| p.code_anyway);
        let make = if dry {
            Make::Nothing
        } else if anyway || ctx.settings.unmapped == Unmapped::Code {
            Make::Provisional
        } else {
            Make::Nothing
        };
        if asks
            .send(Ask {
                ident: prepared.ident.clone(),
                make,
                reply: reply_tx.clone(),
            })
            .is_err()
        {
            break;
        }
        let Ok(answer) = reply_rx.recv() else { break };
        let (subject, code, created, provisional) = match answer {
            Answer::Failed(why) => {
                tally.error = Some(why);
                break;
            }
            Answer::Unknown {
                lookup,
                sealed,
                id_type,
            } => {
                if dry && (anyway || ctx.settings.unmapped == Unmapped::Code) {
                    // would be coded: counted as a write under a code the
                    // run would derive, the plan applied for its counts
                    if items.send(Item::WouldCode { lookup }).is_err() {
                        break;
                    }
                    (0, "would-be-coded".to_string(), true, true)
                } else {
                    let shape = nils_dicom::diagnostic::shape(&prepared.ident.value);
                    progress.file(&progress.held, size);
                    if items
                        .send(Item::Held {
                            rel,
                            dir,
                            size,
                            mtime,
                            shape,
                            lookup,
                            sealed,
                            id_type,
                        })
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
            }
            Answer::Found {
                id,
                code,
                created,
                provisional,
            } => (id, code, created, provisional),
        };
        let out_rel = place(ctx, &code, &prepared.facts, prior.as_ref());
        let target = ctx.settings.anon.join(&out_rel);
        if !dry && let Some(parent) = target.parent() {
            let mut dirs = ctx.dirs.lock().unwrap_or_else(|e| e.into_inner());
            if !dirs.contains(parent) {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tally.error = Some(format!("cannot make {}: {e}", parent.display()));
                    break;
                }
                dirs.insert(parent.to_path_buf());
            }
        }
        match rewrite::write(prepared, &code, &ctx.scrub, &path, &target, &mut buf, dry) {
            Ok(outcome) => {
                tally.applied(&outcome.applied);
                // the output an earlier run wrote elsewhere is let go
                if !dry
                    && let Some(old) = prior.as_ref().and_then(|p| p.out_path.as_deref())
                    && old != out_rel
                {
                    let _ = std::fs::remove_file(ctx.settings.anon.join(old));
                }
                progress.file(&progress.written, size);
                if dry {
                    continue;
                }
                if items
                    .send(Item::Written {
                        rel,
                        dir,
                        size,
                        mtime,
                        out_path: out_rel,
                        out_size: outcome.out_size,
                        digest: outcome.digest,
                        subject,
                        created,
                        provisional,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(why) => {
                let class = why
                    .split(':')
                    .next()
                    .unwrap_or(UNWRITABLE)
                    .trim()
                    .to_string();
                *tally.refused_by.entry(class.clone()).or_insert(0) += 1;
                progress.file(&progress.refused, size);
                if items
                    .send(Item::Refused {
                        rel,
                        dir,
                        size,
                        mtime,
                        class,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    tally
}

/// Where the file goes under the tree (record 26 §3): the place its facts
/// name, with a counter when another file of this run, or one already
/// there that is not this file's own output, holds it.
fn place(ctx: &Ctx<'_>, code: &str, facts: &Facts, prior: Option<&Prior>) -> String {
    let base = layout::relative(code, facts);
    let own = prior.and_then(|p| p.out_path.as_deref());
    let mut rel = base.clone();
    let mut n = 1;
    loop {
        let mut claimed = ctx.claimed.lock().unwrap_or_else(|e| e.into_inner());
        let free = !claimed.contains(&rel)
            && (own == Some(rel.as_str())
                || ctx.settings.dry_run
                || !ctx.settings.anon.join(&rel).exists());
        if free {
            claimed.insert(rel.clone());
            return rel;
        }
        drop(claimed);
        n += 1;
        rel = layout::with_counter(&base, n);
    }
}

/// The registry thread's state.
struct Recorder<'a> {
    registry: &'a mut Registry,
    resolver: Resolver,
    settings: &'a Settings,
    run: Option<&'a Run>,
    progress: &'a Progress,
    cancel: &'a Cancel,
    buffer: Vec<Item>,
    /// Subject id to its code and whether it is provisional.
    codes: HashMap<i64, (String, bool)>,
    subjects_seen: HashSet<i64>,
    provisional_seen: HashSet<i64>,
    /// The subjects this run made, with their files written.
    made: BTreeMap<i64, (String, u64)>,
    /// A dry run's identifiers that would be coded.
    would_make: HashSet<Vec<u8>>,
    held_by_shape: BTreeMap<String, u64>,
    skipped: u64,
    walk_errors: u64,
    last_heartbeat: Instant,
    /// The first error met; the thread goes on draining so the workers
    /// can stop, and the run ends with it.
    error: Option<String>,
}

impl<'a> Recorder<'a> {
    fn new(
        registry: &'a mut Registry,
        resolver: Resolver,
        settings: &'a Settings,
        run: Option<&'a Run>,
        progress: &'a Progress,
        cancel: &'a Cancel,
    ) -> Recorder<'a> {
        Recorder {
            registry,
            resolver,
            settings,
            run,
            progress,
            cancel,
            buffer: Vec::new(),
            codes: HashMap::new(),
            subjects_seen: HashSet::new(),
            provisional_seen: HashSet::new(),
            made: BTreeMap::new(),
            would_make: HashSet::new(),
            held_by_shape: BTreeMap::new(),
            skipped: 0,
            walk_errors: 0,
            last_heartbeat: Instant::now(),
            error: None,
        }
    }

    /// The loop: resolve requests as they come, in batches; items into
    /// the buffer, flushed at `batch_rows`; a heartbeat between them.
    fn run(&mut self, asks: &Receiver<Ask>, items: &Receiver<Item>) -> Result<(), HomeError> {
        let no_asks: Receiver<Ask> = crossbeam_channel::never();
        let no_items: Receiver<Item> = crossbeam_channel::never();
        let mut asks_open = true;
        let mut items_open = true;
        while asks_open || items_open {
            let ask_rx = if asks_open { asks } else { &no_asks };
            let item_rx = if items_open { items } else { &no_items };
            select! {
                recv(ask_rx) -> msg => match msg {
                    Ok(first) => {
                        let mut batch = vec![first];
                        batch.extend(asks.try_iter().take(ASK_BATCH - 1));
                        self.answer(batch);
                    }
                    Err(_) => asks_open = false,
                },
                recv(item_rx) -> msg => match msg {
                    Ok(item) => {
                        self.take(item);
                        if self.buffer.len() >= self.settings.batch_rows.max(1) {
                            self.flush();
                        }
                    }
                    Err(_) => items_open = false,
                },
                default(PROGRESS_EVERY) => {}
            }
            self.heartbeat(false);
        }
        self.flush();
        match self.error.take() {
            Some(e) => Err(HomeError::Message(e)),
            None => Ok(()),
        }
    }

    /// A batch of requests answered: resolved through the linkage store in
    /// one transaction per way of making subjects, the codes read for the
    /// subjects found, each worker told.
    fn answer(&mut self, batch: Vec<Ask>) {
        if self.error.is_some() {
            for ask in batch {
                let _ = ask.reply.send(Answer::Failed("the run is over".into()));
            }
            return;
        }
        let now = now_iso();
        let mut by_make: BTreeMap<u8, Vec<Ask>> = BTreeMap::new();
        for ask in batch {
            let key = match ask.make {
                Make::Nothing => 0,
                Make::Subject => 1,
                Make::Provisional => 2,
            };
            by_make.entry(key).or_default().push(ask);
        }
        for (_, group) in by_make {
            let make = group[0].make;
            let who: Vec<Who<'_>> = group
                .iter()
                .map(|a| Who {
                    ident: &a.ident,
                    subject: Vec::new(),
                })
                .collect();
            let store = self.registry.store();
            let resolved = match store.begin() {
                Ok(()) => self.resolver.resolve(store, &who, &now, make),
                Err(e) => Err(ResolveError::Home(e.into())),
            };
            let resolved = match resolved {
                Ok(r) => {
                    if let Err(e) = store.commit() {
                        let _ = store.rollback();
                        self.resolver.abandon();
                        self.fail(e.to_string(), group);
                        continue;
                    }
                    if let Err(e) = self.resolver.file_identities() {
                        self.fail(e.to_string(), group);
                        continue;
                    }
                    r
                }
                Err(ResolveError::Collision(c)) => {
                    let _ = store.rollback();
                    self.resolver.abandon();
                    let item = self.open_collision(&c).unwrap_or(0);
                    self.fail(collision_message(&c, item), group);
                    continue;
                }
                Err(ResolveError::Home(e)) => {
                    let _ = store.rollback();
                    self.resolver.abandon();
                    self.fail(e.to_string(), group);
                    continue;
                }
            };
            // the codes of the subjects found, read once each
            let mut fetch: Vec<i64> = resolved
                .found
                .iter()
                .filter_map(|f| f.id())
                .filter(|id| !self.codes.contains_key(id))
                .collect();
            fetch.sort_unstable();
            fetch.dedup();
            if !fetch.is_empty()
                && let Err(e) = self.read_codes(&fetch)
            {
                self.fail(e.to_string(), group);
                continue;
            }
            for (ask, found) in group.into_iter().zip(resolved.found) {
                let answer = match found {
                    Found::Unknown => {
                        if self.settings.dry_run
                            && (ask.make != Make::Nothing
                                || self.settings.unmapped == Unmapped::Code)
                        {
                            self.would_make.insert(self.resolver.lookup(&ask.ident));
                        }
                        Answer::Unknown {
                            lookup: self.resolver.lookup(&ask.ident),
                            sealed: self.resolver.seal(&ask.ident.value),
                            id_type: self.resolver.type_of(&ask.ident).name.clone(),
                        }
                    }
                    Found::Known(id) | Found::Created(id) => {
                        let created = matches!(found, Found::Created(_));
                        let (code, provisional) = match self.codes.get(&id) {
                            Some((c, p)) => (c.clone(), *p),
                            None => {
                                let _ = ask.reply.send(Answer::Failed(
                                    "a subject was made and its code not read back".into(),
                                ));
                                continue;
                            }
                        };
                        // met, whether or not its file is then written
                        self.subjects_seen.insert(id);
                        if provisional {
                            self.provisional_seen.insert(id);
                        }
                        Answer::Found {
                            id,
                            code,
                            created,
                            provisional,
                        }
                    }
                };
                let _ = ask.reply.send(answer);
            }
        }
    }

    fn fail(&mut self, why: String, group: Vec<Ask>) {
        if self.error.is_none() {
            self.error = Some(why.clone());
        }
        for ask in group {
            let _ = ask.reply.send(Answer::Failed(why.clone()));
        }
    }

    /// The review item of a collision (§7.1), as the digest's writer opens
    /// it: the subject and the type, never an identifier.
    fn open_collision(&mut self, c: &nils_digest::resolve::Collision) -> Result<i64, HomeError> {
        let now = now_iso();
        let reference = serde_json::json!({ "subject_id": c.subject_id, "code": c.code });
        let evidence = serde_json::json!({
            "id_type": c.id_type,
            "reason": c.reason,
            "scheme": self.resolver.scheme().to_string(),
            "display_length": self.resolver.display_length(),
            "batch_id": self.run.map(|r| r.batch_id),
        });
        let store = self.registry.store();
        store.begin()?;
        let result = store.insert(
            &Insert::new(
                table("review_item"),
                &["kind", "scope", "ref", "evidence", "status", "created_at"],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(COLLISION_KIND),
                Param::from("subject"),
                Param::from(reference.to_string()),
                Param::from(evidence.to_string()),
                Param::from("open"),
                Param::from(now.as_str()),
            ]],
        );
        match result {
            Ok(rows) => {
                store.commit()?;
                Ok(rows.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
            }
            Err(e) => {
                let _ = store.rollback();
                Err(e.into())
            }
        }
    }

    /// The codes of subjects, with whether each is provisional.
    fn read_codes(&mut self, ids: &[i64]) -> Result<(), HomeError> {
        let t = table("subject");
        let cols = [
            t.column("id").expect("subject.id"),
            t.column("code").expect("subject.code"),
            t.column("provisional").expect("subject.provisional"),
        ];
        let rows = self.registry.store().select_by_ids(t, &cols, "id", ids)?;
        for r in &rows {
            self.codes.insert(
                r.int(0)?,
                (r.text(1)?.to_string(), r.opt_int(2)?.unwrap_or(0) != 0),
            );
        }
        Ok(())
    }

    /// One item counted, and buffered when it is a row.
    fn take(&mut self, item: Item) {
        match &item {
            Item::Written {
                subject, created, ..
            } => {
                if *created || self.made.contains_key(subject) {
                    let code = self
                        .codes
                        .get(subject)
                        .map(|(c, _)| c.clone())
                        .unwrap_or_default();
                    self.made.entry(*subject).or_insert((code, 0)).1 += 1;
                }
            }
            Item::Held { shape, .. } => {
                *self.held_by_shape.entry(shape.clone()).or_insert(0) += 1;
            }
            Item::StillHeld { shape, .. } => {
                *self
                    .held_by_shape
                    .entry(shape.clone().unwrap_or_else(|| "?".into()))
                    .or_insert(0) += 1;
            }
            Item::Refused { .. } | Item::Unchanged { .. } | Item::StillRefused { .. } => {}
            Item::WouldCode { lookup } => {
                self.would_make.insert(lookup.clone());
                return;
            }
            Item::Skipped => {
                self.skipped += 1;
                return;
            }
            Item::WalkError { .. } => {
                self.walk_errors += 1;
                return;
            }
        }
        if self.settings.dry_run {
            return;
        }
        self.buffer.push(item);
    }

    /// The rows of the buffer, in one transaction: the written, held and
    /// refused files upserted on `(place_id, path)`, the unchanged and
    /// still held or refused rows touched by id.
    fn flush(&mut self) {
        if self.buffer.is_empty() || self.error.is_some() {
            self.buffer.clear();
            return;
        }
        let items = std::mem::take(&mut self.buffer);
        let Some(run) = self.run else {
            return;
        };
        let batch_id = run.batch_id;
        let place_id = self.settings.place_id;
        let now = now_iso();
        let store = self.registry.store();
        let result = (|| -> Result<(), nils_registry::Error> {
            let t = table("pseudonym_file");
            let mut written: Vec<Vec<Param>> = Vec::new();
            let mut held: Vec<Vec<Param>> = Vec::new();
            let mut refused: Vec<Vec<Param>> = Vec::new();
            let mut touched: Vec<i64> = Vec::new();
            for item in &items {
                match item {
                    Item::Written {
                        rel,
                        dir,
                        size,
                        mtime,
                        out_path,
                        out_size,
                        digest,
                        ..
                    } => written.push(vec![
                        Param::Int(place_id),
                        Param::from(rel.as_str()),
                        Param::from(dir.as_str()),
                        Param::Int(*size as i64),
                        Param::Int(*mtime),
                        Param::from(state::WRITTEN),
                        Param::Null,
                        Param::Null,
                        Param::Null,
                        Param::Null,
                        Param::from(out_path.as_str()),
                        Param::Int(*out_size as i64),
                        Param::from(digest.as_str()),
                        Param::Int(batch_id),
                        Param::from(now.as_str()),
                        Param::from(now.as_str()),
                        Param::Null,
                        Param::Int(0),
                    ]),
                    Item::Held {
                        rel,
                        dir,
                        size,
                        mtime,
                        shape,
                        lookup,
                        sealed,
                        id_type,
                    } => held.push(vec![
                        Param::Int(place_id),
                        Param::from(rel.as_str()),
                        Param::from(dir.as_str()),
                        Param::Int(*size as i64),
                        Param::Int(*mtime),
                        Param::from(state::HELD),
                        Param::from(shape.as_str()),
                        Param::Bytes(lookup.clone()),
                        Param::Bytes(sealed.clone()),
                        Param::from(id_type.as_str()),
                        Param::Int(batch_id),
                        Param::from(now.as_str()),
                        Param::Int(0),
                    ]),
                    Item::Refused {
                        rel,
                        dir,
                        size,
                        mtime,
                        ..
                    } => refused.push(vec![
                        Param::Int(place_id),
                        Param::from(rel.as_str()),
                        Param::from(dir.as_str()),
                        Param::Int(*size as i64),
                        Param::Int(*mtime),
                        Param::from(state::REFUSED),
                        Param::Int(batch_id),
                        Param::from(now.as_str()),
                        Param::Int(0),
                    ]),
                    Item::Unchanged { id }
                    | Item::StillHeld { id, .. }
                    | Item::StillRefused { id } => touched.push(*id),
                    _ => {}
                }
            }
            store.begin()?;
            let done = (|| -> Result<(), nils_registry::Error> {
                if !written.is_empty() {
                    store.insert(
                        &Insert::new(
                            t,
                            &[
                                "place_id",
                                "path",
                                "dir",
                                "size",
                                "mtime",
                                "state",
                                "shape",
                                "lookup",
                                "sealed",
                                "id_type",
                                "out_path",
                                "out_size",
                                "digest",
                                "batch_id",
                                "first_seen",
                                "written_at",
                                "released_at",
                                "code_anyway",
                            ],
                        )
                        .on_conflict(Conflict::Update {
                            target: &["place_id", "path"],
                            set: &[
                                "dir",
                                "size",
                                "mtime",
                                "state",
                                "shape",
                                "lookup",
                                "sealed",
                                "id_type",
                                "out_path",
                                "out_size",
                                "digest",
                                "batch_id",
                                "written_at",
                                "released_at",
                                "code_anyway",
                            ],
                        }),
                        &written,
                    )?;
                }
                if !held.is_empty() {
                    // released_at and code_anyway stay as they are: a file
                    // held again after a release keeps its release
                    store.insert(
                        &Insert::new(
                            t,
                            &[
                                "place_id",
                                "path",
                                "dir",
                                "size",
                                "mtime",
                                "state",
                                "shape",
                                "lookup",
                                "sealed",
                                "id_type",
                                "batch_id",
                                "first_seen",
                                "code_anyway",
                            ],
                        )
                        .on_conflict(Conflict::Update {
                            target: &["place_id", "path"],
                            set: &[
                                "dir", "size", "mtime", "state", "shape", "lookup", "sealed",
                                "id_type", "batch_id",
                            ],
                        }),
                        &held,
                    )?;
                }
                if !refused.is_empty() {
                    store.insert(
                        &Insert::new(
                            t,
                            &[
                                "place_id",
                                "path",
                                "dir",
                                "size",
                                "mtime",
                                "state",
                                "batch_id",
                                "first_seen",
                                "code_anyway",
                            ],
                        )
                        .on_conflict(Conflict::Update {
                            target: &["place_id", "path"],
                            set: &["dir", "size", "mtime", "state", "batch_id"],
                        }),
                        &refused,
                    )?;
                }
                if !touched.is_empty() {
                    store.update_by_ids(
                        t,
                        &[("batch_id", Param::Int(batch_id))],
                        "id",
                        &touched,
                    )?;
                }
                Ok(())
            })();
            match done {
                Ok(()) => store.commit(),
                Err(e) => {
                    let _ = store.rollback();
                    Err(e)
                }
            }
        })();
        if let Err(e) = result
            && self.error.is_none()
        {
            self.error = Some(e.to_string());
        }
    }

    /// The job's heartbeat (§10): every ten seconds unless forced, with
    /// the progress counters beside it; a cancel asked through the door
    /// arrives here and stops the run the way the first signal does.
    fn heartbeat(&mut self, force: bool) {
        let Some(run) = self.run else {
            return;
        };
        if !force && self.last_heartbeat.elapsed() < PROGRESS_EVERY {
            return;
        }
        let mut json = self.progress.json();
        json["batch_id"] = run.batch_id.into();
        match job::beat(self.registry.store(), run.job_id, Some(&json)) {
            Ok(job::Asked::Cancel) if !self.cancel.stop() => {
                self.cancel.request();
            }
            Ok(_) => {}
            Err(e) => {
                if self.error.is_none() {
                    self.error = Some(e.to_string());
                }
            }
        }
        self.last_heartbeat = Instant::now();
    }

    /// The run's last transaction: the review items for the subjects made
    /// and the shapes held, the batch's counts and epoch, the job done or
    /// cancelled with the report as its result; then the tree synced once.
    fn finish(
        mut self,
        tally: Tally,
        elapsed: f64,
        cancelled: Option<Cancelled>,
    ) -> Result<Report, PseudonymizeError> {
        let progress = self.progress;
        let load = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
        let seen = load(&progress.seen);
        let bytes = load(&progress.bytes);
        let mut report = Report {
            name: self.settings.name.clone(),
            dataset: self.settings.dataset.clone(),
            originals: self.settings.originals.display().to_string(),
            root: self.settings.anon.display().to_string(),
            dry_run: self.settings.dry_run,
            held_only: self.settings.held,
            workers: self.settings.workers.max(1),
            // the four outcomes as the stages counted them, which a dry
            // run counts too; the recorder's rows are what was written down
            files: Files {
                seen,
                written: load(&progress.written),
                unchanged: load(&progress.unchanged),
                held: load(&progress.held),
                refused: load(&progress.refused),
                skipped: self.skipped,
            },
            subjects: Subjects {
                new: self.made.len() as u64 + self.would_make.len() as u64,
                seen: self.subjects_seen.len() as u64 + self.would_make.len() as u64,
                provisional: self.provisional_seen.len() as u64 + self.would_make.len() as u64,
            },
            tags_removed: tally.removed,
            private_removed: tally.private_removed,
            refused_by: tally.refused_by,
            held_by_shape: self.held_by_shape.clone(),
            walk_errors: self.walk_errors,
            bytes,
            seconds: elapsed,
            files_per_s: if elapsed > 0.0 {
                seen as f64 / elapsed
            } else {
                0.0
            },
            cancelled,
            batch_id: self.run.map(|r| r.batch_id),
            job_id: self.run.map(|r| r.job_id),
        };
        let Some(run) = self.run else {
            return Ok(report);
        };
        let now = now_iso();
        let state = if cancelled.is_some() {
            "cancelled"
        } else {
            "done"
        };
        let place_id = self.settings.place_id;
        let made = std::mem::take(&mut self.made);
        let created = made.len() as u64;
        let registry = &mut *self.registry;
        registry.store().begin()?;
        let result = (|| -> Result<(), PseudonymizeError> {
            let store = registry.store();
            for (subject, (code, files)) in &made {
                review::raise_provisional(
                    store,
                    *subject,
                    code,
                    *files as i64,
                    Some(run.batch_id),
                    &now,
                )?;
            }
            // one question per dataset and shape held (record 26 §4),
            // answered by the run that holds nothing under it any more
            let t = table("pseudonym_file");
            let first_seen = store
                .dialect()
                .text_of(t.column("first_seen").expect("first_seen"));
            let sql = format!(
                "SELECT shape, COUNT(*), MIN({first_seen}) FROM {} WHERE place_id = {} \
                 AND state = 'held' AND shape IS NOT NULL GROUP BY shape",
                store.qualified("pseudonym_file"),
                store.dialect().param(1, Type::Int)
            );
            let rows = store.query(&sql, &[Param::Int(place_id)])?;
            let mut open: Vec<String> = Vec::new();
            for r in &rows {
                let shape = r.text(0)?.to_string();
                let files = r.int(1)?;
                let since = r.opt_text(2)?.unwrap_or(now.as_str()).to_string();
                review::raise_unmapped(
                    store,
                    place_id,
                    &shape,
                    files,
                    &since,
                    Some(run.batch_id),
                    &now,
                )?;
                open.push(shape);
            }
            for shape in review::open_unmapped_shapes(store, place_id)? {
                if !open.contains(&shape) {
                    review::close_unmapped(store, place_id, &shape, &now)?;
                }
            }
            let epoch = if created > 0 {
                registry.next_epoch()?
            } else {
                registry.meta().epoch
            };
            let report_json = serde_json::to_string(&report).unwrap_or_default();
            let store = registry.store();
            store.update_by_id(
                table("ingest_batch"),
                &[
                    ("finished_at", Param::from(now.as_str())),
                    ("state", Param::from(state)),
                    ("counts", Param::from(report_json.as_str())),
                    ("epoch_after", Param::Int(epoch)),
                ],
                "id",
                run.batch_id,
            )?;
            let mut progress = self.progress.json();
            progress["batch_id"] = run.batch_id.into();
            store.update_by_id(
                table("job"),
                &[
                    ("state", Param::from(state)),
                    ("finished_at", Param::from(now.as_str())),
                    ("heartbeat_at", Param::from(now.as_str())),
                    ("progress", Param::from(progress.to_string())),
                    ("result", Param::from(report_json.as_str())),
                ],
                "id",
                run.job_id,
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => registry.store().commit()?,
            Err(e) => {
                let _ = registry.store().rollback();
                return Err(e);
            }
        }
        // one sync of the tree's directory per batch, never one per file
        if let Ok(dir) = std::fs::File::open(&self.settings.anon) {
            let _ = dir.sync_all();
        }
        report.batch_id = Some(run.batch_id);
        Ok(report)
    }
}

/// The tag lists of a dataset as the settings hold them, for a caller
/// that shows them.
pub fn tag_text(tag: &Tag) -> String {
    format!("{:04X},{:04X}", tag.group(), tag.element())
}
