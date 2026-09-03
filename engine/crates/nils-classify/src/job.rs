// SPDX-License-Identifier: AGPL-3.0-only

//! `nils fingerprint` (`docs/specs/wave2-fingerprint-and-classify.md`, §4.3):
//! a job of the same shape as `digest`, over rows rather than files.
//!
//! It reads the registry in windows of stack ids, derives, and writes in bulk,
//! one transaction per window. A window is bounded, so memory does not follow
//! the size of the corpus. A stop is honoured between windows: what is written
//! is committed, and the next run picks up where this one left off, because
//! the resume check is a predicate over columns and not a list in memory.

use std::fmt;
use std::time::Instant;

use nils_digest::Cancel;
use nils_digest::cancel::process_alive;
use nils_digest::rss::peak_rss;
use nils_registry::dialect::Conflict;
use nils_registry::schema::table;
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::{now_iso, now_secs, secs_of};
use nils_registry::{HomeError, Registry};

use crate::fingerprint::{self, First};
use crate::report::Report;

/// How many stacks a window holds. A fingerprint row is a few hundred bytes,
/// so this is tens of megabytes at most, and it is the same order as the
/// digest's batch (§9.1 of Wave 1).
pub const WINDOW: usize = 4_096;

/// A running job whose heartbeat is younger than this holds the registry.
pub const FRESH_SECS: u64 = 60;

/// What a run was asked to do.
#[derive(Debug, Clone)]
pub struct Settings {
    /// A name for the run, recorded on the job.
    pub name: String,
    /// Derive again for stacks that already have a fingerprint.
    pub force: bool,
    /// Only this modality, when given.
    pub modality: Option<String>,
    /// Stacks per window.
    pub window: usize,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            name: String::new(),
            force: false,
            modality: None,
            window: WINDOW,
        }
    }
}

impl Settings {
    /// What the job row records, so that a run can be repeated from it.
    pub fn config(&self) -> serde_json::Value {
        serde_json::json!({
            "force": self.force,
            "modality": self.modality,
            "window": self.window,
        })
    }
}

/// Why a run could not start or finish.
#[derive(Debug)]
pub enum Error {
    /// Another job holds the registry.
    Busy {
        job_id: i64,
        since: String,
    },
    Home(HomeError),
    Store(nils_registry::store::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Busy { job_id, since } => write!(
                f,
                "job {job_id} is running (last heartbeat {since}); wait for it or stop it"
            ),
            Error::Home(e) => write!(f, "{e}"),
            Error::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<HomeError> for Error {
    fn from(e: HomeError) -> Error {
        Error::Home(e)
    }
}

impl From<nils_registry::store::Error> for Error {
    fn from(e: nils_registry::store::Error) -> Error {
        Error::Store(e)
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Take the registry, failing a stale job and refusing a live one, then insert
/// this run's job row. The same rule the digest uses (Wave 1 §10): a job of
/// this host whose process is gone left no one to beat its heart.
fn claim(registry: &mut Registry, settings: &Settings) -> Result<i64, Error> {
    registry.refresh_meta()?;
    let now = now_iso();
    let host = hostname();
    let pid = i64::from(std::process::id());
    let job_t = table("job");
    let store = registry.store();
    let sql = format!(
        "SELECT id, heartbeat_at, started_at, pid, host FROM {} WHERE state = 'running'",
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
        let gone = its_host == host && its_pid.is_some_and(|p| process_alive(p) == Some(false));
        let fresh = secs_of(&last).is_some_and(|s| now_secs().saturating_sub(s) < FRESH_SECS);
        if fresh && !gone {
            return Err(Error::Busy {
                job_id: id,
                since: last,
            });
        }
        store.update_by_id(
            job_t,
            &[
                ("state", Param::from("failed")),
                ("finished_at", Param::from(now.as_str())),
                ("error", Param::from("stale: no heartbeat")),
            ],
            "id",
            id,
        )?;
    }
    let rows = store.insert(
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
            Param::from("fingerprint"),
            Param::from(settings.name.as_str()),
            Param::from(settings.config().to_string()),
            Param::from("running"),
            Param::Int(pid),
            Param::from(host.as_str()),
            Param::from(now.as_str()),
            Param::from(now.as_str()),
        ]],
    )?;
    Ok(rows
        .first()
        .ok_or_else(|| {
            Error::Store(nils_registry::store::Error::Message(
                "the job row was not written back".into(),
            ))
        })?
        .int(0)?)
}

fn finish(store: &mut Store, job_id: i64, state: &str, error: Option<&str>) -> Result<(), Error> {
    let now = now_iso();
    let mut set: Vec<(&str, Param)> = vec![
        ("state", Param::from(state)),
        ("finished_at", Param::from(now.as_str())),
    ];
    if let Some(e) = error {
        set.push(("error", Param::from(e)));
    }
    store.update_by_id(table("job"), &set, "id", job_id)?;
    Ok(())
}

fn beat(store: &mut Store, job_id: i64) -> Result<(), Error> {
    let now = now_iso();
    store.update_by_id(
        table("job"),
        &[("heartbeat_at", Param::from(now.as_str()))],
        "id",
        job_id,
    )?;
    Ok(())
}

/// Derive and store the fingerprint of every stack in scope.
pub fn fingerprint(
    registry: &mut Registry,
    settings: &Settings,
    cancel: &Cancel,
) -> Result<Report, Error> {
    let started = Instant::now();
    let job_id = claim(registry, settings)?;
    let result = run(registry, settings, cancel, job_id, started);
    let store = registry.store();
    match &result {
        Ok(report) => {
            let state = if report.cancelled {
                "cancelled"
            } else {
                "done"
            };
            finish(store, job_id, state, None)?;
        }
        Err(e) => {
            let text = e.to_string();
            finish(store, job_id, "failed", Some(&text)).ok();
        }
    }
    result
}

fn run(
    registry: &mut Registry,
    settings: &Settings,
    cancel: &Cancel,
    job_id: i64,
    started: Instant,
) -> Result<Report, Error> {
    let epoch = registry.meta().epoch;
    let mut report = Report::new(job_id, epoch);
    let window = settings.window.max(1);
    let extra = match &settings.modality {
        Some(m) => format!(" AND st.modality = '{}'", m.replace('\'', "''")),
        None => String::new(),
    };
    let store = registry.store();
    let select = fingerprint::select(store, &extra);
    let select_first = fingerprint::select_first_instances(store);
    let select_fresh = fingerprint::select_fresh(store);
    let table = fingerprint::fingerprint_table();
    let overwritten = fingerprint::overwritten();
    let insert = Insert::new(table, fingerprint::WRITTEN).on_conflict(Conflict::Update {
        target: &["stack_id"],
        set: &overwritten,
    });

    let mut after: i64 = 0;
    loop {
        if cancel.stop() {
            report.cancelled = true;
            break;
        }
        let rows = store.query(&select, &[Param::Int(after), Param::Int(window as i64)])?;
        if rows.is_empty() {
            break;
        }
        let last = rows.last().expect("a non-empty window").int(0)?;

        // Which of this window are already derived and still agree with their
        // stack's instance count.
        let mut fresh: Vec<i64> = Vec::new();
        if !settings.force {
            for r in store.query(&select_fresh, &[Param::Int(after), Param::Int(last)])? {
                fresh.push(r.int(0)?);
            }
            fresh.sort_unstable();
        }

        let mut firsts: Vec<(i64, First)> = Vec::new();
        for r in store.query(&select_first, &[Param::Int(after), Param::Int(last)])? {
            firsts.push((
                r.int(0)?,
                First {
                    pixel_spacing: r.opt_text(1)?.map(str::to_string),
                    rows: r.opt_int(2)?,
                    columns: r.opt_int(3)?,
                    image_comments: r.opt_text(4)?.map(str::to_string),
                },
            ));
        }
        firsts.sort_by_key(|(id, _)| *id);

        let empty = First::default();
        let mut params: Vec<Vec<Param>> = Vec::with_capacity(rows.len());
        for r in &rows {
            report.read += 1;
            let stack_id = r.int(0)?;
            if fresh.binary_search(&stack_id).is_ok() {
                report.skipped += 1;
                continue;
            }
            let first = firsts
                .binary_search_by_key(&stack_id, |(id, _)| *id)
                .map(|i| &firsts[i].1)
                .unwrap_or(&empty);
            if first.pixel_spacing.is_none() {
                report.without_geometry += 1;
            }
            params.push(fingerprint::derive(r, first, job_id, epoch)?);
        }

        if !params.is_empty() {
            store.begin()?;
            let written = store.insert(&insert, &params);
            match written {
                Ok(_) => {
                    store.commit()?;
                    report.written += params.len() as i64;
                }
                Err(e) => {
                    store.rollback().ok();
                    return Err(Error::Store(e));
                }
            }
        }

        beat(store, job_id)?;
        after = last;
        if rows.len() < window {
            break;
        }
    }

    report.seconds = started.elapsed().as_secs_f64();
    report.peak_rss = peak_rss();
    Ok(report)
}
