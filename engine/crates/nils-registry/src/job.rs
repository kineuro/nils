// SPDX-License-Identifier: AGPL-3.0-only

//! One job model (Wave 4a §9.1). Every verb that runs longer than a second
//! is a row of the `job` table with a heartbeat and a progress record, and
//! this module is the one way such a row is claimed, beaten, finished,
//! cancelled, queued and listed. The digest and the classifier each had a
//! claim path of their own before this; a third would have been a third.
//!
//! The rules, which are Wave 1 §10's: a claim refuses while a job **of the
//! same kind** is fresh, takes over one that is stale or whose process on
//! this host is gone, and records the new job as running with this
//! process's id and host. A stale job of any other kind is failed on the
//! way, since a stale row is a stale row. A cancel from outside sets the
//! state to `cancelling`; the running job sees it at its next heartbeat and
//! stops the way a signal would stop it, so what is written stays written,
//! and a job is resumable because nothing is in flight (principle 4).
//!
//! A queue is rows in state `queued` with the command line to run; a worker
//! takes the oldest and runs it, which is how the doors of §11 run anything
//! heavy and answer 202 with the job's id.

use std::fmt;

use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::{now_iso, now_secs, secs_of};

/// A running job whose heartbeat is younger than this holds its kind.
pub const FRESH_SECS: u64 = 60;

/// What a claim says about the job it makes.
#[derive(Debug, Clone)]
pub struct Claim<'a> {
    /// `digest`, `fingerprint`, `classify`, `pick`, `release`, `handover`,
    /// `clinical-import`, `linkage-purge`, `worker`: the kinds that hold each
    /// other off are the ones of one name.
    pub kind: &'a str,
    pub name: &'a str,
    /// What the run was asked to do, as the verb records it. The process's
    /// own command line is added under `argv`, which is what a resume runs.
    pub args: serde_json::Value,
}

/// The states a job passes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Queued,
    Running,
    Cancelling,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Running => "running",
            State::Cancelling => "cancelling",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }

    pub fn parse(text: &str) -> Option<State> {
        Some(match text {
            "queued" => State::Queued,
            "running" => State::Running,
            "cancelling" => State::Cancelling,
            "done" => State::Done,
            "failed" => State::Failed,
            "cancelled" => State::Cancelled,
            _ => return None,
        })
    }

    /// Over, one way or another.
    pub fn is_over(self) -> bool {
        matches!(self, State::Done | State::Failed | State::Cancelled)
    }
}

#[derive(Debug)]
pub enum Error {
    /// A job of this kind is fresh; the run must not start.
    Busy {
        kind: String,
        job_id: i64,
        since: String,
    },
    Store(StoreError),
    Message(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Busy {
                kind,
                job_id,
                since,
            } => write!(
                f,
                "a {kind} job (id {job_id}) holds this registry, last heard from at {since}; \
                 wait, or `nils jobs cancel {job_id}` if it is not running anywhere"
            ),
            Error::Store(e) => write!(f, "{e}"),
            Error::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

/// The host as the job records it.
pub fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// Whether a process of this host is alive: `Some(false)` only when the
/// system says there is no such process.
#[cfg(unix)]
pub fn process_alive(pid: i64) -> Option<bool> {
    let pid = i32::try_from(pid).ok()?;
    if pid <= 0 {
        return None;
    }
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => Some(true),
        Err(nix::errno::Errno::ESRCH) => Some(false),
        Err(_) => Some(true),
    }
}

#[cfg(not(unix))]
pub fn process_alive(_pid: i64) -> Option<bool> {
    None
}

fn stamp(store: &Store, column: &str) -> String {
    let t = table("job");
    store.dialect().text_of(
        t.column(column)
            .unwrap_or_else(|| panic!("job.{column} is not a column")),
    )
}

/// The environment variable a worker sets for the verb it runs, naming the
/// queued row that verb is: the claim adopts that row instead of making a
/// second one, so a queued job is one row from the queue to the outcome,
/// and the id a door answered 202 with is the id whose progress is read.
pub const ADOPT_VAR: &str = "NILS_JOB_ID";

/// Take the kind, failing what is stale and refusing what is fresh, and
/// record this run as a running job. Returns the job's id.
pub fn claim(store: &mut Store, claim: &Claim<'_>) -> Result<i64, Error> {
    let now = now_iso();
    let host = hostname();
    let pid = i64::from(std::process::id());
    let job_t = table("job");
    let adopted: Option<i64> = std::env::var(ADOPT_VAR).ok().and_then(|v| v.parse().ok());
    // Every running or cancelling job: a stale one of any kind is failed,
    // a fresh one of this kind refuses. The stamps are read as text (the
    // store reads a Postgres timestamp only so), and the select sees a row
    // only when another job is running, which is when it must not fail.
    let sql = format!(
        "SELECT id, kind, {}, {}, pid, host FROM {} WHERE state IN ('running', 'cancelling')",
        stamp(store, "heartbeat_at"),
        stamp(store, "started_at"),
        store.qualified("job")
    );
    for j in store.query(&sql, &[])? {
        let id = j.int(0)?;
        if adopted == Some(id) {
            continue;
        }
        let its_kind = j.text(1)?.to_string();
        let last = j
            .opt_text(2)?
            .or(j.opt_text(3)?)
            .unwrap_or_default()
            .to_string();
        let its_pid = j.opt_int(4)?;
        let its_host = j.opt_text(5)?.unwrap_or_default();
        // A job of this host whose process is gone left no one to beat its
        // heart: it is over, however fresh the last beat.
        let gone = its_host == host && its_pid.is_some_and(|p| process_alive(p) == Some(false));
        let fresh = secs_of(&last).is_some_and(|s| now_secs().saturating_sub(s) < FRESH_SECS);
        if fresh && !gone {
            if its_kind == claim.kind {
                return Err(Error::Busy {
                    kind: its_kind,
                    job_id: id,
                    since: last,
                });
            }
            continue;
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
    }
    let mut args = claim.args.clone();
    if !args.is_object() {
        args = serde_json::json!({});
    }
    args["argv"] = std::env::args().collect::<Vec<String>>().into();
    if let Some(id) = adopted {
        // The row a worker took for this verb: it becomes this run.
        let n = store.update_by_id(
            job_t,
            &[
                ("kind", Param::from(claim.kind)),
                ("name", Param::from(claim.name)),
                ("args", Param::from(args.to_string())),
                ("state", Param::from("running")),
                ("pid", Param::Int(pid)),
                ("host", Param::from(host.as_str())),
                ("heartbeat_at", Param::from(now.as_str())),
            ],
            "id",
            id,
        )?;
        if n == 1 {
            return Ok(id);
        }
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
            Param::from(claim.kind),
            Param::from(claim.name),
            Param::from(args.to_string()),
            Param::from("running"),
            Param::Int(pid),
            Param::from(host.as_str()),
            Param::from(now.as_str()),
            Param::from(now.as_str()),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| Error::Message("the job row was not written back".into()))?
        .int(0)
        .map_err(Error::Store)
}

/// What a heartbeat found: whether someone asked the job to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Nothing,
    Cancel,
}

/// The heartbeat, with the progress beside it, and the answer to whether a
/// cancel was requested meanwhile. A job that sees `Asked::Cancel` stops as
/// a signal would stop it and finishes as `cancelled`.
pub fn beat(
    store: &mut Store,
    job_id: i64,
    progress: Option<&serde_json::Value>,
) -> Result<Asked, Error> {
    let now = now_iso();
    let mut set: Vec<(&str, Param)> = vec![("heartbeat_at", Param::from(now.as_str()))];
    if let Some(p) = progress {
        set.push(("progress", Param::from(p.to_string())));
    }
    store.update_by_id(table("job"), &set, "id", job_id)?;
    let sql = format!(
        "SELECT state FROM {} WHERE id = {}",
        store.qualified("job"),
        store.dialect().param(1, Type::Int)
    );
    let state = store
        .query_opt(&sql, &[Param::Int(job_id)])?
        .map(|r| r.text(0).map(str::to_string))
        .transpose()?;
    Ok(match state.as_deref() {
        Some("cancelling") => Asked::Cancel,
        _ => Asked::Nothing,
    })
}

/// The end of a job, with the error when it failed.
pub fn finish(
    store: &mut Store,
    job_id: i64,
    state: State,
    error: Option<&str>,
) -> Result<(), Error> {
    let now = now_iso();
    let mut set: Vec<(&str, Param)> = vec![
        ("state", Param::from(state.name())),
        ("finished_at", Param::from(now.as_str())),
    ];
    if let Some(e) = error {
        set.push(("error", Param::from(e)));
    }
    store.update_by_id(table("job"), &set, "id", job_id)?;
    Ok(())
}

/// Ask a job to stop. A running one becomes `cancelling` and stops at its
/// next heartbeat; a queued one is cancelled outright. Returns the state
/// it is in now, or nothing if there is no such job.
pub fn request_cancel(store: &mut Store, job_id: i64) -> Result<Option<State>, Error> {
    let Some(job) = show(store, job_id)? else {
        return Ok(None);
    };
    let now = now_iso();
    match job.state {
        State::Running => {
            store.update_by_id(
                table("job"),
                &[("state", Param::from("cancelling"))],
                "id",
                job_id,
            )?;
            Ok(Some(State::Cancelling))
        }
        State::Queued => {
            store.update_by_id(
                table("job"),
                &[
                    ("state", Param::from("cancelled")),
                    ("finished_at", Param::from(now.as_str())),
                    ("error", Param::from("cancelled before it ran")),
                ],
                "id",
                job_id,
            )?;
            Ok(Some(State::Cancelled))
        }
        other => Ok(Some(other)),
    }
}

/// One job as the table holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub name: Option<String>,
    pub state: State,
    pub pid: Option<i64>,
    pub host: Option<String>,
    pub started_at: String,
    pub heartbeat_at: Option<String>,
    pub finished_at: Option<String>,
    pub progress: Option<serde_json::Value>,
    pub error: Option<String>,
    pub args: serde_json::Value,
}

impl Job {
    /// The command line the job was started with, if the claim recorded it.
    pub fn argv(&self) -> Option<Vec<String>> {
        self.args["argv"].as_array().map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
    }

    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "kind": self.kind,
            "name": self.name,
            "state": self.state.name(),
            "pid": self.pid,
            "host": self.host,
            "started_at": self.started_at,
            "heartbeat_at": self.heartbeat_at,
            "finished_at": self.finished_at,
            "progress": self.progress,
            "error": self.error,
            "args": self.args,
        })
    }
}

fn select_columns(store: &Store) -> String {
    format!(
        "id, kind, name, state, pid, host, {}, {}, {}, {}, error, {}",
        stamp(store, "started_at"),
        stamp(store, "heartbeat_at"),
        stamp(store, "finished_at"),
        stamp(store, "progress"),
        stamp(store, "args"),
    )
}

fn job_of(r: &crate::store::Row) -> Result<Job, Error> {
    let json = |i: usize| -> Result<Option<serde_json::Value>, Error> {
        Ok(r.opt_text(i)?
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()))
    };
    let state = r.text(3)?;
    Ok(Job {
        id: r.int(0)?,
        kind: r.text(1)?.to_string(),
        name: r.opt_text(2)?.map(str::to_string),
        state: State::parse(state).ok_or_else(|| {
            Error::Message(format!("job state {state} is not one this binary knows"))
        })?,
        pid: r.opt_int(4)?,
        host: r.opt_text(5)?.map(str::to_string),
        started_at: r.opt_text(6)?.unwrap_or_default().to_string(),
        heartbeat_at: r.opt_text(7)?.map(str::to_string),
        finished_at: r.opt_text(8)?.map(str::to_string),
        progress: json(9)?,
        error: r.opt_text(10)?.map(str::to_string),
        args: json(11)?.unwrap_or(serde_json::Value::Null),
    })
}

/// The jobs, newest first: the ones not over, or with `all` the last
/// `limit` of every state.
pub fn list(store: &mut Store, all: bool, limit: usize) -> Result<Vec<Job>, Error> {
    let filter = if all {
        String::new()
    } else {
        " WHERE state IN ('queued', 'running', 'cancelling')".to_string()
    };
    let sql = format!(
        "SELECT {} FROM {}{filter} ORDER BY id DESC LIMIT {limit}",
        select_columns(store),
        store.qualified("job")
    );
    store.query(&sql, &[])?.iter().map(job_of).collect()
}

pub fn show(store: &mut Store, job_id: i64) -> Result<Option<Job>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        select_columns(store),
        store.qualified("job"),
        store.dialect().param(1, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(job_id)])?
        .as_ref()
        .map(job_of)
        .transpose()
}

/// Put a command line on the queue, to be run by a worker in its turn. The
/// kind is the verb, so that `nils jobs list` reads the same for a queued
/// digest and a running one.
pub fn enqueue(store: &mut Store, argv: &[String], name: Option<&str>) -> Result<i64, Error> {
    let Some(verb) = argv.first() else {
        return Err(Error::Message(
            "nothing to queue: the command line is empty".into(),
        ));
    };
    let now = now_iso();
    let args = serde_json::json!({ "argv": argv });
    let rows = store.insert(
        &Insert::new(
            table("job"),
            &["kind", "name", "args", "state", "started_at"],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(verb.as_str()),
            name.map_or(Param::Null, Param::from),
            Param::from(args.to_string()),
            Param::from("queued"),
            Param::from(now.as_str()),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| Error::Message("the job row was not written back".into()))?
        .int(0)
        .map_err(Error::Store)
}

/// The oldest queued job, if any.
pub fn next_queued(store: &mut Store) -> Result<Option<Job>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE state = 'queued' ORDER BY id LIMIT 1",
        select_columns(store),
        store.qualified("job")
    );
    store.query_opt(&sql, &[])?.as_ref().map(job_of).transpose()
}

/// A worker takes a queued job: it becomes running under the worker's
/// process, and ends when the worker says so. Returns false if someone else
/// took it first.
pub fn take(store: &mut Store, job_id: i64) -> Result<bool, Error> {
    let now = now_iso();
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET state = 'running', pid = {}, host = {}, started_at = {}, heartbeat_at = {} \
         WHERE id = {} AND state = 'queued'",
        store.qualified("job"),
        d.param(1, Type::Int),
        d.param(2, Type::Text),
        d.param(3, Type::Timestamp),
        d.param(4, Type::Timestamp),
        d.param(5, Type::Int),
    );
    let n = store.execute(
        &sql,
        &[
            Param::Int(i64::from(std::process::id())),
            Param::from(hostname().as_str()),
            Param::from(now.as_str()),
            Param::from(now.as_str()),
            Param::Int(job_id),
        ],
    )?;
    Ok(n == 1)
}
