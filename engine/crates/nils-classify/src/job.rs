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
use nils_registry::schema::{Column, table};
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
    /// One threshold for every axis, overriding what the pack declares. The
    /// numbers are the pack's (§8.2): a pack that flags everything has failed
    /// even if it agrees with v0. This is the operator's override of them,
    /// for a run that wants to see more or less than the pack asks for.
    pub review_below: Option<f64>,
    /// Stacks per window.
    pub window: usize,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            name: String::new(),
            force: false,
            modality: None,
            review_below: None,
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
            "review_below": self.review_below,
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
pub fn claim_for(registry: &mut Registry, settings: &Settings, kind: &str) -> Result<i64, Error> {
    registry.refresh_meta()?;
    let now = now_iso();
    let host = hostname();
    let pid = i64::from(std::process::id());
    let job_t = table("job");
    let store = registry.store();
    let dialect = store.dialect();
    let stamp = |name: &str| {
        dialect.text_of_qualified(
            None,
            job_t
                .column(name)
                .unwrap_or_else(|| panic!("job.{name} is not a column")),
        )
    };
    // The two timestamps are read as text: Postgres hands a timestamp back in
    // a type the store does not read, and this select only ever sees a row
    // when another job is running, which is exactly when it must not fail.
    let sql = format!(
        "SELECT id, {}, {}, pid, host FROM {} WHERE state = 'running'",
        stamp("heartbeat_at"),
        stamp("started_at"),
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
            Param::from(kind),
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

pub fn finish(
    store: &mut Store,
    job_id: i64,
    state: &str,
    error: Option<&str>,
) -> Result<(), Error> {
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

pub fn beat(store: &mut Store, job_id: i64) -> Result<(), Error> {
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
    let job_id = claim_for(registry, settings, "fingerprint")?;
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

        // Why each multi-stack series in this window split. v0 stores this on
        // the stack and then never reads it (spikes/pack, finding 1); it is a
        // fact about the series, so it is derived here and a pack reads it.
        let reasons = split_reasons(store, &rows)?;

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
            let series_id = r.int(1)?;
            let reason = reasons
                .binary_search_by_key(&series_id, |(id, _)| *id)
                .ok()
                .and_then(|i| reasons[i].1);
            params.push(fingerprint::derive(r, first, reason, job_id, epoch)?);
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

    if !report.cancelled {
        report.studies_settled = settle_primaries(store)?;
    }

    report.seconds = started.elapsed().as_secs_f64();
    report.peak_rss = peak_rss();
    Ok(report)
}

/// Whether each study holds a stack the scanner called its output (§6).
///
/// Half of the session rescue, and the half that is a fact: it depends only on
/// the study's own stacks. The other half is which studies are one session,
/// which depends on a scheme, so it is composed on read and nothing about a
/// session is stored (§5).
///
/// Run once at the end, over every study rather than only the run's, because a
/// study whose stacks were fingerprinted across two runs would otherwise carry
/// the answer for whichever run finished last. Only studies whose stacks are
/// **all** fingerprinted are given a value: a partly derived study cannot say
/// it has no primary, only that it has not found one yet, and null is how that
/// is said.
fn settle_primaries(store: &mut Store) -> Result<i64, Error> {
    let sql = format!(
        "UPDATE {study} AS s SET has_original_primary = (             SELECT MAX(CASE WHEN f.image_role = 'original_primary' THEN 1 ELSE 0 END)              FROM {fp} f WHERE f.study_id = s.id)          WHERE EXISTS (SELECT 1 FROM {fp} f WHERE f.study_id = s.id)            AND NOT EXISTS (             SELECT 1 FROM {series} se JOIN {stack} k ON k.series_id = se.id              LEFT JOIN {fp} f ON f.stack_id = k.id              WHERE se.study_id = s.id AND f.id IS NULL)",
        study = store.qualified("study"),
        fp = store.qualified("stack_fingerprint"),
        series = store.qualified("series"),
        stack = store.qualified("stack"),
    );
    store.begin()?;
    match store.execute(&sql, &[]) {
        Ok(n) => {
            store.commit()?;
            Ok(n as i64)
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// The split reason of every series in the window that has more than one
/// stack, read from the stacks themselves so that it is the same fact the
/// signature was.
fn split_reasons(
    store: &mut Store,
    window: &[nils_registry::store::Row],
) -> Result<Vec<(i64, Option<&'static str>)>, Error> {
    // series_id is column 1 of the window's select; stacks_in_series is the
    // third of the series block.
    let mut ids: Vec<i64> = Vec::new();
    for r in window {
        if r.int(fingerprint::STACKS_IN_SERIES)? > 1 {
            ids.push(r.int(1)?);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let stack_t = table("stack");
    let signature: Vec<&Column> =
        std::iter::once(stack_t.column("series_id").expect("stack has a series_id"))
            .chain(stack_t.columns.iter().filter(|c| c.catalogue))
            .collect();
    let rows = store.select_by_ids(stack_t, &signature, "series_id", &ids)?;

    // Group by series, then name the first column the stacks disagree on.
    let mut by_series: std::collections::BTreeMap<i64, Vec<&nils_registry::store::Row>> =
        std::collections::BTreeMap::new();
    for r in &rows {
        by_series.entry(r.int(0)?).or_default().push(r);
    }
    let names: Vec<&str> = signature[1..].iter().map(|c| c.name).collect();
    let mut out = Vec::with_capacity(by_series.len());
    for (series_id, stacks) in by_series {
        if stacks.len() < 2 {
            out.push((series_id, None));
            continue;
        }
        let first = stacks[0];
        let mut varying: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for (i, name) in names.iter().enumerate() {
            let at = i + 1;
            if stacks[1..]
                .iter()
                .any(|s| !fingerprint::same(s.get(at), first.get(at)))
            {
                varying.insert(name);
            }
        }
        out.push((series_id, fingerprint::split_reason(&varying)));
    }
    Ok(out)
}
